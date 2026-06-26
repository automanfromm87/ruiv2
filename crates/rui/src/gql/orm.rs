//! GraphQL-native ORM 核心(L3 data,仅服务端,零第三方依赖)。
//!
//! 设计要点(见 docs/arch.md 的 GraphQL-native ORM 方向):
//!   · 单一真相源:`#[derive(rui::Ent)]` 让一个 struct 同时是 GraphQL 对象类型(同构)+ 表映射(native,`SqlEntity`)。
//!     不再有 GqlObject / SimpleObject / FromRow 三重 derive,也不再有 #[gql_root] 与 async-graphql 双 schema。
//!   · selection 驱动 SQL:列集 = 当前 GraphQL selection ∩ 实体列 ∪ {主键}(projection pushdown)。
//!     `{ todos { id } }` → `select id from todos`,框架不再「取全列再内存裁剪」。
//!   · 被预定义实体图框住:只在实体声明的列 / 受限 filter 内构造查询(等价于「暴露已有查询」,而非自由拼任意 SQL);
//!     真正复杂的多表 join / 聚合走 resolver 逃生舱手写 SQL,不进这层。
//!   · 依赖倒置:具体数据库驱动(sqlx / postgres / 内存…)由 host 经 `set_db_executor` 注入(同 set_transport 模式),
//!     L3 只持纯接口 `DbExecutor` + 语义 `Query`/`Write`,不 NAME 任何驱动 → 守住零依赖默认 + 分层 gate。

use crate::gql::value::{FromValue, Value};
use std::sync::OnceLock;

/// 实体的表映射(由 `#[derive(Ent)]` 在 native 端生成)。GraphQL 字段名默认 = SQL 列名。
pub trait SqlEntity {
    const TABLE: &'static str;
    const PK: &'static str; // 主键的 GraphQL 字段名(= 列名)
    const COLUMNS: &'static [&'static str]; // 全部字段名(= 列名)
}

/// 标量绑定值(参数化查询,杜绝注入)。
#[derive(Clone, Debug)]
pub enum Scalar {
    Str(String),
    Bool(bool),
    Int(i64),
}
impl From<String> for Scalar {
    fn from(s: String) -> Self {
        Scalar::Str(s)
    }
}
impl From<&str> for Scalar {
    fn from(s: &str) -> Self {
        Scalar::Str(s.to_string())
    }
}
impl From<bool> for Scalar {
    fn from(b: bool) -> Self {
        Scalar::Bool(b)
    }
}
impl From<i64> for Scalar {
    fn from(i: i64) -> Self {
        Scalar::Int(i)
    }
}

/// 受限过滤(被预定义实体图框住:只 Eq / Contains;复杂条件走逃生舱)。
/// Contains 携**原始**子串(用户输入按字面匹配)—— executor 负责转义 LIKE 元字符 + 包通配,
/// 保证 PG(`like '%'||esc||'%' escape '\'`)与内存(`str.contains`)语义一致、且 % / _ / \ 不被当通配。
#[derive(Clone)]
pub enum Filter {
    Eq(String, Scalar),       // col = $
    Contains(String, String), // col 含子串(原文,executor 转义)
}

/// 一次读查询的语义计划:列来自 selection(projection pushdown),filter / 排序 / 上限受限。
pub struct Query {
    pub table: &'static str,
    pub columns: Vec<String>,
    pub filter: Option<Filter>,
    pub order_by: Option<String>,
    pub limit: Option<i64>,
}

/// 写操作的 set 值:字面量 或 布尔翻转(`done = not done`)。
pub enum SetVal {
    Lit(Scalar),
    Toggle, // not <该列>
}

/// 写操作(insert / update / delete)。被预定义实体图框住,复杂写走逃生舱。
pub enum Write {
    Insert { table: &'static str, columns: Vec<String>, values: Vec<Scalar> },
    Update { table: &'static str, set: Vec<(String, SetVal)>, filter: Option<Filter> },
    Delete { table: &'static str, filter: Option<Filter> },
}

/// host 注入的数据库后端:吃语义 `Query`/`Write`,产出行 / 执行写。纯接口,不 NAME 任何驱动。
/// 每行 = `[(列名, Value)]`(列名 = GraphQL 字段名 → 直接喂实体的 FromValue)。
pub trait DbExecutor: Send + Sync {
    fn fetch(&self, q: &Query) -> Vec<Vec<(String, Value)>>;
    fn write(&self, w: &Write);
}

static DB: OnceLock<Box<dyn DbExecutor>> = OnceLock::new();

/// host 启动时注册数据库后端(PG / 内存…)。未注册 → 查询返回空、写为空操作(不 panic)。
pub fn set_db_executor(e: Box<dyn DbExecutor>) {
    let _ = DB.set(e);
}
fn db() -> Option<&'static dyn DbExecutor> {
    DB.get().map(|b| b.as_ref())
}

/// 列投影:`all=false` → 当前 selection ∩ 实体列 ∪ {主键}(pushdown);`all=true` → 全列(SSE 快照等)。
/// selection 为空(未在执行上下文内 / 未选具体字段)也回退全列。主键恒并入(store 规范化 __id 需要)。
fn columns_for<E: SqlEntity>(all: bool) -> Vec<String> {
    if all {
        return E::COLUMNS.iter().map(|c| c.to_string()).collect();
    }
    let sel = crate::gql::exec::current_selection();
    let mut cols: Vec<String> =
        E::COLUMNS.iter().filter(|c| sel.iter().any(|s| s == *c)).map(|c| c.to_string()).collect();
    if cols.is_empty() {
        cols = E::COLUMNS.iter().map(|c| c.to_string()).collect();
    }
    if !cols.iter().any(|c| c == E::PK) {
        cols.push(E::PK.to_string());
    }
    cols
}

/// 读查询构建器(thin DX:`Q::new().eq("id", id).order("id")`)。
#[derive(Default)]
pub struct Q {
    filter: Option<Filter>,
    order_by: Option<String>,
    limit: Option<i64>,
}
impl Q {
    pub fn new() -> Q {
        Q::default()
    }
    pub fn eq(mut self, col: &str, v: impl Into<Scalar>) -> Q {
        self.filter = Some(Filter::Eq(col.to_string(), v.into()));
        self
    }
    /// 子串匹配(原始用户输入,按字面;% / _ / \ 不当通配)。
    pub fn contains(mut self, col: &str, v: impl Into<String>) -> Q {
        self.filter = Some(Filter::Contains(col.to_string(), v.into()));
        self
    }
    pub fn order(mut self, col: &str) -> Q {
        self.order_by = Some(col.to_string());
        self
    }
    pub fn limit(mut self, n: i64) -> Q {
        self.limit = Some(n);
        self
    }
}

fn fetch_cols<E: SqlEntity + FromValue>(q: Q, all: bool) -> Vec<E> {
    let query = Query {
        table: E::TABLE,
        columns: columns_for::<E>(all),
        filter: q.filter,
        order_by: q.order_by,
        limit: q.limit,
    };
    match db() {
        Some(d) => d.fetch(&query).into_iter().map(|row| E::from_value(&Value::Object(row))).collect(),
        None => Vec::new(),
    }
}

/// 读实体:列按当前 GraphQL selection 投影下推(`{id}` → `select id`)。resolver 主路径用它。
pub fn fetch<E: SqlEntity + FromValue>(q: Q) -> Vec<E> {
    fetch_cols::<E>(q, false)
}

/// 读实体(全列,忽略 selection):SSE 快照 / 需要完整对象的场景用。
pub fn fetch_all<E: SqlEntity + FromValue>() -> Vec<E> {
    fetch_cols::<E>(Q::new(), true)
}

/// 读实体(全列,但尊重 filter / order / limit):需要完整对象**又要**确定顺序的场景。
/// `#[derive(Ent)]` 生成的 Relay 分页(`<E>Connection::page`)用它按主键稳定排序后再切片
///(connection resolve 时 selection 是 edges/page_info,与实体列不交 → 投影会回退全列,故显式取全列更清晰)。
pub fn fetch_full<E: SqlEntity + FromValue>(q: Q) -> Vec<E> {
    fetch_cols::<E>(q, true)
}

/// 执行写操作(insert / update / delete)。未注册后端 → 空操作。
pub fn write(w: Write) {
    if let Some(d) = db() {
        d.write(&w);
    }
}

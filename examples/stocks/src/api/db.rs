//! 数据后端(仅服务端):实现 rui 的 `DbExecutor` 注入接口。
//!   · PgExecutor —— 同步 `postgres` 驱动(契合 rui 同步 exec 引擎:无 tokio / 无 async / 无 block_on 桥)。
//!     fetch 把 ORM 的语义 Query 渲染成参数化 SQL —— `select {投影列} from todos ...`,投影列来自 GraphQL selection。
//!   · MemExecutor —— 内存回退(未设 DATABASE_URL / 连接失败)。同一套 Query/Write 语义,在 Rust 里直接算。
//! 选哪个由 `backend()` 决定;ssr.rs 启动时 `set_db_executor(backend())` 注入。

use crate::data::model::Todo;
use rui::gql::orm::{DbExecutor, Filter, Query, Scalar, SetVal, Write};
use rui::gql::value::Value;
use std::sync::Mutex;

// ── 后端选择 ──

/// DATABASE_URL 存在且连接成功 → PostgreSQL;否则内存(回退,不 panic)。
pub fn backend() -> Box<dyn DbExecutor> {
    match std::env::var("DATABASE_URL") {
        Ok(url) => match PgExecutor::connect(&url) {
            Ok(pg) => {
                println!("rui · 数据后端:PostgreSQL");
                Box::new(pg)
            }
            Err(e) => {
                eprintln!("rui · PG 连接失败({e}),回退内存后端");
                Box::new(MemExecutor::seeded())
            }
        },
        Err(_) => {
            println!("rui · 数据后端:内存(未设 DATABASE_URL)");
            Box::new(MemExecutor::seeded())
        }
    }
}

// ── 内存后端 ──

pub struct MemExecutor(Mutex<Vec<Todo>>);
impl MemExecutor {
    pub fn seeded() -> MemExecutor {
        let seed = [("1", "学习 rui 框架", true), ("2", "用它写个 todolist", false), ("3", "配合 tailwind 调样式", false)];
        MemExecutor(Mutex::new(
            seed.iter().map(|(id, t, d)| Todo { id: id.to_string(), text: t.to_string(), done: *d }).collect(),
        ))
    }
    fn row(t: &Todo) -> Vec<(String, Value)> {
        // 内存后端不裁剪列(投影下推是 SQL 的事):返回全列,解码端按 selection 取用。
        vec![
            ("id".to_string(), Value::Str(t.id.clone())),
            ("text".to_string(), Value::Str(t.text.clone())),
            ("done".to_string(), Value::Bool(t.done)),
        ]
    }
}
fn mem_match(t: &Todo, f: &Option<Filter>) -> bool {
    match f {
        None => true,
        Some(Filter::Eq(c, v)) => mem_eq(t, c, v),
        // 子串匹配:原文按字面 contains(与 PG 转义后的 LIKE 语义一致)。
        Some(Filter::Contains(c, raw)) => match c.as_str() {
            "text" => t.text.contains(raw),
            "id" => t.id.contains(raw),
            _ => false,
        },
    }
}
fn mem_eq(t: &Todo, col: &str, v: &Scalar) -> bool {
    matches!((col, v),
        ("id", Scalar::Str(s)) if &t.id == s)
        || matches!((col, v), ("text", Scalar::Str(s)) if &t.text == s)
        || matches!((col, v), ("done", Scalar::Bool(b)) if t.done == *b)
}
fn mem_set(t: &mut Todo, col: &str, v: &Scalar) {
    match (col, v) {
        ("id", Scalar::Str(s)) => t.id = s.clone(),
        ("id", Scalar::Int(i)) => t.id = i.to_string(),
        ("text", Scalar::Str(s)) => t.text = s.clone(),
        ("done", Scalar::Bool(b)) => t.done = *b,
        _ => {}
    }
}
impl DbExecutor for MemExecutor {
    fn fetch(&self, q: &Query) -> Vec<Vec<(String, Value)>> {
        let v = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let mut rows: Vec<&Todo> = v.iter().filter(|t| mem_match(t, &q.filter)).collect();
        if q.order_by.as_deref() == Some("id") {
            rows.sort_by_key(|t| t.id.parse::<i64>().unwrap_or(i64::MAX));
        }
        if let Some(n) = q.limit {
            rows.truncate(n.max(0) as usize);
        }
        rows.iter().map(|t| MemExecutor::row(t)).collect()
    }
    fn write(&self, w: &Write) {
        let mut v = self.0.lock().unwrap_or_else(|e| e.into_inner());
        match w {
            Write::Insert { columns, values, .. } => {
                let mut t = Todo { id: String::new(), text: String::new(), done: false };
                for (c, val) in columns.iter().zip(values) {
                    mem_set(&mut t, c, val);
                }
                v.push(t);
            }
            Write::Update { set, filter, .. } => {
                for t in v.iter_mut().filter(|t| mem_match(t, filter)) {
                    for (col, sv) in set {
                        match sv {
                            SetVal::Lit(val) => mem_set(t, col, val),
                            SetVal::Toggle if col == "done" => t.done = !t.done,
                            SetVal::Toggle => {}
                        }
                    }
                }
            }
            Write::Delete { filter, .. } => v.retain(|t| !mem_match(t, filter)),
        }
    }
}

// ── PostgreSQL 后端(同步 postgres 驱动)──

use postgres::types::{ToSql, Type};
use postgres::{Client, NoTls, Row};

pub struct PgExecutor(Mutex<Client>);
impl PgExecutor {
    pub fn connect(url: &str) -> Result<PgExecutor, postgres::Error> {
        Ok(PgExecutor(Mutex::new(Client::connect(url, NoTls)?)))
    }
}

fn bind(s: &Scalar) -> Box<dyn ToSql + Sync> {
    match s {
        Scalar::Str(v) => Box::new(v.clone()),
        Scalar::Bool(v) => Box::new(*v),
        Scalar::Int(v) => Box::new(*v),
    }
}
// 按 PG 列类型解码成 rui Value(列名 = GraphQL 字段名 → 直接喂实体 FromValue)。
// 用 try_get:SQL NULL 或类型不符 → Value::Null(而非 row.get 在 NULL 上 panic)。
fn pg_value(row: &Row, i: usize) -> Value {
    match *row.columns()[i].type_() {
        Type::BOOL => row.try_get::<_, bool>(i).map(Value::Bool).unwrap_or(Value::Null),
        Type::INT2 => row.try_get::<_, i16>(i).map(|v| Value::Int(v as i64)).unwrap_or(Value::Null),
        Type::INT4 => row.try_get::<_, i32>(i).map(|v| Value::Int(v as i64)).unwrap_or(Value::Null),
        Type::INT8 => row.try_get::<_, i64>(i).map(Value::Int).unwrap_or(Value::Null),
        Type::FLOAT4 => row.try_get::<_, f32>(i).map(|v| Value::Float(v as f64)).unwrap_or(Value::Null),
        Type::FLOAT8 => row.try_get::<_, f64>(i).map(Value::Float).unwrap_or(Value::Null),
        _ => row.try_get::<_, String>(i).map(Value::Str).unwrap_or(Value::Null),
    }
}
fn where_clause(f: &Filter, n: usize) -> (String, Box<dyn ToSql + Sync>) {
    match f {
        Filter::Eq(col, v) => (format!(" where {col} = ${n}"), bind(v)),
        // 子串匹配:转义用户输入的 LIKE 元字符(% _ \)→ 按字面匹配,不被当通配;显式 escape '\'。
        Filter::Contains(col, raw) => {
            let esc = raw.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
            (format!(" where {col} like ${n} escape '\\'"), Box::new(format!("%{esc}%")))
        }
    }
}
impl DbExecutor for PgExecutor {
    fn fetch(&self, q: &Query) -> Vec<Vec<(String, Value)>> {
        // selection 驱动的列投影:`select {选中列 ∪ 主键} from {table}`。
        let mut sql = format!("select {} from {}", q.columns.join(", "), q.table);
        let mut params: Vec<Box<dyn ToSql + Sync>> = Vec::new();
        if let Some(f) = &q.filter {
            let (w, p) = where_clause(f, 1);
            sql.push_str(&w);
            params.push(p);
        }
        if let Some(o) = &q.order_by {
            // id 是文本列但语义上是数字 → 按数值排序(bigint 与内存后端 i64 范围对齐,避免 int 溢出;seed 用 "1".."N")。
            if o == "id" {
                sql.push_str(" order by id::bigint");
            } else {
                sql.push_str(&format!(" order by {o}"));
            }
        }
        if let Some(n) = q.limit {
            sql.push_str(&format!(" limit {n}"));
        }
        let refs: Vec<&(dyn ToSql + Sync)> = params.iter().map(|b| b.as_ref()).collect();
        let mut client = self.0.lock().unwrap_or_else(|e| e.into_inner());
        match client.query(sql.as_str(), &refs) {
            Ok(rows) => rows
                .iter()
                .map(|r| q.columns.iter().enumerate().map(|(i, c)| (c.clone(), pg_value(r, i))).collect())
                .collect(),
            Err(e) => {
                eprintln!("rui · PG 查询失败:{e}(sql: {sql})");
                Vec::new()
            }
        }
    }
    fn write(&self, w: &Write) {
        let mut params: Vec<Box<dyn ToSql + Sync>> = Vec::new();
        let sql = match w {
            Write::Insert { table, columns, values } => {
                let ph: Vec<String> = values
                    .iter()
                    .enumerate()
                    .map(|(i, v)| {
                        params.push(bind(v));
                        format!("${}", i + 1)
                    })
                    .collect();
                format!("insert into {table} ({}) values ({})", columns.join(", "), ph.join(", "))
            }
            Write::Update { table, set, filter } => {
                let mut sets = Vec::new();
                for (col, sv) in set {
                    match sv {
                        SetVal::Lit(v) => {
                            params.push(bind(v));
                            sets.push(format!("{col} = ${}", params.len()));
                        }
                        SetVal::Toggle => sets.push(format!("{col} = not {col}")),
                    }
                }
                let mut sql = format!("update {table} set {}", sets.join(", "));
                if let Some(f) = filter {
                    let (w, p) = where_clause(f, params.len() + 1);
                    sql.push_str(&w);
                    params.push(p);
                }
                sql
            }
            Write::Delete { table, filter } => {
                let mut sql = format!("delete from {table}");
                if let Some(f) = filter {
                    let (w, p) = where_clause(f, params.len() + 1);
                    sql.push_str(&w);
                    params.push(p);
                }
                sql
            }
        };
        let refs: Vec<&(dyn ToSql + Sync)> = params.iter().map(|b| b.as_ref()).collect();
        let mut client = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Err(e) = client.execute(sql.as_str(), &refs) {
            eprintln!("rui · PG 写失败:{e}(sql: {sql})");
        }
    }
}

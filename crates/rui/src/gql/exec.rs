//! 通用 GraphQL 执行器(仅服务端)。
//! 流程:解析文档 → 对每个根字段调用方注入的 resolver 拿「全字段 Value」→ 按 selection 投影 →
//! 组装标准 `{"data":{...},"errors":[...]}`。
//!
//! 执行器**完全不认识**具体类型(Stock/Order),也**不认识用户的 schema roots** —— 它只操作
//! Value + selection,并通过 `Resolver` 回调把 (kind, field, args) dispatch 给应用层。这正是框架
//! 与应用解耦的关键:框架 crate 不引用 `crate::schema::QueryRoot`,由 `rui::serve` 注入 resolver。

use crate::gql::parser::{self, AVal, Field, OpKind};
use crate::gql::value::Value;

/// 根字段的参数访问(resolver 用)。
pub struct Args<'a>(pub &'a [(String, AVal)]);
impl<'a> Args<'a> {
    pub fn str(&self, k: &str) -> String {
        self.0
            .iter()
            .find(|(n, _)| n == k)
            .map(|(_, v)| match v {
                AVal::Str(s) => s.clone(),
                _ => String::new(),
            })
            .unwrap_or_default()
    }
    pub fn f64(&self, k: &str) -> f64 {
        self.0
            .iter()
            .find(|(n, _)| n == k)
            .map(|(_, v)| match v {
                AVal::Float(f) => *f,
                AVal::Int(i) => *i as f64,
                _ => 0.0,
            })
            .unwrap_or(0.0)
    }
    pub fn i64(&self, k: &str) -> i64 {
        self.0
            .iter()
            .find(|(n, _)| n == k)
            .map(|(_, v)| match v {
                AVal::Int(i) => *i,
                AVal::Float(f) => *f as i64,
                _ => 0,
            })
            .unwrap_or(0)
    }
    pub fn bool(&self, k: &str) -> bool {
        self.0
            .iter()
            .find(|(n, _)| n == k)
            .map(|(_, v)| matches!(v, AVal::Bool(true)))
            .unwrap_or(false)
    }
}

/// 让 #[gql_root] 的方法参数按类型从 args 提取(可扩展自定义标量)。
pub trait FromArg {
    fn from_arg(args: &Args, name: &str) -> Self;
}
impl FromArg for String {
    fn from_arg(a: &Args, n: &str) -> Self {
        a.str(n)
    }
}
impl FromArg for i64 {
    fn from_arg(a: &Args, n: &str) -> Self {
        a.i64(n)
    }
}
impl FromArg for f64 {
    fn from_arg(a: &Args, n: &str) -> Self {
        a.f64(n)
    }
}
impl FromArg for bool {
    fn from_arg(a: &Args, n: &str) -> Self {
        a.bool(n)
    }
}

/// 自由函数 resolver 的统一接口:`#[rui::resolver(kind)]` 为每个根字段在对应根类型上实现它,
/// `graphql!` 生成的 dispatch 按字段 marker 选择对应实现(靠 trait coherence,无需运行时收集 / 无 linker)。
pub trait Resolve<M> {
    fn resolve(args: &Args) -> Value;
}

/// resolver 回调:按 (operation 类型, 字段名, 参数) dispatch 到应用的 roots。
/// 由应用层(通常在 bin/ssr.rs 用一个 match 聚合 `#[gql_root]` 生成的各 Root::resolve)提供,
/// 经 `rui::serve` / `rui::server::set_resolver` 注入。
pub type Resolver = fn(OpKind, &str, &Args) -> Value;

/// 占位 resolver:对任何字段都返回 Null。
/// `rui init` 生成的最小骨架(还没有数据层)默认用它;接入自己的 `#[gql_root]` 后,
/// 在 `bin/ssr.rs` 把 `rui::empty_resolver` 换成应用的 `schema::resolve` 即可。
pub fn empty_resolver(_kind: OpKind, _field: &str, _args: &Args) -> Value {
    Value::Null
}

/// 执行一个 GraphQL 文档,返回标准 JSON 响应字符串。
pub fn execute(req: &str, resolve: Resolver) -> String {
    let doc = parser::parse(req);
    let mut data: Vec<(String, Value)> = Vec::new();
    for op in &doc.ops {
        for f in &op.selection {
            let args = Args(&f.args);
            let raw = resolve(op.kind, &f.name, &args);
            let key = f.alias.clone().unwrap_or_else(|| f.name.clone());
            data.push((key, project(&raw, &f.selection)));
        }
    }
    Value::Object(vec![
        ("data".to_string(), Value::Object(data)),
        ("errors".to_string(), Value::List(Vec::new())),
    ])
    .to_json()
}

/// 按 selection 把 resolver 产出的「全字段 Value」裁剪成只含所选字段(递归 + 列表 + 别名)。
fn project(v: &Value, sel: &[Field]) -> Value {
    match v {
        Value::List(xs) => Value::List(xs.iter().map(|x| project(x, sel)).collect()),
        Value::Object(_) if !sel.is_empty() => {
            let mut out = Vec::new();
            // 始终保留 meta 字段(规范化缓存定位 entity 用,即使查询没显式选取)。
            if let Some(tn) = v.get("__typename") {
                out.push(("__typename".to_string(), tn.clone()));
            }
            if let Some(id) = v.get("__id") {
                out.push(("__id".to_string(), id.clone()));
            }
            for f in sel {
                // __typename/__id 已在上面预置;客户端显式选取(无别名)时跳过,避免重复 key。
                if f.alias.is_none() && (f.name == "__typename" || f.name == "__id") {
                    continue;
                }
                let fv = v.field(&f.name);
                let pv = if f.selection.is_empty() {
                    fv.clone()
                } else {
                    project(fv, &f.selection)
                };
                out.push((f.alias.clone().unwrap_or_else(|| f.name.clone()), pv));
            }
            Value::Object(out)
        }
        _ => v.clone(),
    }
}

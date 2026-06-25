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

// ── 错误收集(本次执行的 GraphQL errors[];线程局部 = 每连接一线程,天然请求隔离)──
thread_local! {
    static ERRORS: std::cell::RefCell<Vec<Value>> = const { std::cell::RefCell::new(Vec::new()) };
    static CUR_FIELD: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

/// resolver 主动报一个 GraphQL 错误:进本次响应的 `errors[]`(path 取当前根字段);该字段通常返回空/默认值。
/// 让 resolver 不改返回类型也能「失败一个字段」(如「未找到」「无权限」「校验失败」)。仅服务端执行期有效。
pub fn report_error(message: impl Into<String>) {
    let field = CUR_FIELD.with(|f| f.borrow().clone());
    push_error(message.into(), &field);
}

fn push_error(message: String, field: &str) {
    let mut obj = vec![("message".to_string(), Value::Str(message))];
    if !field.is_empty() {
        obj.push(("path".to_string(), Value::List(vec![Value::Str(field.to_string())])));
    }
    ERRORS.with(|e| e.borrow_mut().push(Value::Object(obj)));
}

fn panic_message(p: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = p.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = p.downcast_ref::<String>() {
        s.clone()
    } else {
        "resolver panicked".to_string()
    }
}

/// 执行一个 GraphQL 文档,返回标准 JSON 响应字符串。
/// 每个根字段独立隔离:resolver panic 被 catch_unwind 兜住 → 进 errors[] + 该字段 data=null、其余字段照常
/// (标准 GraphQL partial data + errors)。resolver 也可调 `report_error` 主动报错。
pub fn execute(req: &str, resolve: Resolver) -> String {
    // 本次执行的错误集(同线程复用前清空)。前提:execute 同线程不可重入 —— 每次 execute 末尾 drain 并清空,
    // thread-per-conn + resolver 返回 Value(不回调 execute)保证了这点;若将来引入 async / 嵌套执行需另设计。
    ERRORS.with(|e| e.borrow_mut().clear());
    let doc = parser::parse(req);
    // 粗粒度解析失败:非空请求却零 operation → 报一个解析错误(精细定位需 parser 带错误通道,后续)。
    if doc.ops.is_empty() && !req.trim().is_empty() {
        push_error("无法解析 GraphQL 查询".to_string(), "");
    }
    let mut data: Vec<(String, Value)> = Vec::new();
    for op in &doc.ops {
        for f in &op.selection {
            let args = Args(&f.args);
            let key = f.alias.clone().unwrap_or_else(|| f.name.clone());
            CUR_FIELD.with(|cf| *cf.borrow_mut() = f.name.clone()); // 供 report_error / panic 取 path
            // 隔离 resolver + 投影的 panic:都包进 catch_unwind(project 在 catch 外则它 panic 会逃逸杀连接)。
            // panic 不杀线程、转成 errors[] 条目 + 该字段 null,其余字段照常(标准 partial data)。
            // 注:AssertUnwindSafe 只是「保证 panic 安全」的承诺 —— resolver 持锁 panic 仍会毒化该 Mutex,
            // 由 resolver 侧用 `lock().unwrap_or_else(|e| e.into_inner())` 恢复(见 examples 的 todos.rs)。
            let resolved = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                project(&resolve(op.kind, &f.name, &args), &f.selection)
            }));
            let val = match resolved {
                Ok(v) => v,
                Err(p) => {
                    push_error(format!("字段 {} 执行出错:{}", f.name, panic_message(p)), &f.name);
                    Value::Null
                }
            };
            data.push((key, val));
        }
    }
    let errors = ERRORS.with(|e| std::mem::take(&mut *e.borrow_mut()));
    Value::Object(vec![
        ("data".to_string(), Value::Object(data)),
        ("errors".to_string(), Value::List(errors)),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gql::parser::OpKind;

    fn resolver(_k: OpKind, field: &str, _a: &Args) -> Value {
        match field {
            "ok" => Value::Object(vec![("id".to_string(), Value::Str("1".to_string()))]),
            "boom" => panic!("kaboom"),
            "reported" => {
                report_error("未找到");
                Value::Null
            }
            _ => Value::Null,
        }
    }

    #[test]
    fn clean_query_has_empty_errors() {
        let out = execute("{ ok { id } }", resolver);
        assert!(out.contains("\"errors\":[]"), "{out}");
        assert!(out.contains("\"id\":\"1\""), "{out}");
    }

    #[test]
    fn panic_field_isolated_into_errors() {
        // boom 字段 panic 被隔离:进 errors[]、该字段 null,ok 字段照常(partial data)。
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {})); // 静音预期内 panic 输出
        let out = execute("{ ok { id } boom { id } }", resolver);
        std::panic::set_hook(prev);
        assert!(out.contains("kaboom"), "errors 应含 panic 消息: {out}");
        assert!(out.contains("\"id\":\"1\""), "ok 字段应照常: {out}");
        assert!(out.contains("\"boom\":null"), "panic 字段应为 null: {out}");
    }

    #[test]
    fn report_error_surfaces() {
        let out = execute("{ reported { id } }", resolver);
        assert!(out.contains("未找到"), "report_error 应进 errors[]: {out}");
        assert!(!out.contains("\"errors\":[]"), "errors 不应为空: {out}");
    }

    #[test]
    fn parse_failure_reports_error() {
        let out = execute("not a valid query", resolver);
        assert!(out.contains("无法解析"), "解析失败应报错: {out}");
    }
}

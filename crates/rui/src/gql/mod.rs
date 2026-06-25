//! GraphQL data 层。
//!
//!   value   运行时值模型 + JSON 编解码(前后端共用)
//!   (trait) 类型系统:Scalar / GqlElem / Field<M> / Reshape / GqlObject
//!
//! 后续节点会加入:服务端 GraphQL parser + 执行器(parser/exec)、客户端规范化缓存(store)。
//!
//! ## 类型系统如何让「编译器即 schema 校验器」延伸到嵌套 + exact-fit
//!
//! proc-macro 之间互不可见(query! 看不到 #[derive(GqlObject)] 生成了什么),但 derive
//! 产出的是真实 Rust 项,在**类型检查阶段**对 query! 生成的代码可见。于是把「字段存在吗 /
//! 是什么类型」全部推迟到类型检查:
//!
//!   · `Field<Marker>` —— derive 为每个字段生成 `impl Field<gqlf::name> for Order { type Ty = ..; }`。
//!     query! 只需字段名即可用 `<E as Field<gqlf::name>>::Ty` 投影出字段类型 —— 无需知道对象类型名。
//!   · `Scalar` —— 标量字段 exact-fit 类型来源(`<Ty as Scalar>::Out`)。选对象当标量 → 无 impl → 报错。
//!   · `GqlElem` —— 带子 selection 的字段萃取元素对象类型(`<Ty as GqlElem>::Elem`)。选标量当对象 → 报错。
//!     容器走 blanket(`Vec<T>`),单对象走 derive 的具体 impl,无重叠。
//!   · `Reshape` —— 把内层 exact-fit struct 包回原字段容器形状(`Vec<Item>`→`Vec<内层>`;单对象→内层)。
#![allow(dead_code)] // 各 trait/函数分别只在某一端用到

pub mod value;

#[allow(unused_imports)]
pub use value::{errors_message, parse, FromValue, IntoValue, Value};

// 客户端规范化缓存(前后端同构 query! 两端都用到)。
pub mod store;

// 服务端 GraphQL parser + 通用执行器(wasm 端不需要)。
#[cfg(not(target_arch = "wasm32"))]
pub mod exec;
#[cfg(not(target_arch = "wasm32"))]
pub mod parser;

/// 标量字段:exact-fit struct 的字段类型 = `<字段类型 as Scalar>::Out`。
pub trait Scalar {
    type Out: FromValue + Clone;
}
impl Scalar for String {
    type Out = String;
}
impl Scalar for i64 {
    type Out = i64;
}
impl Scalar for f64 {
    type Out = f64;
}
impl Scalar for bool {
    type Out = bool;
}

/// 对象元素萃取:`Vec<T>` → `T`(列表),单对象 → 自身(derive 生成具体 impl)。
pub trait GqlElem {
    type Elem: GqlObject;
}
impl<T: GqlObject> GqlElem for Vec<T> {
    type Elem = T;
}

/// 字段投影:`<对象类型 as Field<字段 marker>>::Ty` = 该字段的 Rust 类型。
/// marker 是按字段名生成的零大小类型(集中声明于 `crate::gqlf`,见 `gql_fields!`)。
pub trait Field<M> {
    type Ty;
}

/// 把内层 exact-fit struct `S` 包回原字段 `Self` 的容器形状:
/// `Vec<T>` → `Vec<S>`(列表),单对象 → `S`(derive 生成具体 impl)。
pub trait Reshape<S> {
    type Out: FromValue;
}
impl<T, S: FromValue> Reshape<S> for Vec<T> {
    type Out = Vec<S>;
}

/// 片段:`fragment!(Name on Type { .. })` 生成的命名 exact-fit 数据结构。
/// 带它的 selection 字符串 —— query! 里 `...Name` 展开时把这段拼进查询(组件只能读片段声明的字段 = data masking)。
pub trait Fragment: FromValue {
    const SELECTION: &'static str;
}

/// 对象类型:运行时按字段名取 Value(执行器/序列化用)+ entity id(规范化缓存的 key)。
pub trait GqlObject {
    const TYPENAME: &'static str;
    fn gql_id(&self) -> Value;
    fn gql_field(&self, name: &str) -> Option<Value>;
}

/// 把 Rust 变量格式化成 GraphQL 参数字面量:字符串加引号并转义,数字/布尔裸输出。
/// query!/subscription! 的变量参数 `field(arg: var)` 用它,保证类型正确且不被引号/反斜杠破坏。
pub fn gql_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}
pub trait ToGqlArg {
    fn to_gql_arg(&self) -> String;
}
impl ToGqlArg for str {
    fn to_gql_arg(&self) -> String {
        format!("\"{}\"", gql_escape(self))
    }
}
impl ToGqlArg for String {
    fn to_gql_arg(&self) -> String {
        self.as_str().to_gql_arg()
    }
}
impl ToGqlArg for i64 {
    fn to_gql_arg(&self) -> String {
        self.to_string()
    }
}
impl ToGqlArg for f64 {
    fn to_gql_arg(&self) -> String {
        self.to_string()
    }
}
impl ToGqlArg for bool {
    fn to_gql_arg(&self) -> String {
        self.to_string()
    }
}

// ── SSR 本地 transport(依赖倒置:host 注入"执行一个 query 串 → 响应文本"的实现)──
// 破除原 dom→server 的**向上循环依赖**:dom 的 SSR 预取(dom::gql/subscribe)原本直接调 crate::server::local_execute,
// 现改调 crate::gql::fetch;host(server)启动时用 set_transport 注册其 local_execute。gql 只知 fn(&str)->String,
// 不 NAME 任何 host 模块 → 编译期无 gql→host 边(运行时才注入函数指针)。dom 改为只向下依赖 gql,循环消除。
#[cfg(not(target_arch = "wasm32"))]
mod transport {
    use std::sync::OnceLock;
    static TRANSPORT: OnceLock<fn(&str) -> String> = OnceLock::new();
    /// host 启动时注册「同步执行一个 query 串 → 响应文本」的实现(native = server::local_execute)。
    pub fn set_transport(f: fn(&str) -> String) {
        let _ = TRANSPORT.set(f);
    }
    /// SSR 预取:执行一个 query 串拿响应文本。未注册 transport(如最小骨架 / 测试)→ 返回空数据,不 panic。
    pub fn fetch(query: &str) -> String {
        match TRANSPORT.get() {
            Some(f) => f(query),
            None => r#"{"data":{},"errors":[]}"#.to_string(),
        }
    }
}
#[cfg(not(target_arch = "wasm32"))]
pub use transport::{fetch, set_transport};

/// 把 transport 回来的响应文本解码成 `Vec<T>`(query!/mutation!/subscription! 共用)。
/// 兼容两种载荷:标准 `{"data":{root:[...]}}` 与裸数组 `[...]`(回退)。
pub fn decode_rows<T: FromValue>(text: &str, root: &str) -> Vec<T> {
    let v = parse(text);
    let payload = v.get("data").and_then(|d| d.get(root)).unwrap_or(&v);
    <Vec<T> as FromValue>::from_value(payload)
}

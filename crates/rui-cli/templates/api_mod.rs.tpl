//! 后端 API 层(crate::api):schema(两端可见的类型层 + 仅服务端 resolver)+ 数据实现(仅服务端)。
//!
//! 加 API:
//!   1. 建 `schema.rs`:写根 impl + 一个【自己手写】的聚合 resolve,然后取消下面 `pub mod schema;`。
//!      注意:`#[gql_root]` 只为【单个】根生成 `Root::resolve`;聚合 `resolve(kind, field, args)` 要你手写,
//!      且【只为你声明过的根】写分支,其余 OpKind 一律 `_ => rui::gql::Value::Null` 兜底
//!      ——别照搬同时引用 MutationRoot/SubscriptionRoot 的多根范例(没声明的根会编译报错)。
//!      query-only 最小例:
//!        pub struct Query;
//!        #[rui::gql_root(query)]
//!        impl Query {
//!            fn todos(&self) -> Vec<crate::data::model::Todo> { crate::api::todos::all() }
//!        }
//!        #[cfg(not(target_arch = "wasm32"))]
//!        pub fn resolve(kind: rui::gql::parser::OpKind, field: &str, args: &rui::gql::exec::Args) -> rui::gql::Value {
//!            match kind {
//!                rui::gql::parser::OpKind::Query => QueryRoot::resolve(field, args),
//!                _ => rui::gql::Value::Null,
//!            }
//!        }
//!   2. 数据实现按领域建文件(如 `todos.rs`),`#[cfg(not(target_arch = "wasm32"))] pub mod todos;`。
//!   3. 在 crate 根 lib.rs 加 `rui::app! {}`(注册表)+ `rui::gql_fields!(字段名...)`(字段 marker)。
//!   4. 在 src/app.rs 的 `platform! { .. }` 里加 `resolve = crate::api::schema::resolve,`(要 SSE 订阅则加 `subscribe { snapshot = .., feed = .. }`)。

// pub mod schema;
// #[cfg(not(target_arch = "wasm32"))] pub mod todos;

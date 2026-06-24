//! # rui
//!
//! 一个全栈 Rust 框架:`reactive`(Signal/effect/memo)+ `view!` 宏 + GraphQL 数据层
//! (`query!`/`mutation!`/`subscription!`,编译期 schema 校验、exact-fit、规范化缓存)+ 同构 SSR。
//!
//! 应用只写 model / schema(`#[gql_root]`) / pages(`view!`) / route;框架提供其余一切:
//!   · 客户端入口:`rui::client!(route)`(生成 wasm 导出)
//!   · 服务端:`rui::serve(App { .. })`(SSR + GraphQL + SSE)
//!
//! 约定(目录即规范,宏只认这套路径):共享模型 `crate::data::model`、后端 API `crate::api::schema`
//! (`#[gql_root]`)、视图 `crate::view`(components / layout / pages)、字段 marker `crate::gqlf`(`gql_fields!` 在 crate 根)。

pub mod dom;
pub mod gql;
pub mod reactive;
pub mod runtime;

#[cfg(not(target_arch = "wasm32"))]
pub mod server;

// 宏:应用直接用 rui::view! / rui::query! / #[derive(rui::GqlObject)] / #[rui::gql_root(..)] 等。
pub use rui_macros::{
    fragment, gql_fields, gql_root, gql_schema, mutation, paginated, query, subscription, view, GqlObject,
};

#[cfg(not(target_arch = "wasm32"))]
pub use server::{serve, App, Sse};

// 占位 resolver:最小骨架(无数据层)用它满足 App.resolve,接入 #[gql_root] 后替换。
#[cfg(not(target_arch = "wasm32"))]
pub use gql::exec::empty_resolver;

/// 常用项一站式导入:`use rui::prelude::*;`
pub mod prelude {
    pub use crate::gql::{FromValue, IntoValue, ToGqlArg, Value};
    pub use crate::reactive::{effect, memo, scope, Signal};
    pub use rui_macros::{
        fragment, gql_fields, gql_root, gql_schema, mutation, paginated, query, subscription, view, GqlObject,
    };
}

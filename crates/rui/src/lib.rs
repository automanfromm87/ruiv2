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

// ── 分层(依赖方向严格向下;由 tests/layering.rs gate 强制,详见 docs/arch.md)──
// 物理上仍是平铺 mod(modules-with-discipline:不嵌套文件 / 不拆 crate,保持宏的 rui::dom::* 等路径契约 +
// 零依赖图);层边界靠 layering gate 测试守。将来 kernel 可作为 rui-kernel 单独发布。
//   L1 kernel  —— 纯响应式内核,零 crate:: 依赖
pub mod props; // #[component] typed-builder 支持(Missing/Set/OrDefault)
pub mod reactive;
//   L2 view/runtime —— 视图树 / 元素模型 / 路由;只向下依赖 L1
pub mod dom;
pub mod runtime;
pub mod view;
//   L3 data —— GraphQL 值/类型/规范化缓存/执行 + SSR transport 注入点;只向下依赖 L1
pub mod gql;
//   L4 host(server / server_axum)在文件下方按 cfg 声明。

pub use view::{node_ref, NodeRef, Page, Strategy, View};
// 事件:on:<event> 的 handler 内用 `rui::event()` 取当前事件快照(键盘 key/修饰键、鼠标坐标、files 等);
// 零参 handler 不受影响。事件修饰符在 view! 里写 `on:keydown.enter.prevent={..}`。
pub use dom::{event, Event, FileMeta};
// ErrorBoundary:子树出错 → 局部 fallback + reset 重试(`<ErrorBoundary fallback=..>`,建在 Context 上)。
// throw_error 渲染期上报;error_reporter() 渲染期取上报器闭包,事件 / 异步回调里调。
pub use view::{error_reporter, throw_error, ErrorSink};
// 生命周期:on_mount(节点入 DOM 后,客户端)/ on_cleanup(scope 销毁时)。
pub use reactive::on_cleanup;
pub use runtime::on_mount;
// Context:provide_context / use_context(跨层传 theme / store / 当前用户,免 prop-drill);
// provider 子树局部 provide(不泄漏给兄弟)。
pub use reactive::{provide_context, provider, use_context};
// batch:合并多次 set 为一次 flush(事件入口已自动 batch)。
pub use reactive::batch;
// GraphQL resolver 主动报错(服务端):在 #[gql_root] 方法体里调 `rui::report_error("..")` → 进响应 errors[]
//(该字段返回空/默认即可)。resolver panic 也会被执行器隔离成 errors[] + 该字段 null,不再静默吞掉。
#[cfg(not(target_arch = "wasm32"))]
pub use gql::exec::report_error;
// 路由:`param(i)`/`param_as::<T>(i)` 读 path 第 i 段(reactive,通常由 `#[rui::page]` 据模式串自动接);
// `query_param("k")`/`query_param_as::<T>("k")` 读 `?k=`(在 body 里按需读,独立于 path);
// `path()`/`query_string()` 原始 signal;`go` 程序化导航(pushState);`matches` 给 `router!` 分发。
pub use runtime::{
    go, matches, navigate, param, param_as, path, query_encode, query_param, query_param_as, query_string,
};

#[cfg(not(target_arch = "wasm32"))]
pub mod server;

// 生产 HTTP 后端(feature = "axum"):rui::serve_axum(App) / serve_axum_with(App, AppConfig)。仅非 wasm。
#[cfg(all(not(target_arch = "wasm32"), feature = "axum"))]
pub mod server_axum;
// set_graphql_schema:注册 async-graphql Schema 作为 GraphQL 引擎(应用直接 dep async-graphql 定义 schema,
// 与 rui 同版本 → cargo 统一为一份,Schema 满足 rui 这边的 async_graphql::Executor bound)。
#[cfg(all(not(target_arch = "wasm32"), feature = "axum"))]
pub use server_axum::{serve_axum, serve_axum_with, set_graphql_schema};

// 宏:应用直接用 rui::view! / rui::query! / #[derive(rui::GqlObject)] / #[rui::gql_root(..)] 等。
pub use rui_macros::{
    app, component, fragment, gql_fields, gql_root, gql_schema, mutation, page, paginated, query, resource,
    router, subscription, view, GqlObject,
};

// 宿主:serve(零配置)/ serve_with(自定义 AppConfig:bind / 资源路由 / body 上限 / HTML 外壳 / router.js)。
#[cfg(not(target_arch = "wasm32"))]
pub use server::{default_shell, serve, serve_with, App, AppConfig, AssetMap, ShellCtx, Sse};

// 占位 resolver:最小骨架(无数据层)用它满足 App.resolve,接入 #[gql_root] 后替换。
#[cfg(not(target_arch = "wasm32"))]
pub use gql::exec::empty_resolver;

/// 常用项一站式导入:`use rui::prelude::*;`
pub mod prelude {
    pub use crate::gql::{FromValue, IntoValue, ToGqlArg, Value};
    pub use crate::reactive::{batch, effect, memo, provide_context, provider, scope, use_context, Signal};
    pub use crate::dom::{event, Event};
    pub use crate::view::{error_reporter, throw_error, IntoView, View};
    pub use rui_macros::{
        component, fragment, gql_fields, gql_root, gql_schema, mutation, page, paginated, query, resource, router,
        subscription, view, GqlObject,
    };
}

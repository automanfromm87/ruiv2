//! todo —— 用 rui 框架搭的 TodoList 应用(crate 名沿用 stocks,内容是 todo)。
//! 用上框架全部能力:GraphQL(query!/mutation!/subscription!/paginated!/fragment!)+ 规范化缓存、
//! 同构 SSR + 真水合 + 数据交接、渲染策略(#[rui::page] ssr/csr/static)、组件、keyed <For>、
//! <Show>/<Switch>、IntoView 条件、bind:value、memo、<Transition>、AsyncJob。
//!
//! **App 装配集中在 `app.rs`**(platform! 声明路由/数据/订阅/jobs → app());本文件只做
//! 模块声明 + crate 根级注册(有路径约束、必须在根)+ 装配入口 re-export。

pub mod api;
pub mod data;
mod app; // App 装配:platform!(路由 + resolve + 订阅 + jobs)+ client! 客户端入口 + 404
mod view;

// crate 根级注册(有路径约束,必须在 crate 根):
//   · `app! {}` 生成 `crate::__rui_registry`(所有消费宏统一引用),把 components/model/schema/fields
//     映射到应用实际目录(此处全用默认约定;改目录 / 跨 crate 时在此覆盖)。宏放哪生成哪 → 只能在根。
//   · `gql_fields!` 声明全部 GraphQL 字段名 marker(= 注册表默认的 fields 路径 `crate::gqlf`)。
rui::app! {}
rui::gql_fields!(
    id, text, done, todos, todo_page, search, detail, add_todo, toggle_todo, remove_todo, clear_done, complete_all,
    todo_updates
);

// 装配入口 re-export:
//   · `route`(同构):页面里 `rui::go(crate::route, ..)` 程序化导航用;客户端 / SSR 都需要。
//   · `app()`(仅服务端):bin/ssr.rs 用 `rui::serve_axum(stocks::app())`。
pub use app::route;
#[cfg(not(target_arch = "wasm32"))]
pub use app::{app, describe}; // app(): 启动;describe(): 部署模型(rui plan / cargo run -- plan)

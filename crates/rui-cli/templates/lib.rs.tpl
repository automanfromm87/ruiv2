//! 由 `rui init` 生成的骨架 —— 目录即规范(rui 约定,宏只认这套路径):
//!   src/data/   前后端共享数据模型   → crate::data::model
//!   src/api/    后端 API            → crate::api::schema(#[gql_root])+ 数据实现
//!   src/view/   前端视图            → crate::view::{components, layout, pages}
//!   src/app.rs  App 装配            → platform!(路由 + resolve + 订阅 + jobs)+ client! 客户端入口 + 404
//!   src/lib.rs  入口               → 模块挂载 + 装配入口 re-export(+ 接数据层后的 crate 根级注册)
//!
//! 起始页是 src/view/pages/home.rs。接数据层时:data/ 写模型、api/ 写 #[gql_root] schema,
//! 然后在本文件加 `rui::app! {}`(注册表)+ `rui::gql_fields!(字段...)`(字段 marker),
//! 并在 src/app.rs 的 platform! 里加 `resolve = crate::api::schema::resolve`(详见 src/api/mod.rs)。

pub mod api;
pub mod data;
pub mod view;
mod app; // App 装配:platform! + client! + 404(见 src/app.rs)

// 装配入口 re-export:
//   · route(同构):页面里 `rui::go(crate::route, ..)` 程序化导航用;客户端 / SSR 都需要。
//   · app()(仅服务端):bin/ssr.rs 用 `rui::serve(crate::app())`。
pub use app::route;
#[cfg(not(target_arch = "wasm32"))]
pub use app::{app, describe}; // app(): 启动;describe(): 部署模型(`rui plan`)

// ── 接数据层后,在此打开 crate 根级注册(有路径约束,必须在 crate 根)──
// `app! {}` 生成 crate::__rui_registry(所有消费宏统一引用);`gql_fields!` 声明全部 GraphQL 字段名 marker。
// 注意:在 data::model / api::schema / crate::gqlf 都存在之前不要打开(否则 re-export 指向不存在的模块会报错)。
//
// rui::app! {}
// rui::gql_fields!(/* id, text, todos, ... */);

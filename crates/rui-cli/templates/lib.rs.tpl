//! 由 `rui init` 生成的骨架 —— 目录即规范(rui 约定,宏只认这套路径):
//!   src/data/   前后端共享数据模型   → crate::data::model
//!   src/api/    后端 API            → crate::api::schema(#[gql_root])+ 数据实现
//!   src/view/   前端视图            → crate::view::{components, layout, pages}
//!   src/lib.rs  入口:模块挂载 + client! + router! 路由表
//!
//! 起始页是 src/view/pages/home.rs。接数据层(query!/mutation!)时:data/ 写模型、
//! api/ 写 #[gql_root] schema、这里加 `rui::gql_fields!(字段...)`、再把 ssr.rs 的 resolve 换成你的。

pub mod api;
pub mod data;
pub mod view;

use view::pages;

// wasm 客户端入口:生成 alloc / render_route / navigate / dispatch / on_fetch 等导出(仅 wasm 目标)。
rui::client!(crate::route);

// 路由表:每个页面在 src/view/pages/ 下用 #[rui::page("/...")] 声明,在这里登记。
// 需要全局外壳时加 `layout = view::layout::shell`;需要共享侧栏的区段时加
// `group("/前缀", layout = view::layout::xxx) { pages::a, pages::b }`。
rui::router! {
    pages::home,
    fallback = not_found,
}

fn not_found() -> rui::View {
    use rui::dom::{attr, el, set_text};
    let d = el("div");
    attr(d, "class", "mx-auto max-w-2xl px-6 py-16 text-center text-2xl");
    set_text(d, "404 · 页面不存在");
    rui::View(d)
}

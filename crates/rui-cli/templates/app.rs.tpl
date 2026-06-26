//! App 装配:一处声明 路由(页面)+ GraphQL resolver + SSE 订阅 + 后台 jobs → `app() -> AppRuntime`。
//! 路由模式声明在各 page 的 `#[rui::page("/...")]` 上(唯一真相源);这里只列哪些页参与 + 接 resolve / 订阅 / jobs。
//!
//! 接数据层后在 platform! 里按需加(都可选):
//!   resolve = crate::api::schema::resolve,                                  // GraphQL(/graphql + 同构预取)
//!   subscribe { snapshot = crate::api::xxx::snapshot, feed = crate::api::xxx::subscribe },  // SSE 订阅
//!   jobs { crate::api::jobs::some_job },                                    // 后台 AsyncJob
//! 需要全局外壳:`layout = crate::view::layout::shell`;共享侧栏区段:`group("/前缀", layout = ..) { pages::a, pages::b }`。

use crate::view::pages;

// 统一装配:生成 `route()`(同构)+ `app() -> AppRuntime`(仅服务端)。
// 页面少时直接平铺;多了可收进 `routes { layout=, pages..., group(){}, fallback= }` 让顶层更清爽。
rui::platform! {
    pages::home,
    fallback = not_found,
}

// wasm 客户端入口:生成 alloc / render_route / navigate / dispatch / on_fetch 等导出(仅 wasm 目标)。
rui::client!(crate::app::route);

fn not_found() -> rui::View {
    use rui::dom::{attr, el, set_text};
    let d = el("div");
    attr(d, "class", "mx-auto max-w-2xl px-6 py-16 text-center text-2xl");
    set_text(d, "404 · 页面不存在");
    rui::View(d)
}

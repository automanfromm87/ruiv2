//! stocks —— 用 rui 框架搭的示例 app。
//!
//! 目录即规范(rui 约定,清晰且强制 —— 宏只认这套路径):
//!   src/lib.rs        入口:模块挂载 + gql_fields!(字段 marker)+ client!(wasm 入口)+ route(路由)
//!   src/data/         共享数据 → crate::data::model
//!   src/api/          后端 API → crate::api::schema(#[gql_root])+ 数据实现(stocks / orders)
//!   src/view/         视图层 → crate::view::{components, layout, pages}
//!   src/bin/ssr.rs    SSR 服务器入口
//!
//! 框架(rui)提供响应式、DOM、GraphQL 引擎、规范化缓存、同构 SSR、wasm 入口、CLI。

pub mod data; // 共享数据模型(crate::data::model)
pub mod api; // 后端 API:bin/ssr.rs 取 api::schema::resolve 与 api::stocks 的 SSE / ticker hook
mod view; // crate 内可见即可:route 用 view::{layout, pages}

// 字段 marker(集中声明一次,在 crate 根)→ crate::gqlf::*(derive(GqlObject) 与 query! 据此投影字段类型)。
rui::gql_fields!(
    symbol, name, price, change, stocks, stock, set_price, price_updates, orders, id, items, sku, qty,
    stock_page, edges, node, cursor, page_info, has_next_page, end_cursor
);

// wasm 客户端入口:生成 alloc / render_route / dispatch / on_fetch 导出。
rui::client!(crate::route);

/// 路由:路径 → 页面节点,用 navbar 外壳包裹后交给框架挂载 / 序列化。
pub fn route(path: &str) -> u32 {
    use view::{layout, pages};
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let page = match segs.as_slice() {
        [] => pages::index::view(),
        ["counter"] => pages::counter::view(),
        ["about"] => pages::about::view(),
        ["table"] => pages::table::view(),
        ["live"] => pages::live::view(),
        ["orders"] => pages::orders::view(),
        ["feed"] => pages::feed::view(),
        ["cards"] => pages::cards::view(),
        ["macro"] => pages::mac::view(),
        ["stock", id] => pages::stock::view(id.to_string()),
        _ => not_found(),
    };
    layout::shell(path, page)
}

fn not_found() -> u32 {
    use rui::dom::{attr, el, set_text};
    let d = el("div");
    attr(d, "class", "text-2xl");
    set_text(d, "404 · 页面不存在");
    d
}

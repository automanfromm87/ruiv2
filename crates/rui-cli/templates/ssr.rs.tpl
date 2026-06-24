//! SSR 服务器入口。
//!
//! 接入数据层后,把 `resolve` 从 `rui::empty_resolver` 换成 `{NAME}::api::schema::resolve`;
//! 需要 subscription(SSE)时再填 `sse: Some(rui::Sse { .. })`。
fn main() {
    rui::serve(rui::App {
        route: {NAME}::route,
        resolve: rui::empty_resolver,
        sse: None,
    });
}

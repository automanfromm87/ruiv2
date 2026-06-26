//! SSR 服务器入口。
//!
//! 路由 / resolve / 订阅 / jobs 都在 src/app.rs 的 `rui::platform! { .. }` 一处声明 → `app()`,这里只负责启动。
//! 需要生产后端(有界并发 / 优雅关闭 / SSE keep-alive)时:给 Cargo.toml 的 rui 开 `features = ["axum"]`,
//! 把 `serve` 换成 `serve_axum`(同签名)。接了数据库等存储,在这里 serve 之前注入(如 set_db_executor)。
fn main() {
    // `rui plan`(= `cargo run --bin ssr -- plan`):打印部署 DAG + provision plan 后退出(不连 DB / 不起服务)。
    rui::maybe_plan({NAME}::describe);
    rui::serve({NAME}::app());
}

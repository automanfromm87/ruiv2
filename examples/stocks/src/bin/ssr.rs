//! todo SSR 服务器:把 route / resolve / SSE(订阅广播)装进 rui::App 启动。

fn main() {
    // 生产后端:axum + tokio(有界并发 / 优雅关闭 / body 上限 / SSE keep-alive)。
    // 想退回零依赖 std 服务器,把 serve_axum 换成 serve 即可(同签名)。
    // 想自定义宿主(bind / 资源路由 / body 上限 / HTML 外壳 / router.js):用 serve_axum_with(app, AppConfig)/serve_with,例:
    //   rui::serve_axum_with(app, rui::AppConfig { bind: ("0.0.0.0".into(), 3000), ..Default::default() });
    rui::serve_axum(rui::App {
        route: stocks::route,
        resolve: stocks::api::schema::resolve,
        sse: Some(rui::Sse {
            snapshot: stocks::api::todos::snapshot_json,
            subscribe: stocks::api::todos::add_subscriber,
        }),
    });
}

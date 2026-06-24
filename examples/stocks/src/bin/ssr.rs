//! todo SSR 服务器:把 route / resolve / SSE(订阅广播)装进 rui::App 启动。

fn main() {
    rui::serve(rui::App {
        route: stocks::route,
        resolve: stocks::api::schema::resolve,
        sse: Some(rui::Sse {
            snapshot: stocks::api::todos::snapshot_json,
            subscribe: stocks::api::todos::add_subscriber,
        }),
    });
}

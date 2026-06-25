//! todo SSR 服务器:把 route / resolve / SSE(订阅广播)装进 rui::App 启动。

fn main() {
    // 生产后端:axum + tokio(有界并发 / 优雅关闭 / body 上限 / SSE keep-alive)。
    // 想退回零依赖 std 服务器,把 serve_axum 换成 serve 即可(同签名)。
    // 想自定义宿主(bind / 资源路由 / body 上限 / HTML 外壳 / router.js):用 serve_axum_with(app, AppConfig)/serve_with,例:
    //   rui::serve_axum_with(app, rui::AppConfig { bind: ("0.0.0.0".into(), 3000), ..Default::default() });
    // GraphQL 引擎:注册 async-graphql Schema(/graphql + SSR 预取都经它执行)。须在 serve_axum 前(先占住 transport)。
    // DATABASE_URL 存在 → 注入 PgPool(resolver 走 PG);否则 None → resolver 回退内存。
    let pg = stocks::api::db::Pg::from_env();
    rui::set_graphql_schema(stocks::api::schema::ag::build_schema(pg));
    rui::serve_axum(rui::App {
        route: stocks::route,
        resolve: stocks::api::schema::resolve, // 仍传(std host / legacy fallback 用;axum 路径已走 async-graphql transport)

        sse: Some(rui::Sse {
            snapshot: stocks::api::todos::snapshot_json,
            subscribe: stocks::api::todos::add_subscriber,
        }),
    });
}

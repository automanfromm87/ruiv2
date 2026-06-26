//! todo SSR 服务器:注入存储后端 → 启动 platform! 统一装配出的 AppRuntime(app())。
//! 路由 / API / resolve / 订阅都在 crate::lib 的 `rui::platform! { .. }` 一处声明,这里不再手搓。

fn main() {
    // `cargo run --bin ssr -- plan`(即 rui plan):打印部署 DAG + provision plan 后退出(在连 DB 之前)。
    rui::maybe_plan(stocks::describe);

    // 数据后端:DATABASE_URL 存在且连接成功 → PostgreSQL(同步 postgres 驱动),否则内存(回退)。
    // 经 rui 的 ORM 注入接缝 set_db_executor 注册;resolver 经 rui::gql::orm::fetch/write 走它(selection→SQL 投影)。
    // 必须在 serve_axum 前注册(resolver 首次执行前)。
    rui::gql::orm::set_db_executor(stocks::api::db::backend());

    // 生产后端:axum + tokio(有界并发 / 优雅关闭 / body 上限 / SSE keep-alive)。
    // GraphQL 引擎走 rui 自带同步 exec(serve_axum 内 set_resolver 注入 transport)—— 不再用 async-graphql,无双 schema。
    // 想退回零依赖 std 服务器,把 serve_axum 换成 serve 即可(同签名)。
    // 统一装配:platform! 生成的 app() 一处声明路由 + resolve + 订阅 → AppRuntime。
    rui::serve_axum(stocks::app());
}

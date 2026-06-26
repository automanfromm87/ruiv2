//! App 装配:一处声明 路由(页面)+ GraphQL resolver + SSE 订阅 + 后台 jobs → `app() -> AppRuntime`。
//! 路由模式声明在各 page 的 `#[rui::page("/...")]` 上(唯一真相源);这里只列哪些页参与 + 接 resolve / 订阅 / jobs。
//! JSON API = GraphQL(`/graphql`),无 REST。bin/ssr.rs:`rui::serve_axum(stocks::app())`。

use crate::view::{layout, pages};

// 统一装配:生成 `route()`(同构)+ `app() -> AppRuntime`(+ jobs 的 `run_job`,均 native-only)。
// 顶层读起来是几个并列关注点:routes(Web 路由)/ resolve(GraphQL)/ subscribe(SSE)/ jobs(后台任务)。
rui::platform! {
    // Web 路由表(SSR/CSR/Static 页面 + 路由组);整体收进 routes 段,顶层不嘈杂。
    routes {
        layout = layout::shell,
        pages::index,
        pages::archive,
        pages::detail, // /todo/:id —— 路由参数,模式 + 类型化签名都在 detail.rs
        // 路由组:/dash 与 /dash/settings 共享 dash_shell 侧栏(组内导航不重建侧栏,只换内层 outlet)。
        group("/dash", layout = pages::dash::dash_shell) {
            pages::dash::overview, // /dash
            pages::dash::settings, // /dash/settings
        },
        pages::about,
        pages::draft,
        pages::boundary,    // /boundary —— <ErrorBoundary> 演示
        pages::forms,       // /forms —— 表单(bind:value/checked/group + select + 校验)
        pages::transitions, // /transitions —— <Transition> 进出场动画
        fallback = not_found,
    },
    // 数据层 / 订阅(GraphQL-native:JSON API 即 /graphql,无 REST)。
    resolve = crate::api::schema::resolve,
    database = postgres, // 部署模型:声明需要一个 Postgres(Ent 经注入的 DbExecutor 统一接入,无需逐个列)
    subscribe {
        snapshot = crate::api::todos::snapshot_json,
        feed = crate::api::todos::add_subscriber,
    },
    // 后台 AsyncJob:add_todo mutation 入队 notify_added,worker 线程异步执行(见 api/jobs.rs)。
    jobs {
        crate::api::jobs::notify_added,
    },
    // 定时任务:scheduler 线程按间隔 enqueue heartbeat,worker 异步执行(复用 jobs 那套)。
    crons {
        crate::api::jobs::heartbeat,
    },
}

// 客户端 wasm 入口(导出 alloc/render_route/dispatch/...);驱动 platform! 生成的 route()。
rui::client!(crate::app::route);

fn not_found() -> rui::View {
    use rui::dom::{attr, el, set_text};
    let d = el("div");
    attr(d, "class", "text-2xl");
    set_text(d, "404 · 页面不存在");
    rui::View(d)
}

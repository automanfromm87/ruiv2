//! 生产 HTTP 后端(feature = "axum"):把 rui 的 GraphQL 执行 + 同构 SSR 挂进 axum/tokio,
//! 拿到生产网络栈(有界并发、优雅关闭、body 上限、SSE keep-alive、POST-only /graphql)。
//!
//! 桥接要点(详见各处注释):
//!   · rui 引擎是**同步 + 重度线程局部**,axum 是 async 多线程 → 渲染 / 执行经 `spawn_blocking`
//!     隔离到阻塞线程一次跑完(不卡 async worker、线程局部安全)。
//!   · 阻塞池**复用线程**(不像 std `serve` 一连接一线程会死)→ 渲染走 `render_page` → `with_request_runtime`:
//!     进入时 take 四子系统状态(留下新鲜空态)、退出时(正常或 panic)RAII restore,实现 per-request 隔离
//!     并杜绝跨渲染累积 / 泄漏(取代早期"每渲染开头 reactive::reset()"的清空式做法)。
//!   · SSE:应用给的是 std mpsc 广播(阻塞 recv)→ 一根专用 std 线程把它泵进 tokio 通道 → axum 异步事件流。

use crate::server::{config, page, set_config, set_resolver, AppConfig};
use crate::App;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{header, StatusCode, Uri};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use axum::Router;
use std::convert::Infallible;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;

/// 启动 axum 后端(零配置;= serve_axum_with(app, AppConfig::default()))。阻塞,内部自建 tokio 运行时。
pub fn serve_axum(app: App) {
    serve_axum_with(app, AppConfig::default());
}

/// 启动 axum 后端并指定宿主配置(bind / 资源路由 / body 上限 / HTML 外壳 / router.js)。与 std serve_with 共享配置。
pub fn serve_axum_with(app: App, cfg: AppConfig) {
    set_config(cfg); // 必须在任何 config() 读取前设置(下面 run 里读路由/bind)
    set_resolver(app.resolve); // 同构 SSR 本地预取用同一 resolver
    crate::jobs::start_worker_if_configured(); // platform! 声明了 jobs/crons → 起后台 worker 线程
    crate::jobs::start_crons_if_configured(); // platform! 声明了 crons → 起定时调度线程(按间隔 enqueue)
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    rt.block_on(run(app));
}

async fn run(app: App) {
    let c = config();
    // 全部入口路由(协议 + 资源)+ bind + body 上限都来自 cfg(可配置宿主,单一真相源)。
    let router = Router::new()
        .route(c.assets.graphql_route.as_str(), post(graphql)) // POST-only(GET mutation → 405)
        .route(c.assets.subscribe_route.as_str(), get(subscribe))
        .route(c.assets.router_js_route.as_str(), get(router_js))
        .route(c.assets.wasm_route.as_str(), get(app_wasm))
        .route(c.assets.css_route.as_str(), get(styles_css))
        .fallback(ssr)
        .with_state(app)
        .layer(DefaultBodyLimit::max(c.body_limit)); // body 上限(默认 1MB;std 版现也强制到 body)
    let listener = tokio::net::TcpListener::bind((c.bind.0.as_str(), c.bind.1)).await.expect("bind");
    println!("rui · axum SSR  →  http://{}:{}", c.bind.0, c.bind.1);
    axum::serve(listener, router).with_graceful_shutdown(shutdown()).await.expect("serve");
}

// 优雅关闭:Ctrl-C(SIGINT)或 SIGTERM(容器编排 docker stop / k8s 发的就是它)任一触发。
async fn shutdown() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {}
        _ = term => {}
    }
    println!("\nrui · 优雅关闭…");
}

// /graphql:同步执行器经 spawn_blocking 隔离(不卡 async worker;ERRORS 线程局部每次 execute 自清)。
async fn graphql(State(app): State<App>, body: axum::body::Bytes) -> impl IntoResponse {
    let json_ct = [(header::CONTENT_TYPE, "application/json; charset=utf-8")];
    // body 非法 UTF-8 → 400(而非 lossy 静默替换成 � 喂给解析器,产出语义上骗人的结果)。
    let q = match std::str::from_utf8(&body) {
        Ok(s) => s.to_owned(),
        Err(_) => {
            return (StatusCode::BAD_REQUEST, json_ct, r#"{"data":null,"errors":[{"message":"invalid UTF-8 in request body"}]}"#.to_string())
                .into_response()
        }
    };
    // 经 transport 执行:已注册 async-graphql(set_graphql_schema)则走它(spawn_blocking 里 block_on schema.execute),
    // 否则回退 legacy exec(set_resolver 注册)。spawn_blocking 隔离同步执行,不卡 async worker。
    let _ = app; // resolver 现经 transport;App.resolve 仅 std host / legacy 用
    let json = tokio::task::spawn_blocking(move || crate::gql::fetch(&q)).await.unwrap_or_else(|e| {
        eprintln!("rui: /graphql 执行任务 panic:{e}"); // JoinError 不再静默吞掉
        r#"{"data":null,"errors":[{"message":"internal error"}]}"#.to_string()
    });
    (StatusCode::OK, json_ct, json).into_response()
}

/// 注册 async-graphql Schema 作为 GraphQL 引擎(取代手搓 exec):/graphql + SSR 预取都经它执行。
/// 经 transport 接缝注入 —— SSR 同步预取在 spawn_blocking 线程上 `Handle::block_on(schema.execute())` 合法、不 panic
/// (该线程非 runtime worker)。在 serve_axum 前调用(先占住 transport;serve_axum 内 set_resolver 的 legacy
/// set_transport 因 OnceLock set-once 被忽略)。Response 经 serde_json 序列化成标准 {data,errors?} JSON。
/// 可选路线(feature = "graphql_async");默认走 rui 自带同步 exec 引擎 + native ORM(gql::orm)。
#[cfg(feature = "graphql_async")]
pub fn set_graphql_schema<E>(schema: E)
where
    E: async_graphql::Executor,
{
    crate::gql::set_transport(Box::new(move |q: &str| {
        // SSR 预取订阅取「初值 = 当前值」:async-graphql 的 subscription 是流、execute 不支持 →
        // 把前导 subscription 改写成 query(应用须在 Query 根镜像同名字段)。只 SSR 预取会把 subscription 发到 transport
        //(浏览器订阅走 SSE /graphql/subscribe,不经此)。
        let q = q.trim_start();
        let q = if let Some(rest) = q.strip_prefix("subscription") {
            format!("query{rest}")
        } else {
            q.to_string()
        };
        let resp = tokio::runtime::Handle::current().block_on(schema.execute(async_graphql::Request::new(q)));
        serde_json::to_string(&resp)
            .unwrap_or_else(|_| r#"{"data":null,"errors":[{"message":"serialize error"}]}"#.to_string())
    }));
}

// 兜底:同构 SSR。spawn_blocking 跑同步渲染(render_page 经 with_request_runtime 隔离本次请求的竞技场)。
async fn ssr(State(app): State<App>, uri: Uri) -> impl IntoResponse {
    let path = uri.path().to_string();
    let query = uri.query().unwrap_or("").to_string();
    let path_log = path.clone(); // 渲染任务 panic 时用于日志(path 被移进 spawn_blocking)
    let html = tokio::task::spawn_blocking(move || page(app.route, &path, &query)).await.unwrap_or_else(|e| {
        eprintln!("rui: SSR 渲染任务 panic({path_log}):{e}"); // JoinError 不再静默吞掉
        "<!doctype html><h1>500 internal error</h1>".to_string()
    });
    Html(html)
}

async fn router_js() -> impl IntoResponse {
    // config() 是 &'static(OnceLock),router_js(Cow)的 as_ref 给出 &'static str,axum 可直接作响应体。
    ([(header::CONTENT_TYPE, "text/javascript; charset=utf-8")], config().router_js.as_ref())
}
async fn app_wasm() -> impl IntoResponse {
    static_file(&config().assets.wasm_disk, "application/wasm")
}
async fn styles_css() -> impl IntoResponse {
    static_file(&config().assets.css_disk, "text/css; charset=utf-8")
}
fn static_file(p: &str, ctype: &'static str) -> (StatusCode, [(header::HeaderName, &'static str); 1], Vec<u8>) {
    match std::fs::read(p) {
        Ok(b) => (StatusCode::OK, [(header::CONTENT_TYPE, ctype)], b),
        Err(_) => (StatusCode::NOT_FOUND, [(header::CONTENT_TYPE, "text/plain; charset=utf-8")], format!("{p} missing").into_bytes()),
    }
}

// SSE 订阅:应用给的是 std mpsc(阻塞 recv);专用 std 线程把它泵进 tokio 通道 → axum 异步事件流。
// 首条先推 snapshot;keep-alive 注释流防僵尸连接(顺带补了 SSE 无心跳那条 gap)。
async fn subscribe(State(app): State<App>) -> impl IntoResponse {
    use std::sync::mpsc::RecvTimeoutError;
    use tokio::sync::mpsc::error::TrySendError;
    // 有界通道(256):慢客户端跟不上时 try_send 满了直接丢消息(背压 shed),不让积压无界增长吃内存。
    let (txt, rxt) = tokio::sync::mpsc::channel::<String>(256);
    if let Some(sse) = app.sse {
        let _ = txt.try_send((sse.snapshot)()); // snapshot 先行
        let std_rx = (sse.subscribe)();
        // 专用线程(非阻塞池,避免长生阻塞把池占满):recv_timeout 等广播 → 泵进 tokio 通道。
        std::thread::spawn(move || loop {
            match std_rx.recv_timeout(std::time::Duration::from_secs(15)) {
                Ok(msg) => match txt.try_send(msg) {
                    Ok(()) => {}
                    Err(TrySendError::Full(_)) => {}        // 客户端跟不上:丢这条
                    Err(TrySendError::Closed(_)) => break,  // 客户端断开
                },
                // 超时无广播:借机检查客户端是否已断开 → 是则退出,否则零广播连接会让本线程永生(僵尸)。
                Err(RecvTimeoutError::Timeout) => {
                    if txt.is_closed() {
                        break;
                    }
                }
                Err(RecvTimeoutError::Disconnected) => break, // 广播源关闭
            }
        });
    }
    let stream = ReceiverStream::new(rxt).map(|m| Ok::<Event, Infallible>(Event::default().data(m)));
    Sse::new(stream).keep_alive(KeepAlive::default())
}

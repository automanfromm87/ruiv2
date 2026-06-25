//! 生产 HTTP 后端(feature = "axum"):把 rui 的 GraphQL 执行 + 同构 SSR 挂进 axum/tokio,
//! 拿到生产网络栈(有界并发、优雅关闭、body 上限、SSE keep-alive、POST-only /graphql)。
//!
//! 桥接要点(详见各处注释):
//!   · rui 引擎是**同步 + 重度线程局部**,axum 是 async 多线程 → 渲染 / 执行经 `spawn_blocking`
//!     隔离到阻塞线程一次跑完(不卡 async worker、线程局部安全)。
//!   · 阻塞池**复用线程**(不像 std `serve` 一连接一线程会死)→ `render_page` 开头 `reactive::reset()`
//!     清掉只增不减的 EFFECTS 竞技场(否则跨渲染累积 = 泄漏)。
//!   · SSE:应用给的是 std mpsc 广播(阻塞 recv)→ 一根专用 std 线程把它泵进 tokio 通道 → axum 异步事件流。

use crate::gql::exec;
use crate::server::{page, set_resolver, ROUTER_JS};
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

/// 启动 axum 后端(阻塞,内部自建 tokio 运行时;签名同 std `serve`,应用 main 直接调)。
pub fn serve_axum(app: App) {
    set_resolver(app.resolve); // 同构 SSR 本地预取用同一 resolver
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    rt.block_on(run(app));
}

async fn run(app: App) {
    let router = Router::new()
        .route("/graphql", post(graphql)) // POST-only(GET mutation → 405,修了 std 的 method 不校验)
        .route("/graphql/subscribe", get(subscribe))
        .route("/router.js", get(router_js))
        .route("/app.wasm", get(app_wasm))
        .route("/styles.css", get(styles_css))
        .fallback(ssr)
        .with_state(app)
        .layer(DefaultBodyLimit::max(1 << 20)); // body 1MB 上限(std 版 body 无上限 → 内存 DoS)
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 8084)).await.expect("bind 8084");
    println!("rui · axum SSR  →  http://127.0.0.1:8084");
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
    let json = tokio::task::spawn_blocking(move || exec::execute(&q, app.resolve)).await.unwrap_or_else(|e| {
        eprintln!("rui: /graphql 执行任务 panic:{e}"); // JoinError 不再静默吞掉
        r#"{"data":null,"errors":[{"message":"internal error"}]}"#.to_string()
    });
    (StatusCode::OK, json_ct, json).into_response()
}

// 兜底:同构 SSR。spawn_blocking 跑同步渲染(render_page 开头 reactive::reset 清竞技场)。
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
    ([(header::CONTENT_TYPE, "text/javascript; charset=utf-8")], ROUTER_JS)
}
async fn app_wasm() -> impl IntoResponse {
    static_file("web/app.wasm", "application/wasm")
}
async fn styles_css() -> impl IntoResponse {
    static_file("web/styles.css", "text/css; charset=utf-8")
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

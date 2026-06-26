//! SSR runtime —— 仅服务端。纯 std、零依赖、每连接一线程。
//! 提供:GraphQL(`/graphql` + `/graphql/subscribe` SSE)、页面 SSR(同构预取)、静态资源
//! (内嵌 `/router.js`,磁盘 `web/app.wasm`、`web/styles.css`)。
//! 应用通过 `rui::serve(App { route, resolve, sse })` 启动。

use crate::gql::exec::{self, Resolver};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::Receiver;
use std::sync::OnceLock;
use std::thread;

// 框架内嵌的客户端 glue(所有 rui app 通用):wasm 加载 + ffi + SPA 导航。
pub(crate) const ROUTER_JS: &str = include_str!("assets/router.js");

// ── 同构本地执行(native 端 dom::gql 用)──

static RESOLVE: OnceLock<Resolver> = OnceLock::new();

/// 注册应用的 GraphQL resolver(serve 启动时调用一次)。
pub fn set_resolver(r: Resolver) {
    let _ = RESOLVE.set(r);
    crate::gql::set_transport(Box::new(local_execute)); // 依赖倒置:把 SSR 预取 transport 注入 gql(legacy exec;async-graphql 路径见 set_graphql_schema)
}

/// 同构 SSR:native 端 `dom::gql` 用已注册的 resolver 本地执行查询(首屏预取)。
pub fn local_execute(req: &str) -> String {
    match RESOLVE.get() {
        Some(r) => exec::execute(req, *r),
        None => r#"{"data":{},"errors":[]}"#.to_string(),
    }
}

// ── 应用描述 ──

/// 订阅(SSE)hook —— 内容是应用数据,不是框架职责。
#[derive(Clone, Copy)]
pub struct Sse {
    /// SSE 初值(连接建立时先推一次)。
    pub snapshot: fn() -> String,
    /// 订阅通道:server 的 SSE 循环从返回的 Receiver 收推送。
    pub subscribe: fn() -> Receiver<String>,
}

/// 一个 rui 应用运行时:路由 + GraphQL resolver +(可选)订阅 +(可选)API 路由。
/// 由 `rui::platform! { .. }` 一处声明生成的 `app()` 返回(取代手搓结构体);`serve` / `serve_axum` 收它启动。
#[derive(Clone, Copy)]
pub struct AppRuntime {
    /// 路径 → Page(策略 + 延迟渲染);由 `#[rui::page]` / `#[rui::route(ssr|csr|static)]` + route() 提供。
    pub route: fn(&str) -> crate::view::Page,
    /// GraphQL resolver(应用用一个 match 聚合 #[gql_root] 生成的各 Root::resolve)。
    pub resolve: Resolver,
    /// 可选订阅(subscription)。
    pub sse: Option<Sse>,
}

/// 向后兼容别名:旧代码的 `rui::App { .. }` 仍可用(推荐改用 `platform!` 生成的 `app() -> AppRuntime`)。
pub type App = AppRuntime;

// ── 宿主配置(AppConfig):把宿主级常量从硬编码变成可配置 —— 框架从「自带固定交付方式的模板」变成「引擎」。
//    进程级 OnceLock(一服务一配置,启动设一次、只读;同 RESOLVE 模式)。所有字段 Default = 当前字面量,
//    故零配置 serve(app) / serve_axum(app) 行为字节不变;serve_with / serve_axum_with 传自定义。两后端共享同一份配置。

/// 宿主的全部入口映射:协议入口(graphql/subscribe)+ 静态资源(router.js/wasm/css)路由 + 磁盘路径。
/// 这是宿主入口的**单一真相源** —— 服务端据此路由,客户端经 default_shell 注入的 window.__rui 读同一份(贯穿两端)。
#[derive(Clone)]
pub struct AssetMap {
    pub graphql_route: String,   // 默认 "/graphql"(POST 查询/变更)
    pub subscribe_route: String, // 默认 "/graphql/subscribe"(SSE 订阅)
    pub router_js_route: String, // 默认 "/router.js"(内嵌 glue)
    pub wasm_route: String,      // 默认 "/app.wasm"
    pub wasm_disk: String,       // 默认 "web/app.wasm"
    pub css_route: String,       // 默认 "/styles.css"
    pub css_disk: String,        // 默认 "web/styles.css"
}
impl Default for AssetMap {
    fn default() -> Self {
        AssetMap {
            graphql_route: "/graphql".into(),
            subscribe_route: "/graphql/subscribe".into(),
            router_js_route: "/router.js".into(),
            wasm_route: "/app.wasm".into(),
            wasm_disk: "web/app.wasm".into(),
            css_route: "/styles.css".into(),
            css_disk: "web/styles.css".into(),
        }
    }
}

/// HTML 外壳渲染上下文(默认外壳 default_shell 用;自定义 shell 据此拼整页)。
pub struct ShellCtx<'a> {
    pub app_html: &'a str,        // SSR 片段(空 = CSR 空壳)
    pub data: &'a str,            // 注入的脱水数据 JSON
    pub css_href: &'a str,        // <link> href(= assets.css_route)
    pub router_src: &'a str,      // <script src>(= assets.router_js_route)
    // 以下三项注入 window.__rui 供客户端 router.js 读(协议/wasm 入口的单一真相源,贯穿服务端配置)。
    pub wasm_route: &'a str,      // = assets.wasm_route
    pub graphql_route: &'a str,   // = assets.graphql_route
    pub subscribe_route: &'a str, // = assets.subscribe_route
}

/// 宿主配置。所有字段 Default = 当前行为(零配置 serve 字节不变)。
pub struct AppConfig {
    pub bind: (String, u16),                       // 默认 ("127.0.0.1", 8084)
    pub body_limit: usize,                         // 默认 1<<20(std 现也强制到 body,对齐 axum)
    pub assets: AssetMap,                          // 静态资源路由/磁盘
    pub shell: fn(&ShellCtx) -> String,            // 整页 HTML 模板(默认 default_shell)
    pub router_js: std::borrow::Cow<'static, str>, // /router.js 内容(默认内嵌 ROUTER_JS)
}
impl Default for AppConfig {
    fn default() -> Self {
        AppConfig {
            bind: ("127.0.0.1".into(), 8084),
            body_limit: 1 << 20,
            assets: AssetMap::default(),
            shell: default_shell,
            router_js: std::borrow::Cow::Borrowed(ROUTER_JS),
        }
    }
}

static CONFIG: OnceLock<AppConfig> = OnceLock::new();
/// 当前宿主配置(未经 serve_with 设置则返回默认)。
pub(crate) fn config() -> &'static AppConfig {
    CONFIG.get_or_init(AppConfig::default)
}
pub(crate) fn set_config(cfg: AppConfig) {
    // 校验资源路由互不冲突、也不撞协议端点。否则两后端行为分歧:axum 启动 panic(Router 重复路由),
    // std 静默首匹配(if/else 链先中先用)。这里统一在设配置时 loud 失败,两后端一致、尽早暴露。
    let routes = [
        cfg.assets.graphql_route.as_str(),
        cfg.assets.subscribe_route.as_str(),
        cfg.assets.router_js_route.as_str(),
        cfg.assets.wasm_route.as_str(),
        cfg.assets.css_route.as_str(),
    ];
    for i in 0..routes.len() {
        for j in (i + 1)..routes.len() {
            assert!(
                routes[i] != routes[j],
                "rui: AppConfig 入口路由冲突 `{}` —— graphql · subscribe · router_js · wasm · css 须各异",
                routes[i]
            );
        }
    }
    let _ = CONFIG.set(cfg); // 启动设一次(serve_with / serve_axum_with);重复设忽略
}

/// 默认整页骨架。css_href / router_src 进 <link>/<script src>(服务端链接);wasm/graphql/subscribe 路由统一注入
/// `window.__rui` 供客户端 router.js 读 —— 宿主入口(协议 + 资源)单一真相源贯穿两端,不再有客户端硬编码地址。
pub fn default_shell(c: &ShellCtx) -> String {
    // 转义 `\` / `"`(复用 gql::gql_escape)+ `</`(防提前闭合 <script>):防路由串破坏注入的脚本。
    let esc = |s: &str| crate::gql::gql_escape(s).replace("</", "<\\/");
    let rui_cfg = format!(
        "<script>window.__rui={{wasm:\"{}\",graphql:\"{}\",subscribe:\"{}\"}}</script>",
        esc(c.wasm_route),
        esc(c.graphql_route),
        esc(c.subscribe_route),
    );
    format!(
        "<!doctype html><html lang=\"zh\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<title>rui</title><style>rui-slot,rui-frag{{display:contents}}</style>\
<link rel=\"stylesheet\" href=\"{css}\"></head>\
<body><div id=\"app\">{app}</div>\
<script id=\"__rui_data\" type=\"application/json\">{data}</script>{rui_cfg}\
<script type=\"module\" src=\"{router}\"></script></body></html>",
        css = c.css_href,
        app = c.app_html,
        data = c.data,
        router = c.router_src,
    )
}

/// 启动 SSR 服务器(零配置;= serve_with(app, AppConfig::default()))。阻塞,永不返回。
pub fn serve(app: App) {
    serve_with(app, AppConfig::default());
}

/// 启动 SSR 服务器并指定宿主配置(bind / 资源路由 / body 上限 / HTML 外壳 / router.js)。
pub fn serve_with(app: App, cfg: AppConfig) {
    set_config(cfg); // 必须在任何 config() 读取前设置
    set_resolver(app.resolve); // 让同构 SSR 的本地执行用同一个 resolver
    crate::jobs::start_worker_if_configured(); // platform! 声明了 jobs/crons → 起后台 worker 线程
    crate::jobs::start_crons_if_configured(); // platform! 声明了 crons → 起定时调度线程(按间隔 enqueue)
    let c = config();
    let listener = TcpListener::bind((c.bind.0.as_str(), c.bind.1)).expect("bind");
    println!("rui · SSR  →  http://{}:{}", c.bind.0, c.bind.1);
    for stream in listener.incoming() {
        if let Ok(s) = stream {
            thread::spawn(move || handle(s, app)); // App 是 Copy
        }
    }
}

/// per-request 运行时:把四子系统的"当前态"打包,供 with_request_runtime 进入时换新鲜、退出时还原。
/// 非 Send(全是 thread_local 拥有态)—— 只在单线程内 take→run→restore,不跨线程。
struct RequestRuntime {
    reactive: crate::reactive::ReactiveState,
    store: crate::gql::store::StoreState,
    dom: crate::dom::DomNativeState,
    route: crate::runtime::RouteState,
}

// 嵌套深度(仅 debug):顶层渲染进入时才断言"竞技场已空"。嵌套时外层渲染中途可能已建 live effect,正常,不断言。
#[cfg(debug_assertions)]
thread_local! {
    static RT_DEPTH: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// 在一个隔离的、新鲜的 per-request 运行时里执行 f。进入时把四子系统全部 thread_local 换成新鲜空态
/// (取代原"每渲染手动按序 4 次 reset"),退出时(正常或 panic)经 RAII 还原旧态。隔离从"记得按序 reset"
/// 的约定升级为"在 guard 作用域内"的结构性属性,且天然 panic 安全 + 支持嵌套(还原旧态而非清空)。
/// **仅服务端**:wasm 端 app 长生单全局,绝不调用(否则会清掉运行中的 app)。四子系统互不读对方 thread_local,
/// 故 take 顺序无关;但必须**整组**一起换(EFFECTS 的 id 被 store VERSIONS / PATH-QUERY 的订阅表交叉引用,
/// 只换一部分 = 换入的 arena 持有指向另一 arena EFFECTS 的 id → 静默腐蚀)。
fn with_request_runtime<R>(f: impl FnOnce() -> R) -> R {
    // 进入前(仅顶层)上一渲染应已 restore 干净(否则是状态泄漏)——debug 哨兵尽早暴露。
    #[cfg(debug_assertions)]
    debug_assert!(
        RT_DEPTH.with(|d| d.get()) != 0 || crate::reactive::state_is_empty(),
        "进入顶层 with_request_runtime 时反应式竞技场非空:上一渲染未还原(状态泄漏)"
    );
    let saved = RequestRuntime {
        reactive: crate::reactive::take_state(),
        store: crate::gql::store::take_state(),
        dom: crate::dom::take_native_state(),
        route: crate::runtime::take_route_state(),
    };
    struct Restore(Option<RequestRuntime>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let s = self.0.take().unwrap();
            crate::reactive::restore_state(s.reactive);
            crate::gql::store::restore_state(s.store);
            crate::dom::restore_native_state(s.dom);
            crate::runtime::restore_route_state(s.route);
            #[cfg(debug_assertions)]
            RT_DEPTH.with(|d| d.set(d.get() - 1));
        }
    }
    #[cfg(debug_assertions)]
    RT_DEPTH.with(|d| d.set(d.get() + 1));
    let _restore = Restore(Some(saved)); // RAII:f 正常返回或 panic 都还原旧态 + 递减深度
    f()
}

/// 服务端渲染一个 Page,产出 (HTML 片段, 脱水数据)。整个渲染在隔离的 per-request 运行时内完成
/// —— set_path/query 与 dehydrate 都必须在作用域内(退出后 PATH/SSR_RESP 已还原清空,读不到)。
fn render_page(p: crate::view::Page, path: &str, query: &str) -> (String, String) {
    with_request_runtime(|| {
        crate::runtime::set_current_path(path); // path 参数:首屏 param() 读到正确段(须在新鲜 PATH 装入后)
        crate::runtime::set_current_query(query); // query 参数:首屏 query_param() 读到正确值
        let render = p.render;
        let (node, _sc) = crate::reactive::scope(move || render()); // effect 立即跑填好 signal,scope 随后 drop(竞技场仍在)
        crate::dom::mount(node.node());
        let html = crate::dom::take_html();
        let data = crate::dom::dehydrate_responses(); // 必须在作用域内读:退出后 SSR_RESP 已还原清空
        (html, data)
    })
}

/// 读取完整 HTTP 请求:先读到 headers 结束(\r\n\r\n),再按 Content-Length 补齐 body。
/// 处理 TCP 分段与大请求体(单次 read 不保证读全)。
fn read_request(s: &mut TcpStream, body_limit: usize) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    loop {
        match s.read(&mut tmp) {
            Ok(0) | Err(_) => return buf,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
        }
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let cl = content_length(&buf[..p]);
            let body_start = p + 4;
            // body 上限(对齐 axum 的 DefaultBodyLimit):只读到 limit,不为超大 Content-Length 无界分配(防内存 DoS)。
            while buf.len() < body_start + cl && buf.len() < body_start + body_limit {
                match s.read(&mut tmp) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => buf.extend_from_slice(&tmp[..n]),
                }
            }
            return buf;
        }
        if buf.len() > body_limit {
            return buf; // headers 上限保护(同 body_limit)
        }
    }
}

fn content_length(headers: &[u8]) -> usize {
    let h = String::from_utf8_lossy(headers);
    for line in h.lines() {
        if let Some((k, v)) = line.split_once(':') {
            if k.trim().eq_ignore_ascii_case("content-length") {
                return v.trim().parse().unwrap_or(0);
            }
        }
    }
    0
}

fn handle(mut s: TcpStream, app: App) {
    let cfg = config();
    let raw = read_request(&mut s, cfg.body_limit);
    let req = String::from_utf8_lossy(&raw);
    let mut parts = req.split_whitespace();
    let method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("/");
    // 拆成 pathname + query(fragment 浏览器不会发,稳妥起见也去掉):pathname 走路由匹配 / path 参数,
    // query 单独喂 query 参数(两条线)。修了 /todo/1?x=1 的 param 不再变 "1?x=1"、/about?utm=x 不掉 404。
    let no_frag = target.split('#').next().unwrap_or(target);
    let (path, query) = match no_frag.split_once('?') {
        Some((p, q)) => (if p.is_empty() { "/" } else { p }, q),
        None => (if no_frag.is_empty() { "/" } else { no_frag }, ""),
    };

    // ── subscription:SSE 长连接(本线程独占,循环推送)──
    // 精确匹配(== 而非 starts_with):与 axum 的 .route 精确路由一致;路由名来自 cfg(可配置宿主)。
    if path == cfg.assets.subscribe_route {
        if let Some(sse) = app.sse {
            let _ = s.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: keep-alive\r\n\r\n",
            );
            let _ = s.write_all(format!("data: {}\n\n", (sse.snapshot)()).as_bytes());
            let rx = (sse.subscribe)();
            while let Ok(json) = rx.recv() {
                if s.write_all(format!("data: {}\n\n", json).as_bytes()).is_err() {
                    break; // 客户端断开
                }
            }
        }
        return;
    }

    // 包 catch_unwind:GraphQL 执行 / SSR 渲染里的 panic 不再静默丢连接,而是回 500 + 打日志。
    // (resolver 的字段级 panic 已在 exec 内隔离成 errors[];这里兜的是渲染期等其它 panic。)
    // 资源路由 / 磁盘路径 / router.js 内容均来自 cfg(可配置宿主);/graphql 端点固定(协议入口)。
    let built = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> (&str, &str, Vec<u8>) {
        if path == cfg.assets.graphql_route {
            // POST-only(对齐 axum):GET mutation 是 CSRF 向量,非 POST → 405。
            if method != "POST" {
                ("405 Method Not Allowed", "application/json; charset=utf-8", br#"{"data":null,"errors":[{"message":"method not allowed"}]}"#.to_vec())
            } else {
                graphql_response(&raw, cfg.body_limit, app.resolve) // body 从原始字节切 + from_utf8→400 + 超限→413(对齐 axum)
            }
        } else if path == cfg.assets.router_js_route {
            ("200 OK", "text/javascript; charset=utf-8", cfg.router_js.as_bytes().to_vec())
        } else if path == cfg.assets.wasm_route {
            file(&cfg.assets.wasm_disk, "application/wasm")
        } else if path == cfg.assets.css_route {
            file(&cfg.assets.css_disk, "text/css; charset=utf-8")
        } else {
            ("200 OK", "text/html; charset=utf-8", page(app.route, path, query).into_bytes())
        }
    }));
    let (status, ctype, body): (&str, &str, Vec<u8>) = match built {
        Ok(t) => t,
        Err(p) => {
            let msg = p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_else(|| "unknown".to_string());
            eprintln!("rui: 请求处理 panic({path}):{msg}");
            ("500 Internal Server Error", "text/plain; charset=utf-8", b"internal server error".to_vec())
        }
    };

    let header = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = s.write_all(header.as_bytes());
    let _ = s.write_all(&body);
}

fn file(p: &str, ctype: &'static str) -> (&'static str, &'static str, Vec<u8>) {
    match std::fs::read(p) {
        Ok(b) => ("200 OK", ctype, b),
        Err(_) => ("404 Not Found", "text/plain", format!("{p} missing").into_bytes()),
    }
}

// /graphql 响应:body 从**原始字节**首个 \r\n\r\n 后切出(保留体内 CRLF,不被 req.split 截断)→
// 声明超 body_limit → 413(对齐 axum;read_request 已只读到 limit 防 OOM)→ 非法 UTF-8 → 400(对齐 axum)→ 执行。
fn graphql_response(raw: &[u8], body_limit: usize, resolve: Resolver) -> (&'static str, &'static str, Vec<u8>) {
    const JSON: &str = "application/json; charset=utf-8";
    let boundary = match raw.windows(4).position(|w| w == b"\r\n\r\n") {
        Some(p) => p,
        None => return ("400 Bad Request", JSON, br#"{"data":null,"errors":[{"message":"malformed request"}]}"#.to_vec()),
    };
    if content_length(&raw[..boundary]) > body_limit {
        return ("413 Payload Too Large", JSON, br#"{"data":null,"errors":[{"message":"request body too large"}]}"#.to_vec());
    }
    match std::str::from_utf8(&raw[boundary + 4..]) {
        Ok(q) => ("200 OK", JSON, exec::execute(q, resolve).into_bytes()),
        Err(_) => ("400 Bad Request", JSON, br#"{"data":null,"errors":[{"message":"invalid UTF-8 in request body"}]}"#.to_vec()),
    }
}

// 按 #[rui::page] 策略产出整页 HTML:
//   Ssr    渲染 + 注入数据(客户端水合)
//   Csr    只发空壳(#app 为空、无数据)→ 客户端探测到无 SSR 内容,走纯 CSR
//   Static 首次按 Ssr 渲染后按 path 缓存,后续直接复用
pub(crate) fn page(route: fn(&str) -> crate::view::Page, path: &str, query: &str) -> String {
    use crate::view::Strategy;
    // 注:set_current_path/query 已移进 render_page(必须在 with_request_runtime 装入新鲜 PATH 之后设)。
    // route(path) 只匹配模式 + 构造延迟 render 闭包,不读 PATH signal,故在 runtime 作用域外调用是安全的。
    let pg = route(path);
    match pg.strategy {
        Strategy::Csr => doc("", ""), // 空壳:不渲染、无需 runtime
        Strategy::Ssr => ssr_doc(pg, path, query),
        Strategy::Static => {
            static CACHE: OnceLock<std::sync::Mutex<std::collections::HashMap<String, String>>> = OnceLock::new();
            let cache = CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
            // 缓存键含 query(query 依赖型 static 页各参数各缓存一份);参数排序使 ?a=1&b=2 与 ?b=2&a=1 命中同一份。
            let key = if query.is_empty() {
                path.to_string()
            } else {
                let mut parts: Vec<&str> = query.split('&').filter(|s| !s.is_empty()).collect();
                parts.sort_unstable();
                format!("{path}?{}", parts.join("&"))
            };
            if let Some(h) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
                return h.clone();
            }
            let html = ssr_doc(pg, path, query);
            // 上限保护:防任意 query 串(?utm=.. 等)无界增长 / 缓存洪泛;满了就停止缓存(继续按需渲染)。
            let mut map = cache.lock().unwrap_or_else(|e| e.into_inner());
            if map.len() < 1024 {
                map.insert(key, html.clone());
            }
            html
        }
    }
}

// SSR:渲染 + 把本次 query 响应注入页面(客户端首屏复用,免重新联网)。
fn ssr_doc(pg: crate::view::Page, path: &str, query: &str) -> String {
    let (app_html, raw_data) = render_page(pg, path, query); // 渲染 + 在 runtime 作用域内取脱水数据
    // `</` → `<\/` 防止数据里出现 `</script>` 截断标签(JSON 里 \/ 合法)。
    let data = raw_data.replace("</", "<\\/");
    doc(&app_html, &data)
}

// 整页 HTML 骨架(经可配置 shell)。app_html 为空 + data 为空 = CSR 空壳。css/router/wasm 路由来自 AssetMap。
fn doc(app_html: &str, data: &str) -> String {
    let cfg = config();
    (cfg.shell)(&ShellCtx {
        app_html,
        data,
        css_href: &cfg.assets.css_route,
        router_src: &cfg.assets.router_js_route,
        wasm_route: &cfg.assets.wasm_route,
        graphql_route: &cfg.assets.graphql_route,
        subscribe_route: &cfg.assets.subscribe_route,
    })
}

#[cfg(test)]
mod runtime_isolation_tests {
    // #3 完整运行时上下文(swap)的隔离回归。守住:复用线程上请求间隔离(INV-2)、RAII panic 安全、嵌套还原。
    // 这些是 with_request_runtime 相对"裸 thread_local"的真正增量(thread_local 本就跨线程隔离;
    // swap 额外保证**同线程顺序/嵌套隔离** + panic 时也还原)。SSR 压测是端到端补充,这里是 CI 可跑的持久版。
    use super::with_request_runtime;
    use crate::gql::{parse, store};

    #[test]
    fn sequential_renders_isolated_on_same_thread() {
        // INV-2 端到端:同一线程上,请求 B 看不到请求 A 写进 store 的数据。
        let seen_in_b = with_request_runtime(|| {
            store::normalize_list(&parse(r#"[{"__typename":"Todo","__id":"1","text":"req-A"}]"#));
            assert!(store::read_entity("Todo:1").is_some(), "请求 A 内可见自己写入的数据");
            with_request_runtime(|| store::read_entity("Todo:1")) // 紧接着的"下一请求"
        });
        assert!(seen_in_b.is_none(), "请求 B 不得看到请求 A 的 store(swap 隔离,非靠线程死亡)");
    }

    #[test]
    fn panic_during_render_leaves_clean_state_for_next() {
        // 渲染期 panic 后,同线程下一渲染仍是干净竞技场(RAII restore 在 unwind 时也会跑)。
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            with_request_runtime(|| {
                store::normalize_list(&parse(r#"[{"__typename":"Todo","__id":"9","text":"脏"}]"#));
                panic!("模拟渲染期 panic");
            })
        }));
        let leaked = with_request_runtime(|| store::read_entity("Todo:9"));
        assert!(leaked.is_none(), "panic 渲染写入的脏 store 必须被 RAII 还原,不泄漏给下一渲染");
    }

    #[test]
    fn render_panic_with_live_child_scope_effects_does_not_abort() {
        // 回归(review 抓到的 critical):渲染期 panic 时,半建的 reactive_block/keyed_for effect(其闭包持有
        // 一个"带 effect 的子 Scope")若残留 EFFECTS,with_request_runtime 退出 restore 丢弃脏竞技场会触发
        // Scope::drop → dispose_effect 重入 → RefCell 双借 → unwind 中二次 panic → 进程 abort。
        // 修复:scope 的 PanicGuard 在 unwind 时先 dispose 半建 effect(restore 丢弃的已是全 None 竞技场)+
        // restore_state 改"先释放借用再 drop"。本测试应被 catch_unwind 捕获(返回 Err),而非 abort 整个进程。
        use crate::reactive::{effect, scope, Scope};
        use std::cell::RefCell;
        use std::rc::Rc;
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            with_request_runtime(|| {
                scope(|| {
                    let held: Rc<RefCell<Option<Scope>>> = Rc::new(RefCell::new(None));
                    let h = held.clone();
                    effect(move || {
                        let (_, child) = scope(|| {
                            effect(|| {}); // 子作用域内的 effect → 子 Scope.ids 非空 → 其 drop 会重入 dispose_effect
                        });
                        *h.borrow_mut() = Some(child); // effect 闭包持有带 effect 的子 Scope(同 reactive_block/keyed_for)
                    });
                    panic!("模拟渲染期 panic(EFFECTS 里有持子 Scope 的 live effect)");
                });
            })
        }));
        assert!(r.is_err(), "渲染 panic 必须被 catch_unwind 捕获返回 Err,而非进程 abort");
        // 下一渲染:竞技场已干净,正常运行(panic 渲染未泄漏 live effect / 未腐蚀竞技场)。
        let after = with_request_runtime(|| {
            store::normalize_list(&parse(r#"[{"__typename":"Todo","__id":"1","text":"after"}]"#));
            store::read_entity("Todo:1").map(|t| t.field("text").as_str().to_string())
        });
        assert_eq!(after.as_deref(), Some("after"), "panic 渲染后下一渲染应在干净竞技场上正常运行");
    }

    #[test]
    fn nested_runtime_restores_outer() {
        // 嵌套:内层见新鲜态、可写自己的;退出内层后外层状态原样还原(支持多上下文,非破坏)。
        with_request_runtime(|| {
            store::normalize_list(&parse(r#"[{"__typename":"Todo","__id":"7","text":"outer"}]"#));
            let inner = with_request_runtime(|| {
                assert!(store::read_entity("Todo:7").is_none(), "内层不应看到外层数据");
                store::normalize_list(&parse(r#"[{"__typename":"Todo","__id":"7","text":"inner"}]"#));
                store::read_entity("Todo:7").unwrap().field("text").as_str().to_string()
            });
            assert_eq!(inner, "inner");
            assert_eq!(
                store::read_entity("Todo:7").unwrap().field("text").as_str(),
                "outer",
                "退出内层后外层 store 被 RAII 还原为 outer(未被内层污染)"
            );
        });
    }
}

#[cfg(test)]
mod config_tests {
    // #5 可配置宿主:默认 shell 与历史 doc() 字节一致(零配置不回归 + 两后端共用此 shell = parity);自定义生效。
    use super::{default_shell, AppConfig, AssetMap, ShellCtx};

    fn shell_with(a: &AssetMap, app: &str, data: &str) -> String {
        default_shell(&ShellCtx {
            app_html: app,
            data,
            css_href: &a.css_route,
            router_src: &a.router_js_route,
            wasm_route: &a.wasm_route,
            graphql_route: &a.graphql_route,
            subscribe_route: &a.subscribe_route,
        })
    }

    #[test]
    fn default_shell_injects_host_config() {
        // 默认 shell:静态骨架不变 + 统一注入 window.__rui(协议/wasm 入口的客户端真相源,贯穿服务端配置)。
        let html = shell_with(&AssetMap::default(), "<p>x</p>", "{\"q\":1}");
        let expected = "<!doctype html><html lang=\"zh\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<title>rui</title><style>rui-slot,rui-frag{display:contents}</style>\
<link rel=\"stylesheet\" href=\"/styles.css\"></head>\
<body><div id=\"app\"><p>x</p></div>\
<script id=\"__rui_data\" type=\"application/json\">{\"q\":1}</script>\
<script>window.__rui={wasm:\"/app.wasm\",graphql:\"/graphql\",subscribe:\"/graphql/subscribe\"}</script>\
<script type=\"module\" src=\"/router.js\"></script></body></html>";
        assert_eq!(html, expected, "默认 shell(含 window.__rui 注入)不符");
    }

    #[test]
    fn graphql_response_body_handling() {
        // std /graphql body 处理(对齐 axum):body 从原始字节切(保体内 CRLF)+ from_utf8→400 + 超限→413。
        use super::graphql_response;
        use crate::gql::exec::empty_resolver;
        let req = |cl: usize, body: &[u8]| {
            let mut v = format!("POST /graphql HTTP/1.1\r\nContent-Length: {cl}\r\n\r\n").into_bytes();
            v.extend_from_slice(body);
            v
        };
        // 正常:200
        let (st, _, _) = graphql_response(&req(16, b"{ todos { id } }"), 1 << 20, empty_resolver);
        assert_eq!(st, "200 OK");
        // 非法 UTF-8 → 400(而非 lossy 静默喂解析器)
        let (st, _, _) = graphql_response(&req(5, b"{ \xff }"), 1 << 20, empty_resolver);
        assert_eq!(st, "400 Bad Request");
        // 声明超 body_limit → 413(对齐 axum)
        let (st, _, _) = graphql_response(&req(999999, b"{}"), 100, empty_resolver);
        assert_eq!(st, "413 Payload Too Large");
        // 体内含 \r\n\r\n 不被截断(从原始字节首个边界后整体取):查询含空行仍完整执行(返 200,非截断报错)
        let (st, _, _) = graphql_response(&req(20, b"{ a }\r\n\r\n{ b }"), 1 << 20, empty_resolver);
        assert_eq!(st, "200 OK");
    }

    #[test]
    fn custom_config_honored() {
        // 自定义入口(含挂前缀场景:/app1/...)→ 服务端链接 + 客户端 window.__rui 都反映,单一真相源贯穿两端。
        let a = AssetMap {
            css_route: "/app1/styles.css".into(),
            router_js_route: "/app1/router.js".into(),
            wasm_route: "/app1/app.wasm".into(),
            graphql_route: "/app1/graphql".into(),
            subscribe_route: "/app1/graphql/subscribe".into(),
            ..AssetMap::default()
        };
        let html = shell_with(&a, "", "");
        assert!(html.contains("href=\"/app1/styles.css\""), "自定义 css 路由应进 <link>");
        assert!(html.contains("src=\"/app1/router.js\""), "自定义 router 路由应进 <script src>");
        // 协议入口打穿到客户端:window.__rui 三项都用自定义路由(router.js 据此请求,不再去根路径)
        assert!(
            html.contains("window.__rui={wasm:\"/app1/app.wasm\",graphql:\"/app1/graphql\",subscribe:\"/app1/graphql/subscribe\"}"),
            "自定义协议/wasm 入口应注入 window.__rui 供客户端读"
        );
        // 默认字面量回归(bind / body_limit)
        let d = AppConfig::default();
        assert_eq!(d.bind, ("127.0.0.1".to_string(), 8084));
        assert_eq!(d.body_limit, 1 << 20);
    }
}

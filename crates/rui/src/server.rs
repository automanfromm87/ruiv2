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
const ROUTER_JS: &str = include_str!("assets/router.js");

// ── 同构本地执行(native 端 dom::gql 用)──

static RESOLVE: OnceLock<Resolver> = OnceLock::new();

/// 注册应用的 GraphQL resolver(serve 启动时调用一次)。
pub fn set_resolver(r: Resolver) {
    let _ = RESOLVE.set(r);
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

/// 一个 rui 应用:路由 + GraphQL resolver +(可选)订阅。
#[derive(Clone, Copy)]
pub struct App {
    /// 路径 → Page(策略 + 延迟渲染);由 `#[rui::page]` + route() 提供。
    pub route: fn(&str) -> crate::view::Page,
    /// GraphQL resolver(应用用一个 match 聚合 #[gql_root] 生成的各 Root::resolve)。
    pub resolve: Resolver,
    /// 可选订阅(subscription)。
    pub sse: Option<Sse>,
}

/// 启动 SSR 服务器(阻塞,永不返回)。
pub fn serve(app: App) {
    set_resolver(app.resolve); // 让同构 SSR 的本地执行用同一个 resolver
    let listener = TcpListener::bind(("127.0.0.1", 8084)).expect("bind 8084");
    println!("rui · SSR  →  http://127.0.0.1:8084");
    for stream in listener.incoming() {
        if let Ok(s) = stream {
            thread::spawn(move || handle(s, app)); // App 是 Copy
        }
    }
}

/// 服务端渲染一个 Page 的 render 闭包,产出 HTML 片段(同构,query! 本地预取带数据)。
fn render_page(p: crate::view::Page) -> String {
    crate::dom::reset();
    crate::gql::store::reset(); // 清空规范化缓存,保证请求间隔离
    let render = p.render;
    let (node, _sc) = crate::reactive::scope(move || render()); // SSR 一次性:effect 立即跑填好 signal,scope 随后 drop
    crate::dom::mount(node.node());
    crate::dom::take_html()
}

/// 读取完整 HTTP 请求:先读到 headers 结束(\r\n\r\n),再按 Content-Length 补齐 body。
/// 处理 TCP 分段与大请求体(单次 read 不保证读全)。
fn read_request(s: &mut TcpStream) -> Vec<u8> {
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
            while buf.len() < body_start + cl {
                match s.read(&mut tmp) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => buf.extend_from_slice(&tmp[..n]),
                }
            }
            return buf;
        }
        if buf.len() > 1 << 20 {
            return buf; // 1MB headers 上限保护
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
    let raw = read_request(&mut s);
    let req = String::from_utf8_lossy(&raw);
    let target = req.split_whitespace().nth(1).unwrap_or("/");
    // 拆成 pathname + query(fragment 浏览器不会发,稳妥起见也去掉):pathname 走路由匹配 / path 参数,
    // query 单独喂 query 参数(两条线)。修了 /todo/1?x=1 的 param 不再变 "1?x=1"、/about?utm=x 不掉 404。
    let no_frag = target.split('#').next().unwrap_or(target);
    let (path, query) = match no_frag.split_once('?') {
        Some((p, q)) => (if p.is_empty() { "/" } else { p }, q),
        None => (if no_frag.is_empty() { "/" } else { no_frag }, ""),
    };

    // ── subscription:SSE 长连接(本线程独占,循环推送)──
    if path.starts_with("/graphql/subscribe") {
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

    let (status, ctype, body): (&str, &str, Vec<u8>) = if path == "/graphql" {
        let q = req.split("\r\n\r\n").nth(1).unwrap_or("");
        ("200 OK", "application/json; charset=utf-8", exec::execute(q, app.resolve).into_bytes())
    } else if path == "/router.js" {
        ("200 OK", "text/javascript; charset=utf-8", ROUTER_JS.as_bytes().to_vec())
    } else if path == "/app.wasm" {
        file("web/app.wasm", "application/wasm")
    } else if path == "/styles.css" {
        file("web/styles.css", "text/css; charset=utf-8")
    } else {
        ("200 OK", "text/html; charset=utf-8", page(app.route, path, query).into_bytes())
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

// 按 #[rui::page] 策略产出整页 HTML:
//   Ssr    渲染 + 注入数据(客户端水合)
//   Csr    只发空壳(#app 为空、无数据)→ 客户端探测到无 SSR 内容,走纯 CSR
//   Static 首次按 Ssr 渲染后按 path 缓存,后续直接复用
fn page(route: fn(&str) -> crate::view::Page, path: &str, query: &str) -> String {
    use crate::view::Strategy;
    crate::runtime::set_current_path(path); // path 参数:让首屏 param() 读到正确段
    crate::runtime::set_current_query(query); // query 参数:让首屏 query_param() 读到正确值
    let pg = route(path);
    match pg.strategy {
        Strategy::Csr => doc("", ""),
        Strategy::Ssr => ssr_doc(pg),
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
            if let Some(h) = cache.lock().unwrap().get(&key) {
                return h.clone();
            }
            let html = ssr_doc(pg);
            // 上限保护:防任意 query 串(?utm=.. 等)无界增长 / 缓存洪泛;满了就停止缓存(继续按需渲染)。
            let mut map = cache.lock().unwrap();
            if map.len() < 1024 {
                map.insert(key, html.clone());
            }
            html
        }
    }
}

// SSR:渲染 + 把本次 query 响应注入页面(客户端首屏复用,免重新联网)。
fn ssr_doc(pg: crate::view::Page) -> String {
    let app_html = render_page(pg); // 渲染时填充 SSR 响应缓存
    // `</` → `<\/` 防止数据里出现 `</script>` 截断标签(JSON 里 \/ 合法)。
    let data = crate::dom::dehydrate_responses().replace("</", "<\\/");
    doc(&app_html, &data)
}

// 整页 HTML 骨架。app_html 为空 + data 为空 = CSR 空壳。
fn doc(app_html: &str, data: &str) -> String {
    format!(
        "<!doctype html><html lang=\"zh\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<title>rui</title><style>rui-slot,rui-frag{{display:contents}}</style>\
<link rel=\"stylesheet\" href=\"/styles.css\"></head>\
<body><div id=\"app\">{app_html}</div>\
<script id=\"__rui_data\" type=\"application/json\">{data}</script>\
<script type=\"module\" src=\"/router.js\"></script></body></html>"
    )
}

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
    /// 路径 → 页面根节点(应用提供;内部用 view! 组装 layout + page)。
    pub route: fn(&str) -> u32,
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

/// 服务端渲染:按路径渲染对应页,产出 HTML 片段(同构,query! 本地预取带数据)。
pub fn render_to_string(route: fn(&str) -> u32, path: &str) -> String {
    crate::dom::reset();
    crate::gql::store::reset(); // 清空规范化缓存,保证请求间隔离
    crate::runtime::render_path(route, path);
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
    let path = req.split_whitespace().nth(1).unwrap_or("/");

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
        ("200 OK", "text/html; charset=utf-8", page(app.route, path).into_bytes())
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

fn page(route: fn(&str) -> u32, path: &str) -> String {
    let app_html = render_to_string(route, path);
    format!(
        "<!doctype html><html lang=\"zh\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<title>rui</title><link rel=\"stylesheet\" href=\"/styles.css\"></head>\
<body><div id=\"app\">{app_html}</div>\
<script type=\"module\" src=\"/router.js\"></script></body></html>"
    )
}

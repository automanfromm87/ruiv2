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
    crate::gql::set_transport(local_execute); // 依赖倒置:把 SSR 预取 transport 注入 gql(dom 经 gql::fetch 调它,不 NAME server)
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

    // 包 catch_unwind:GraphQL 执行 / SSR 渲染里的 panic 不再静默丢连接,而是回 500 + 打日志。
    // (resolver 的字段级 panic 已在 exec 内隔离成 errors[];这里兜的是渲染期等其它 panic。)
    let built = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> (&str, &str, Vec<u8>) {
        if path == "/graphql" {
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

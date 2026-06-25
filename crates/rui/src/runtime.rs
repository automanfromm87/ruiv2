//! 同构运行时:页面渲染(scope + mount + 切页 dispose)+ wasm 入口辅助。
//! `client!` 宏在应用 crate 里展开成 wasm 导出(alloc/render_route/dispatch/on_fetch),内部转调这里。

use crate::dom;
use crate::reactive::{memo, scope, untrack, Scope, Signal};
use std::cell::{Cell, RefCell};

thread_local! {
    // 当前页的响应式作用域;切页时先 dispose(销毁上一页 query memo,防泄漏 + 幽灵重算)。
    static PAGE_SCOPE: RefCell<Option<Scope>> = const { RefCell::new(None) };
    // 当前页 key(= 页面 module_path):导航时据此判断"同页换参数"还是"换页"。
    static CUR_KEY: RefCell<Option<String>> = const { RefCell::new(None) };
    // 当前 location.pathname —— path 参数的真相源。包在 RefCell 里以便服务端 per-request swap
    //(with_request_runtime 进入时换新鲜 Signal、退出时还原 → 取代原 reset_route_signals 清订阅表)。
    static PATH: RefCell<Signal<String>> = RefCell::new(Signal::new(String::new()));
    // 当前 location.search 的原始 query 串(去掉前导 '?'),如 "q=foo&sort=asc" —— query 参数的真相源。
    // 与 PATH 完全独立:不参与路由匹配,只供 query_param 派生(path / query 各一条线,互不掺和)。
    static QUERY: RefCell<Signal<String>> = RefCell::new(Signal::new(String::new()));
}

/// 当前路径 signal(路由参数从它派生)。
pub fn path() -> Signal<String> {
    PATH.with(|p| p.borrow().clone())
}
// 路由组叶子的 param 偏移:页内相对段索引 + 组前缀段数 = 绝对段索引。顶层页偏移 0(相对=绝对,行为不变)。
// 组 outlet 用 with_param_offset(前缀段数, ..) 包住叶子 render,使组内页的 `:param`(如 group("/dash"){page("/item/:id")})
// 读到绝对段。仅在 param/param_as 建 memo 时被读一次(memo 把绝对索引固化),故无需纳入 per-request swap;RAII 复位 panic 安全。
thread_local! {
    static PARAM_OFFSET: Cell<usize> = const { Cell::new(0) };
}
/// 在 param 偏移 = n 的上下文里运行 f(组路由叶子用,n = 组前缀段数)。RAII 复位(正常或 panic 都还原前值)。
pub fn with_param_offset<R>(n: usize, f: impl FnOnce() -> R) -> R {
    struct Reset(usize);
    impl Drop for Reset {
        fn drop(&mut self) {
            PARAM_OFFSET.with(|o| o.set(self.0));
        }
    }
    let _reset = Reset(PARAM_OFFSET.with(|o| o.replace(n)));
    f()
}

/// 第 i 个路径段(0 基,按 '/' 切、忽略空段;再叠加当前组前缀偏移)的 reactive 视图。`/todo/1` 的 `param(1)` = "1"。
/// 同页导航换参数时它会变 → 订阅它的 `resource!` 自动重取,无需整页重建。
pub fn param(i: usize) -> Signal<String> {
    let p = path();
    let idx = PARAM_OFFSET.with(|o| o.get()) + i; // 固化绝对索引(组叶子 = 前缀段数 + i;顶层 = i)
    memo(move || p.get().split('/').filter(|s| !s.is_empty()).nth(idx).unwrap_or("").to_string())
}
/// 类型化的第 i 段:解析成 `T`(失败回退 `T::default()`),其余同 `param`。
/// `#[rui::page("/todo/:id")] fn view(id: Signal<i64>)` 由宏据模式串接到对应 `param_as`。
pub fn param_as<T>(i: usize) -> Signal<T>
where
    T: std::str::FromStr + Default + Clone + PartialEq + 'static,
{
    let p = path();
    let idx = PARAM_OFFSET.with(|o| o.get()) + i;
    memo(move || {
        p.get()
            .split('/')
            .filter(|s| !s.is_empty())
            .nth(idx)
            .and_then(|s| s.parse::<T>().ok())
            .unwrap_or_default()
    })
}
/// 路由模式匹配:`/todo/:id` 匹配 `/todo/1`(段数相等,`:name` 段通配,其余字面相等)。
/// `router!` 用页面声明的 `PATTERN` 据此分发;模式 `/` / "" 匹配根路径。只看 pathname,不涉 query。
pub fn matches(pattern: &str, path: &str) -> bool {
    let ps: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
    let xs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    ps.len() == xs.len() && ps.iter().zip(&xs).all(|(p, x)| p.starts_with(':') || p == x)
}

// ── query 参数(与 path 同机制、独立一条线:memo 派生 → signal → 喂 resource!)──

/// 当前 query 串 signal(原始,如 "q=foo&sort=asc")。
pub fn query_string() -> Signal<String> {
    QUERY.with(|q| q.borrow().clone())
}
/// 命名 query 参数的 reactive 视图:`?q=foo` 的 `query_param("q")` = "foo"(缺省 "";值经 percent/`+` 解码)。
/// 同页换 query(`?q=a`→`?q=b`)时它变 → 订阅它的 `resource!` 自动重取,无需重建。
pub fn query_param(key: &str) -> Signal<String> {
    let qs = query_string();
    let key = key.to_string();
    memo(move || lookup_query(&qs.get(), &key).map(|v| pct_decode(&v)).unwrap_or_default())
}
/// 类型化命名 query 参数:解码后解析成 `T`(缺省 / 解析失败回退 `T::default()`)。
pub fn query_param_as<T>(key: &str) -> Signal<T>
where
    T: std::str::FromStr + Default + Clone + PartialEq + 'static,
{
    let qs = query_string();
    let key = key.to_string();
    memo(move || {
        lookup_query(&qs.get(), &key).and_then(|v| pct_decode(&v).parse::<T>().ok()).unwrap_or_default()
    })
}
// 在原始 query 串里找 key 的(未解码)值;key 也按解码后比较(键一般是纯 ascii,但稳妥起见)。
fn lookup_query(qs: &str, key: &str) -> Option<String> {
    qs.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        if pct_decode(k) == key {
            Some(v.to_string())
        } else {
            None
        }
    })
}
/// percent + `+` 解码(query 值用):`hello%20world` / `hello+world` → `hello world`。
fn pct_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < b.len() => {
                let hex = |c: u8| (c as char).to_digit(16);
                match (hex(b[i + 1]), hex(b[i + 2])) {
                    (Some(h), Some(l)) => {
                        out.push((h * 16 + l) as u8);
                        i += 3;
                    }
                    _ => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}
/// percent 编码一个 query 值(写 URL 用,与 `query_param` 的解码对称):应用层拼 `?q=` 时调用。
pub fn query_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}
fn set_path(path: &str) {
    // 值不变就不写:Signal/memo 都不做 PartialEq 去抖,否则导航到同一 URL 会让
    // param memo 重算并重通知 → resource! 冗余重取。这里挡住"同路径"的多余刷新。
    PATH.with(|p| {
        let p = p.borrow();
        if untrack(|| p.get()) != path {
            p.set(path.to_string());
        }
    });
}
fn set_query(query: &str) {
    QUERY.with(|q| {
        let q = q.borrow();
        if untrack(|| q.get()) != query {
            q.set(query.to_string());
        }
    });
}
/// SSR:服务端渲染前设置当前路径(让首屏 `param()` 读到正确值)。
pub fn set_current_path(path: &str) {
    set_path(path);
}
/// SSR:服务端渲染前设置当前 query 串(让首屏 `query_param()` 读到正确值)。
pub fn set_current_query(query: &str) {
    set_query(query);
}

// ── per-request 运行时隔离(服务端):路由 signal 的 take/restore,配合 with_request_runtime ──
// 只含 PATH/QUERY(path 参数真相源)。PAGE_SCOPE/CUR_KEY 是客户端导航态,服务端从不填充 → 不纳入。
// swap 换入新鲜 Signal(订阅表天然为空)→ 原 reset_route_signals 的"清订阅表"被结构性地取代。

/// 路由 signal 的一次性快照(PATH + QUERY)。
#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct RouteState {
    path: Signal<String>,
    query: Signal<String>,
}

/// 取出当前 PATH/QUERY(就地换入新鲜空 Signal),返回旧态供 restore。
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn take_route_state() -> RouteState {
    RouteState {
        path: PATH.with(|p| std::mem::replace(&mut *p.borrow_mut(), Signal::new(String::new()))),
        query: QUERY.with(|q| std::mem::replace(&mut *q.borrow_mut(), Signal::new(String::new()))),
    }
}

/// 把旧的 PATH/QUERY Signal 写回(丢弃当前请求的脏 Signal)。
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn restore_route_state(s: RouteState) {
    PATH.with(|p| *p.borrow_mut() = s.path);
    QUERY.with(|q| *q.borrow_mut() = s.query);
}

// ── 生命周期:on_mount(节点入 DOM 后执行,仅客户端)──
// 在 render / 事件 / fetch / 定时器期间排队,等本次同步重建跑完(节点已进 DOM)统一 flush。
// 服务端无 DOM → on_mount 整体 no-op。flush 在所有 wasm 入口尾部调用,故动态子树(<Show>/<For>
// 在事件里重建)的 on_mount 也会及时跑,而不是漏掉或留到下次导航。
#[cfg(target_arch = "wasm32")]
thread_local! {
    // (context 快照, 回调):快照在注册期(祖先 context 还在栈上)捕获,flush 时回放,
    // 使 on_mount 里的 use_context / reactive_block 也能看见祖先 context(与 reactive_block/keyed_for 一致)。
    static MOUNT_QUEUE: RefCell<Vec<(crate::reactive::ContextSnapshot, Box<dyn FnOnce()>)>> = const { RefCell::new(Vec::new()) };
    static NAV_GEN: std::cell::Cell<u64> = const { std::cell::Cell::new(0) }; // 导航代际:flush 中途换页则丢弃旧批
}
/// 注册一个挂载回调:本次渲染的节点进入 DOM 后执行(聚焦 / 启动定时器 / 初始化第三方库)。
/// 仅客户端;服务端无 DOM,直接丢弃(SSR 不跑命令式副作用)。
#[cfg(target_arch = "wasm32")]
pub fn on_mount(f: impl FnOnce() + 'static) {
    let ctx = crate::reactive::capture_contexts(); // 注册期祖先 context 仍在栈 → 快照
    MOUNT_QUEUE.with(|q| q.borrow_mut().push((ctx, Box::new(f))));
}
#[cfg(not(target_arch = "wasm32"))]
pub fn on_mount(_f: impl FnOnce() + 'static) {}
/// 跑队列里的挂载回调。每个回调在其 context 快照 + 子 scope 内执行、产物并入 PAGE_SCOPE(其内创建的
/// effect/memo 归当前页 → 切页时一并销毁,不泄漏);代际变化(回调里同步导航)则中止剩余旧批。
/// client! 在 dispatch / on_fetch / run_interval 尾部也调它,故 wasm 各入口的动态重建都能挂载。
#[cfg(target_arch = "wasm32")]
pub fn flush_mounts() {
    let gen = NAV_GEN.with(|g| g.get());
    loop {
        let fns: Vec<(crate::reactive::ContextSnapshot, Box<dyn FnOnce()>)> =
            MOUNT_QUEUE.with(|q| std::mem::take(&mut *q.borrow_mut()));
        if fns.is_empty() {
            break;
        }
        for (ctx, f) in fns {
            if NAV_GEN.with(|g| g.get()) != gen {
                return; // 回调里换页了 → 剩余回调属于已 dispose 的旧页,丢弃
            }
            // 回放祖先 context,再在其上压子 scope 执行回调(use_context / reactive_block 可见祖先)。
            let (_, mut child) = crate::reactive::with_contexts(&ctx, || scope(move || f()));
            let (ids, cleanups) = child.take_parts();
            PAGE_SCOPE.with(|s| {
                if let Some(p) = s.borrow_mut().as_mut() {
                    p.absorb_parts(ids, cleanups);
                }
            });
        }
    }
}
#[cfg(not(target_arch = "wasm32"))]
pub fn flush_mounts() {}
#[cfg(target_arch = "wasm32")]
fn bump_nav_gen() {
    NAV_GEN.with(|g| g.set(g.get() + 1));
}
#[cfg(not(target_arch = "wasm32"))]
fn bump_nav_gen() {}

/// 把整条 URL 拆成 (pathname, query):去掉 `#fragment`,按第一个 `?` 切。
fn split_url(full: &str) -> (&str, &str) {
    let no_frag = full.split('#').next().unwrap_or(full);
    no_frag.split_once('?').unwrap_or((no_frag, ""))
}

/// 首屏 / 全量渲染:dispose 上一页 → 设路径/query → 在新 scope 渲 route 的根节点 → mount。
/// (先 dispose 再写 signal:断开旧页 memo 对 PATH/QUERY 的订阅,避免触发即将销毁页面的幽灵重算。)
pub fn render_path(route: fn(&str) -> crate::view::Page, full: &str) {
    let (path, query) = split_url(full);
    PAGE_SCOPE.with(|s| {
        if let Some(sc) = s.borrow_mut().take() {
            sc.dispose();
        }
    });
    set_path(path);
    set_query(query);
    let p = route(path); // 路由匹配只看 pathname
    CUR_KEY.with(|k| *k.borrow_mut() = Some(p.key.clone()));
    let render = p.render; // 客户端忽略 strategy(由 JS 探测水合/CSR);只渲
    bump_nav_gen();
    let (node, sc) = scope(move || render());
    dom::mount(node.node());
    PAGE_SCOPE.with(|s| *s.borrow_mut() = Some(sc));
    flush_mounts(); // 节点已入 DOM → 跑 on_mount(聚焦 / 定时器 / 第三方初始化)
}

/// SPA 导航:同页(key 相同)→ 只更新 path/query signal,页面留着(`param()`/`query_param()` 变 →
/// `resource!` 重取,无闪烁);换页(key 不同)→ 先 dispose 旧页(断订阅,避免幽灵重算)再写 signal + 清空 #app + 重渲。
pub fn navigate(route: fn(&str) -> crate::view::Page, full: &str) {
    let (path, query) = split_url(full);
    let p = route(path); // 只构造 Page(view 闭包延迟执行),不跑页面体
    let same = CUR_KEY.with(|k| k.borrow().as_deref() == Some(p.key.as_str()));
    if same {
        // 同页:页面留着,只更新 signal → 存活的 param/query_param memo 重算 → resource! 重取。
        // 路由组同组导航(/dash→/dash/settings)也走这里:set_path 同步触发 outlet(reactive_block 订阅 path)
        // 重建新叶子,其 on_mount 入队 → 必须 flush,否则叶子的 on_mount(聚焦/定时器/第三方初始化)
        // 永不在导航时跑,会留到下次 dispatch/on_fetch 才误触发。bump_nav_gen 先栅栏旧批。
        bump_nav_gen();
        set_path(path);
        set_query(query);
        flush_mounts();
        return;
    }
    // 换页:先 dispose 旧页,再写 signal(此刻旧页 memo 已断订阅、新页未建 → 不触发任何幽灵重算)。
    PAGE_SCOPE.with(|s| {
        if let Some(sc) = s.borrow_mut().take() {
            sc.dispose();
        }
    });
    set_path(path);
    set_query(query);
    dom::clear_app();
    dom::clear_handlers(); // 旧 DOM 已抹掉 → 回收其事件处理器(否则换页时 HANDLERS 无界增长)
    CUR_KEY.with(|k| *k.borrow_mut() = Some(p.key.clone()));
    let render = p.render;
    bump_nav_gen();
    let (node, sc) = scope(move || render());
    dom::mount(node.node());
    PAGE_SCOPE.with(|s| *s.borrow_mut() = Some(sc));
    flush_mounts(); // 新页节点已入 DOM → 跑 on_mount
}

/// 程序化导航(应用 Rust 代码调用,如输入框提交去搜索):pushState 进历史 + 走 navigate。
/// JS 拦截的 <a>/popstate 已自带 pushState,故它们直接调 `navigate`;只有代码主动导航才用 `go`。
pub fn go(route: fn(&str) -> crate::view::Page, full: &str) {
    dom::push_url(full); // 更新浏览器地址栏 + 历史(可分享 / 可后退)
    navigate(route, full);
}

// ── wasm 入口辅助(由 client! 宏包成 #[no_mangle] 导出)──

/// JS 在 wasm 内存里分配缓冲区,用来把路径 / JSON 字符串传进来。
pub fn alloc(len: usize) -> *mut u8 {
    let mut v = vec![0u8; len];
    let p = v.as_mut_ptr();
    core::mem::forget(v);
    p
}

/// JS 写好路径后调用:渲染对应页。
///
/// # Safety
/// `ptr`/`len` 必须来自上面的 `alloc`(由 JS 侧保证)。
pub unsafe fn render_route(ptr: *mut u8, len: usize, route: fn(&str) -> crate::view::Page) {
    let path = String::from_utf8_lossy(&Vec::from_raw_parts(ptr, len, len)).into_owned();
    render_path(route, &path);
}

/// SPA 导航(JS 拦截 <a>/popstate 后调用):同页换参数不重建。
///
/// # Safety
/// `ptr`/`len` 必须来自 `alloc`。
pub unsafe fn navigate_route(ptr: *mut u8, len: usize, route: fn(&str) -> crate::view::Page) {
    let path = String::from_utf8_lossy(&Vec::from_raw_parts(ptr, len, len)).into_owned();
    navigate(route, &path);
}

/// 事件触发时由 JS 调用,附带 payload(通常是 target.value;无值则空串)。
///
/// # Safety
/// `ptr`/`len` 必须来自 `alloc`。
pub unsafe fn dispatch(id: u32, ptr: *mut u8, len: usize) {
    let value = String::from_utf8_lossy(&Vec::from_raw_parts(ptr, len, len)).into_owned();
    // 自动 batch:一个事件处理器里的多次 set 合并成一次 flush(下游 memo/effect/视图只重算一次)。
    crate::reactive::batch(|| dom::run_handler(id, &value));
    flush_mounts(); // 事件里 <Show> 打开 / <For> 新增行等动态子树的 on_mount,这里跑
}

/// 首屏:JS 把 SSR 注入的「查询串 → 响应」JSON 灌进客户端缓存(在 render_route 之前调一次)。
///
/// # Safety
/// `ptr`/`len` 必须来自 `alloc`。
pub unsafe fn hydrate_data(ptr: *mut u8, len: usize) {
    let json = String::from_utf8_lossy(&Vec::from_raw_parts(ptr, len, len)).into_owned();
    dom::seed_responses(&json);
}

/// fetch 完成时由 JS 调用。
///
/// # Safety
/// `ptr`/`len` 必须来自 `alloc`。
pub unsafe fn on_fetch(id: u32, ptr: *mut u8, len: usize) {
    let text = String::from_utf8_lossy(&Vec::from_raw_parts(ptr, len, len)).into_owned();
    // 自动 batch(与 dispatch 一致):一个 fetch 响应里的多次 store 写入(乐观回滚 restore +
    // normalize_list,或多实体 merge_all 的逐 entity bump)合并成一次 flush。否则:
    //   · 乐观 mutation 回滚→真值会闪一帧旧值(restore 先 flush 到回滚态,再 flush 到服务端态);
    //   · 多实体查询会按 entity 数重复 flush(每个 bump 一次)。
    crate::reactive::batch(|| dom::run_fetch(id, &text));
    flush_mounts(); // resource!/query! 结果到达后构建的行(keyed <For>)的 on_mount,这里跑
}

/// 安装 panic hook:wasm panic 默认会静默 trap → 白屏无任何提示。装上后 panic 的消息 + 源码位置
/// 会先打到浏览器 console.error,再 abort —— 至少不"静默崩溃",便于排查。由 client! 的 `init` 启动时调一次。
pub fn set_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        crate::dom::console_error(&format!("rui panic: {info}"));
    }));
}

/// 在应用 crate 里生成 wasm 客户端入口(导出 alloc/render_route/dispatch/on_fetch)。
/// 用法(应用 lib.rs):`rui::client!(crate::route);`
#[macro_export]
macro_rules! client {
    ($route:path) => {
        // 这些 #[no_mangle] extern "C" 导出只对 wasm 目标有意义。
        // 必须 cfg 门控到 wasm32,否则 native 构建也会发出 `alloc`/`dispatch` 等
        // 通用全局符号,在 cdylib / 与 libc 链接时有冲突风险。
        #[cfg(target_arch = "wasm32")]
        #[no_mangle]
        pub extern "C" fn alloc(len: usize) -> *mut u8 {
            $crate::runtime::alloc(len)
        }
        // 启动初始化(router.js 实例化后、渲染前调一次):装 panic hook 防静默白屏。
        #[cfg(target_arch = "wasm32")]
        #[no_mangle]
        pub extern "C" fn init() {
            $crate::runtime::set_panic_hook();
        }
        #[cfg(target_arch = "wasm32")]
        #[no_mangle]
        pub extern "C" fn render_route(ptr: *mut u8, len: usize) {
            unsafe { $crate::runtime::render_route(ptr, len, $route) }
        }
        #[cfg(target_arch = "wasm32")]
        #[no_mangle]
        pub extern "C" fn navigate(ptr: *mut u8, len: usize) {
            unsafe { $crate::runtime::navigate_route(ptr, len, $route) }
        }
        #[cfg(target_arch = "wasm32")]
        #[no_mangle]
        pub extern "C" fn dispatch(id: u32, ptr: *mut u8, len: usize) {
            unsafe { $crate::runtime::dispatch(id, ptr, len) }
        }
        #[cfg(target_arch = "wasm32")]
        #[no_mangle]
        pub extern "C" fn on_fetch(id: u32, ptr: *mut u8, len: usize) {
            unsafe { $crate::runtime::on_fetch(id, ptr, len) }
        }
        #[cfg(target_arch = "wasm32")]
        #[no_mangle]
        pub extern "C" fn hydrate_data(ptr: *mut u8, len: usize) {
            unsafe { $crate::runtime::hydrate_data(ptr, len) }
        }
        #[cfg(target_arch = "wasm32")]
        #[no_mangle]
        pub extern "C" fn set_hydrate(on: u32) {
            $crate::dom::set_hydrate(on != 0)
        }
        #[cfg(target_arch = "wasm32")]
        #[no_mangle]
        pub extern "C" fn run_interval(hid: u32) {
            $crate::dom::run_interval(hid);
            $crate::runtime::flush_mounts(); // 定时器里动态重建的 on_mount
        }
        #[cfg(target_arch = "wasm32")]
        #[no_mangle]
        pub extern "C" fn run_oneshot(id: u32) {
            $crate::dom::run_oneshot(id); // 过渡:出场动画结束后移除节点
        }
    };
}

#[cfg(test)]
mod tests {
    use super::{matches, param_as, set_current_path, with_param_offset};

    #[test]
    fn group_param_absolute_offset() {
        // #4 组内 :param:with_param_offset(组前缀段数) 让组内页的 param_as(相对索引) 读到绝对段。
        // 路径 /dash/item/5 段:[dash, item, 5](idx 0/1/2)。group("/dash") 前缀 1 段。
        set_current_path("/dash/item/5");
        // 顶层(偏移 0):param_as(1) 读相对索引 1 = "item"
        assert_eq!(param_as::<String>(1).get(), "item");
        // 组内(偏移 1):组成员 #[page("/item/:id")] 烘焙 :id 相对索引 1 → 绝对索引 2 = "5"
        let id = with_param_offset(1, || param_as::<i64>(1));
        assert_eq!(id.get(), 5, "组内页 :param 应读到绝对段(前缀段数 + 相对索引)");
        // 偏移已 RAII 复位:此后顶层 param_as 不受影响
        assert_eq!(param_as::<String>(1).get(), "item");
        set_current_path("/"); // 复位,避免影响其它测试
    }

    #[test]
    fn inv1_route_identity_param_vs_crosspage() {
        // INV-1(同页导航不重建)的路由半边:/todo/1 与 /todo/2 都匹配同一模式 "/todo/:id"
        // → 解析到同一页 → CUR_KEY 相同 → navigate 走 same 分支(不 clear_app、不重建,只更新 param signal)。
        // 强制来源:runtime.rs:48 matches() · runtime.rs:254 CUR_KEY 比较。整体不重建行为由 verify.mjs section 8 守。
        assert!(matches("/todo/:id", "/todo/1"));
        assert!(matches("/todo/:id", "/todo/2")); // 同模式 → 同页身份 → 同 key → 不重建
        assert!(!matches("/todo/:id", "/archive")); // 段不匹配 → 换页
        assert!(!matches("/todo/:id", "/todo/1/x")); // 段数不同 → 不匹配
        assert!(matches("/", "/")); // 根
        assert!(matches("/dash/settings", "/dash/settings")); // 多段字面量
        assert!(!matches("/dash/settings", "/dash")); // 段数不同
        assert!(matches("/dash/:tab/:id", "/dash/a/1")); // 多 param 段
    }
}

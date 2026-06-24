//! DOM 绑定层 —— 三种后端共用同一套 API(el / append / attr / set_text / dyn_text / on_click / mount)。
//! 组件(app.rs)只调这套 API,完全不知道背后是哪种后端:
//!   · 浏览器 · create  : 首次渲染,createElement 建真实 DOM           (CSR)
//!   · 浏览器 · hydrate : 接管 SSR 的 DOM,认领已有节点 + 挂事件,不重建 (SSR 客户端)
//!   · 原生   · string  : 服务端渲染,把组件拼成 HTML 字符串            (SSR 服务端)

pub use backend::*;

// ─────────────────────────── 浏览器后端(wasm)───────────────────────────
#[cfg(target_arch = "wasm32")]
mod backend {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    mod ffi {
        extern "C" {
            pub fn create_element(ptr: *const u8, len: usize) -> u32;
            pub fn create_text(ptr: *const u8, len: usize) -> u32; // 文本节点(view! 宏用)
            pub fn claim_element(hid: u32) -> u32; // 认领服务端渲好的元素(按 data-h)
            pub fn claim_text(hid: u32) -> u32; // 认领服务端渲好的文本节点(按 <!--h:N--> 标记)
            pub fn set_text(node: u32, ptr: *const u8, len: usize);
            pub fn append_child(parent: u32, child: u32);
            pub fn remove_child(parent: u32, child: u32); // keyed <For>:移除某行
            pub fn set_attr(node: u32, np: *const u8, nl: usize, vp: *const u8, vl: usize);
            pub fn add_event(node: u32, ep: *const u8, elen: usize, handler: u32);
            pub fn set_value(node: u32, ptr: *const u8, len: usize); // 受控输入:设 .value 属性
            pub fn clear_children(node: u32);
            pub fn gql_query(q_ptr: *const u8, q_len: usize, handler: u32);
            pub fn gql_subscribe(q_ptr: *const u8, q_len: usize, handler: u32);
            pub fn mount(node: u32);
            pub fn clear_app(); // SPA 换页:清空 #app 容器(整页换 key 时)
            pub fn push_url(ptr: *const u8, len: usize); // 程序化导航:history.pushState(更新地址栏)
            pub fn focus(node: u32); // 命令式:聚焦元素(on_mount 用)
            pub fn scroll_into_view(node: u32); // 命令式:滚动到元素
            pub fn set_interval(ms: u32, handler: u32) -> u32; // 定时器:返回 timer id
            pub fn clear_interval(timer: u32); // 清定时器(on_cleanup 用)
            pub fn run_js(ptr: *const u8, len: usize); // JS 逃生舱:即发即弃 eval(全局作用域)
            pub fn run_js_on(node: u32, ptr: *const u8, len: usize); // 同上,但 code 里 `el` = 该节点
            pub fn eval_js(ptr: *const u8, len: usize, handler: u32); // eval 取返回(支持 Promise)→ 回调
        }
    }

    thread_local! {
        static HYDRATE: Cell<bool> = const { Cell::new(false) };
        static HID: Cell<u32> = const { Cell::new(0) };
        static HANDLERS: RefCell<Vec<Rc<dyn Fn(&str)>>> = const { RefCell::new(Vec::new()) };
        static FETCH_HANDLERS: RefCell<Vec<Option<Rc<dyn Fn(&str)>>>> = const { RefCell::new(Vec::new()) };
    }

    // 首屏由 JS 置 true(认领 SSR DOM,不重建);渲染完置回 false,SPA 导航走 CSR。
    pub fn set_hydrate(on: bool) {
        HYDRATE.with(|h| h.set(on));
    }
    fn hydrating() -> bool {
        HYDRATE.with(|h| h.get())
    }

    pub fn el(tag: &str) -> u32 {
        if hydrating() {
            // 认领模式:不创建,取服务端用相同顺序渲的那个节点
            let hid = HID.with(|c| {
                let v = c.get();
                c.set(v + 1);
                v
            });
            unsafe { ffi::claim_element(hid) }
        } else {
            unsafe { ffi::create_element(tag.as_ptr(), tag.len()) }
        }
    }
    pub fn text(s: &str) -> u32 {
        if hydrating() {
            // 认领模式:按 hid 取 SSR 渲好的文本节点(与 el() 同一计数器)。
            let hid = HID.with(|c| {
                let v = c.get();
                c.set(v + 1);
                v
            });
            unsafe { ffi::claim_text(hid) }
        } else {
            unsafe { ffi::create_text(s.as_ptr(), s.len()) }
        }
    }
    pub fn set_text(node: u32, s: &str) {
        unsafe { ffi::set_text(node, s.as_ptr(), s.len()) }
    }
    pub fn append(parent: impl Into<u32>, child: impl Into<u32>) {
        if hydrating() {
            return; // 已经在 DOM 里
        }
        unsafe { ffi::append_child(parent.into(), child.into()) }
    }
    // keyed <For>:从父节点移除某个子节点(appendChild 会移动已在 DOM 的节点,故重排用 append;删除用此)。
    pub fn remove_child(parent: impl Into<u32>, child: impl Into<u32>) {
        unsafe { ffi::remove_child(parent.into(), child.into()) }
    }
    pub fn attr(node: u32, k: &str, v: &str) {
        if hydrating() {
            return; // 服务端已设好属性
        }
        unsafe { ffi::set_attr(node, k.as_ptr(), k.len(), v.as_ptr(), v.len()) }
    }
    pub fn mount(node: u32) {
        if hydrating() {
            return; // 内容已在页面上
        }
        unsafe { ffi::mount(node) } // 挂到 #app(app 入口)
    }
    // SPA 换页(导航到不同 key 的页面):清空 #app,随后 mount 新页根节点。
    pub fn clear_app() {
        unsafe { ffi::clear_app() }
    }
    // 程序化导航:把新 URL 推进浏览器历史(地址栏更新、可分享、可后退)。
    pub fn push_url(url: &str) {
        unsafe { ffi::push_url(url.as_ptr(), url.len()) }
    }
    // 命令式 DOM(on_mount 里用,接焦点 / 滚动 / 第三方库)。node 来自 node_ref。
    pub fn focus(node: u32) {
        unsafe { ffi::focus(node) }
    }
    pub fn scroll_into_view(node: u32) {
        unsafe { ffi::scroll_into_view(node) }
    }
    // 定时器:注册 Rust 回调 → JS setInterval 每 ms 回调一次。slot 存 (timer id, 回调):
    // run_interval 按 hid 索引调用,clear_interval 按 timer id 找到 slot 置空 → 释放回调(及其捕获的
    // signal),避免 INTERVAL_HANDLERS 随每次组件挂载无界增长(<Uptime/> 在 shell,每次导航都会建)。
    thread_local! {
        static INTERVAL_HANDLERS: RefCell<Vec<Option<(u32, Rc<dyn Fn()>)>>> = const { RefCell::new(Vec::new()) };
    }
    pub fn set_interval(ms: u32, f: impl Fn() + 'static) -> u32 {
        let hid = INTERVAL_HANDLERS.with(|h| {
            let mut v = h.borrow_mut();
            v.push(None); // 先占位拿 hid(首次 tick 是异步的,晚于本函数返回)
            (v.len() - 1) as u32
        });
        let timer = unsafe { ffi::set_interval(ms, hid) };
        INTERVAL_HANDLERS.with(|h| h.borrow_mut()[hid as usize] = Some((timer, Rc::new(f))));
        timer
    }
    pub fn clear_interval(timer: u32) {
        unsafe { ffi::clear_interval(timer) }
        INTERVAL_HANDLERS.with(|h| {
            for slot in h.borrow_mut().iter_mut() {
                if let Some((t, _)) = slot {
                    if *t == timer {
                        *slot = None; // 释放回调 → drop 捕获的 signal,不再泄漏
                        break;
                    }
                }
            }
        });
    }
    pub fn run_interval(hid: u32) {
        let f = INTERVAL_HANDLERS.with(|h| h.borrow().get(hid as usize).and_then(|s| s.as_ref().map(|(_, f)| f.clone())));
        if let Some(f) = f {
            f();
        }
    }
    // ── JS 逃生舱:直接调任意 JS / 浏览器 API(无 wasm-bindgen 时的通用出口)──
    /// 即发即弃执行 JS(全局作用域):写剪贴板 / localStorage.setItem / scrollTo / 调第三方库等。
    pub fn run_js(code: &str) {
        unsafe { ffi::run_js(code.as_ptr(), code.len()) }
    }
    /// 在某节点上下文执行 JS:code 里 `el` 绑定到该 DOM 节点(配 node_ref:初始化图表 / 编辑器等)。
    pub fn run_js_on(node: u32, code: &str) {
        unsafe { ffi::run_js_on(node, code.as_ptr(), code.len()) }
    }
    /// 执行 JS 并取返回值(支持 Promise,如 clipboard.readText / fetch):
    /// 成功 → `Ok(值)`,JS 抛错 / Promise reject → `Err(消息)`。值是 `String(r)` 字符串化结果:
    /// 基本类型直接可用;要传对象请在 code 里自己 `JSON.stringify(...)`(对象会变 "[object Object]")。
    /// 回调最多触发一次:交付后自动回收 handler;所属 scope(页 / on_mount)先销毁也回收(防泄漏 + 幽灵写)。
    pub fn eval(code: &str, f: impl Fn(Result<&str, &str>) + 'static) {
        let cell: Rc<Cell<u32>> = Rc::new(Cell::new(0));
        let (c2, c3) = (cell.clone(), cell.clone());
        let h = on_fetch_handler(move |s: &str| {
            // 首字节:0x00 = ok,0x01 = err;其余视为 ok(防御)。状态字节是 1 字节 ASCII,切片在字符边界。
            let res = match s.as_bytes().first() {
                Some(0) => Ok(&s[1..]),
                Some(1) => Err(&s[1..]),
                _ => Ok(s),
            };
            f(res);
            drop_fetch_handler(c2.get()); // 一次性:交付后释放槽
        });
        cell.set(h);
        crate::reactive::on_cleanup(move || drop_fetch_handler(c3.get())); // 未交付即销毁:回收 + 置空(晚到的交付变 no-op)
        unsafe { ffi::eval_js(code.as_ptr(), code.len(), h) }
    }
    // 通用事件:event 为事件名("click"/"input"/"submit"/…);handler 收 payload(target.value,无则 "")。
    pub fn on(node: u32, event: &str, f: impl Fn(&str) + 'static) {
        let id = HANDLERS.with(|h| {
            let mut v = h.borrow_mut();
            v.push(Rc::new(f));
            (v.len() - 1) as u32
        });
        unsafe { ffi::add_event(node, event.as_ptr(), event.len(), id) }
    }
    // 便捷:零参 click(内部转成忽略 payload 的通用事件)。
    pub fn on_click(node: u32, f: impl Fn() + 'static) {
        on(node, "click", move |_| f());
    }
    // 受控输入:设置元素的 .value(property,不是 attribute)。
    pub fn set_value(node: u32, s: &str) {
        unsafe { ffi::set_value(node, s.as_ptr(), s.len()) }
    }
    pub fn run_handler(id: u32, value: &str) {
        let f = HANDLERS.with(|h| h.borrow().get(id as usize).cloned());
        if let Some(f) = f {
            f(value);
        }
    }
    pub fn clear(node: u32) {
        if hydrating() {
            return; // 水合期:SSR 的子节点要被认领,绝不能清掉(否则 <For> 等会抹掉首屏)
        }
        unsafe { ffi::clear_children(node) }
    }

    // 注册一个 fetch 完成回调,返回 id;JS 拿到数据后调 on_fetch(id, text)。
    pub fn on_fetch_handler(f: impl Fn(&str) + 'static) -> u32 {
        FETCH_HANDLERS.with(|h| {
            let mut v = h.borrow_mut();
            v.push(Some(Rc::new(f)));
            (v.len() - 1) as u32
        })
    }
    // 释放 fetch 回调槽(query!/resource!/subscription! 在所属 scope 销毁时经 on_cleanup 调):
    // 否则每次导航都泄漏一个 handler(及其捕获的 signal),且弃页的在途响应回来会幽灵写缓存。
    pub fn drop_fetch_handler(id: u32) {
        FETCH_HANDLERS.with(|h| {
            if let Some(slot) = h.borrow_mut().get_mut(id as usize) {
                *slot = None;
            }
        });
    }
    // 客户端响应缓存:SSR 注入的「查询串 → 响应」,首屏命中则同步交付、跳过网络请求。
    thread_local! {
        static HYDRATE_RESP: RefCell<Vec<(String, String)>> = const { RefCell::new(Vec::new()) };
    }
    pub fn seed_responses(json: &str) {
        let v = crate::gql::value::parse(json);
        if let crate::gql::value::Value::Object(fs) = v {
            HYDRATE_RESP.with(|r| {
                let mut r = r.borrow_mut();
                for (k, val) in fs {
                    if let crate::gql::value::Value::Str(s) = val {
                        r.push((k, s));
                    }
                }
            });
        }
    }
    pub fn gql(query: impl AsRef<str>, handler: u32) {
        let q = query.as_ref();
        // 首屏:命中 SSR 注入的响应缓存就同步交付,不发 POST(消费一次;后续 SPA 导航走真请求)。
        let cached = HYDRATE_RESP.with(|r| {
            let mut r = r.borrow_mut();
            r.iter().position(|(k, _)| k == q).map(|pos| r.remove(pos).1)
        });
        if let Some(resp) = cached {
            run_fetch(handler, &resp);
            return;
        }
        unsafe { ffi::gql_query(q.as_ptr(), q.len(), handler) }
    }
    pub fn subscribe(query: impl AsRef<str>, handler: u32) {
        let q = query.as_ref();
        // 首屏:若有 SSR 注入的初值,先同步交付(让水合渲染与 SSR 一致),再开 SSE 接管更新。
        let cached = HYDRATE_RESP.with(|r| {
            let mut r = r.borrow_mut();
            r.iter().position(|(k, _)| k == q).map(|pos| r.remove(pos).1)
        });
        if let Some(resp) = cached {
            run_fetch(handler, &resp);
        }
        unsafe { ffi::gql_subscribe(q.as_ptr(), q.len(), handler) } // 开 SSE 持续收
    }
    pub fn run_fetch(id: u32, text: &str) {
        // 槽可能已被 drop_fetch_handler 置空(弃页的在途响应)→ flatten 后 no-op,不再幽灵写。
        let f = FETCH_HANDLERS.with(|h| h.borrow().get(id as usize).cloned().flatten());
        if let Some(f) = f {
            f(text);
        }
    }
}

// ─────────────────────────── 服务端后端(native)───────────────────────────
#[cfg(not(target_arch = "wasm32"))]
mod backend {
    use std::cell::{Cell, RefCell};

    #[derive(Default, Clone)]
    struct Node {
        tag: String,
        attrs: Vec<(String, String)>,
        children: Vec<u32>,
        text: Option<String>,
        hid: u32,
        is_text: bool, // 文本节点(view! 宏用):序列化时只输出文本,无标签
    }

    thread_local! {
        static ARENA: RefCell<Vec<Node>> = const { RefCell::new(Vec::new()) };
        static HID: Cell<u32> = const { Cell::new(0) };
        static DOC_ROOT: Cell<Option<u32>> = const { Cell::new(None) };
    }

    pub fn reset() {
        ARENA.with(|a| a.borrow_mut().clear());
        HID.with(|h| h.set(0));
        DOC_ROOT.with(|d| d.set(None));
        SSR_RESP.with(|r| r.borrow_mut().clear());
    }
    pub fn take_html() -> String {
        let mut s = String::new();
        if let Some(r) = DOC_ROOT.with(|d| d.get()) {
            serialize(r, &mut s);
        }
        s
    }

    #[allow(dead_code)]
    pub fn set_hydrate(_on: bool) {} // 服务端无 hydrate
    pub fn run_handler(_: u32, _: &str) {} // 服务端无事件

    pub fn el(tag: &str) -> u32 {
        let hid = HID.with(|h| {
            let v = h.get();
            h.set(v + 1);
            v
        });
        ARENA.with(|a| {
            let mut a = a.borrow_mut();
            a.push(Node { tag: tag.to_string(), hid, ..Default::default() });
            (a.len() - 1) as u32
        })
    }
    pub fn text(s: &str) -> u32 {
        // 文本节点也占一个 hid(序列化成 <!--h:N--> 标记),客户端据此认领 —— 与 el() 同一计数器。
        let hid = HID.with(|h| {
            let v = h.get();
            h.set(v + 1);
            v
        });
        ARENA.with(|a| {
            let mut a = a.borrow_mut();
            a.push(Node { is_text: true, text: Some(s.to_string()), hid, ..Default::default() });
            (a.len() - 1) as u32
        })
    }
    pub fn set_text(node: u32, s: &str) {
        ARENA.with(|a| a.borrow_mut()[node as usize].text = Some(s.to_string()));
    }
    pub fn append(parent: impl Into<u32>, child: impl Into<u32>) {
        let (parent, child) = (parent.into(), child.into());
        ARENA.with(|a| a.borrow_mut()[parent as usize].children.push(child));
    }
    pub fn remove_child(parent: impl Into<u32>, child: impl Into<u32>) {
        let (parent, child) = (parent.into(), child.into());
        ARENA.with(|a| a.borrow_mut()[parent as usize].children.retain(|&c| c != child));
    }
    pub fn attr(node: u32, k: &str, v: &str) {
        ARENA.with(|a| a.borrow_mut()[node as usize].attrs.push((k.to_string(), v.to_string())));
    }
    pub fn on(_: u32, _: &str, _: impl Fn(&str) + 'static) {} // 服务端不绑事件(留给客户端)
    pub fn on_click(_: u32, _: impl Fn() + 'static) {} // 服务端不绑事件(留给客户端 hydrate)
    // SSR:受控值渲染成 value 属性,首屏可见正确初值(替换或追加)。
    pub fn set_value(node: u32, s: &str) {
        ARENA.with(|a| {
            let mut a = a.borrow_mut();
            let attrs = &mut a[node as usize].attrs;
            if let Some(slot) = attrs.iter_mut().find(|(k, _)| k == "value") {
                slot.1 = s.to_string();
            } else {
                attrs.push(("value".to_string(), s.to_string()));
            }
        });
    }
    pub fn clear(node: u32) {
        ARENA.with(|a| {
            let mut a = a.borrow_mut();
            let n = &mut a[node as usize];
            n.children.clear();
            n.text = None;
        });
    }
    // SSR 预取:fetch 在服务端同步解析(数据本就在服务端)→ 渲染即带数据(SEO 可见)
    thread_local! {
        static FETCH_HANDLERS: std::cell::RefCell<Vec<Option<std::rc::Rc<dyn Fn(&str)>>>> = const { std::cell::RefCell::new(Vec::new()) };
        // 本次 SSR 执行过的「查询串 → 响应」,序列化后注入 HTML 供客户端首屏复用。
        static SSR_RESP: RefCell<Vec<(String, String)>> = const { RefCell::new(Vec::new()) };
    }
    pub fn dehydrate_responses() -> String {
        use crate::gql::value::Value;
        SSR_RESP.with(|r| {
            Value::Object(
                r.borrow().iter().map(|(q, resp)| (q.clone(), Value::Str(resp.clone()))).collect(),
            )
            .to_json()
        })
    }
    pub fn seed_responses(_json: &str) {} // 服务端不用(对称占位,让 runtime::hydrate_data 两端都能编译)
    pub fn on_fetch_handler(f: impl Fn(&str) + 'static) -> u32 {
        FETCH_HANDLERS.with(|h| {
            let mut v = h.borrow_mut();
            v.push(Some(std::rc::Rc::new(f)));
            (v.len() - 1) as u32
        })
    }
    pub fn drop_fetch_handler(id: u32) {
        FETCH_HANDLERS.with(|h| {
            if let Some(slot) = h.borrow_mut().get_mut(id as usize) {
                *slot = None;
            }
        });
    }
    pub fn gql(query: impl AsRef<str>, handler: u32) {
        let q = query.as_ref();
        let text = crate::server::local_execute(q); // 服务端本地执行同一个 query(SSR 预取)
        SSR_RESP.with(|r| r.borrow_mut().push((q.to_string(), text.clone()))); // 记录,注入客户端复用
        run_fetch(handler, &text); // 立即回调 → signal 填充 → SSR 渲染带数据
    }
    pub fn subscribe(query: impl AsRef<str>, handler: u32) {
        // SSR 不流式:只解析一次当前值(初值);也记入响应缓存,供客户端水合时同步重建出一致的树。
        let q = query.as_ref();
        let text = crate::server::local_execute(q);
        SSR_RESP.with(|r| r.borrow_mut().push((q.to_string(), text.clone())));
        run_fetch(handler, &text);
    }
    pub fn run_fetch(id: u32, text: &str) {
        let f = FETCH_HANDLERS.with(|h| h.borrow().get(id as usize).cloned().flatten());
        if let Some(f) = f {
            f(text);
        }
    }
    pub fn mount(node: u32) {
        DOC_ROOT.with(|d| d.set(Some(node))); // 文档根(take_html 时序列化)
    }
    pub fn clear_app() {} // 服务端无 SPA 导航(对称占位)
    pub fn push_url(_url: &str) {} // 服务端无浏览器历史(对称占位)
    // 命令式 DOM / 定时器:服务端无 DOM / 无事件循环,全部 no-op(on_mount 本就不在服务端跑)。
    pub fn focus(_node: u32) {}
    pub fn scroll_into_view(_node: u32) {}
    pub fn set_interval(_ms: u32, _f: impl Fn() + 'static) -> u32 {
        0
    }
    pub fn clear_interval(_timer: u32) {}
    pub fn run_interval(_hid: u32) {}
    // JS 逃生舱:服务端无 JS 运行时 → 全部 no-op(eval 的回调不触发,与 on_mount 同样只在客户端跑)。
    pub fn run_js(_code: &str) {}
    pub fn run_js_on(_node: u32, _code: &str) {}
    pub fn eval(_code: &str, _f: impl Fn(Result<&str, &str>) + 'static) {}

    fn serialize(id: u32, out: &mut String) {
        // 先把节点克隆出来再放掉借用,然后递归(避免 RefCell 重入借用)
        let node = ARENA.with(|a| a.borrow()[id as usize].clone());
        if node.is_text {
            // 文本前打注释标记 <!--h:N-->,客户端水合按 N 认领该文本节点(空文本也有标记)。
            out.push_str(&format!("<!--h:{}-->", node.hid));
            out.push_str(&esc_text(node.text.as_deref().unwrap_or("")));
            return;
        }
        out.push('<');
        out.push_str(&node.tag);
        out.push_str(&format!(" data-h=\"{}\"", node.hid)); // ← hydration 锚点
        for (k, v) in &node.attrs {
            out.push_str(&format!(" {}=\"{}\"", k, esc_attr(v)));
        }
        out.push('>');
        if let Some(t) = &node.text {
            out.push_str(&esc_text(t));
        }
        for c in &node.children {
            serialize(*c, out);
        }
        out.push_str(&format!("</{}>", node.tag));
    }
    fn esc_text(s: &str) -> String {
        s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
    }
    fn esc_attr(s: &str) -> String {
        s.replace('&', "&amp;").replace('"', "&quot;")
    }
}

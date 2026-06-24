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
            pub fn claim_element(hid: u32) -> u32; // 认领服务端渲好的节点(按 data-h)
            pub fn set_text(node: u32, ptr: *const u8, len: usize);
            pub fn append_child(parent: u32, child: u32);
            pub fn set_attr(node: u32, np: *const u8, nl: usize, vp: *const u8, vl: usize);
            pub fn add_event(node: u32, ep: *const u8, elen: usize, handler: u32);
            pub fn clear_children(node: u32);
            pub fn gql_query(q_ptr: *const u8, q_len: usize, handler: u32);
            pub fn gql_subscribe(q_ptr: *const u8, q_len: usize, handler: u32);
            pub fn mount(node: u32);
        }
    }

    thread_local! {
        static HYDRATE: Cell<bool> = const { Cell::new(false) };
        static HID: Cell<u32> = const { Cell::new(0) };
        static HANDLERS: RefCell<Vec<Rc<dyn Fn()>>> = const { RefCell::new(Vec::new()) };
        static FETCH_HANDLERS: RefCell<Vec<Rc<dyn Fn(&str)>>> = const { RefCell::new(Vec::new()) };
    }

    #[allow(dead_code)] // hydration 机制保留;当前路由 demo 走 CSR 重渲染未接入
    pub fn set_hydrate() {
        HYDRATE.with(|h| h.set(true));
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
        unsafe { ffi::create_text(s.as_ptr(), s.len()) }
    }
    pub fn set_text(node: u32, s: &str) {
        unsafe { ffi::set_text(node, s.as_ptr(), s.len()) }
    }
    pub fn append(parent: u32, child: u32) {
        if hydrating() {
            return; // 已经在 DOM 里
        }
        unsafe { ffi::append_child(parent, child) }
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
    pub fn on_click(node: u32, f: impl Fn() + 'static) {
        let id = HANDLERS.with(|h| {
            let mut v = h.borrow_mut();
            v.push(Rc::new(f));
            (v.len() - 1) as u32
        });
        let ev = "click";
        unsafe { ffi::add_event(node, ev.as_ptr(), ev.len(), id) }
    }
    pub fn run_handler(id: u32) {
        let f = HANDLERS.with(|h| h.borrow().get(id as usize).cloned());
        if let Some(f) = f {
            f();
        }
    }
    pub fn clear(node: u32) {
        unsafe { ffi::clear_children(node) }
    }

    // 注册一个 fetch 完成回调,返回 id;JS 拿到数据后调 on_fetch(id, text)。
    pub fn on_fetch_handler(f: impl Fn(&str) + 'static) -> u32 {
        FETCH_HANDLERS.with(|h| {
            let mut v = h.borrow_mut();
            v.push(Rc::new(f));
            (v.len() - 1) as u32
        })
    }
    pub fn gql(query: impl AsRef<str>, handler: u32) {
        let q = query.as_ref();
        unsafe { ffi::gql_query(q.as_ptr(), q.len(), handler) }
    }
    pub fn subscribe(query: impl AsRef<str>, handler: u32) {
        let q = query.as_ref();
        unsafe { ffi::gql_subscribe(q.as_ptr(), q.len(), handler) } // 客户端开 SSE 持续收
    }
    pub fn run_fetch(id: u32, text: &str) {
        let f = FETCH_HANDLERS.with(|h| h.borrow().get(id as usize).cloned());
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
    }
    pub fn take_html() -> String {
        let mut s = String::new();
        if let Some(r) = DOC_ROOT.with(|d| d.get()) {
            serialize(r, &mut s);
        }
        s
    }

    #[allow(dead_code)]
    pub fn set_hydrate() {} // 服务端无 hydrate
    pub fn run_handler(_: u32) {} // 服务端无事件

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
        ARENA.with(|a| {
            let mut a = a.borrow_mut();
            a.push(Node { is_text: true, text: Some(s.to_string()), ..Default::default() });
            (a.len() - 1) as u32
        })
    }
    pub fn set_text(node: u32, s: &str) {
        ARENA.with(|a| a.borrow_mut()[node as usize].text = Some(s.to_string()));
    }
    pub fn append(parent: u32, child: u32) {
        ARENA.with(|a| a.borrow_mut()[parent as usize].children.push(child));
    }
    pub fn attr(node: u32, k: &str, v: &str) {
        ARENA.with(|a| a.borrow_mut()[node as usize].attrs.push((k.to_string(), v.to_string())));
    }
    pub fn on_click(_: u32, _: impl Fn() + 'static) {} // 服务端不绑事件(留给客户端 hydrate)
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
        static FETCH_HANDLERS: std::cell::RefCell<Vec<std::rc::Rc<dyn Fn(&str)>>> = const { std::cell::RefCell::new(Vec::new()) };
    }
    pub fn on_fetch_handler(f: impl Fn(&str) + 'static) -> u32 {
        FETCH_HANDLERS.with(|h| {
            let mut v = h.borrow_mut();
            v.push(std::rc::Rc::new(f));
            (v.len() - 1) as u32
        })
    }
    pub fn gql(query: impl AsRef<str>, handler: u32) {
        let text = crate::server::local_execute(query.as_ref()); // 服务端本地执行同一个 query(SSR 预取)
        run_fetch(handler, &text); // 立即回调 → signal 填充 → SSR 渲染带数据
    }
    pub fn subscribe(query: impl AsRef<str>, handler: u32) {
        // SSR 不流式:只解析一次当前值(初值),客户端再接管 SSE 推送
        let text = crate::server::local_execute(query.as_ref());
        run_fetch(handler, &text);
    }
    pub fn run_fetch(id: u32, text: &str) {
        let f = FETCH_HANDLERS.with(|h| h.borrow().get(id as usize).cloned());
        if let Some(f) = f {
            f(text);
        }
    }
    pub fn mount(node: u32) {
        DOC_ROOT.with(|d| d.set(Some(node))); // 文档根(take_html 时序列化)
    }

    fn serialize(id: u32, out: &mut String) {
        // 先把节点克隆出来再放掉借用,然后递归(避免 RefCell 重入借用)
        let node = ARENA.with(|a| a.borrow()[id as usize].clone());
        if node.is_text {
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

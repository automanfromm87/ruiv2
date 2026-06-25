//! DOM 绑定层 —— 三种后端共用同一套 API(el / append / attr / set_text / dyn_text / on_click / mount)。
//! 组件(app.rs)只调这套 API,完全不知道背后是哪种后端:
//!   · 浏览器 · create  : 首次渲染,createElement 建真实 DOM           (CSR)
//!   · 浏览器 · hydrate : 接管 SSR 的 DOM,认领已有节点 + 挂事件,不重建 (SSR 客户端)
//!   · 原生   · string  : 服务端渲染,把组件拼成 HTML 字符串            (SSR 服务端)

pub use backend::*;

// ─────────────────────────── 事件载荷(两后端共用)───────────────────────────
// JS 把当前事件的常用字段紧凑编码成一个字符串(随 dispatch 传入),Rust 解码成 Event。
// 事件 handler 内用 `rui::event()` 读它(线程局部,仅在 handler 运行期有效);零参 handler
// 完全不受影响(故现有 on:click={move||..} 不用改)。native 端无事件 → 恒为默认值。
// 编码:字段以 \u{1f} 分隔;files 每项 name\u{1e}size\u{1e}type、项间 \u{1d}。

/// 文件选择的元数据(file input / 拖拽 drop);读取内容用 JS 逃生舱(FileReader)或后续专用 API。
#[derive(Clone, Default, Debug, PartialEq)]
pub struct FileMeta {
    pub name: String,
    pub size: u64,
    pub mime: String,
}

/// 一次 DOM 事件的快照。`value`/`checked` 来自 target;键盘字段(`key`/`code`/修饰键)、
/// 指针字段(`client_x/y`/`button`/`delta_y`)、`files` 按需取用(无则默认值)。
#[derive(Clone, Default, Debug)]
pub struct Event {
    pub value: String,
    pub checked: bool,
    pub key: String,  // KeyboardEvent.key:"Enter" / "a" / "Escape" / "ArrowDown" / " "
    pub code: String, // KeyboardEvent.code:"KeyA" / "Enter"
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub meta: bool,
    pub client_x: f64,
    pub client_y: f64,
    pub button: i32,
    pub delta_y: f64,
    pub files: Vec<FileMeta>,
}

impl Event {
    #[allow(dead_code)] // 仅 wasm 的 run_handler 用;native 无事件
    fn decode(s: &str) -> Event {
        let mut it = s.split('\u{1f}');
        let mut next = || it.next().unwrap_or("");
        let value = next().to_string();
        let checked = next() == "1";
        let key = next().to_string();
        let code = next().to_string();
        let ctrl = next() == "1";
        let shift = next() == "1";
        let alt = next() == "1";
        let meta = next() == "1";
        let client_x = next().parse().unwrap_or(0.0);
        let client_y = next().parse().unwrap_or(0.0);
        let button = next().parse().unwrap_or(0);
        let delta_y = next().parse().unwrap_or(0.0);
        let files_raw = next();
        let files = if files_raw.is_empty() {
            Vec::new()
        } else {
            files_raw
                .split('\u{1d}')
                .map(|f| {
                    let mut fp = f.split('\u{1e}');
                    FileMeta {
                        name: fp.next().unwrap_or("").to_string(),
                        size: fp.next().unwrap_or("").parse().unwrap_or(0),
                        mime: fp.next().unwrap_or("").to_string(),
                    }
                })
                .collect()
        };
        Event { value, checked, key, code, ctrl, shift, alt, meta, client_x, client_y, button, delta_y, files }
    }
}

thread_local! {
    static CURRENT_EVENT: std::cell::RefCell<Event> = std::cell::RefCell::new(Event::default());
}

/// 取当前事件快照。**仅在事件 handler 内有效**:渲染期 / handler 外 / native(SSR)调用返回上一次事件
/// 或默认值(不报错)。异步回调(set_timeout/Promise)里读到的是届时的"当前"事件,非当初那次 —— 需在
/// handler 内先 `let e = rui::event();` 抓取再带进异步。数字字段解析失败默认 0;含分隔控制符的输入已在
/// JS 侧剥除。CURRENT_EVENT 保留上一次事件(含 files Vec)到下次事件覆盖(thread-local,随线程/页释放)。
pub fn event() -> Event {
    CURRENT_EVENT.with(|e| e.borrow().clone())
}

/// 由 run_handler 在调用 handler 前设置(把 dispatch 传入的编码串解码进线程局部)。
#[allow(dead_code)] // 仅 wasm 的 run_handler 用
pub(crate) fn set_current_event(encoded: &str) {
    CURRENT_EVENT.with(|e| *e.borrow_mut() = Event::decode(encoded));
}

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
            pub fn set_checked(node: u32, on: u32); // 受控复选框 / 单选:设 .checked property
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
            pub fn set_timeout(ms: u32, handler: u32); // 一次性延时回调(过渡:出场动画结束后移除)
            pub fn add_class(node: u32, ptr: *const u8, len: usize); // classList.add(过渡 enter/leave 类)
            pub fn remove_class(node: u32, ptr: *const u8, len: usize); // classList.remove
            pub fn run_js(ptr: *const u8, len: usize); // JS 逃生舱:即发即弃 eval(全局作用域)
            pub fn run_js_on(node: u32, ptr: *const u8, len: usize); // 同上,但 code 里 `el` = 该节点
            pub fn eval_js(ptr: *const u8, len: usize, handler: u32); // eval 取返回(支持 Promise)→ 回调
            pub fn console_error(ptr: *const u8, len: usize); // console.error(panic hook 用)
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
    // 换页时清空事件处理器表(必须在 clear_app 之后调:旧 DOM 已抹掉,其 handler id 不会再触发;
    // 新页从 id 0 重新注册)。这样事件处理器不再随每次换页无界增长(对称于 INTERVAL/FETCH_HANDLERS 的回收)。
    // 注意:只在**换页**(clear_app 处)调,绝不在同页 / 组内导航调 —— 那时 DOM 仍在,handler 必须留着。
    pub fn clear_handlers() {
        HANDLERS.with(|h| h.borrow_mut().clear());
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
    // 一次性延时回调(过渡的出场动画结束后移除节点)。slot 存 FnOnce,run_oneshot 取走并调一次。
    thread_local! {
        static ONESHOTS: RefCell<Vec<Option<Box<dyn FnOnce()>>>> = const { RefCell::new(Vec::new()) };
    }
    pub fn set_timeout(ms: u32, f: impl FnOnce() + 'static) {
        let id = ONESHOTS.with(|h| {
            let mut v = h.borrow_mut();
            let b = Some(Box::new(f) as Box<dyn FnOnce()>);
            // 复用已触发(None)的槽位 → 避免 Vec 随每次过渡无界增长。已触发的槽无在途定时器引用其 id,
            // 故复用不会 id 碰撞;未触发的槽保持占用、其 id 不被复用(等它触发后自然变 None 回收)。
            match v.iter().position(|s| s.is_none()) {
                Some(i) => {
                    v[i] = b;
                    i as u32
                }
                None => {
                    v.push(b);
                    (v.len() - 1) as u32
                }
            }
        });
        unsafe { ffi::set_timeout(ms, id) }
    }
    pub fn run_oneshot(id: u32) {
        let f = ONESHOTS.with(|h| h.borrow_mut().get_mut(id as usize).and_then(|s| s.take()));
        if let Some(f) = f {
            f();
        }
    }
    // 过渡:增删单个 class(不动其它 class —— attr 设整串会覆盖)。
    pub fn add_class(node: u32, cls: &str) {
        unsafe { ffi::add_class(node, cls.as_ptr(), cls.len()) }
    }
    pub fn remove_class(node: u32, cls: &str) {
        unsafe { ffi::remove_class(node, cls.as_ptr(), cls.len()) }
    }
    // ── JS 逃生舱:直接调任意 JS / 浏览器 API(无 wasm-bindgen 时的通用出口)──
    /// 即发即弃执行 JS(全局作用域):写剪贴板 / localStorage.setItem / scrollTo / 调第三方库等。
    pub fn run_js(code: &str) {
        unsafe { ffi::run_js(code.as_ptr(), code.len()) }
    }
    /// 打到浏览器 console.error(panic hook 用:wasm panic 默认静默白屏,这里先把消息+位置打出来)。
    pub fn console_error(msg: &str) {
        unsafe { ffi::console_error(msg.as_ptr(), msg.len()) }
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
    // 受控复选框 / 单选:设置 .checked(property)。
    pub fn set_checked(node: u32, on: bool) {
        unsafe { ffi::set_checked(node, on as u32) }
    }
    pub fn run_handler(id: u32, encoded: &str) {
        // encoded = JS 编码的事件载荷:先解码进线程局部(供 rui::event() 读),再把 value 字段
        // 作为 &str 传给 handler(bind:value/group 用它;零参 on:click 忽略它、按需 rui::event())。
        super::set_current_event(encoded);
        let value = super::event().value;
        let f = HANDLERS.with(|h| h.borrow().get(id as usize).cloned());
        if let Some(f) = f {
            f(&value);
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
        FETCH_HANDLERS.with(|h| h.borrow_mut().clear()); // query!/resource! 回调随渲染累积:复用线程时必清,否则只增不减
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
    // SSR:受控复选框渲染成 checked 属性(on=true 加 `checked=""`,false 则移除),首屏勾选状态可见。
    pub fn set_checked(node: u32, on: bool) {
        ARENA.with(|a| {
            let mut a = a.borrow_mut();
            let attrs = &mut a[node as usize].attrs;
            attrs.retain(|(k, _)| k != "checked");
            if on {
                attrs.push(("checked".to_string(), String::new()));
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
    pub fn clear_handlers() {} // 服务端不绑事件(对称占位)
    pub fn push_url(_url: &str) {} // 服务端无浏览器历史(对称占位)
    // 命令式 DOM / 定时器:服务端无 DOM / 无事件循环,全部 no-op(on_mount 本就不在服务端跑)。
    pub fn focus(_node: u32) {}
    pub fn scroll_into_view(_node: u32) {}
    pub fn set_interval(_ms: u32, _f: impl Fn() + 'static) -> u32 {
        0
    }
    pub fn clear_interval(_timer: u32) {}
    pub fn run_interval(_hid: u32) {}
    pub fn set_timeout(_ms: u32, _f: impl FnOnce() + 'static) {} // 服务端无定时器(过渡的延时移除不发生)
    pub fn run_oneshot(_id: u32) {}
    // 过渡 class:SSR 时把 enter/leave 类并入 class 属性(首屏初态可见;无动画)。
    pub fn add_class(node: u32, cls: &str) {
        ARENA.with(|a| {
            let mut a = a.borrow_mut();
            let attrs = &mut a[node as usize].attrs;
            if let Some(slot) = attrs.iter_mut().find(|(k, _)| k == "class") {
                if !slot.1.split_whitespace().any(|c| c == cls) {
                    if !slot.1.is_empty() {
                        slot.1.push(' ');
                    }
                    slot.1.push_str(cls);
                }
            } else {
                attrs.push(("class".to_string(), cls.to_string()));
            }
        });
    }
    pub fn remove_class(node: u32, cls: &str) {
        ARENA.with(|a| {
            let mut a = a.borrow_mut();
            if let Some(slot) = a[node as usize].attrs.iter_mut().find(|(k, _)| k == "class") {
                slot.1 = slot.1.split_whitespace().filter(|c| *c != cls).collect::<Vec<_>>().join(" ");
            }
        });
    }
    pub fn console_error(msg: &str) {
        eprintln!("{msg}"); // 服务端:打到 stderr(native panic 本就会打印,这里仅对称)
    }
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

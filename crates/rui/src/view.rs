//! 视图值模型:`View`(已构建节点的句柄)+ `IntoView`(把任意可渲染值变成节点,带 build/rebuild
//! 以支持**就地更新**)+ `reactive_block`(`view!` 里 `{ 闭包 }` 的运行时)。
//!
//! 这是「表达式式条件渲染」的关键:`view!{}` 与组件都返回 `View`,于是 `view!` 里的 `{ }` 块
//! 能按**返回类型**分派 —— 返回 `View`(子树)就挂载/替换,返回 `&str`/数字等就当文本。
//! 于是 `{ move || if c.get() { view!{..} } else { view!{..} } }` 这种用 Rust 原生 `if`/`match`
//! 的写法天然反应式,而响应式文本仍走**原地 `set_text`**(无包裹元素、不重建节点),不退化。

use crate::dom;
use crate::reactive::{effect, scope, Scope, Signal};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// 页面渲染策略(`#[rui::page(..)]` 标注):
///   Ssr    每请求服务端渲染 + 客户端水合(默认)
///   Csr    服务端只发空壳,客户端从零渲(浏览器专属 / 重交互 / 鉴权后台)
///   Static 服务端渲一次后缓存复用(SSG)
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Strategy {
    Ssr,
    Csr,
    Static,
}

/// 一个「待渲染的页面」:key(页面身份)+ 策略 + 延迟渲染闭包。
/// `#[rui::page]` 把页面函数包成它(key = module_path!());`route()` 的 match 就是 path→Page 的"表"。
/// 服务端先看 `strategy` 决定渲不渲;客户端按 `key` 判断导航是"同页换参数"(不重建)还是"换页"(重渲)。
pub struct Page {
    pub key: String,
    pub strategy: Strategy,
    pub render: Box<dyn FnOnce() -> View>,
}
impl Page {
    pub fn new(key: impl Into<String>, strategy: Strategy, render: impl FnOnce() -> View + 'static) -> Self {
        Page { key: key.into(), strategy, render: Box::new(render) }
    }
}

/// 元素引用句柄:`view!{ <input ref={r} /> }` 把元素节点 id 写进它;`on_mount` 里 `r.get()` 取到,
/// 交给 `dom::focus` 等命令式 API(接焦点 / 滚动 / 第三方库)。0 = 尚未挂载。
#[derive(Clone, Default)]
pub struct NodeRef(Rc<Cell<u32>>);
impl NodeRef {
    pub fn get(&self) -> u32 {
        self.0.get()
    }
    pub fn set(&self, id: u32) {
        self.0.set(id);
    }
}
/// 新建一个空元素引用(配 `ref={..}` 用)。
pub fn node_ref() -> NodeRef {
    NodeRef::default()
}

/// 一个已构建视图节点的句柄 —— `view!{}` 与组件函数的返回类型。内部是引擎的节点 id。
#[derive(Clone, Copy)]
pub struct View(pub u32);

impl View {
    /// 取底层节点 id(交给 `dom::mount` 等 u32 API)。
    pub fn node(self) -> u32 {
        self.0
    }
}

// 让 View 能直接喂给 dom::append(impl Into<u32>)等 API。
impl From<View> for u32 {
    fn from(v: View) -> u32 {
        v.0
    }
}

/// 把一个值渲染成 DOM 节点,并支持后续**就地更新**。
///   · `build`   首次构建,返回 (节点 id, 后续更新所需状态)。
///   · `rebuild` 用新值原地更新已有节点(文本 → `set_text`;子树 → 在锚点处换内容)。
pub trait IntoView {
    type State;
    fn build(self) -> (u32, Self::State);
    fn rebuild(self, state: &mut Self::State);
}

impl IntoView for View {
    type State = u32; // rui-slot 锚点
    fn build(self) -> (u32, u32) {
        let anchor = dom::el("rui-slot");
        dom::append(anchor, self.0);
        (anchor, anchor)
    }
    fn rebuild(self, anchor: &mut u32) {
        dom::clear(*anchor);
        dom::append(*anchor, self.0);
    }
}

impl IntoView for () {
    type State = u32; // 空文本节点(什么都不渲染)
    fn build(self) -> (u32, u32) {
        let t = dom::text("");
        (t, t)
    }
    fn rebuild(self, _state: &mut u32) {}
}

impl<T: IntoView> IntoView for Option<T> {
    type State = u32; // rui-slot 锚点:Some 挂子树,None 空
    fn build(self) -> (u32, u32) {
        let anchor = dom::el("rui-slot");
        if let Some(v) = self {
            let (n, _s) = v.build();
            dom::append(anchor, n);
        }
        (anchor, anchor)
    }
    fn rebuild(self, anchor: &mut u32) {
        dom::clear(*anchor);
        if let Some(v) = self {
            let (n, _s) = v.build();
            dom::append(*anchor, n);
        }
    }
}

impl<T: IntoView> IntoView for Vec<T> {
    type State = u32; // rui-slot 锚点(内联列表;需要 keyed 复用请用 <For>)
    fn build(self) -> (u32, u32) {
        let anchor = dom::el("rui-slot");
        for v in self {
            let (n, _s) = v.build();
            dom::append(anchor, n);
        }
        (anchor, anchor)
    }
    fn rebuild(self, anchor: &mut u32) {
        dom::clear(*anchor);
        for v in self {
            let (n, _s) = v.build();
            dom::append(*anchor, n);
        }
    }
}

// Display 类标量 → 文本节点;rebuild 是**原地 set_text**(不重建、无包裹),所以响应式文本不退化。
macro_rules! into_view_text {
    ($($t:ty),*) => { $(
        impl IntoView for $t {
            type State = u32; // 文本节点 id
            fn build(self) -> (u32, u32) {
                let t = dom::text(&::std::format!("{}", self));
                (t, t)
            }
            fn rebuild(self, node: &mut u32) {
                dom::set_text(*node, &::std::format!("{}", self));
            }
        }
    )* };
}
into_view_text!(
    &str, String, &String, i8, i16, i32, i64, isize, u8, u16, u32, u64, usize, f32, f64, bool, char
);

/// `view!` 里 `{ move || .. }` 的运行时:在一个 effect 里反复求值闭包(订阅它读到的 signal),
/// 首次 `build` 出节点,之后每次依赖变化 `rebuild` **就地更新**。返回挂载用的节点 id。
///
/// 每轮在子作用域里求值闭包:若它构建了子树(返回 `View`),子树内层的 effect 由该子作用域持有,
/// 下一轮一并销毁 —— 杜绝幽灵 effect / 泄漏(依赖 `Scope` 的 Drop 即销毁)。
pub fn reactive_block<V, F>(f: F) -> u32
where
    V: IntoView + 'static,
    V::State: 'static,
    F: Fn() -> V + 'static,
{
    let cell: Rc<RefCell<Option<(V::State, Scope)>>> = Rc::new(RefCell::new(None));
    let node: Rc<Cell<u32>> = Rc::new(Cell::new(0));
    let ctx = crate::reactive::capture_contexts(); // 祖先 context 快照:后续重建里 use_context 仍能看见
    {
        let cell = cell.clone();
        let node = node.clone();
        effect(move || {
            // 子作用域:持有本轮构建产生的内层 effect。with_contexts 让重建时也看得到祖先 context。
            let (v, sc) = crate::reactive::with_contexts(&ctx, || scope(|| f()));
            let mut slot = cell.borrow_mut();
            match slot.as_mut() {
                Some((state, scope_slot)) => {
                    let old = std::mem::replace(scope_slot, sc);
                    old.dispose(); // 销毁上一轮子树的内层 effect
                    v.rebuild(state); // 就地更新:文本 set_text / 子树换内容
                }
                None => {
                    let (n, state) = v.build();
                    node.set(n);
                    *slot = Some((state, sc));
                }
            }
        });
    }
    node.get()
}

// ── ErrorBoundary(错误边界:子树出错 → 局部 fallback + 重试,建在 Context 之上)──

/// 错误边界经 Context 下发给子树的句柄:子树里 `throw_error` / `error_reporter()` 把错误写进它,
/// 边界本身订阅它 → `Some` 渲 fallback、`None` 渲 children。按类型(TypeId)注入,故每子树取最近一个。
#[derive(Clone)]
pub struct ErrorSink(pub Signal<Option<String>>);

/// 在最近 `<ErrorBoundary>` 子树内**当下**上报一个错误(应在渲染期调:此时 context 栈含边界的 sink)。
/// 触发该边界渲 fallback。不在任何边界内 → 返回 `false`(忽略)。事件 / effect 里上报请用 `error_reporter`。
pub fn throw_error(msg: impl Into<String>) -> bool {
    if let Some(sink) = crate::reactive::use_context::<ErrorSink>() {
        sink.0.set(Some(msg.into()));
        true
    } else {
        // 无祖先边界时静默丢弃一个**真实错误**很难排查 → debug 构建打印一行(release 零开销)。
        #[cfg(debug_assertions)]
        eprintln!("rui: throw_error 调用处没有祖先 <ErrorBoundary>,错误被忽略:{}", msg.into());
        false
    }
}

/// 渲染期取一个「上报器」:返回的闭包捕获了最近边界的 sink,之后在事件 / effect / 异步回调里调它
/// 即可把错误送达边界(无需届时还在 context 栈上)。不在任何边界内 → 返回 no-op 闭包。
pub fn error_reporter() -> Rc<dyn Fn(String)> {
    match crate::reactive::use_context::<ErrorSink>() {
        Some(sink) => Rc::new(move |m| sink.0.set(Some(m))),
        None => Rc::new(|_| {}),
    }
}

/// `<ErrorBoundary fallback={ |err, reset| .. }>children</ErrorBoundary>` 的运行时。
/// 建一个 error signal,经 Context provide 给 children 子树(供 `throw_error`/`error_reporter` 上报);
/// 用 `reactive_block` 据它分派:
///   · `None`     → 在 sink 可见的上下文里构建 children(native 额外 `catch_unwind` 兜住 SSR 渲染 panic);
///   · `Some(e)`  → 渲 `fallback(e, reset)`,`reset()` 清错 → reactive_block 重建 children(即"重试")。
/// 嵌套边界:children 里的内层 `<ErrorBoundary>` 会就近 provide 自己的 sink → 内层先接住(冒泡到最近)。
pub fn error_boundary<FB, CH>(fallback: FB, children: CH) -> u32
where
    FB: Fn(String, Rc<dyn Fn()>) -> View + 'static,
    CH: Fn() -> View + 'static,
{
    let err: Signal<Option<String>> = Signal::new(None);
    let sink = ErrorSink(err.clone());
    let reset: Rc<dyn Fn()> = {
        let err = err.clone();
        // 空写守卫:已是 None 不再 set(否则 None→None 仍会调度一次无谓重建,呼应 memo / set_path 的去抖)。
        Rc::new(move || {
            if err.get().is_some() {
                err.set(None);
            }
        })
    };
    reactive_block(move || -> View {
        match err.get() {
            Some(e) => fallback(e, reset.clone()),
            None => {
                // 把 sink provide 进本轮 children 子树(写 reactive_block 的 scope 帧,随重建弹出,不泄漏给兄弟)。
                crate::reactive::provide_context(sink.clone());
                match build_children(&children) {
                    // native 渲染 panic → 直接渲 fallback(不写 err signal:避免在本 effect 运行中 set 自身
                    // 依赖触发的重入重跑 → fallback 被调两次。重渲时仍会重走 children → 再次 catch,行为自洽)。
                    Err(msg) => fallback(msg, reset.clone()),
                    Ok(v) => {
                        // 渲染期 throw_error / error_reporter 在本次 build 中写了 err → 改渲 fallback,
                        // 否则会错误地显示 children(那次 set 触发的重入重跑会被本分支的结果覆盖)。untrack 不再订阅。
                        match crate::reactive::untrack(|| err.get()) {
                            Some(e) => fallback(e, reset.clone()),
                            None => v,
                        }
                    }
                }
            }
        }
    })
}

// children 构建:native 用 catch_unwind 把渲染期 panic 转成 Err(msg)(SSR 不因一处 panic 整请求崩);
// wasm(panic=abort,无法 catch_unwind)直接构建 —— 该端的渲染错误走 throw_error/error_reporter 显式通道。
#[cfg(not(target_arch = "wasm32"))]
fn build_children<CH: Fn() -> View>(children: &CH) -> Result<View, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| children())).map_err(panic_message)
}

#[cfg(target_arch = "wasm32")]
fn build_children<CH: Fn() -> View>(children: &CH) -> Result<View, String> {
    Ok(children())
}

#[cfg(not(target_arch = "wasm32"))]
fn panic_message(p: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = p.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = p.downcast_ref::<String>() {
        s.clone()
    } else {
        "渲染时发生未知错误".to_string()
    }
}

// ── Transition(单子元素进出场动画:enter/leave CSS 类 + 延时移除)──

/// `<Transition name="fade" when={ move || cond.get() }>child</Transition>` 的运行时。
/// child **只构建一次**(其内部反应式照常更新);用 CSS 类(配 `@keyframes`,故进场无需双帧 reflow 仪式):
///   · `when` false→true:append child → 去 `{name}-leave` + 加 `{name}-enter`(播放进场动画);
///   · `when` true→false:去 `{name}-enter` + 加 `{name}-leave`(播放出场动画)→ `dur` ms 后**真正移除**。
/// 快速来回切:代际计数令旧的"延时移除"作废(出场途中又被切回则不移除)。
/// 注:动画是客户端行为(SSR 下 set_timeout 为 no-op、不会延时移除)。
/// **建议仅用于 csr 页**(`#[rui::page(csr, ..)]`):child 在渲染期无条件 build,SSR 若 `when()`=false
/// 则 child 被建却不进 HTML(仍消耗 hid)→ 客户端水合时该 child 自身 hid 对不上 SSR DOM 而错位
/// (页面其余节点因 hid 计数两端一致、不受影响)。csr 页无水合,完全规避。
/// 换页若发生在出场延时回调之前:回调仍跑,但 remove_child 对已不在树上的节点是安全 no-op(router.js 有 parentNode 校验)。
pub fn transition<W, B>(name: &str, dur: u32, when: W, build: B) -> u32
where
    W: Fn() -> bool + 'static,
    B: FnOnce() -> View + 'static,
{
    let anchor = dom::el("rui-slot");
    // 直接构建 child(其 effect 归当前页 scope;context 此刻在栈上可见)—— 只建一次,之后只切显隐。
    let child_node = build().0;
    let enter = format!("{name}-enter");
    let leave = format!("{name}-leave");
    let shown = Rc::new(Cell::new(false));
    let gen = Rc::new(Cell::new(0u32));
    effect(move || {
        let on = when();
        if on == shown.get() {
            return; // 状态没变(when 的其它依赖变化)→ 不重复动画
        }
        shown.set(on);
        gen.set(gen.get().wrapping_add(1)); // 每次切换都使上一次的"延时移除"作废
        if on {
            dom::remove_class(child_node, &leave);
            dom::append(anchor, child_node); // 进场:appendChild 移动/挂载(出场途中切回也安全)
            dom::add_class(child_node, &enter);
        } else {
            dom::remove_class(child_node, &enter);
            dom::add_class(child_node, &leave);
            // 抓拍本次出场的代际 gn:出场途中若被切回(再切换会 bump gen),回调里 g.get() != gn → 不移除。
            let (g, gn) = (gen.clone(), gen.get());
            dom::set_timeout(dur, move || {
                if g.get() == gn {
                    dom::remove_child(anchor, child_node); // 期间未被切回 → 出场动画结束,真正移除
                }
            });
        }
    });
    anchor
}

// keyed <For> 的一行:key、根节点 id、item 快照(用于检测内容是否变)、子作用域(行内 effect 的归属)。
struct KeyedRow<T, K> {
    key: K,
    node: u32,
    item: T,
    // 持有以保活行内 effect;行被 drop 时 Scope::drop 自动销毁(故"未读"是有意的)。
    #[allow(dead_code)]
    scope: Scope,
}

/// keyed `<For ... key={...}>` 的运行时 reconciliation(直接挂在父节点下,不加包裹元素 → 表格 `<tr>` 合法)。
/// 依赖变化时按 key 协调:
///   · key 消失   → remove_child + 销毁该行作用域
///   · key 保留且 item 相等 → 复用原节点(append 移动到新位置;DOM 节点不变 → 焦点/选区/动画保留)
///   · key 保留但 item 变   → 重建该行(销毁旧作用域、移除旧节点、构建新行)
///   · key 新增   → 构建新行
/// 重排靠 append(appendChild 会移动已在 DOM 的节点,不销毁 → 保焦点)。
pub fn keyed_for<T, K, KF, BF>(parent: u32, list: Signal<Vec<T>>, key_of: KF, build: BF)
where
    T: Clone + PartialEq + 'static,
    K: PartialEq + Clone + 'static,
    KF: Fn(&T) -> K + 'static,
    BF: Fn(&T) -> u32 + 'static,
{
    let state: Rc<RefCell<Vec<KeyedRow<T, K>>>> = Rc::new(RefCell::new(Vec::new()));
    let st = state.clone();
    let ctx = crate::reactive::capture_contexts(); // 祖先 context 快照(新行/重建行的 use_context 也能见)
    effect(move || {
        let items = list.get();
        let new_keys: Vec<K> = items.iter().map(|i| key_of(i)).collect();
        let old: Vec<KeyedRow<T, K>> = std::mem::take(&mut st.borrow_mut());

        // ① 移除 key 已消失的行(KeyedRow drop → Scope drop → 行内 effect 销毁)。
        let mut keep: Vec<KeyedRow<T, K>> = Vec::new();
        for row in old {
            if new_keys.iter().any(|k| *k == row.key) {
                keep.push(row);
            } else {
                dom::remove_child(parent, row.node);
            }
        }

        // ② 按新顺序复用 / 重建 / 新建,并 append 到正确位置(append 移动已存在节点)。
        let mut next: Vec<KeyedRow<T, K>> = Vec::with_capacity(items.len());
        for (idx, item) in items.iter().enumerate() {
            let key = new_keys[idx].clone();
            if let Some(pos) = keep.iter().position(|r| r.key == key) {
                let row = keep.remove(pos);
                if row.item == *item {
                    dom::append(parent, row.node); // 复用:仅移动顺序,节点不变
                    next.push(row);
                } else {
                    dom::remove_child(parent, row.node); // item 变 → 重建本行
                    let (n, sc) = crate::reactive::with_contexts(&ctx, || scope(|| build(item)));
                    dom::append(parent, n);
                    next.push(KeyedRow { key, node: n, item: item.clone(), scope: sc });
                }
            } else {
                let (n, sc) = crate::reactive::with_contexts(&ctx, || scope(|| build(item))); // 新 key
                dom::append(parent, n);
                next.push(KeyedRow { key, node: n, item: item.clone(), scope: sc });
            }
        }

        // ③ keep 里的残余(如重复 key)清掉。
        for row in keep {
            dom::remove_child(parent, row.node);
        }
        *st.borrow_mut() = next;
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reactive::scope;

    #[test]
    fn event_decode_fields() {
        // 编码顺序:value│checked│key│code│ctrl│shift│alt│meta│clientX│clientY│button│deltaY│files
        //   (│=\u{1f};files 每项 name\u{1e}size\u{1e}type,项间 \u{1d})
        let enc = "hello\u{1f}1\u{1f}Enter\u{1f}Enter\u{1f}1\u{1f}\u{1f}\u{1f}1\u{1f}3.5\u{1f}4\u{1f}2\u{1f}\u{1f}a.txt\u{1e}12\u{1e}text/plain\u{1d}b.png\u{1e}99\u{1e}image/png";
        crate::dom::set_current_event(enc);
        let e = crate::dom::event();
        assert_eq!(e.value, "hello");
        assert!(e.checked);
        assert_eq!(e.key, "Enter");
        assert!(e.ctrl && !e.shift && !e.alt && e.meta);
        assert_eq!(e.client_x, 3.5);
        assert_eq!(e.button, 2);
        assert_eq!(e.files.len(), 2);
        assert_eq!((e.files[0].name.as_str(), e.files[0].size), ("a.txt", 12));
        assert_eq!(e.files[1].mime, "image/png");
        // 空载荷 → 全默认(零参 handler / 非输入元素)
        crate::dom::set_current_event("");
        let d = crate::dom::event();
        assert!(d.value.is_empty() && !d.checked && d.files.is_empty());
    }

    #[test]
    fn error_reporter_noop_outside_boundary() {
        // 不在任何边界内:throw_error 返回 false,error_reporter() 返回 no-op(调用不 panic)。
        let (_r, sc) = scope(|| {
            assert!(!throw_error("无人接住"));
            let rep = error_reporter();
            rep("也不会 panic".to_string());
        });
        sc.dispose();
    }

    #[test]
    fn error_boundary_throw_then_reset() {
        // 子树取上报器 → 上报 → 渲 fallback;reset → 重建子树恢复(端到端,native dom 后端)。
        dom::reset();
        let reporter: Rc<RefCell<Option<Rc<dyn Fn(String)>>>> = Rc::new(RefCell::new(None));
        let resetter: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(None));
        let rep_cell = reporter.clone();
        let res_cell = resetter.clone();
        let (node, _sc) = scope(|| {
            error_boundary(
                move |e, reset| {
                    *res_cell.borrow_mut() = Some(reset); // 存 reset 句柄供测试调用
                    View(dom::text(&format!("ERR:{}", e)))
                },
                move || {
                    *rep_cell.borrow_mut() = Some(error_reporter()); // 子树渲染期取上报器
                    View(dom::text("OK"))
                },
            )
        });
        dom::mount(node);
        assert!(dom::take_html().contains("OK"), "初始应渲正常子树");

        // 模拟事件回调里上报错误。
        (reporter.borrow().clone().unwrap())("boom".to_string());
        let html = dom::take_html();
        assert!(html.contains("ERR:boom"), "上报后应渲 fallback: {html}");

        // reset → children 重建 → 恢复正常子树。
        (resetter.borrow().clone().unwrap())();
        assert!(dom::take_html().contains("OK"), "reset 后应恢复");
    }

    #[test]
    fn set_checked_toggles_checked_attr() {
        // 受控复选框的 SSR 渲染:set_checked(true) 加 `checked` 属性,false 移除(首屏勾选状态可见)。
        dom::reset();
        let n = dom::el("input");
        dom::mount(n);
        dom::set_checked(n, true);
        assert!(dom::take_html().contains("checked"), "true → 应有 checked 属性");
        dom::set_checked(n, false);
        assert!(!dom::take_html().contains("checked"), "false → 应移除 checked 属性");
    }

    #[test]
    fn transition_toggles_enter_leave_classes() {
        // 进出场:show→child 带 fade-enter;切关→换 fade-leave(native set_timeout no-op,故 SSR 不延时移除)。
        dom::reset();
        let show = Signal::new(true);
        let s2 = show.clone();
        let (node, _sc) = scope(|| {
            transition("fade", 300, move || s2.get(), || {
                let d = dom::el("div");
                dom::append(d, dom::text("hi"));
                View(d)
            })
        });
        dom::mount(node);
        let h = dom::take_html();
        assert!(h.contains("hi") && h.contains("fade-enter"), "初始 show:child + fade-enter:{h}");
        show.set(false);
        let h = dom::take_html();
        assert!(h.contains("fade-leave"), "切关:加 fade-leave:{h}");
        assert!(!h.contains("fade-enter"), "切关:去掉 fade-enter:{h}");
    }

    #[test]
    fn native_panic_renders_fallback_exactly_once() {
        // 子树渲染 panic(native catch_unwind 兜)→ fallback 只渲一次(回归:旧实现 err.set 触发重入 → 双调)。
        dom::reset();
        let fb_calls = Rc::new(Cell::new(0));
        let fc = fb_calls.clone();
        let prev_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {})); // 静音预期内的 panic 输出
        let (node, _sc) = scope(|| {
            error_boundary(
                move |e, _reset| {
                    fc.set(fc.get() + 1);
                    View(dom::text(&format!("ERR:{e}")))
                },
                || panic!("boom"),
            )
        });
        std::panic::set_hook(prev_hook);
        dom::mount(node);
        assert_eq!(fb_calls.get(), 1, "fallback 应只渲一次");
        assert!(dom::take_html().contains("ERR:boom"), "应渲出 panic 消息");
    }
}

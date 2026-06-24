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
    {
        let cell = cell.clone();
        let node = node.clone();
        effect(move || {
            let (v, sc) = scope(|| f()); // 子作用域:持有本轮构建产生的内层 effect
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
                    let (n, sc) = scope(|| build(item));
                    dom::append(parent, n);
                    next.push(KeyedRow { key, node: n, item: item.clone(), scope: sc });
                }
            } else {
                let (n, sc) = scope(|| build(item)); // 新 key
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

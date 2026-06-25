//! 响应式核心:Signal(状态) + effect(自动订阅并在依赖变化时重跑) + memo(派生值)。
//! 与 DOM 无关 —— 纯粹的依赖追踪图。
//!
//! 相对最初版本的三处加强(规范化缓存的前置):
//!   · 动态依赖清理:effect 每次重跑前,先从上次订阅的所有 signal 里摘掉自己 —— 否则
//!     条件分支换了依赖后,旧 signal 仍会误触发该 effect(stale 订阅只增不减)。
//!   · dispose:effect/memo 可显式销毁,断开订阅并释放闭包(否则 SPA 路由反复建视图会无限堆积)。
//!   · memo:派生出一个可被再订阅的 Signal —— 规范化缓存的「查询视图」就是 memo。

use std::any::{Any, TypeId};
use std::cell::{Cell, RefCell};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::rc::Rc;

/// signal 的订阅者集合。signal 内部持有它,effect 也引用它(以便重跑/销毁时摘除自己)。
type SubList = Rc<RefCell<Vec<usize>>>;

struct EffectNode {
    f: Rc<dyn Fn()>,
    deps: Vec<SubList>, // 本 effect 当前订阅的所有 signal 的订阅表
    scheduled: bool,    // 已在 flush 队列里(去重:同一轮 flush 内不重复入队)
    // 拓扑高度:源 signal=0,直接 effect/memo=1,依赖更深 memo 的再 +1……取「读到的所有源高度 + 1」之 max。
    // flush 按高度升序排空 → 任一 effect 跑之前,它所有(更浅的)上游 memo 必已重算完 → 无毛刺、不双跑。
    // 与 signal.src_height 共享同一 Cell(memo:本 effect 高度即其输出 signal 的源高度);每次重跑按当前
    // 依赖重新累加(单调增,放心:依赖变深时下次重跑会被对应上游通知触发,届时再 +1 修正高度)。
    height: Rc<Cell<usize>>,
}

thread_local! {
    static CURRENT: Cell<Option<usize>> = const { Cell::new(None) };
    /// id -> effect 节点(None = 已销毁的空槽;id 不复用,保持稳定)。
    static EFFECTS: RefCell<Vec<Option<EffectNode>>> = const { RefCell::new(Vec::new()) };
    /// owner 栈:scope() 期间创建的 effect id 会登记到栈顶,供整组 dispose(路由切换用)。
    static OWNER: RefCell<Vec<Vec<usize>>> = const { RefCell::new(Vec::new()) };
    /// cleanup 栈:scope() 期间 on_cleanup 注册的回调登记到栈顶,scope 销毁时执行(卸载副作用)。
    static CLEANUPS: RefCell<Vec<Vec<Box<dyn FnOnce()>>>> = const { RefCell::new(Vec::new()) };
    /// context 栈:provide_context 写栈顶帧;use_context 自顶向下查(内层覆盖外层),按 TypeId 索引。
    static CONTEXTS: RefCell<Vec<HashMap<TypeId, Rc<dyn Any>>>> = const { RefCell::new(Vec::new()) };
    /// flush 队列(按高度升序的小顶堆 `Reverse<(height, id)>`):set 把订阅者入队(去重),flush 按
    /// 高度从浅到深排空。memo 在 flush 中 set 把下游再入队 —— 因深节点高度严格大于其所有上游 memo,
    /// 故任一 effect 跑之前其上游 memo 必已重算完:单次 set 的菱形依赖(含**不等深**)只跑一次、读到
    /// 一致快照(无毛刺)。同高度按 id(创建序≈依赖序)分先后。FLUSHING 防嵌套 flush;BATCH 计数 batch 深度。
    static PENDING: RefCell<BinaryHeap<Reverse<(usize, usize)>>> = const { RefCell::new(BinaryHeap::new()) };
    static FLUSHING: Cell<bool> = const { Cell::new(false) };
    static BATCH: Cell<u32> = const { Cell::new(0) };
}

/// 注册一个卸载回调:当前 `scope`(页面 / 子树)被销毁时执行 —— 清定时器 / 解绑 / 销毁第三方实例。
/// 不在任何 scope 内调用则忽略(无归属)。服务端:scope 渲染后即 drop,回调随之执行(DOM 操作是 no-op)。
pub fn on_cleanup(f: impl FnOnce() + 'static) {
    CLEANUPS.with(|c| {
        if let Some(top) = c.borrow_mut().last_mut() {
            top.push(Box::new(f));
        }
    });
}

// ── Context(provide / inject:跨层传 theme / store / 当前用户 / i18n,免 prop-drill)──

/// 在当前反应式作用域(页面 / reactive_block 等)提供一个上下文值,后代 `use_context::<T>()` 可取到。
/// 按类型存,故每种类型一个值(要多个用 newtype 区分,如 `struct Theme(Signal<String>)`)。
/// 不在任何作用域内调用则忽略。注:写当前栈顶帧 → 同帧的兄弟节点也能看到(rui 组件是普通 fn 调用,不单独建帧);
/// 要限定到子树用 `provider`。
pub fn provide_context<T: 'static>(value: T) {
    CONTEXTS.with(|c| {
        if let Some(top) = c.borrow_mut().last_mut() {
            top.insert(TypeId::of::<T>(), Rc::new(value));
        }
    });
}

/// 取最近祖先提供的 `T`(自顶向下找第一个);没有则 None。
/// 应在组件 / 页面**渲染期**调用并捕获结果(再在闭包里用);reactive_block / keyed_for / on_mount 会把
/// 上下文快照带进它们的延迟执行,故动态子树(<Show>/<For> 后建节点)+ on_mount 里调用也能看见祖先上下文。
/// 注意:返回的是「存入那一刻」的**克隆** —— 只有 `T` 是 Signal / Rc / Store 等共享句柄时,克隆才与提供方
/// 共享内部、保持响应式;provide 普通值(String / 配置)则拿到死快照(提供方后续改不可见)。凡需实时,
/// 请 provide 一个 Signal / Store 句柄(如 `struct Theme(Signal<String>)`)。
pub fn use_context<T: Clone + 'static>() -> Option<T> {
    CONTEXTS.with(|c| {
        for frame in c.borrow().iter().rev() {
            if let Some(rc) = frame.get(&TypeId::of::<T>()) {
                return rc.downcast_ref::<T>().cloned();
            }
        }
        None
    })
}

/// 上下文栈快照类型(reactive_block / keyed_for / on_mount 用:跨延迟执行保留祖先 context)。
pub(crate) type ContextSnapshot = Vec<HashMap<TypeId, Rc<dyn Any>>>;
/// 捕获当前上下文栈快照(建立时调:把祖先上下文记下,供后续延迟执行复用)。
pub(crate) fn capture_contexts() -> ContextSnapshot {
    CONTEXTS.with(|c| c.borrow().clone())
}
/// 在给定上下文快照下运行 f(临时替换上下文栈;f 内 scope() 会在其上再压自己的帧)。
/// save/restore 用局部变量,可重入(嵌套动态子树各自的 with_contexts 互不干扰)。
pub(crate) fn with_contexts<R>(snapshot: &ContextSnapshot, f: impl FnOnce() -> R) -> R {
    let saved = CONTEXTS.with(|c| std::mem::replace(&mut *c.borrow_mut(), snapshot.clone()));
    let r = f();
    CONTEXTS.with(|c| *c.borrow_mut() = saved);
    r
}
/// 子树局部 provide:在一个临时 context 帧里运行 f —— f 内的 `provide_context` 只对 f 及其(同步构建的)
/// 子树可见,f 返回后帧弹出,不泄漏给兄弟。组件想把 provide 限定在自己子树时用:
///   `provider(|| { provide_context(theme); view!{ .. }.node() })`
/// 注:只压 context 帧(不影响 effect 归属 / dispose);动态子树会快照到这帧,故 provide 也能传进去。
pub fn provider<R>(f: impl FnOnce() -> R) -> R {
    CONTEXTS.with(|c| c.borrow_mut().push(HashMap::new()));
    let r = f();
    CONTEXTS.with(|c| {
        c.borrow_mut().pop();
    });
    r
}

/// 响应式状态单元。在 effect 内 `get()` = 订阅;`set()` = 通知订阅者重跑。
pub struct Signal<T> {
    inner: Rc<RefCell<T>>,
    subs: SubList,
    /// 本 signal 的「源高度」:普通 signal 恒为 0(图的根);memo 输出则共享其重算 effect 的高度 Cell,
    /// 故订阅它的 effect 的高度 ≥ 此值 + 1 → 排在该 memo 之后。见 EffectNode.height。
    src_height: Rc<Cell<usize>>,
}

impl<T> Clone for Signal<T> {
    fn clone(&self) -> Self {
        Signal { inner: self.inner.clone(), subs: self.subs.clone(), src_height: self.src_height.clone() }
    }
}

impl<T: Clone> Signal<T> {
    pub fn new(v: T) -> Self {
        Signal {
            inner: Rc::new(RefCell::new(v)),
            subs: Rc::new(RefCell::new(Vec::new())),
            src_height: Rc::new(Cell::new(0)), // 普通 signal 是图的根,源高度恒 0
        }
    }

    /// 读取。若此刻有 effect 在运行,登记它为订阅者,把本 signal 的订阅表挂到该 effect 的依赖集,
    /// 并把该 effect 的高度抬到 ≥ 本 signal 源高度 + 1(保证调度时排在本 signal 的生产者之后)。
    pub fn get(&self) -> T {
        if let Some(id) = CURRENT.with(|c| c.get()) {
            let first = {
                let mut subs = self.subs.borrow_mut();
                if subs.contains(&id) {
                    false
                } else {
                    subs.push(id);
                    true
                }
            };
            if first {
                // 每轮重跑前 deps 都被清空、本 effect 也从各 signal 摘除,故重跑时此处 first 必为 true
                // → 高度按「当前」依赖重新累加(依赖变深时随之修正)。height 单调取 max,故只增不减。
                EFFECTS.with(|e| {
                    if let Some(Some(node)) = e.borrow_mut().get_mut(id) {
                        node.deps.push(self.subs.clone());
                        let want = self.src_height.get() + 1;
                        if want > node.height.get() {
                            node.height.set(want);
                        }
                    }
                });
            }
        }
        self.inner.borrow().clone()
    }

    /// 写入。更新值,把订阅者入 flush 堆(去重,按高度排序),然后(非 batch / 非 flush 中)立即 flush。
    /// 单次 set:同步跑完订阅者(与旧行为一致,SSR 也照常);订阅者重算时再 set 只入堆、由本轮 flush
    /// 按高度升序处理 → 菱形依赖(含不等深)只跑一次、读到一致快照(无毛刺)。多次 set 想合并请用 `batch`。
    pub fn set(&self, v: T) {
        *self.inner.borrow_mut() = v;
        let subs = self.subs.borrow().clone(); // 先快照,放掉借用
        for id in subs {
            schedule(id);
        }
        if BATCH.with(|b| b.get()) == 0 {
            flush();
        }
    }
}

// 把 effect 入 flush 堆(已在堆里则跳过 = 去重),按其当前高度排序。
fn schedule(id: usize) {
    let height = EFFECTS.with(|e| {
        if let Some(Some(node)) = e.borrow_mut().get_mut(id) {
            if !node.scheduled {
                node.scheduled = true;
                return Some(node.height.get());
            }
        }
        None
    });
    if let Some(h) = height {
        PENDING.with(|p| p.borrow_mut().push(Reverse((h, id))));
    }
}

// 按高度升序排空 flush 堆。flush 内的 set 只入堆(FLUSHING 防嵌套 flush),由本循环继续处理:
// 深节点高度严格大于其上游 memo,故弹到它时上游必已重算 → 无毛刺、每个 effect 单次 set 内只跑一次。
fn flush() {
    if FLUSHING.with(|f| f.get()) {
        return; // 已在 flush 中(嵌套 set)→ 仅入堆,交给正在跑的循环
    }
    FLUSHING.with(|f| f.set(true));
    // debug 构建下设迭代上限:无收敛守卫的自触发 effect(或两 memo/effect 循环互相 set)会让本循环
    // 永不停 —— release 是静默忙等(挂起,难排查),故 debug 下超限即 panic 给出可诊断信息。
    #[cfg(debug_assertions)]
    let mut steps: u64 = 0;
    while let Some(Reverse((_, id))) = PENDING.with(|p| p.borrow_mut().pop()) {
        #[cfg(debug_assertions)]
        {
            steps += 1;
            if steps > 1_000_000 {
                FLUSHING.with(|f| f.set(false));
                panic!(
                    "rui: 响应式 flush 跑了 100 万次仍未收敛 —— 多半是某 effect 无条件 set() 了它自己读的 \
                     signal,或两个 memo/effect 成环且无值相等守卫。请加收敛守卫(如 `if v != cur {{ s.set(v) }}`)。"
                );
            }
        }
        EFFECTS.with(|e| {
            if let Some(Some(node)) = e.borrow_mut().get_mut(id) {
                node.scheduled = false;
            }
        });
        run_effect(id);
    }
    FLUSHING.with(|f| f.set(false));
}

/// 批处理:f 内的多次 `set` 推迟到 f 结束统一 flush 一次(合并下游重算)。
/// f 内读 signal 仍立即拿到新值,但 memo / effect 推迟到 flush(标准 batch 语义)。嵌套只最外层 flush。
pub fn batch<R>(f: impl FnOnce() -> R) -> R {
    BATCH.with(|b| b.set(b.get() + 1));
    let r = f();
    let outermost = BATCH.with(|b| {
        let n = b.get() - 1;
        b.set(n);
        n == 0
    });
    if outermost {
        flush();
    }
    r
}

/// 注册 effect 并立即运行一次(运行期间自动记录它读了哪些 signal)。返回可销毁的句柄。
pub fn effect<F: Fn() + 'static>(f: F) -> EffectHandle {
    effect_with_height(f, Rc::new(Cell::new(0)))
}

/// 同 `effect`,但用调用方给定的高度 Cell —— memo 用它把「重算 effect 的高度」与「输出 signal 的源高度」
/// 绑成同一个 Cell,从而 effect 重跑抬高自己高度时,下游对该 memo 的高度认知自动跟着更新。
fn effect_with_height<F: Fn() + 'static>(f: F, height: Rc<Cell<usize>>) -> EffectHandle {
    let id = EFFECTS.with(|e| {
        let mut e = e.borrow_mut();
        e.push(Some(EffectNode { f: Rc::new(f), deps: Vec::new(), scheduled: false, height }));
        e.len() - 1
    });
    register_owned(id);
    run_effect(id);
    EffectHandle { id }
}

/// 派生值:f 的依赖变化时重算,自身也是一个可被再订阅的 Signal。
/// 规范化缓存的「查询视图」即 memo:从 store 按 selection 重建结果 + 订阅相关 entity。
pub fn memo<T, F>(f: F) -> Signal<T>
where
    T: Clone + PartialEq + 'static,
    F: Fn() -> T + 'static,
{
    // 高度 Cell 由「重算 effect」与「输出 signal」共享:effect 读更深的源时抬高它,下游随即把自己排到本 memo 之后。
    let height = Rc::new(Cell::new(0usize));
    let sig = Signal {
        inner: Rc::new(RefCell::new(untrack(&f))), // 初值非追踪求,避免污染外层 effect 依赖集
        subs: Rc::new(RefCell::new(Vec::new())),
        src_height: height.clone(),
    };
    let out = sig.clone();
    effect_with_height(
        move || {
            let v = f(); // 在本 effect 上下文 → 自动订阅 f 读到的 signal
            // 值相等去抖:派生值真没变就不通知下游 —— 否则无关依赖变化(如 ?q 不变但 ?sort 变)
            // 也会让本 memo 重通知 → 订阅它的 resource! 冗余重取。untrack 读自身值,避免自订阅成环。
            if untrack(|| sig.get()) != v {
                sig.set(v); // 通知本 memo 的下游
            }
        },
        height,
    );
    out
}

/// 在不追踪依赖的上下文里运行 f(读 signal 不会产生订阅)。
pub fn untrack<R>(f: impl FnOnce() -> R) -> R {
    let prev = CURRENT.with(|c| c.replace(None));
    let r = f();
    CURRENT.with(|c| c.set(prev));
    r
}

fn run_effect(id: usize) {
    // 取出闭包,并先清理上次的订阅(动态依赖的关键)。
    let f = EFFECTS.with(|e| {
        let mut e = e.borrow_mut();
        let node = match e.get_mut(id) {
            Some(Some(n)) => n,
            _ => return None, // 已销毁
        };
        for dep in node.deps.drain(..) {
            dep.borrow_mut().retain(|&x| x != id);
        }
        Some(node.f.clone())
    });
    let f = match f {
        Some(f) => f,
        None => return,
    };
    let prev = CURRENT.with(|c| c.replace(Some(id)));
    f(); // 运行期间重新 get → 重新订阅 + 重新填充 deps
    CURRENT.with(|c| c.set(prev));
}

fn dispose_effect(id: usize) {
    // 先取出节点并放掉 EFFECTS 借用,再让它(及其闭包)析构 —— 闭包可能持有子 Scope,
    // 其 Drop 会重入 dispose_effect;若在借用内析构会触发 RefCell 双重借用 panic。
    //
    // 用 try_with:线程结束销毁 thread_local 时,残留的 Scope/effect 闭包会被 drop,从而
    // 重入到这里;此刻 EFFECTS 可能已在析构(每连接一线程的 SSR 必然触发),with 会 panic
    // (TLS 析构期访问 → abort)。整个 arena 反正在拆,访问不到就直接跳过。
    let node = EFFECTS
        .try_with(|e| e.borrow_mut().get_mut(id).and_then(|slot| slot.take()))
        .ok()
        .flatten();
    if let Some(node) = node {
        for dep in node.deps {
            dep.borrow_mut().retain(|&x| x != id);
        }
    }
}

fn register_owned(id: usize) {
    OWNER.with(|o| {
        if let Some(top) = o.borrow_mut().last_mut() {
            top.push(id);
        }
    });
}

/// effect/memo 的销毁句柄。
pub struct EffectHandle {
    id: usize,
}
impl EffectHandle {
    pub fn dispose(self) {
        dispose_effect(self.id);
    }
}

/// 在一个 owner 作用域内运行 f,收集期间创建的所有 effect/memo;
/// 返回的 Scope 可一次性 dispose 全部(路由切换前清理上一页的视图)。
pub struct Scope {
    ids: Vec<usize>,
    cleanups: Vec<Box<dyn FnOnce()>>,
}
impl Scope {
    /// 显式、即时销毁本作用域内全部 effect/memo(等价于让 Scope 离开作用域被 drop)。
    pub fn dispose(self) {}
    /// 取走本 scope 的 effect id + cleanup(取空后让其 drop 即无副作用)。
    pub fn take_parts(&mut self) -> (Vec<usize>, Vec<Box<dyn FnOnce()>>) {
        (std::mem::take(&mut self.ids), std::mem::take(&mut self.cleanups))
    }
    /// 把另一组 effect id + cleanup 并入本 scope(on_mount 回调在子 scope 跑完后并入页面 scope,
    /// 使其中创建的 effect/memo 归当前页所有 → 切页时一并销毁,杜绝幽灵 effect)。
    pub fn absorb_parts(&mut self, ids: Vec<usize>, cleanups: Vec<Box<dyn FnOnce()>>) {
        self.ids.extend(ids);
        self.cleanups.extend(cleanups);
    }
}
// Drop 即销毁:先跑 on_cleanup 回调(节点还在、信号还可读),再 dispose 本组 effect/memo。
// 于是嵌套作用域(如 reactive_block 每次重建的子作用域)在其父 effect 被销毁、闭包随之析构、
// 持有的子 Scope 被 drop 时也会递归清理 —— 无需手工逐层 dispose。
impl Drop for Scope {
    fn drop(&mut self) {
        for c in std::mem::take(&mut self.cleanups) {
            c(); // 卸载回调(clear_interval / 解绑 / 销毁第三方实例)
        }
        for id in std::mem::take(&mut self.ids) {
            dispose_effect(id);
        }
    }
}
pub fn scope<R>(f: impl FnOnce() -> R) -> (R, Scope) {
    OWNER.with(|o| o.borrow_mut().push(Vec::new()));
    CLEANUPS.with(|c| c.borrow_mut().push(Vec::new()));
    CONTEXTS.with(|c| c.borrow_mut().push(HashMap::new())); // 本作用域的 provide_context 写这帧
    let r = f();
    let ids = OWNER.with(|o| o.borrow_mut().pop().unwrap_or_default());
    let cleanups = CLEANUPS.with(|c| c.borrow_mut().pop().unwrap_or_default());
    CONTEXTS.with(|c| {
        c.borrow_mut().pop();
    }); // 上下文仅渲染期用于查找,无需随 Scope 持有
    (r, Scope { ids, cleanups })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memo_recomputes_on_dep_change() {
        let a = Signal::new(1i32);
        let a2 = a.clone();
        let m = memo(move || a2.get() * 10);
        assert_eq!(m.get(), 10);
        a.set(2);
        assert_eq!(m.get(), 20);
    }

    #[test]
    fn dynamic_deps_cleanup() {
        let cond = Signal::new(true);
        let a = Signal::new(0i32);
        let b = Signal::new(0i32);
        let runs = Rc::new(Cell::new(0));
        let (c, a2, b2, r2) = (cond.clone(), a.clone(), b.clone(), runs.clone());
        effect(move || {
            if c.get() {
                let _ = a2.get();
            } else {
                let _ = b2.get();
            }
            r2.set(r2.get() + 1);
        });
        assert_eq!(runs.get(), 1);
        a.set(1); // 依赖 a → 触发
        assert_eq!(runs.get(), 2);
        cond.set(false); // 重跑,改为读 b、不再读 a
        assert_eq!(runs.get(), 3);
        a.set(2); // 旧依赖 a 已清理 → 不应触发
        assert_eq!(runs.get(), 3);
        b.set(1); // 现在依赖 b → 触发
        assert_eq!(runs.get(), 4);
    }

    #[test]
    fn dispose_stops_effect() {
        let a = Signal::new(0i32);
        let runs = Rc::new(Cell::new(0));
        let (a2, r2) = (a.clone(), runs.clone());
        let h = effect(move || {
            let _ = a2.get();
            r2.set(r2.get() + 1);
        });
        assert_eq!(runs.get(), 1);
        a.set(1);
        assert_eq!(runs.get(), 2);
        h.dispose();
        a.set(2);
        assert_eq!(runs.get(), 2); // 已销毁 → 不再跑
    }

    #[test]
    fn scope_dispose_all() {
        let a = Signal::new(0i32);
        let runs = Rc::new(Cell::new(0));
        let (a2, r2) = (a.clone(), runs.clone());
        let (_, sc) = scope(|| {
            effect(move || {
                let _ = a2.get();
                r2.set(r2.get() + 1);
            });
        });
        assert_eq!(runs.get(), 1);
        a.set(1);
        assert_eq!(runs.get(), 2);
        sc.dispose();
        a.set(2);
        assert_eq!(runs.get(), 2); // 整组销毁 → 不再跑
    }

    #[derive(Clone, PartialEq, Debug)]
    struct Theme(&'static str);

    #[test]
    fn context_provide_inject() {
        let (_r, sc) = scope(|| {
            provide_context(Theme("dark"));
            let (_inner, isc) = scope(|| {
                assert_eq!(use_context::<Theme>(), Some(Theme("dark"))); // 祖先可见
            });
            isc.dispose();
        });
        sc.dispose();
        assert_eq!(use_context::<Theme>(), None); // 作用域外:栈空 → None
    }

    #[test]
    fn context_inner_overrides_outer() {
        let (_r, sc) = scope(|| {
            provide_context(Theme("outer"));
            let (_i, isc) = scope(|| {
                provide_context(Theme("inner"));
                assert_eq!(use_context::<Theme>(), Some(Theme("inner")));
            });
            isc.dispose();
            assert_eq!(use_context::<Theme>(), Some(Theme("outer"))); // 内层弹出后恢复
        });
        sc.dispose();
    }

    #[test]
    fn context_same_frame_double_provide_overwrites() {
        let (_r, sc) = scope(|| {
            provide_context(Theme("first"));
            provide_context(Theme("second")); // 同帧同类型 → 永久覆盖
            assert_eq!(use_context::<Theme>(), Some(Theme("second")));
        });
        sc.dispose();
    }

    #[test]
    fn provider_confines_to_subtree() {
        let (_r, sc) = scope(|| {
            provider(|| {
                provide_context(Theme("inner"));
                assert_eq!(use_context::<Theme>(), Some(Theme("inner")));
            });
            assert_eq!(use_context::<Theme>(), None); // provider 帧弹出 → 兄弟看不到
        });
        sc.dispose();
    }

    #[test]
    fn context_snapshot_survives_rerun() {
        // 快照让"动态子树"重建时仍看见祖先 context(模拟 reactive_block:捕获快照 + with_contexts 重跑)。
        let seen: Rc<RefCell<Vec<&'static str>>> = Rc::new(RefCell::new(Vec::new()));
        let trigger = Signal::new(0i32);
        let (snap, sc) = scope(|| {
            provide_context(Theme("ctx"));
            capture_contexts()
        });
        let (t2, seen2) = (trigger.clone(), seen.clone());
        let h = effect(move || {
            let _ = t2.get();
            with_contexts(&snap, || {
                seen2.borrow_mut().push(use_context::<Theme>().map(|t| t.0).unwrap_or("MISSING"));
            });
        });
        sc.dispose();
        trigger.set(1);
        h.dispose();
        assert_eq!(*seen.borrow(), vec!["ctx", "ctx"]); // 首跑 + 重跑都拿到
    }

    #[test]
    fn equal_depth_diamond_no_glitch() {
        // 等深菱形:A→B(memo)、A→C(memo)、B&C→D(effect)。a 变一次,D 只跑一次。
        let a = Signal::new(1i32);
        let (a1, a2) = (a.clone(), a.clone());
        let b = memo(move || a1.get() * 2);
        let c = memo(move || a2.get() + 100);
        let runs = Rc::new(Cell::new(0));
        let seen = Rc::new(RefCell::new(Vec::new()));
        let (r2, s2) = (runs.clone(), seen.clone());
        let _h = effect(move || {
            r2.set(r2.get() + 1);
            s2.borrow_mut().push((b.get(), c.get()));
        });
        assert_eq!(runs.get(), 1);
        a.set(5);
        assert_eq!(runs.get(), 2); // 只再跑一次(无毛刺)
        assert_eq!(*seen.borrow(), vec![(2, 101), (10, 105)]); // 末次 B,C 同步一致
    }

    #[test]
    fn unequal_depth_diamond_no_glitch() {
        // 不等深菱形(回归:FIFO 调度会让 D 双跑且首跑读到 stale C;高度调度修复)。
        //   a → d 直达(0 跳);a → b(memo)→ c(memo)→ d(2 跳)。
        // 高度:b=1,c=2,d=max(读a→1, 读c→3)=3。flush 按高度升序 → d 在 c 重算后才跑,且只跑一次。
        let a = Signal::new(1i32);
        let (ab, ad) = (a.clone(), a.clone());
        let b = memo(move || ab.get() * 2); // b = a*2
        let bc = b.clone();
        let c = memo(move || bc.get() + 1); // c = b+1 = a*2+1
        let runs = Rc::new(Cell::new(0));
        let seen = Rc::new(RefCell::new(Vec::new()));
        let (r2, s2) = (runs.clone(), seen.clone());
        let _h = effect(move || {
            r2.set(r2.get() + 1);
            s2.borrow_mut().push((ad.get(), c.get())); // 读 a(直达)+ c(2 跳)
        });
        assert_eq!(runs.get(), 1);
        assert_eq!(*seen.borrow(), vec![(1, 3)]); // 初:a=1, c=3
        a.set(5);
        assert_eq!(runs.get(), 2); // 只再跑一次(FIFO 会是 3 = 双跑)
        assert_eq!(*seen.borrow(), vec![(1, 3), (5, 11)]); // 一致快照:无 (5,3) 这种 stale 中间帧
    }

    #[test]
    fn self_set_with_guard_converges() {
        // 自触发 effect + 收敛守卫:迭代式跑到稳定(不爆栈、不挂起)。无守卫则会触发 flush 迭代上限 panic。
        let s = Signal::new(0i32);
        let s2 = s.clone();
        let _h = effect(move || {
            let v = s2.get();
            if v < 3 {
                s2.set(v + 1); // 收敛守卫:到 3 停
            }
        });
        assert_eq!(s.get(), 3);
    }

    #[test]
    fn batch_coalesces_sets() {
        let a = Signal::new(0i32);
        let b = Signal::new(0i32);
        let runs = Rc::new(Cell::new(0));
        let (a2, b2, r2) = (a.clone(), b.clone(), runs.clone());
        let _h = effect(move || {
            let _ = (a2.get(), b2.get());
            r2.set(r2.get() + 1);
        });
        assert_eq!(runs.get(), 1);
        a.set(1);
        b.set(1);
        assert_eq!(runs.get(), 3); // 不 batch:两次 set 各跑一次
        batch(|| {
            a.set(2);
            b.set(2);
        });
        assert_eq!(runs.get(), 4); // batch:合并成一次
    }
}

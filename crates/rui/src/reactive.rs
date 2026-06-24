//! 响应式核心:Signal(状态) + effect(自动订阅并在依赖变化时重跑) + memo(派生值)。
//! 与 DOM 无关 —— 纯粹的依赖追踪图。
//!
//! 相对最初版本的三处加强(规范化缓存的前置):
//!   · 动态依赖清理:effect 每次重跑前,先从上次订阅的所有 signal 里摘掉自己 —— 否则
//!     条件分支换了依赖后,旧 signal 仍会误触发该 effect(stale 订阅只增不减)。
//!   · dispose:effect/memo 可显式销毁,断开订阅并释放闭包(否则 SPA 路由反复建视图会无限堆积)。
//!   · memo:派生出一个可被再订阅的 Signal —— 规范化缓存的「查询视图」就是 memo。

use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// signal 的订阅者集合。signal 内部持有它,effect 也引用它(以便重跑/销毁时摘除自己)。
type SubList = Rc<RefCell<Vec<usize>>>;

struct EffectNode {
    f: Rc<dyn Fn()>,
    deps: Vec<SubList>, // 本 effect 当前订阅的所有 signal 的订阅表
}

thread_local! {
    static CURRENT: Cell<Option<usize>> = const { Cell::new(None) };
    /// id -> effect 节点(None = 已销毁的空槽;id 不复用,保持稳定)。
    static EFFECTS: RefCell<Vec<Option<EffectNode>>> = const { RefCell::new(Vec::new()) };
    /// owner 栈:scope() 期间创建的 effect id 会登记到栈顶,供整组 dispose(路由切换用)。
    static OWNER: RefCell<Vec<Vec<usize>>> = const { RefCell::new(Vec::new()) };
}

/// 响应式状态单元。在 effect 内 `get()` = 订阅;`set()` = 通知订阅者重跑。
pub struct Signal<T> {
    inner: Rc<RefCell<T>>,
    subs: SubList,
}

impl<T> Clone for Signal<T> {
    fn clone(&self) -> Self {
        Signal { inner: self.inner.clone(), subs: self.subs.clone() }
    }
}

impl<T: Clone> Signal<T> {
    pub fn new(v: T) -> Self {
        Signal { inner: Rc::new(RefCell::new(v)), subs: Rc::new(RefCell::new(Vec::new())) }
    }

    /// 读取。若此刻有 effect 在运行,登记它为订阅者,并把本 signal 的订阅表挂到该 effect 的依赖集。
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
                EFFECTS.with(|e| {
                    if let Some(Some(node)) = e.borrow_mut().get_mut(id) {
                        node.deps.push(self.subs.clone());
                    }
                });
            }
        }
        self.inner.borrow().clone()
    }

    /// 写入。更新值,然后只重跑订阅它的 effect。
    pub fn set(&self, v: T) {
        *self.inner.borrow_mut() = v;
        let subs = self.subs.borrow().clone(); // 先快照,放掉借用再跑 effect
        for id in subs {
            run_effect(id);
        }
    }
}

/// 注册 effect 并立即运行一次(运行期间自动记录它读了哪些 signal)。返回可销毁的句柄。
pub fn effect<F: Fn() + 'static>(f: F) -> EffectHandle {
    let id = EFFECTS.with(|e| {
        let mut e = e.borrow_mut();
        e.push(Some(EffectNode { f: Rc::new(f), deps: Vec::new() }));
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
    T: Clone + 'static,
    F: Fn() -> T + 'static,
{
    let sig = Signal::new(untrack(&f)); // 初值非追踪求,避免污染外层 effect 依赖集
    let out = sig.clone();
    effect(move || {
        let v = f(); // 在本 effect 上下文 → 自动订阅 f 读到的 signal
        sig.set(v); // 通知本 memo 的下游
    });
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
    EFFECTS.with(|e| {
        let mut e = e.borrow_mut();
        if let Some(slot) = e.get_mut(id) {
            if let Some(node) = slot.take() {
                for dep in node.deps {
                    dep.borrow_mut().retain(|&x| x != id);
                }
            }
        }
    });
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
}
impl Scope {
    pub fn dispose(self) {
        for id in self.ids {
            dispose_effect(id);
        }
    }
}
pub fn scope<R>(f: impl FnOnce() -> R) -> (R, Scope) {
    OWNER.with(|o| o.borrow_mut().push(Vec::new()));
    let r = f();
    let ids = OWNER.with(|o| o.borrow_mut().pop().unwrap_or_default());
    (r, Scope { ids })
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
}

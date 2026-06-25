use crate::data::model::Todo;
use crate::view::components::{Filter, Greeting, TodoView};
use rui::reactive::{memo, Signal};
use rui::{mutation, subscription, view};

#[rui::page("/")] // ssr:首屏服务端渲 + 注入数据 + 客户端水合
pub fn view() -> rui::View {
    // Context:页面 provide 一个 Greeting(Signal),深层 <GreetingBadge/> inject 并可写回(内联改名)。
    rui::provide_context(Greeting(Signal::new(String::from("rui"))));

    // 列表 = 订阅(实时,真相源)。服务端任何写操作都广播 → 推新全表 → 增删改自动反映。
    let todos = subscription!(todo_updates { ...TodoView });
    let filter = Signal::new(Filter::All);
    let show_tips = Signal::new(true); // 顶部可关闭提示条(<Transition> 离场动画演示)

    // memo:过滤后的列表 + 计数
    let filtered = {
        let (t, f) = (todos.clone(), filter.clone());
        memo(move || t.get().into_iter().filter(|r| f.get().keep(r.TodoView.done)).collect::<Vec<_>>())
    };
    let total = {
        let t = todos.clone();
        memo(move || t.get().len() as i64)
    };
    let active = {
        let t = todos.clone();
        memo(move || t.get().iter().filter(|r| !r.TodoView.done).count() as i64)
    };
    // Counters 在 ErrorBoundary 之外用:预克隆一份(StatusBanner 在 boundary 的 move 闭包里会 move 走 total/active)。
    let (counter_total, counter_active) = (total.clone(), active.clone());

    // 批量动作(mutation! + 乐观更新):全部完成对已有实体做 field 更新,瞬时勾上。
    let complete = todos.clone();
    let complete_all = mutation!(complete, complete_all() { id text done },
        optimistic: complete.get().iter()
            .map(|r| Todo { id: r.TodoView.id.clone(), text: r.TodoView.text.clone(), done: true })
            .collect::<Vec<Todo>>());
    // 失败横幅:mutation! 的 on_error 把错误写进它(GraphQL errors / 网络失败时显示)。
    let err: Signal<Option<String>> = Signal::new(None);
    let clear = {
        let e = err.clone();
        let m = mutation!(todos, clear_done() { id text done }, on_error: { let e = e.clone(); move |msg: String| e.set(Some(msg)) });
        // 每次重试先清旧横幅(mutation! 无 on_success 钩子,故在调用点清);失败时 on_error 再置上。
        move || {
            e.set(None);
            m();
        }
    };

    view! {
        <div class="flex flex-col gap-5">
            <div>
                <h1 class="text-3xl font-bold tracking-tight">"待办清单"</h1>
                <p class="mt-1 text-sm text-slate-400">"rui 全特性展示:订阅实时列表 · 乐观更新 · 受控复选框 · 进场动画 · 表单校验 · Context · ErrorBoundary"</p>
            </div>

            // 可关闭提示条:<Transition> 默认显示(SSR 安全),点 × 走离场淡出动画后移除。
            <Transition name="fade" duration=300 when={ let s = show_tips.clone(); move || s.get() }>
                <div class="flex items-start justify-between gap-3 rounded-lg border border-slate-800 bg-slate-900/60 px-4 py-3 text-sm text-slate-400">
                    <span>"✨ 本页用上了:bind:checked 复选框 · 列表进场动画 · 表单校验 · Context 内联改名(点右下 ✎)· ErrorBoundary 兜底 · 订阅 + 乐观更新"</span>
                    <button class="shrink-0 text-slate-500 hover:text-white transition-all" on:click={ let s = show_tips.clone(); move || s.set(false) }>"×"</button>
                </div>
            </Transition>

            <AddForm add={ Box::new(move |txt: String| { mutation!(todos, add_todo(text: txt) { id })(); }) } />

            // 错误横幅:有错才渲(Option<View>),.toast-enter 淡入,× 可手动关闭。
            { let e = err.clone(); move || e.get().map(|m| view! {
                <div class="toast-enter flex items-center justify-between gap-3 rounded-lg bg-rose-500/15 px-3 py-2 text-sm text-rose-300">
                    <span>{ format!("操作失败:{}", m) }</span>
                    <button class="shrink-0 hover:text-rose-200 transition-all" on:click={ let e = e.clone(); move || e.set(None) }>"×"</button>
                </div>
            }) }

            <Toolbar filter={ filter.clone() } complete={ Box::new(complete_all) } clear={ Box::new(clear) } />

            // ErrorBoundary:清单子树若渲染/逻辑出错 → 局部 fallback + 重试(而非整页崩)。
            <ErrorBoundary fallback={ |e: String, reset: std::rc::Rc<dyn Fn()>| view! {
                <div class="flex flex-col gap-2 rounded-2xl border border-rose-500/40 bg-rose-500/10 p-5">
                    <p class="font-semibold text-rose-200">"清单渲染出错"</p>
                    <p class="text-sm text-rose-300">{ e }</p>
                    <button class="self-start rounded-lg bg-rose-500/20 px-3 py-1.5 text-sm text-rose-200 hover:bg-rose-500/30 transition-all"
                        on:click={ move || reset() }>"重试"</button>
                </div>
            } }>
                <Panel title="清单">
                    <GreetingBadge /> // 深层组件,经 context 取/写页面 provide 的 Greeting(无 props 传递)
                    <StatusBanner total={ total.clone() } active={ active.clone() } />
                    // 列表 / 空状态(行类型 __Row 不可命名,故 For 留在页面;每行 TodoItem 用 mutation! 运行时参数)
                    { let fl = filtered.clone(); move || if fl.get().is_empty() {
                        view! { <p class="px-4 py-8 text-center text-slate-600">"(此筛选下没有待办)"</p> }
                    } else {
                        let rows = fl.clone();
                        view! {
                            <ul>
                                <For list=rows item=t key={ t.TodoView.id.clone() }>
                                    <TodoItem
                                        todo={ t.TodoView.clone() }
                                        toggle={ Box::new({ let id = t.TodoView.id.clone(); mutation!(todos, toggle_todo(id: id) { id done }) }) }
                                        remove={ Box::new({ let id = t.TodoView.id.clone(); mutation!(todos, remove_todo(id: id) { id }) }) }
                                    />
                                </For>
                            </ul>
                        }
                    } }
                </Panel>
            </ErrorBoundary>

            <Counters total={ counter_total.clone() } active={ counter_active.clone() } />
        </div>
    }
}

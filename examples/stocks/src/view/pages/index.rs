use crate::data::model::Todo;
use crate::view::components::{Filter, TodoView};
use rui::reactive::{memo, Signal};
use rui::{mutation, subscription, view};

#[rui::page("/")] // ssr:首屏服务端渲 + 注入数据 + 客户端水合
pub fn view() -> rui::View {
    // 列表 = 订阅(实时,真相源)。服务端任何写操作都广播 → 推新全表 → 增删改自动反映。
    let todos = subscription!(todo_updates { ...TodoView });
    let filter = Signal::new(Filter::All);

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
                <p class="mt-1 text-sm text-slate-400">"组件化:AddForm · Toolbar · StatusBanner · TodoItem · Counters"</p>
            </div>

            <AddForm add={ Box::new(move |txt: String| { mutation!(todos, add_todo(text: txt) { id })(); }) } />

            // 错误横幅:有错才渲(Option<View>:Some→渲染,None→空),mutation! on_error 写入。
            { let e = err.clone(); move || e.get().map(|m| view! {
                <p class="rounded-lg bg-rose-500/15 px-3 py-2 text-sm text-rose-300">{ format!("操作失败:{}", m) }</p>
            }) }

            <Toolbar filter={ filter.clone() } complete={ Box::new(complete_all) } clear={ Box::new(clear) } />

            <Panel title="清单">
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

            <Counters total={ total.clone() } active={ active.clone() } />
        </div>
    }
}

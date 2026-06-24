//! 可复用组件:`#[rui::component]`(具名 props + children 槽 + 闭包/signal props)。
//! 组件各自拥有自己的 props,所以不用在页面里到处 clone。

use rui::reactive::Signal;
use rui::{component, view};
use std::cell::Cell;
use std::rc::Rc;

// 过滤器(页面与 Toolbar 共享)。
#[derive(Clone, Copy, PartialEq)]
pub enum Filter {
    All,
    Active,
    Done,
}
impl Filter {
    pub fn keep(self, done: bool) -> bool {
        match self {
            Filter::All => true,
            Filter::Active => !done,
            Filter::Done => done,
        }
    }
}

// Relay 式片段 + data masking:TodoItem 声明自己要的数据(id/text/done)。
rui::fragment!(TodoView on Todo { id text done });

// 一行待办:数据走片段;toggle/remove 是闭包 props(组件不直接发请求,解耦)。
#[component]
pub fn todo_item(todo: TodoView, toggle: Box<dyn Fn()>, remove: Box<dyn Fn()>) -> rui::View {
    let done = todo.done;
    let href = format!("/todo/{}", todo.id); // 点文本进详情页(SPA 导航 → 路由参数 signal)
    view! {
        <li class="flex items-center gap-3 px-4 py-3 border-t border-slate-800/70">
            <button
                class={ if done { "h-5 w-5 shrink-0 rounded border border-emerald-400 bg-emerald-500/30 text-emerald-300 text-xs" } else { "h-5 w-5 shrink-0 rounded border border-slate-600 text-transparent text-xs" } }
                on:click={ move || toggle() }>{ if done { "✓" } else { "·" } }</button>
            <a href={ href }
                class={ if done { "flex-1 text-slate-500 line-through hover:text-slate-300 transition-colors" } else { "flex-1 text-slate-100 hover:text-white transition-colors" } }>
                { todo.text.clone() }
            </a>
            <button class="text-slate-500 hover:text-rose-400 transition-colors" on:click={ move || remove() }>"×"</button>
        </li>
    }
}

// 新增表单:自带 draft signal;提交(回车 / 点按钮)调 add(text)。
// 生命周期演示:ref + on_mount → 进入页面自动聚焦输入框。
#[component]
pub fn add_form(add: Box<dyn Fn(String)>) -> rui::View {
    let draft = Signal::new(String::new());
    let input = rui::node_ref();
    rui::on_mount({
        let input = input.clone();
        move || rui::dom::focus(input.get()) // 节点入 DOM 后聚焦(命令式)
    });
    view! {
        <form class="flex gap-2"
            on:submit={ let d = draft.clone(); move || { let t = d.get(); if !t.trim().is_empty() { add(t); d.set(String::new()); } } }>
            <input ref={input} class="flex-1 rounded-lg bg-slate-800 px-3 py-2 outline-none placeholder:text-slate-500"
                placeholder="加一个待办,回车添加…" bind:value={draft} />
            <button class="rounded-lg bg-slate-100 px-4 py-2 font-medium text-slate-900 hover:bg-white transition-colors">"添加"</button>
        </form>
    }
}

// 运行计时:on_mount 启 setInterval 每秒 +1;on_cleanup 离开页面时 clearInterval(否则定时器泄漏)。
#[component]
pub fn uptime() -> rui::View {
    let secs = Signal::new(0i64);
    let timer: Rc<Cell<u32>> = Rc::new(Cell::new(0)); // 持有 timer id 供 cleanup
    rui::on_mount({
        let (secs, timer) = (secs.clone(), timer.clone());
        move || {
            let id = rui::dom::set_interval(1000, move || secs.set(secs.get() + 1));
            timer.set(id);
        }
    });
    rui::on_cleanup({
        let timer = timer.clone();
        move || rui::dom::clear_interval(timer.get())
    });
    view! {
        <span class="rounded-md bg-slate-800/70 px-2 py-1 text-xs tabular-nums text-slate-400">
            { move || format!("⏱ {}s", secs.get()) }
        </span>
    }
}

fn tab_cls(active: bool) -> &'static str {
    if active {
        "rounded-md bg-slate-700 px-3 py-1 text-sm font-medium text-white"
    } else {
        "rounded-md px-3 py-1 text-sm text-slate-400 hover:text-white transition-colors"
    }
}

// 工具条:过滤 tab(响应式属性高亮)+ 批量动作(闭包 props)。
#[component]
pub fn toolbar(filter: Signal<Filter>, complete: Box<dyn Fn()>, clear: Box<dyn Fn()>) -> rui::View {
    view! {
        <div class="flex items-center gap-2">
            <button class={ let f = filter.clone(); move || tab_cls(f.get() == Filter::All) }
                on:click={ let f = filter.clone(); move || f.set(Filter::All) }>"全部"</button>
            <button class={ let f = filter.clone(); move || tab_cls(f.get() == Filter::Active) }
                on:click={ let f = filter.clone(); move || f.set(Filter::Active) }>"未完成"</button>
            <button class={ let f = filter.clone(); move || tab_cls(f.get() == Filter::Done) }
                on:click={ let f = filter.clone(); move || f.set(Filter::Done) }>"已完成"</button>
            <span class="ml-auto"></span>
            <button class="rounded-md px-3 py-1 text-sm text-slate-400 hover:text-white transition-colors"
                on:click={ move || complete() }>"全部完成"</button>
            <button class="rounded-md px-3 py-1 text-sm text-slate-400 hover:text-rose-400 transition-colors"
                on:click={ move || clear() }>"清除已完成"</button>
        </div>
    }
}

// 状态横幅:Switch 三态(空 / 全完成 / 还剩 N)。
#[component]
pub fn status_banner(total: Signal<i64>, active: Signal<i64>) -> rui::View {
    view! {
        <Switch>
            <Match when={ let t = total.clone(); move || t.get() == 0 }>
                <p class="px-4 py-3 text-sm text-slate-500">"还没有待办,加一个吧 👆"</p>
            </Match>
            <Match when={ let a = active.clone(); move || a.get() == 0 }>
                <p class="px-4 py-3 text-sm text-emerald-300">"全部完成 🎉"</p>
            </Match>
            <Match when={ move || true }>
                <p class="px-4 py-3 text-sm text-slate-400">{ let a = active.clone(); move || format!("还剩 {} 项未完成", a.get()) }</p>
            </Match>
        </Switch>
    }
}

// 统计行(memo + 响应式文本)。
#[component]
pub fn counters(total: Signal<i64>, active: Signal<i64>) -> rui::View {
    view! {
        <p class="text-center text-sm text-slate-500">
            { move || format!("共 {} 项 · {} 未完成 · {} 已完成", total.get(), active.get(), total.get() - active.get()) }
        </p>
    }
}

// 带 children 槽的容器组件:<Panel title="..">任意子节点</Panel>。
#[component]
pub fn panel(title: String, children: rui::View) -> rui::View {
    view! {
        <div class="overflow-hidden rounded-2xl border border-slate-800 bg-slate-900/60">
            <div class="border-b border-slate-800 px-5 py-3 text-sm font-semibold text-slate-400">{ title }</div>
            { children }
        </div>
    }
}

// 统计小卡(具名 props,关于页用)。
#[component]
pub fn stat(label: String, value: String) -> rui::View {
    view! {
        <div class="rounded-xl border border-slate-800 bg-slate-900/60 px-4 py-3 text-center">
            <div class="text-2xl font-bold tabular-nums">{ value }</div>
            <div class="mt-0.5 text-xs text-slate-500">{ label }</div>
        </div>
    }
}

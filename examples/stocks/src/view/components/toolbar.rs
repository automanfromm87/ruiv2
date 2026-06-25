use super::shared::Filter;
use rui::reactive::Signal;
use rui::{component, view};

fn tab_cls(active: bool) -> &'static str {
    if active {
        "rounded-md bg-slate-700 px-3 py-1 text-sm font-medium text-white border-b-2 border-emerald-400 transition-all"
    } else {
        "rounded-md px-3 py-1 text-sm text-slate-400 hover:text-white border-b-2 border-transparent transition-all"
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

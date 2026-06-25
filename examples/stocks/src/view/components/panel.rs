use rui::{component, view};

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

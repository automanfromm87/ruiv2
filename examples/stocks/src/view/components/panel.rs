use rui::{component, view};

// 带 children 槽的容器组件:<Panel title="..">任意子节点</Panel>。
// 可选 prop 演示:`subtitle` 用 #[prop(default = ...)] 标注 → 调用点可省略(默认空、不渲染),
// 也可 <Panel title=".." subtitle="..">。必填 prop(title/children)漏填仍是编译错(typed builder)。
#[component]
pub fn panel(
    title: String,
    #[prop(default = String::new())] subtitle: String,
    children: rui::View,
) -> rui::View {
    // subtitle 是普通 String prop(非 signal)→ 渲染期定一次:空则不渲。
    let sub = if subtitle.is_empty() {
        rui::View(rui::dom::text(""))
    } else {
        view! { <span class="ml-2 text-xs font-normal text-slate-500">{ subtitle }</span> }
    };
    view! {
        <div class="overflow-hidden rounded-2xl border border-slate-800 bg-slate-900/60">
            <div class="border-b border-slate-800 px-5 py-3 text-sm font-semibold text-slate-400">{ title } { sub }</div>
            { children }
        </div>
    }
}

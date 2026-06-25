use rui::reactive::Signal;
use rui::{component, view};

// 统计:三个 stat 徽章(响应式文本)。
#[component]
pub fn counters(total: Signal<i64>, active: Signal<i64>) -> rui::View {
    let badge = "rounded-md bg-slate-800/60 px-3 py-1 text-sm text-slate-400 tabular-nums";
    view! {
        <div class="flex items-center justify-center gap-2">
            <span class={badge}>{ let t = total.clone(); move || format!("共 {}", t.get()) }</span>
            <span class={badge}>{ let a = active.clone(); move || format!("未完成 {}", a.get()) }</span>
            <span class={badge}>{ move || format!("已完成 {}", total.get() - active.get()) }</span>
        </div>
    }
}

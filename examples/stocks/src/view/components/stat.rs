use rui::{component, view};

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

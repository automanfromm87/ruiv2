//! 仪表盘总览 —— 路由组 /dash 的成员(模式 "/" → "/dash")。演示嵌套布局:
//! 它渲在 dash_shell 的内层 outlet 里;/dash ↔ /dash/settings 切换时 dash_shell 侧栏不重建。

use rui::{query, view};

#[rui::page("/")] // 组内相对模式;实际路径 /dash
pub fn view() -> rui::View {
    let todos = query!(todos { id done }); // 复用 query!:统计待办(SSR 首屏带数据)
    view! {
        <div class="flex flex-col gap-4">
            <h2 class="text-xl font-semibold">"总览"</h2>
            <div class="grid grid-cols-3 gap-3">
                <div class="rounded-xl border border-slate-800 bg-slate-900/60 px-4 py-3 text-center">
                    <div class="text-2xl font-bold tabular-nums">{ let t = todos.clone(); move || format!("{}", t.get().len()) }</div>
                    <div class="mt-0.5 text-xs text-slate-500">"全部"</div>
                </div>
                <div class="rounded-xl border border-slate-800 bg-slate-900/60 px-4 py-3 text-center">
                    <div class="text-2xl font-bold tabular-nums text-emerald-400">{ let t = todos.clone(); move || format!("{}", t.get().iter().filter(|r| r.done).count()) }</div>
                    <div class="mt-0.5 text-xs text-slate-500">"已完成"</div>
                </div>
                <div class="rounded-xl border border-slate-800 bg-slate-900/60 px-4 py-3 text-center">
                    <div class="text-2xl font-bold tabular-nums">{ let t = todos.clone(); move || format!("{}", t.get().iter().filter(|r| !r.done).count()) }</div>
                    <div class="mt-0.5 text-xs text-slate-500">"未完成"</div>
                </div>
            </div>
            <p class="text-sm text-slate-400">"这是 /dash —— 与 /dash/settings 共享左侧 dash_shell 侧栏(切换不重建侧栏)。"</p>
        </div>
    }
}

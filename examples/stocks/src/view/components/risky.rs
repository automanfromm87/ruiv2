//! ErrorBoundary 演示的子组件:渲染期经 `error_reporter()`(读最近边界的 Context sink)取一个上报器,
//! 点按钮时在事件里调它上报错误 —— 错误跨组件边界冒泡到最近 `<ErrorBoundary>` → 渲 fallback。
//! reset 后本组件随 children 重建恢复正常子树。

use rui::view;

#[rui::component]
pub fn risky_panel() -> rui::View {
    // 渲染期(此刻栈上有边界的 sink)取上报器闭包;之后在事件回调里调即可(无需届时还在 context 栈上)。
    let report = rui::error_reporter();
    view! {
        <div class="rounded-lg border border-slate-800 bg-slate-900/60 p-4 flex flex-col gap-2">
            <p class="text-slate-300">"正常子树内容 —— 点下面按钮模拟子树抛错。"</p>
            <button class="self-start rounded-lg bg-slate-800 px-3 py-1.5 text-sm hover:bg-slate-700"
                on:click={ move || report("RiskyPanel 主动上报的错误".to_string()) }>
                "触发错误"
            </button>
        </div>
    }
}

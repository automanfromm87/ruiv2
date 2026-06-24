//! 仪表盘设置 —— 路由组 /dash 的成员(模式 "/settings" → "/dash/settings")。
//! 自带本地草稿 signal + bind:value:切到 /dash 再切回来若侧栏没重建,但本页是新建(内容随 outlet 换)。

use rui::reactive::Signal;
use rui::view;

#[rui::page("/settings")] // 组内相对模式;实际路径 /dash/settings
pub fn view() -> rui::View {
    let name = Signal::new(String::from("rui"));
    view! {
        <div class="flex flex-col gap-4">
            <h2 class="text-xl font-semibold">"设置"</h2>
            <label class="flex flex-col gap-1 text-sm text-slate-400">
                "显示名"
                <input class="rounded-lg bg-slate-800 px-3 py-2 text-slate-100 outline-none" bind:value={name} />
            </label>
            <p class="text-sm text-slate-400">{ let n = name.clone(); move || format!("你好,{} 👋(这是 /dash/settings)", n.get()) }</p>
        </div>
    }
}

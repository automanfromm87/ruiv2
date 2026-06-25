//! 过渡 / 动画演示:`<Transition>` 单子元素进出场。
//! 机制:CSS `@keyframes`(`fade-enter` / `fade-leave` 类,见 web/styles.css)+ 出场后延时移除节点。
//! child 只构建一次,when 切换显隐;出场动画播完(duration)才真正从 DOM 移除。

use rui::reactive::Signal;
use rui::view;

#[rui::page(csr, "/transitions")] // 纯客户端:动画是客户端行为
pub fn view() -> rui::View {
    let show = Signal::new(true);

    view! {
        <div class="flex flex-col gap-5">
            <div>
                <h1 class="text-3xl font-bold tracking-tight">"过渡 / 动画"</h1>
                <p class="mt-1 text-sm text-slate-400">
                    "<Transition>:单子元素进出场 · CSS @keyframes(fade-enter / fade-leave)· 出场后延时移除"
                </p>
            </div>

            <button class="self-start rounded-lg bg-slate-800 px-4 py-2 text-sm hover:bg-slate-700"
                on:click={ let s = show.clone(); move || s.set(!s.get()) }>
                "切换"
            </button>

            <Transition name="fade" duration=300 when={ let s = show.clone(); move || s.get() }>
                <div class="rounded-lg border border-emerald-500/40 bg-emerald-500/10 p-6 text-emerald-200">
                    "👋 我会淡入 / 淡出 —— 出场动画播完才从 DOM 移除"
                </div>
            </Transition>
        </div>
    }
}

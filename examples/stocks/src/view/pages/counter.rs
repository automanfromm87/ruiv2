use rui::reactive::Signal;
use rui::view;

pub fn view() -> u32 {
    let n = Signal::new(0i32);

    view! {
        <div class="rounded-2xl border border-slate-800 bg-slate-900/60 p-8 text-center">
            <h1 class="text-2xl font-semibold">"计数器"</h1>
            <p class="mt-2 text-slate-400">"signal + 条件(响应式文本)"</p>
            <p class="mt-6 text-6xl font-bold tabular-nums">
                { let c = n.clone(); move || c.get() }
            </p>
            // 条件渲染:用响应式文本(闭包按 signal 返回不同字符串)
            <p class="mt-3 text-slate-400">
                { let c = n.clone(); move || if c.get() > 0 { "正数 👍" } else { "零或负数" } }
            </p>
            <div class="mt-6 flex justify-center gap-3">
                <button class="rounded-lg bg-slate-800 px-5 py-2 text-lg hover:bg-slate-700 transition-colors"
                    on:click={ let c = n.clone(); move || c.set(c.get() - 1) }>"-1"</button>
                <button class="rounded-lg bg-slate-100 px-5 py-2 text-lg font-medium text-slate-900 hover:bg-white transition-colors"
                    on:click={ let c = n.clone(); move || c.set(c.get() + 1) }>"+1"</button>
            </div>
        </div>
    }
}

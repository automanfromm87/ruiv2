use rui::view;

pub fn view() -> u32 {
    view! {
        <div>
            <h1 class="text-3xl font-bold tracking-tight">"欢迎来到 rui"</h1>
            <p class="mt-3 text-slate-400">"全部用 view! 宏(纯 Rust)写 —— 无 .html / 无 .types / 无 compile.mjs。"</p>
            <div class="mt-8 grid grid-cols-2 gap-4">
                <a href="/counter" class="rounded-xl border border-slate-800 bg-slate-900/60 p-5 hover:border-slate-600 transition-colors">
                    <div class="text-lg font-semibold">"计数器"</div>
                    <div class="mt-1 text-sm text-slate-400">"signal + 条件渲染"</div>
                </a>
                <a href="/table" class="rounded-xl border border-slate-800 bg-slate-900/60 p-5 hover:border-slate-600 transition-colors">
                    <div class="text-lg font-semibold">"表格"</div>
                    <div class="mt-1 text-sm text-slate-400">"fetch + 响应式 For"</div>
                </a>
                <a href="/about" class="rounded-xl border border-slate-800 bg-slate-900/60 p-5 hover:border-slate-600 transition-colors">
                    <div class="text-lg font-semibold">"关于"</div>
                    <div class="mt-1 text-sm text-slate-400">"这套是怎么工作的"</div>
                </a>
                <a href="/macro" class="rounded-xl border border-slate-800 bg-slate-900/60 p-5 hover:border-slate-600 transition-colors">
                    <div class="text-lg font-semibold">"view! 宏"</div>
                    <div class="mt-1 text-sm text-slate-400">"proc macro 展开"</div>
                </a>
            </div>
            <div class="mt-4 grid grid-cols-3 gap-4">
                <StatCard label="依赖(运行时)" value="0" />
                <StatCard label="路由" value="5 个页面" />
                <StatCard label="数据" value="共享 struct" />
            </div>
        </div>
    }
}

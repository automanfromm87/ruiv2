use rui::reactive::Signal;
use rui::view;

#[rui::page("/")] // ssr:服务端渲染 + 客户端水合(改 csr / static 切换策略)
pub fn view() -> rui::View {
    let n = Signal::new(0i32);
    view! {
        <div class="mx-auto max-w-2xl px-6 py-16 text-center font-sans">
            <h1 class="text-4xl font-bold tracking-tight">"Hello, rui 👋"</h1>
            <p class="mt-3 text-slate-500">"编辑 src/view/pages/home.rs 开始构建你的 app。"</p>

            <div class="mt-8 flex items-center justify-center gap-3">
                <button class="rounded-md bg-slate-200 px-3 py-1 text-slate-900"
                    on:click={ let n = n.clone(); move || n.set(n.get() - 1) }>"-"</button>
                <span class="min-w-10 text-2xl tabular-nums">{ let c = n.clone(); move || c.get() }</span>
                <button class="rounded-md bg-slate-200 px-3 py-1 text-slate-900"
                    on:click={ let n = n.clone(); move || n.set(n.get() + 1) }>"+"</button>
            </div>
        </div>
    }
}

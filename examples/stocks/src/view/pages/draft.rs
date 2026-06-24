use rui::reactive::Signal;
use rui::view;

#[rui::page(csr, "/draft")] // 纯客户端:服务端只发空壳,客户端从零渲(本地草稿,无需 SSR / 数据)
pub fn view() -> rui::View {
    let text = Signal::new(String::new());

    view! {
        <div class="flex flex-col gap-4">
            <div>
                <h1 class="text-3xl font-bold tracking-tight">"草稿"</h1>
                <p class="mt-1 text-sm text-slate-400">"#[rui::page(csr)]:纯客户端渲染 · 本地草稿不入库 · bind:value 双向绑定"</p>
            </div>
            <textarea class="h-40 rounded-lg bg-slate-800 px-3 py-2 outline-none placeholder:text-slate-500"
                placeholder="随手记点什么…" bind:value={text}></textarea>
            <p class="text-sm text-slate-500">
                { let t = text.clone(); move || format!("{} 字", t.get().chars().count()) }
            </p>
            // 条件渲染:有内容才显示预览
            { let t = text.clone(); move || if t.get().trim().is_empty() {
                view! { <p class="text-slate-600">"(预览会出现在这里)"</p> }
            } else {
                view! { <div class="rounded-lg border border-slate-800 bg-slate-900/60 p-4 text-slate-300">{ let t = t.clone(); move || t.get() }</div> }
            } }
        </div>
    }
}

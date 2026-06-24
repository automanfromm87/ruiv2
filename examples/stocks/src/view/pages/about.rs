use rui::view;

pub fn view() -> u32 {
    view! {
        <div class="rounded-2xl border border-slate-800 bg-slate-900/60 p-8">
            <h1 class="text-2xl font-semibold">"关于"</h1>
            <p class="mt-3 text-slate-400">"页面全部是 src/pages/*.rs,用 view! 宏写。没有 .html 模板,没有 .types。"</p>
            <p class="mt-2 text-slate-400">"view! 是 proc macro:编译期把 JSX 式标记展开成引擎调用,跑在 cargo build 内。"</p>
            <p class="mt-2 text-slate-400">"数据模型 Stock 是一个普通 Rust struct,前端 wasm 与后端 SSR 直接共用。"</p>
        </div>
    }
}

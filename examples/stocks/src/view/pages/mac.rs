use rui::view;

pub fn view() -> u32 {
    view! {
        <div class="rounded-2xl border border-slate-800 bg-slate-900/60 p-8">
            <h1 class="text-2xl font-semibold">"view! 宏"</h1>
            <p class="mt-3 text-slate-400">"整个站点都用 view! 写。它是 proc macro,编译期把 JSX 式标记展开成 el()/attr()/on_click()/text()/effect() 调用。"</p>
            <p class="mt-2 text-slate-400">"<For list=.. item=..> 是响应式列表,{ move || .. } 是响应式文本,<StatCard/> 是组件。"</p>
            <p class="mt-2 text-slate-400">"没有 .html、没有 .types、没有外部 compile.mjs —— 编译就是 cargo build。"</p>
        </div>
    }
}

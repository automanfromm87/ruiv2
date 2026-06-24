use rui::{query, view};

pub fn view(id: String) -> u32 {
    // 参数直接用 rust 变量 id;对后端 schema 编译期校验。title: name 演示别名(结果字段叫 title)。
    let rows = query!(stock(id: id) { symbol title: name price change });

    view! {
        <div class="rounded-2xl border border-slate-800 bg-slate-900/60 p-8">
            <a href="/table" class="text-sm text-slate-400 hover:text-white transition-colors">"← 返回列表"</a>
            <h1 class="mt-3 text-3xl font-bold tracking-tight">{ &id }</h1>
            <div>
                <For list=rows item=r>
                    <div class="mt-4">
                        <div class="text-slate-400">{ &r.title }</div>
                        <div class="mt-3 text-5xl font-bold tabular-nums">{ format!("${}", r.price) }</div>
                        <div class="mt-2 text-slate-400">{ format!("涨跌 {}%", r.change) }</div>
                    </div>
                </For>
            </div>
        </div>
    }
}

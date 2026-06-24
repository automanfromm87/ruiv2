use crate::data::model::Stock;
use rui::{mutation, query, view};

pub fn view() -> u32 {
    // Relay 式:对后端 schema 编译期校验,根/字段写错 cargo build 报错;类型自动推断
    let rows = query!(stocks { symbol name price change });
    // mutation:点击把 AAPL 设成 $200。optimistic 预测先 merge 进 store(视图秒变 $200),
    // 服务端响应回来再用真值覆盖(失败则回滚)。
    let bump = mutation!(rows, set_price(symbol: "AAPL", price: 200.0) { symbol name price change },
        optimistic: vec![Stock { symbol: "AAPL".into(), name: "Apple Inc.".into(), price: 200.0, change: 1.24 }]);

    view! {
        <div class="overflow-hidden rounded-2xl border border-slate-800 bg-slate-900/60">
            <div class="flex items-center justify-between border-b border-slate-800 px-6 py-4">
                <div class="text-lg font-semibold">"持仓 · 共享类型 + SSR 预取"</div>
                <div class="flex gap-2">
                    <button class="rounded-lg bg-slate-800 px-3 py-1.5 text-sm hover:bg-slate-700 transition-colors"
                        on:click={ let r = rows.clone(); move || { let mut v = r.get(); v.reverse(); r.set(v); } }>"倒序"</button>
                    <button class="rounded-lg bg-slate-800 px-3 py-1.5 text-sm hover:bg-slate-700 transition-colors"
                        on:click={ let r = rows.clone(); move || { let mut v = r.get(); v.pop(); r.set(v); } }>"删一行"</button>
                    <button class="rounded-lg bg-slate-100 px-3 py-1.5 text-sm font-medium text-slate-900 hover:bg-white transition-colors"
                        on:click={bump}>"AAPL → $200(mutation)"</button>
                </div>
            </div>
            <table class="w-full text-sm">
                <thead>
                    <tr class="text-left text-slate-400">
                        <th class="px-6 py-3 font-medium">"代码"</th>
                        <th class="px-6 py-3 font-medium">"名称"</th>
                        <th class="px-6 py-3 font-medium text-right">"价格"</th>
                        <th class="px-6 py-3 font-medium text-right">"涨跌"</th>
                    </tr>
                </thead>
                <tbody>
                    <For list=rows item=r>
                        <tr class="border-t border-slate-800/70 hover:bg-slate-800/40 transition-colors">
                            <td class="px-6 py-3 font-semibold">
                                <a href={format!("/stock/{}", r.symbol)} class="hover:text-white">{ &r.symbol }</a>
                            </td>
                            <td class="px-6 py-3 text-slate-400">{ &r.name }</td>
                            <td class="px-6 py-3 text-right tabular-nums">{ format!("${}", r.price) }</td>
                            <td class="px-6 py-3 text-right tabular-nums">{ format!("{}%", r.change) }</td>
                        </tr>
                    </For>
                </tbody>
            </table>
        </div>
    }
}

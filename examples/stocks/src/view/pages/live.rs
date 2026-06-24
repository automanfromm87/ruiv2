use rui::{subscription, view};

pub fn view() -> u32 {
    // subscription:SSR 给初值,客户端开 SSE 持续收推送,每次都刷新 rows
    let rows = subscription!(price_updates { symbol name price change });

    view! {
        <div class="overflow-hidden rounded-2xl border border-slate-800 bg-slate-900/60">
            <div class="border-b border-slate-800 px-6 py-4 text-lg font-semibold">"实时行情 · subscription(SSE,每 1.5s 推送)"</div>
            <table class="w-full text-sm">
                <thead>
                    <tr class="text-left text-slate-400">
                        <th class="px-6 py-3 font-medium">"代码"</th>
                        <th class="px-6 py-3 font-medium">"名称"</th>
                        <th class="px-6 py-3 font-medium text-right">"价格(实时跳动)"</th>
                    </tr>
                </thead>
                <tbody>
                    <For list=rows item=r>
                        <tr class="border-t border-slate-800/70">
                            <td class="px-6 py-3 font-semibold">{ &r.symbol }</td>
                            <td class="px-6 py-3 text-slate-400">{ &r.name }</td>
                            <td class="px-6 py-3 text-right tabular-nums">{ format!("${}", r.price) }</td>
                        </tr>
                    </For>
                </tbody>
            </table>
        </div>
    }
}

use rui::{paginated, view};

// Relay connection 分页:每页 3 条;load_next 把下一页追加进 store 的 connection record(累积);
// 节点(Stock)按 ref 存,被 mutation 改写后这个分页列表也会自动更新(完整 Relay 一致性)。
pub fn view() -> u32 {
    let (rows, load_next, has_next, loading) = paginated!(stock_page(first: 3) { symbol name price });

    view! {
        <div class="overflow-hidden rounded-2xl border border-slate-800 bg-slate-900/60">
            <div class="border-b border-slate-800 px-6 py-4 text-lg font-semibold">"分页 · connection(游标 + 加载更多)"</div>
            <table class="w-full text-sm">
                <thead>
                    <tr class="text-left text-slate-400">
                        <th class="px-6 py-3 font-medium">"代码"</th>
                        <th class="px-6 py-3 font-medium">"名称"</th>
                        <th class="px-6 py-3 font-medium text-right">"价格"</th>
                    </tr>
                </thead>
                <tbody>
                    <For list=rows item=e>
                        <tr class="border-t border-slate-800/70 hover:bg-slate-800/40 transition-colors">
                            <td class="px-6 py-3 font-semibold">{ &e.node.symbol }</td>
                            <td class="px-6 py-3 text-slate-400">{ &e.node.name }</td>
                            <td class="px-6 py-3 text-right tabular-nums">{ format!("${}", e.node.price) }</td>
                        </tr>
                    </For>
                </tbody>
            </table>
            <div class="flex items-center gap-3 border-t border-slate-800 px-6 py-4">
                <button class="rounded-lg bg-slate-100 px-4 py-1.5 text-sm font-medium text-slate-900 hover:bg-white transition-colors"
                    on:click={ move || load_next() }>"加载更多"</button>
                <span class="text-sm text-slate-400">
                    { let h = has_next.clone(); move || if h.get() { "还有更多" } else { "已全部加载" } }
                </span>
                <span class="text-sm text-slate-500">
                    { let l = loading.clone(); move || if l.get() { "加载中…" } else { "" } }
                </span>
            </div>
        </div>
    }
}

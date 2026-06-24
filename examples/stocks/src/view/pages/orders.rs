use rui::{query, view};

// 嵌套 selection 的 exact-fit 验证页:orders { id items { sku qty } }。
// Row 含 id(String) 与 items(Vec<ItemRow>),ItemRow 含 sku(String)/qty(i64) —— 全链路类型由编译器保证。
// (服务端通用执行器按 selection 投影;客户端按 Field/GqlElem/Reshape 合成精确类型。)
pub fn view() -> u32 {
    // num: id 与嵌套 s: sku 演示别名(root + 嵌套两层别名)。
    let rows = query!(orders { num: id items { s: sku qty } });
    view! {
        <div class="overflow-hidden rounded-2xl border border-slate-800 bg-slate-900/60">
            <div class="border-b border-slate-800 px-6 py-4 text-lg font-semibold">"订单 · 嵌套 selection(exact-fit)"</div>
            <table class="w-full text-sm">
                <tbody>
                    <For list=rows item=o>
                        <tr class="border-t border-slate-800/70">
                            <td class="px-6 py-3 font-semibold">{ format!("订单 {}", o.num) }</td>
                            <td class="px-6 py-3 text-slate-400">{ format!("{} 项:{}", o.items.len(), o.items.iter().map(|i| format!("{}×{}", i.s, i.qty)).collect::<Vec<_>>().join("、")) }</td>
                        </tr>
                    </For>
                </tbody>
            </table>
        </div>
    }
}

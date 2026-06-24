//! 可复用组件:普通 Rust 函数,返回根节点 id。
//! 在 view! 里用 <StatCard label="x" value="y"/>(首字母大写)即调用 stat_card(...)。
//! 约定:属性按函数参数顺序传。

use rui::view;

pub fn stat_card(label: String, value: String) -> u32 {
    view! {
        <div class="rounded-xl border border-slate-800 bg-slate-900/60 p-5">
            <div class="text-sm text-slate-400">{ label }</div>
            <div class="mt-1 text-2xl font-semibold">{ value }</div>
        </div>
    }
}

// Relay 式片段 + data masking:StockCard 声明自己需要的数据(symbol/name/price)。
// 页面用 query!(stocks { ...StockCard }) 组合;组件只拿到 StockCard(读不到 change 等未声明字段)。
rui::fragment!(StockCard on Stock { symbol name price });

pub fn stock_card(card: StockCard) -> u32 {
    view! {
        <div class="rounded-xl border border-slate-800 bg-slate-900/60 p-5">
            <div class="text-lg font-semibold">{ &card.symbol }</div>
            <div class="mt-1 text-sm text-slate-400">{ &card.name }</div>
            <div class="mt-2 text-2xl font-bold tabular-nums">{ format!("${}", card.price) }</div>
        </div>
    }
}

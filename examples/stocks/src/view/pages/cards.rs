use crate::view::components::StockCard;
use rui::{query, view};

// 片段 spread:组件 StockCard 声明自己要的数据,query!(stocks { ...StockCard }) 组合进来;
// 父行只持有 StockCard 子结构(data masking),整体传给组件,组件读不到未声明字段(如 change)。
pub fn view() -> u32 {
    let rows = query!(stocks { ...StockCard });

    view! {
        <div>
            <div class="mb-4 text-lg font-semibold">"片段 · fragment + data masking"</div>
            <div class="grid grid-cols-3 gap-4">
                <For list=rows item=r>
                    <StockCard card={r.StockCard.clone()} />
                </For>
            </div>
        </div>
    }
}

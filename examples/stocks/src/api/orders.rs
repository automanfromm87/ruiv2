//! 订单数据后端(仅服务端)。被 crate::api::schema 的 orders 字段调用。
use crate::data::model::{Item, Order};

pub fn orders() -> Vec<Order> {
    vec![
        Order {
            id: "1001".to_string(),
            items: vec![
                Item { sku: "AAPL".to_string(), qty: 10 },
                Item { sku: "MSFT".to_string(), qty: 5 },
            ],
        },
        Order {
            id: "1002".to_string(),
            items: vec![Item { sku: "NVDA".to_string(), qty: 3 }],
        },
    ]
}

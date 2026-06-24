//! 共享数据模型 —— 普通 Rust struct,前端(wasm)与后端(SSR)直接共用。
//! `#[derive(GqlObject)]` 自动生成 GraphQL 对象类型所需的一切(字段投影 / 编解码 / entity id)。

use rui::GqlObject;

#[derive(Clone, GqlObject)]
pub struct Stock {
    #[gql(id)]
    pub symbol: String,
    pub name: String,
    pub price: f64,
    pub change: f64,
}

// 关系型示例:Order 1—N Item(验证嵌套 selection 的 exact-fit)。
#[derive(Clone, GqlObject)]
pub struct Item {
    #[gql(id)]
    pub sku: String,
    pub qty: i64,
}

#[derive(Clone, GqlObject)]
pub struct Order {
    #[gql(id)]
    pub id: String,
    pub items: Vec<Item>,
}

// Relay connection 分页三件套:都是 value object(无 #[gql(id)])—— 不进规范化缓存,
// 真正的 entity 是 edge.node(Stock,有 id),由 store 的 connection record 按 ref 累积。
#[derive(Clone, GqlObject)]
pub struct StockEdge {
    pub node: Stock,
    pub cursor: String,
}

#[derive(Clone, GqlObject)]
pub struct PageInfo {
    pub has_next_page: bool,
    pub end_cursor: String,
}

#[derive(Clone, GqlObject)]
pub struct StockConnection {
    pub edges: Vec<StockEdge>,
    pub page_info: PageInfo,
}

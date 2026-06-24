//! 后端 API —— 「写方法即 schema」。
//!
//! 每个根(Query/Mutation/Subscription)是一个普通 impl,方法签名就是 schema:
//!   · 方法名      → GraphQL 根字段名
//!   · 参数        → 字段参数(执行器按类型从 args 提取)
//!   · 返回类型    → 字段返回类型(`#[gql_root]` 据此生成 Field 投影,供前端 query! 编译期校验)
//!   · 方法体      → resolver(仅服务端编译;调 crate::api::stocks / crate::api::orders 的数据函数)
//!
//! 注:同一根的方法须在一个 #[gql_root] 块内(宏对每个根生成一份 Root + resolve,不能分散);
//! 数据实现按领域拆到 stocks.rs / orders.rs。
#![allow(dead_code)] // 根 struct 的方法体仅服务端编译;wasm 端只用其类型层 schema

use crate::data::model::{Order, Stock, StockConnection};
use rui::gql_root;

pub struct Query;
pub struct Mutation;
pub struct Subscription;

#[gql_root(query)]
impl Query {
    fn stocks(&self) -> Vec<Stock> {
        crate::api::stocks::all_stocks()
    }
    fn stock(&self, id: String) -> Vec<Stock> {
        crate::api::stocks::stock(&id)
    }
    fn orders(&self) -> Vec<Order> {
        crate::api::orders::orders()
    }
    // Relay connection 分页:first/after 游标分页,返回 StockConnection。
    fn stock_page(&self, first: i64, after: String) -> Vec<StockConnection> {
        crate::api::stocks::page(first, &after)
    }
}

#[gql_root(mutation)]
impl Mutation {
    fn set_price(&self, symbol: String, price: f64) -> Vec<Stock> {
        crate::api::stocks::set_price(&symbol, price)
    }
}

#[gql_root(subscription)]
impl Subscription {
    fn price_updates(&self) -> Vec<Stock> {
        crate::api::stocks::all_stocks()
    }
}

/// 聚合 resolver:把三个根的 dispatch 合成一个,供 rui::serve 注入(/graphql + 同构 SSR 共用)。
#[cfg(not(target_arch = "wasm32"))]
pub fn resolve(
    kind: rui::gql::parser::OpKind,
    field: &str,
    args: &rui::gql::exec::Args,
) -> rui::gql::Value {
    use rui::gql::parser::OpKind;
    match kind {
        OpKind::Query => QueryRoot::resolve(field, args),
        OpKind::Mutation => MutationRoot::resolve(field, args),
        OpKind::Subscription => SubscriptionRoot::resolve(field, args),
    }
}

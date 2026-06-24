//! 后端 API 层(crate::api)。
//!   schema   #[gql_root] 写方法即 GraphQL schema(类型层两端可见;resolver 仅服务端)
//!   stocks   股票数据 + 行情 ticker + SSE 订阅广播(仅服务端)
//!   orders   订单数据(仅服务端)
//!
//! schema 的方法体调用 stocks/orders 里的纯数据函数;model(共享实体)在 crate::data::model。
pub mod schema;

#[cfg(not(target_arch = "wasm32"))]
pub mod stocks;
#[cfg(not(target_arch = "wasm32"))]
pub mod orders;

//! 后端 API 层(crate::api)。
//!   schema  #[gql_root] 写方法即 GraphQL schema(类型层两端可见;resolver 仅服务端)
//!   todos   待办数据 + 增删改 + SSE 订阅广播(仅服务端)
pub mod schema;

#[cfg(not(target_arch = "wasm32"))]
pub mod todos;

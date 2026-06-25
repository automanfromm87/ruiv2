//! 共享数据模型 —— 前端(wasm)与后端(SSR)直接共用。
//! `#[derive(GqlObject)]` 生成客户端类型层(Field 投影 / 编解码 / entity id);
//! native 额外 `#[derive(async_graphql::SimpleObject)]` 生成服务端 async-graphql 对象类型(双 schema / B2)。
//! 两个 derive 的 helper attr 互不干扰:`#[gql(..)]` 归 GqlObject,`#[graphql(..)]` 归 SimpleObject。
//! rename_fields="snake_case":让 async-graphql 字段名与客户端 selection 的 snake_case 一致(默认会改成 camelCase)。

use rui::GqlObject;

#[derive(Clone, PartialEq, GqlObject)]
#[cfg_attr(not(target_arch = "wasm32"), derive(async_graphql::SimpleObject))]
#[cfg_attr(not(target_arch = "wasm32"), graphql(rename_fields = "snake_case"))]
pub struct Todo {
    #[gql(id)]
    pub id: String,
    pub text: String,
    pub done: bool,
}

// Relay connection 分页三件套(归档页用):value object,无 #[gql(id)] —— 真正的 entity 是 edge.node。
#[derive(Clone, GqlObject)]
#[cfg_attr(not(target_arch = "wasm32"), derive(async_graphql::SimpleObject))]
#[cfg_attr(not(target_arch = "wasm32"), graphql(rename_fields = "snake_case"))]
pub struct TodoEdge {
    pub node: Todo,
    pub cursor: String,
}

#[derive(Clone, GqlObject)]
#[cfg_attr(not(target_arch = "wasm32"), derive(async_graphql::SimpleObject))]
#[cfg_attr(not(target_arch = "wasm32"), graphql(rename_fields = "snake_case"))]
pub struct PageInfo {
    pub has_next_page: bool,
    pub end_cursor: String,
}

#[derive(Clone, GqlObject)]
#[cfg_attr(not(target_arch = "wasm32"), derive(async_graphql::SimpleObject))]
#[cfg_attr(not(target_arch = "wasm32"), graphql(rename_fields = "snake_case"))]
pub struct TodoConnection {
    pub edges: Vec<TodoEdge>,
    pub page_info: PageInfo,
}

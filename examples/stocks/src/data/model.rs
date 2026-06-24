//! 共享数据模型 —— 前端(wasm)与后端(SSR)直接共用。
//! `#[derive(GqlObject)]` 自动生成 GraphQL 对象类型所需的一切(字段投影 / 编解码 / entity id)。

use rui::GqlObject;

#[derive(Clone, PartialEq, GqlObject)]
pub struct Todo {
    #[gql(id)]
    pub id: String,
    pub text: String,
    pub done: bool,
}

// Relay connection 分页三件套(归档页用):value object,无 #[gql(id)] —— 真正的 entity 是 edge.node。
#[derive(Clone, GqlObject)]
pub struct TodoEdge {
    pub node: Todo,
    pub cursor: String,
}

#[derive(Clone, GqlObject)]
pub struct PageInfo {
    pub has_next_page: bool,
    pub end_cursor: String,
}

#[derive(Clone, GqlObject)]
pub struct TodoConnection {
    pub edges: Vec<TodoEdge>,
    pub page_info: PageInfo,
}

//! 共享数据模型 —— 前端(wasm)与后端(SSR)直接共用。
//! `#[derive(Ent)]` 是**单一真相源**:同一个 struct 同时是 GraphQL 对象类型(同构,Field 投影 / 编解码 / entity id)
//! + 表映射(native:`#[ent(table=..)]` + #[gql(id)] 主键 → SqlEntity)。
//! 不再有 GqlObject + async-graphql SimpleObject + sqlx FromRow 三重 derive,也不再有双 schema。
//! selection → SQL 列投影由 rui::gql::orm 在运行期完成(`{ todos { id } }` → `select id from todos`)。
//!
//! Relay 分页三件套也**随 Ent 自动生成**:`TodoEdge` / `TodoPageInfo` / `TodoConnection`(+ 原生
//! `TodoConnection::page(first, after)` 切片器)。归档页 `paginated!(todo_page ...)` 与 schema 的
//! `todo_page` resolver 直接用,无需在此手写这些 value object。

use rui::Ent;

#[derive(Clone, PartialEq, Ent)]
#[ent(table = "todos")]
pub struct Todo {
    #[gql(id)]
    pub id: String,
    pub text: String,
    pub done: bool,
}

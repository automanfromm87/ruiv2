//! 数据层(crate::data):前后端共享的实体类型。
//!
//! 加实体:在本目录建 `model.rs`,然后取消下面注释。
//!   · model.rs 里用 `#[derive(rui::GqlObject)]` 定义 struct(用 `#[gql(id)]` 标 id 字段);
//!   · 并在 crate 根 lib.rs 加 `rui::app! {}` + `rui::gql_fields!(字段名...)` 声明字段 marker。

// pub mod model;

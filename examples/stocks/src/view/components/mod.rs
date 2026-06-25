//! 可复用组件(目录形式:一组件一文件)。mod.rs 把各组件统一 re-export 进
//! `crate::view::components` 命名空间 —— 这样 view! 里的 `<TodoItem/>` 仍解析到
//! `crate::view::components::todo_item`(宏约定的固定路径)。
//!
//! 新增一个组件:建 `my_comp.rs` 写 `#[rui::component] pub fn my_comp(..)`,再在这里加:
//!   `mod my_comp;  pub use my_comp::{my_comp, MyCompProps};`
//! (`#[component]` 生成的 `MyCompProps` 也要 re-export,view! 用具名结构体字面量调用它。)

mod add_form;
mod counters;
mod greeting_badge;
mod panel;
mod risky;
mod shared;
mod stat;
mod status_banner;
mod todo_item;
mod toolbar;
mod uptime;

// 共享类型(页面也用):过滤器 + Relay 片段 + Context 类型。
pub use shared::{Filter, Greeting, TodoView};
// 组件:fn + 其 Props。
pub use add_form::{add_form, AddFormProps};
pub use counters::{counters, CountersProps};
pub use greeting_badge::{greeting_badge, GreetingBadgeProps};
pub use panel::{panel, PanelProps};
pub use risky::{risky_panel, RiskyPanelProps};
pub use stat::{stat, StatProps};
pub use status_banner::{status_banner, StatusBannerProps};
pub use todo_item::{todo_item, TodoItemProps};
pub use toolbar::{toolbar, ToolbarProps};
pub use uptime::{uptime, UptimeProps};

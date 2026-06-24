//! 视图层(crate::view)。
//!   components  可复用组件(view! 里 <StatCard/> → crate::view::components::stat_card)
//!   layout      共享布局(navbar 外壳)
//!   pages       各页面(每个一个 view! 函数)
pub mod components;
pub mod layout;
pub mod pages;

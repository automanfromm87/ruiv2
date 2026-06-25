//! 可复用组件(目录形式:一组件一文件)。本 mod.rs 把各组件 re-export 进
//! `crate::view::components` 命名空间 —— view! 里的 `<MyComp/>` 解析到 `crate::view::components::my_comp`。
//!
//! 加一个组件:新建 `my_comp.rs`:
//!
//!   use rui::{component, view};
//!   #[component]
//!   pub fn my_comp(title: String) -> rui::View {
//!       view! { <div class="font-semibold">{ title }</div> }
//!   }
//!
//! 然后在这里登记(`#[component]` 生成的 `MyCompProps` 也要 re-export):
//!
//!   mod my_comp;  pub use my_comp::{my_comp, MyCompProps};

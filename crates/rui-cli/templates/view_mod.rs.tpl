//! 视图层(crate::view)。
//!
//!   · components.rs  可复用组件(view! 里 `<Comp/>` → `crate::view::components::comp`)
//!   · layout.rs      共享布局(navbar 外壳等)
//!   · pages/         各页面(每个一个 `view!` 函数;在 lib.rs 的 route 里分发)

// pub mod components;
// pub mod layout;
// pub mod pages;

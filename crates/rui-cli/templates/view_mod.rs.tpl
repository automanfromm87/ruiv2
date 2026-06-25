//! 视图层(crate::view)。
//!
//!   · components/   可复用组件(目录:一组件一文件 + mod.rs re-export;
//!                   view! 里 `<Comp/>` → `crate::view::components::comp`)
//!   · layout.rs     共享布局(navbar 外壳等)
//!   · pages/        各页面(每个一个 #[rui::page] 函数;在 router! 路由表里登记)

pub mod components;
pub mod pages;
// pub mod layout;

//! 由 `rui init` 生成的骨架 —— 目录即规范(rui 约定,宏只认这套路径):
//!   src/data/   前后端共享数据模型   → crate::data::model
//!   src/api/    后端 API            → crate::api::schema(#[gql_root])+ 数据实现
//!   src/view/   前端视图            → crate::view::{components, layout, pages}
//!   src/lib.rs  入口:模块挂载 + gql_fields!(字段 marker)+ client! + route
//!
//! data/api/view 现在是空骨架(见各自 mod.rs 注释),按需往里填;首页是下面的 route。
use rui::prelude::*;

pub mod data;
pub mod api;
pub mod view;

// wasm 客户端入口:生成 alloc / render_route / dispatch / on_fetch 导出(仅 wasm 目标)。
rui::client!(crate::route);

/// 路由:路径 → 页面根节点。要分页时在这里 `match` 路径分发(页面放 src/view/pages/)。
pub fn route(path: &str) -> u32 {
    let _ = path;
    let n = Signal::new(0i32);

    view! {
        <div class="mx-auto max-w-2xl px-6 py-16 text-center font-sans">
            <h1 class="text-4xl font-bold tracking-tight">"Hello, rui 👋"</h1>
            <p class="mt-3 text-slate-500">"编辑 src/lib.rs 与 data/ api/ view/ 开始构建你的 app。"</p>

            <div class="mt-8 flex items-center justify-center gap-3">
                <button on:click={ let n = n.clone(); move || n.set(n.get() - 1) }>"-"</button>
                <span class="min-w-10 text-2xl tabular-nums">{ let c = n.clone(); move || c.get() }</span>
                <button on:click={ let n = n.clone(); move || n.set(n.get() + 1) }>"+"</button>
            </div>
        </div>
    }
}

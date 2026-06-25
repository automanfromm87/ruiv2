//! /dash 路由组:组内页面(overview / settings)+ 组**专属**的内层布局 `dash_shell`。
//! 组局部布局放这里、**不进全局 `layout.rs`** —— 它只服务这个组,与全局 navbar 外壳(shell)不是一回事。
//! /dash ↔ /dash/settings 切换时 dash_shell 不重建,只有右侧 outlet 内容随 path 换。

pub mod overview;
pub mod settings;

use rui::view;

/// 仪表盘内层布局:左侧栏 + 右内容区。声明式:`{ side(..) }` 注入侧栏链接、`{ content }` 是 outlet。
/// 侧栏高亮读 reactive `rui::path()`(同组导航侧栏不重建,故高亮必须响应式才会更新)。
pub fn dash_shell(_path: &str, content: rui::View) -> rui::View {
    fn side(label: &'static str, href: &'static str, exact: bool) -> rui::View {
        view! {
            <a href={href}
                class={ move || {
                    let p = rui::path().get();
                    let on = if exact { p == href } else { p.starts_with(href) };
                    if on { "rounded-md bg-slate-800 px-3 py-2 text-sm font-medium text-white" }
                    else { "rounded-md px-3 py-2 text-sm text-slate-400 hover:text-white transition-colors" }
                } }>
                { label }
            </a>
        }
    }
    view! {
        <div class="flex gap-6">
            <aside class="flex w-40 shrink-0 flex-col gap-1 border-r border-slate-800 pr-4">
                { side("总览", "/dash", true) }
                { side("设置", "/dash/settings", false) }
            </aside>
            <div class="flex-1">{ content }</div> // ← outlet:reactive_block 渲染的当前叶子
        </div>
    }
}

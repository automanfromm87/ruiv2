//! 全局布局:顶部 navbar + 内容区。route() 把页面 Page 的内容交给 shell 包裹。
//! (组专属布局如 dash_shell 不在这里 —— 见 view/pages/dash/;本文件只放真正全局的 layout。)

use rui::view;

// 高亮读 reactive path():这样即使 layout 不重建(组内导航等),高亮也会更新。
fn navlink(label: &'static str, href: &'static str, exact: bool) -> rui::View {
    view! {
        <a href={href}
            class={ move || {
                let p = rui::path().get();
                let on = if exact { p == href } else { p.starts_with(href) };
                if on { "rounded-md bg-slate-800 px-3 py-1.5 text-sm font-medium text-white" }
                else { "rounded-md px-3 py-1.5 text-sm text-slate-400 hover:text-white transition-colors" }
            } }>
            { label }
        </a>
    }
}

/// 用 navbar 外壳包裹页面节点(声明式:`{ navlink(..) }` 注入链接、`{ page }` 注入页面内容)。
pub fn shell(_path: &str, page: rui::View) -> rui::View {
    view! {
        <div class="min-h-screen bg-slate-950 text-slate-100">
            <nav class="sticky top-0 z-10 flex items-center gap-1 border-b border-slate-800 bg-slate-950/80 px-6 py-3 backdrop-blur">
                <div class="text-base font-semibold tracking-tight">"✓ rui · todo"</div>
                <Uptime />
                <span class="mr-auto"></span>
                { navlink("待办", "/", true) }
                { navlink("仪表盘", "/dash", false) }
                { navlink("归档", "/archive", true) }
                { navlink("草稿", "/draft", true) }
                { navlink("表单", "/forms", true) }
                { navlink("过渡", "/transitions", true) }
                { navlink("边界", "/boundary", true) }
                { navlink("关于", "/about", true) }
            </nav>
            <div class="mx-auto max-w-2xl px-6 py-10">{ page }</div>
        </div>
    }
}

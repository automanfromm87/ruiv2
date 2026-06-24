//! 共享布局:顶部 navbar + 内容区。route() 把页面 Page 的内容交给 shell 包裹。

use rui::dom::append;
use rui::view;

// 高亮读 reactive path():这样即使 layout 不重建(如组内导航、未来全局壳持久化),高亮也会更新。
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

/// 用 navbar 外壳包裹页面节点,返回整页根节点(框架负责挂载 / 序列化)。
pub fn shell(_path: &str, page: rui::View) -> rui::View {
    let nav = view! {
        <nav class="sticky top-0 z-10 flex items-center gap-1 border-b border-slate-800 bg-slate-950/80 px-6 py-3 backdrop-blur">
            <div class="text-base font-semibold tracking-tight">"✓ rui · todo"</div>
            <Uptime />
            <span class="mr-auto"></span>
        </nav>
    };
    append(nav, navlink("待办", "/", true)); // 精确(否则什么都 starts_with "/")
    append(nav, navlink("仪表盘", "/dash", false)); // 前缀:/dash 与 /dash/settings 都高亮
    append(nav, navlink("归档", "/archive", true));
    append(nav, navlink("草稿", "/draft", true));
    append(nav, navlink("关于", "/about", true));

    let main = view! { <div class="mx-auto max-w-2xl px-6 py-10"></div> };
    append(main, page);

    let root = view! { <div class="min-h-screen bg-slate-950 text-slate-100"></div> };
    append(root, nav);
    append(root, main);
    root
}

/// 仪表盘内层布局(路由组 /dash 的共享外壳):左侧栏 + 右内容区。
/// 关键:/dash ↔ /dash/settings 切换时本函数**不重新执行**(组 key 不变),侧栏 DOM 与状态保留,
/// 只有右侧 children(reactive outlet)随 path 换。侧栏高亮读 reactive `rui::path()`(不依赖 &str 入参)。
pub fn dash_shell(_path: &str, content: rui::View) -> rui::View {
    // 侧栏链接:active 用 reactive path() —— 同组导航侧栏不重建,故高亮必须响应式才会更新。
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
    let aside = view! { <aside class="flex w-40 shrink-0 flex-col gap-1 border-r border-slate-800 pr-4"></aside> };
    append(aside, side("总览", "/dash", true));
    append(aside, side("设置", "/dash/settings", false));

    let body = view! { <div class="flex-1"></div> };
    append(body, content); // ← outlet:reactive_block 渲染的当前叶子

    let root = view! { <div class="flex gap-6"></div> };
    append(root, aside);
    append(root, body);
    root
}

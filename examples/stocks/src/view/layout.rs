//! 共享布局:顶部 navbar + 内容区。route() 把页面节点交给 shell 包裹,框架负责挂载。

use rui::dom::append;
use rui::view;

fn navlink(label: &str, href: &str, active: bool) -> u32 {
    let cls = if active {
        "rounded-md bg-slate-800 px-3 py-1.5 text-sm font-medium text-white"
    } else {
        "rounded-md px-3 py-1.5 text-sm text-slate-400 hover:text-white transition-colors"
    };
    view! { <a href={href} class={cls}>{ label }</a> }
}

/// 用 navbar 外壳包裹页面节点,返回整页根节点(不挂载 —— 由框架 runtime 挂载/序列化)。
pub fn shell(path: &str, page: u32) -> u32 {
    let nav = view! {
        <nav class="sticky top-0 z-10 flex items-center gap-1 border-b border-slate-800 bg-slate-950/80 px-6 py-3 backdrop-blur">
            <div class="mr-auto text-base font-semibold tracking-tight">"rui"</div>
        </nav>
    };
    append(nav, navlink("首页", "/", path == "/"));
    append(nav, navlink("计数器", "/counter", path == "/counter"));
    append(nav, navlink("表格", "/table", path == "/table"));
    append(nav, navlink("订单", "/orders", path == "/orders"));
    append(nav, navlink("分页", "/feed", path == "/feed"));
    append(nav, navlink("卡片", "/cards", path == "/cards"));
    append(nav, navlink("实时", "/live", path == "/live"));
    append(nav, navlink("关于", "/about", path == "/about"));

    let main = view! { <div class="mx-auto max-w-5xl px-6 py-10"></div> };
    append(main, page);

    let root = view! { <div class="min-h-screen bg-slate-950 text-slate-100"></div> };
    append(root, nav);
    append(root, main);
    root
}

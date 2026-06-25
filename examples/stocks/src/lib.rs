//! todo —— 用 rui 框架搭的 TodoList 应用(crate 名沿用 stocks,内容是 todo)。
//! 用上框架全部能力:GraphQL(query!/mutation!/subscription!/paginated!/fragment!)+ 规范化缓存、
//! 同构 SSR + 真水合 + 数据交接、渲染策略标注(#[rui::page] ssr/csr/static)、组件(具名 props+children)、
//! keyed <For>、<Show>/<Switch>、IntoView 表达式条件、响应式属性、bind:value、memo。

pub mod api;
pub mod data;
mod view;

// 字段 marker:gql_root 为每个方法生成 Field<gqlf::方法名>,故所有 query/mutation/subscription 字段名 + 数据字段都要在此声明。
rui::gql_fields!(
    id, text, done, todos, todo_page, search, detail, add_todo, toggle_todo, remove_todo, clear_done, complete_all,
    todo_updates, edges, node, cursor, page_info, has_next_page, end_cursor
);

rui::client!(crate::route);

use view::{layout, pages};

// 路由表 = 候选页清单。路由模式声明在各 page 的 #[rui::page("/...")] 上(唯一真相源);
// 这里只列出哪些页参与路由 + 命中页用 navbar 外壳(layout)包裹。生成 `pub fn route(path)->Page`。
rui::router! {
    layout = layout::shell,
    pages::index,
    pages::archive,
    pages::detail, // /todo/:id —— 路由参数,模式 + 类型化签名都在 detail.rs
    // 路由组:/dash 与 /dash/settings 共享 dash_shell 侧栏(组内导航不重建侧栏,只换内层 outlet)。
    // 组专属布局 dash_shell 与组的页面同住在 view/pages/dash/(不在全局 layout.rs)。
    group("/dash", layout = pages::dash::dash_shell) {
        pages::dash::overview, // /dash
        pages::dash::settings, // /dash/settings
    },
    pages::about,
    pages::draft,
    pages::boundary, // /boundary —— <ErrorBoundary> 演示
    pages::forms,    // /forms —— 表单(bind:value/checked/group + select + 校验)
    pages::transitions, // /transitions —— <Transition> 进出场动画
    fallback = not_found,
}

fn not_found() -> rui::View {
    use rui::dom::{attr, el, set_text};
    let d = el("div");
    attr(d, "class", "text-2xl");
    set_text(d, "404 · 页面不存在");
    rui::View(d)
}

// 注:增删改用 mutation! 宏(现已支持运行时参数,如 toggle_todo(id: id)),无需手搓 GraphQL 串。
// 列表是 subscription! 驱动:服务端任何写操作都会广播 → 订阅推新列表 → UI 自动反映增删改。

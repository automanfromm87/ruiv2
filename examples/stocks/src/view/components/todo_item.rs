use super::shared::TodoView;
use rui::reactive::Signal;
use rui::{component, view};

// 一行待办。展示:bind:checked(受控复选框 Signal<bool>)+ on:change 触发 toggle mutation;
// `.todo-enter` 进场动画(keyed <For> 只构建新行 → 仅新增/重建的行播放);删除线随复选框即时切换。
// 数据走片段(TodoView);toggle/remove 是闭包 props(组件不直接发请求,解耦)。
#[component]
pub fn todo_item(todo: TodoView, toggle: Box<dyn Fn()>, remove: Box<dyn Fn()>) -> rui::View {
    // 本行复选框的受控 signal,初值取服务端 done。keyed <For> 在 done 变化时会重建本行,
    // 届时用新 done 重新初始化;点击先即时翻转(乐观视觉)再发 toggle 真请求。
    let checked = Signal::new(todo.done);
    let href = format!("/todo/{}", todo.id); // 点文本进详情页(SPA 导航 → 路由参数 signal)
    view! {
        <li class="todo-enter flex items-center gap-3 px-4 py-3 border-t border-slate-800/70">
            // bind:checked 双向绑定即时视觉;on:change 另一个 change 监听触发 toggle 真请求。
            <input type="checkbox" bind:checked={ checked.clone() } on:change={ move || toggle() } />
            <a href={ href }
                class={ let c = checked.clone(); move || if c.get() {
                    "flex-1 text-slate-500 line-through hover:text-slate-300 transition-all"
                } else {
                    "flex-1 text-slate-100 hover:text-white transition-all"
                } }>
                { todo.text.clone() }
            </a>
            <button class="text-slate-500 hover:text-rose-400 transition-all" on:click={ move || remove() }>"×"</button>
        </li>
    }
}

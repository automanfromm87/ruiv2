use rui::reactive::{memo, Signal};
use rui::{component, view};

const MAX: usize = 80;

// 新增表单。展示:bind:value + memo 派生校验(非空、≤80 字)+ 实时字数 + 非法时弱化提交按钮;
// 生命周期:ref + on_mount → 进入页面自动聚焦输入框。提交(回车 / 点按钮)校验通过才调 add(text)。
#[component]
pub fn add_form(add: Box<dyn Fn(String)>) -> rui::View {
    let draft = Signal::new(String::new());
    let input = rui::node_ref();
    rui::on_mount({
        let input = input.clone();
        move || rui::dom::focus(input.get()) // 节点入 DOM 后聚焦(命令式)
    });
    // memo 校验:派生「是否可提交」+「字数」—— 这就是表单校验的惯用法。
    let valid = {
        let d = draft.clone();
        memo(move || {
            let n = d.get().chars().count();
            !d.get().trim().is_empty() && n <= MAX
        })
    };
    view! {
        <form class="flex flex-col gap-1"
            on:submit={ let (d, v) = (draft.clone(), valid.clone()); move || {
                if v.get() { add(d.get().trim().to_string()); d.set(String::new()); }
            } }>
            <div class="flex gap-2">
                // 键盘:Esc 清空草稿(on:keydown.escape 按键过滤修饰符 → 只在 Esc 时触发)。
                <input ref={input} class="flex-1 rounded-lg bg-slate-800 px-3 py-2 outline-none placeholder:text-slate-500"
                    placeholder="加一个待办,回车添加(Esc 清空)…" bind:value={draft.clone()}
                    on:keydown.escape={ let d = draft.clone(); move || d.set(String::new()) } />
                <button class={ let v = valid.clone(); move || if v.get() {
                    "rounded-lg bg-slate-100 px-4 py-2 font-medium text-slate-900 hover:bg-white transition-all"
                } else {
                    "rounded-lg bg-slate-100 px-4 py-2 font-medium text-slate-900 opacity-60 cursor-not-allowed transition-all"
                } }>"添加"</button>
            </div>
            // 实时字数:超长变红(memo 之外单独读 draft,即时反映)
            <p class={ let d = draft.clone(); move || {
                let over = d.get().chars().count() > MAX;
                if over { "px-1 text-xs text-rose-400" } else { "px-1 text-xs text-slate-500" }
            } }>
                { let d = draft.clone(); move || format!("{}/{}", d.get().chars().count(), MAX) }
            </p>
        </form>
    }
}

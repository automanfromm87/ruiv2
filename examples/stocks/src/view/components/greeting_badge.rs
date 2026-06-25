use super::shared::Greeting;
use rui::reactive::Signal;
use rui::{component, view};

// Context 演示(升级版):深层组件(在 <Panel> 里、跨组件边界)用 use_context 取页面 provide 的 Greeting,
// 并能**写回**它 —— 点 ✎ 进入编辑(bind:value + on_mount 聚焦),on:blur 保存到 context signal。
// 拿到的是同一个 signal,故写回后页面各处(凡 use_context 此 Greeting 的)都即时更新,全程零 prop-drill。
#[component]
pub fn greeting_badge() -> rui::View {
    let g = rui::use_context::<Greeting>().map(|x| x.0).unwrap_or_else(|| Signal::new(String::new()));
    let editing = Signal::new(false);
    let draft = Signal::new(String::new());
    view! {
        <div class="border-t border-slate-800/70 px-4 py-2 text-xs text-slate-500">
            { let (g, editing, draft) = (g.clone(), editing.clone(), draft.clone()); move || if editing.get() {
                // 编辑态:自动聚焦的输入框,失焦即保存到 context signal。
                let inp = rui::node_ref();
                rui::on_mount({ let inp = inp.clone(); move || rui::dom::focus(inp.get()) });
                let save = {
                    let (g, editing, draft) = (g.clone(), editing.clone(), draft.clone());
                    move || {
                        let t = draft.get();
                        if !t.trim().is_empty() {
                            g.set(t.trim().to_string()); // 写回 context → 页面各处即时更新
                        }
                        editing.set(false);
                    }
                };
                view! {
                    <span class="flex items-center gap-2">
                        "👤 改名:"
                        <input ref={inp} class="rounded bg-slate-800 px-2 py-0.5 text-xs text-slate-200 outline-none"
                            bind:value={ draft.clone() } on:blur={ save } />
                    </span>
                }
            } else {
                let start = {
                    let (g, editing, draft) = (g.clone(), editing.clone(), draft.clone());
                    move || { draft.set(g.get()); editing.set(true); }
                };
                view! {
                    <span class="flex items-center gap-2">
                        { let g = g.clone(); move || format!("👤 当前用户(来自 context):{}", g.get()) }
                        <button class="text-slate-400 hover:text-white transition-all" on:click={ start }>"✎"</button>
                    </span>
                }
            } }
        </div>
    }
}

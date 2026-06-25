use rui::reactive::Signal;
use rui::{component, view};

// 状态横幅:Switch 四态(空 / 全完成 / 过半 / 还剩 N),文案随进度变色;
// 「全部完成」分支带 .celebrate-enter 弹入动画(Switch 命中时该分支是新建节点 → 播放,SSR 安全)。
#[component]
pub fn status_banner(total: Signal<i64>, active: Signal<i64>) -> rui::View {
    view! {
        <Switch>
            <Match when={ let t = total.clone(); move || t.get() == 0 }>
                <p class="px-4 py-3 text-sm text-slate-500">"还没有待办,加一个吧 👆"</p>
            </Match>
            <Match when={ let a = active.clone(); move || a.get() == 0 }>
                <p class="celebrate-enter px-4 py-3 text-sm font-medium text-emerald-300">"全部完成 🎉"</p>
            </Match>
            // 过半完成(done*2 >= total)
            <Match when={ let (t, a) = (total.clone(), active.clone()); move || (t.get() - a.get()) * 2 >= t.get() }>
                <p class="px-4 py-3 text-sm text-sky-300">{ let a = active.clone(); move || format!("进展不错!还剩 {} 项 ✨", a.get()) }</p>
            </Match>
            <Match when={ move || true }>
                <p class="px-4 py-3 text-sm text-indigo-300">{ let a = active.clone(); move || format!("保持专注:还剩 {} 项 💪", a.get()) }</p>
            </Match>
        </Switch>
    }
}

//! 表单完整度演示:bind:value(文本 + 数字)· bind:checked(复选框)· bind:group(单选组)·
//! `<select>`(bind:value 走 change)· 校验(memo 派生错误消息,惯用法,无需新原语)。

use rui::reactive::{memo, Signal};
use rui::view;

#[rui::page(csr, "/forms")] // 纯客户端:表单状态是本地 signal,无需 SSR / 数据
pub fn view() -> rui::View {
    let name = Signal::new(String::new()); // 文本(Signal<String>)
    let age = Signal::new(18i64); // 数字(Signal<i64>:bind:value parse 回 i64)
    let subscribe = Signal::new(false); // 复选框(Signal<bool>)
    let plan = Signal::new(String::from("free")); // 单选组(Signal<String>)
    let color = Signal::new(String::from("blue")); // <select>(Signal<String>)

    // 校验:用 memo 从输入派生错误消息 —— 这就是「校验」的惯用法(响应式、可组合),不需要专门的原语。
    let name_err = {
        let n = name.clone();
        memo(move || {
            let v = n.get();
            if v.trim().is_empty() {
                Some("姓名必填".to_string())
            } else if v.chars().count() > 20 {
                Some("姓名最多 20 字".to_string())
            } else {
                None
            }
        })
    };
    let age_err = {
        let a = age.clone();
        memo(move || if !(0..=150).contains(&a.get()) { Some("年龄需在 0–150".to_string()) } else { None })
    };
    let valid = {
        let (ne, ae) = (name_err.clone(), age_err.clone());
        memo(move || ne.get().is_none() && ae.get().is_none())
    };

    let field = "rounded-lg bg-slate-800 px-3 py-2 outline-none placeholder:text-slate-500";

    view! {
        <div class="flex flex-col gap-5">
            <div>
                <h1 class="text-3xl font-bold tracking-tight">"表单"</h1>
                <p class="mt-1 text-sm text-slate-400">
                    "bind:value(文本 / 数字)· bind:checked · bind:group(单选)· <select> · memo 校验"
                </p>
            </div>

            // 文本 + 校验
            <label class="flex flex-col gap-1">
                <span class="text-sm text-slate-300">"姓名"</span>
                <input class={field} placeholder="你的名字" bind:value={ name.clone() } />
                { let e = name_err.clone(); move || e.get().map(|m| view! { <p class="text-sm text-rose-400">{ m }</p> }) }
            </label>

            // 数字(bind:value 自动 parse 回 i64;非法 / 空输入被忽略)
            <label class="flex flex-col gap-1">
                <span class="text-sm text-slate-300">"年龄"</span>
                <input type="number" class={field} bind:value={ age.clone() } />
                { let e = age_err.clone(); move || e.get().map(|m| view! { <p class="text-sm text-rose-400">{ m }</p> }) }
            </label>

            // 复选框
            <label class="flex items-center gap-2 text-sm text-slate-300">
                <input type="checkbox" bind:checked={ subscribe.clone() } />
                "订阅邮件"
            </label>

            // 单选组(name 分组,bind:group 同一 signal)
            <div class="flex flex-col gap-1">
                <span class="text-sm text-slate-300">"套餐"</span>
                <div class="flex gap-4 text-sm text-slate-300">
                    <label class="flex items-center gap-1.5">
                        <input type="radio" name="plan" value="free" bind:group={ plan.clone() } /> "免费"
                    </label>
                    <label class="flex items-center gap-1.5">
                        <input type="radio" name="plan" value="pro" bind:group={ plan.clone() } /> "专业"
                    </label>
                </div>
            </div>

            // <select>
            <label class="flex flex-col gap-1">
                <span class="text-sm text-slate-300">"主题色"</span>
                <select class={field} bind:value={ color.clone() }>
                    <option value="blue">"蓝"</option>
                    <option value="green">"绿"</option>
                    <option value="rose">"红"</option>
                </select>
            </label>

            // 实时回显(所有受控值)
            <div class="rounded-lg border border-slate-800 bg-slate-900/60 p-4 text-sm text-slate-300">
                { let (n, a, s, p, c) = (name.clone(), age.clone(), subscribe.clone(), plan.clone(), color.clone());
                  move || format!("姓名={} · 年龄={} · 订阅={} · 套餐={} · 主题={}", n.get(), a.get(), s.get(), p.get(), c.get()) }
            </div>

            // 整体校验状态
            { let v = valid.clone(); move || if v.get() {
                view! { <p class="text-sm text-emerald-400">"✓ 表单有效"</p> }
            } else {
                view! { <p class="text-sm text-slate-500">"修正上面的错误"</p> }
            } }
        </div>
    }
}

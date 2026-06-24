//! 详情页 —— 演示「路由参数即 signal」:`/todo/:id` 的 :id 用 `rui::param(1)` 读(reactive)。
//! 在 /todo/1 ↔ /todo/2 之间导航时页面**不重建**:param(1) 变 → resource! 自动重取 → 只换内容,无闪烁。

use rui::reactive::Signal;
use rui::{resource, view};

// 路由模式 + 命名类型化参数都声明在这里:`:id` 在签名里就是 `id: Signal<String>`,直接可用。
// 中间的 param 接线由 #[rui::page] 据模式串透明完成。
#[rui::page("/todo/:id")] // ssr:首屏服务端按 id 渲好 + 注入数据 + 水合
pub fn view(id: Signal<String>) -> rui::View {
    let idr = id.clone();
    // resource!:读 id signal,导航换参数即重取(服务端按 id 查单条)。失败 → error 态。
    let (rows, loading, error) = resource!(detail(id: idr.get()) { id text done });
    // JS 逃生舱:on_mount 用 eval 读浏览器 API(navigator.language),结果回传到 signal。
    let lang = Signal::new(String::new());
    rui::on_mount({
        let lang = lang.clone();
        move || rui::dom::eval("navigator.language || 'en'", move |r: Result<&str, &str>| {
            lang.set(r.unwrap_or("en").to_string()) // 出错回退 en
        })
    });

    view! {
        <div class="flex flex-col gap-4">
            <div>
                <a href="/" class="text-sm text-slate-400 hover:text-white transition-colors">"← 返回清单"</a>
                <h1 class="mt-2 text-3xl font-bold tracking-tight">
                    { let i = id.clone(); move || format!("待办 #{}", i.get()) }
                </h1>
                <p class="mt-1 text-sm text-slate-400">"路由参数即 signal:param(1) 变 → resource! 重取,页面不重建"</p>
                // JS 逃生舱演示:run_js 调剪贴板 API;eval 读到的浏览器语言显示在右边。
                <div class="mt-2 flex items-center gap-3">
                    <button class="rounded-lg bg-slate-800 px-3 py-1.5 text-sm hover:bg-slate-700 transition-colors"
                        on:click={ move || rui::dom::run_js("navigator.clipboard.writeText(location.href)") }>"📋 复制链接"</button>
                    <span class="text-xs text-slate-500">{ let l = lang.clone(); move || format!("🌐 {}", l.get()) }</span>
                </div>
            </div>

            <Panel title="详情 · resource!(detail(id: param(1)))">
                { let (r, l, e) = (rows.clone(), loading.clone(), error.clone()); move ||
                    if let Some(msg) = e.get() {
                        view! { <p class="px-4 py-8 text-center text-rose-400">{ format!("出错了:{}", msg) }</p> }
                    } else if l.get() {
                        view! { <p class="px-4 py-8 text-center text-slate-600">"加载中…"</p> }
                    } else if r.get().is_empty() {
                        view! { <p class="px-4 py-8 text-center text-slate-600">"找不到这条待办"</p> }
                    } else {
                        let rows = r.clone();
                        view! {
                            <ul>
                                <For list=rows item=t>
                                    <li class="flex items-center gap-3 px-4 py-5">
                                        <span class={ if t.done { "rounded-full bg-emerald-500/20 px-2 py-1 text-xs text-emerald-300" } else { "rounded-full bg-slate-700/50 px-2 py-1 text-xs text-slate-400" } }>
                                            { if t.done { "已完成" } else { "未完成" } }
                                        </span>
                                        <span class="text-lg text-slate-100">{ t.text.clone() }</span>
                                        <span class="ml-auto text-xs text-slate-600">{ format!("#{}", t.id) }</span>
                                    </li>
                                </For>
                            </ul>
                        }
                    } }
            </Panel>

            // 同页换参数:点这些链接页面不重建,只有详情内容随 param 重取而变。
            <div class="flex items-center gap-2 text-sm">
                <span class="text-slate-500">"跳到:"</span>
                <a href="/todo/1" class="rounded-lg bg-slate-800 px-3 py-1.5 hover:bg-slate-700 transition-colors">"#1"</a>
                <a href="/todo/2" class="rounded-lg bg-slate-800 px-3 py-1.5 hover:bg-slate-700 transition-colors">"#2"</a>
                <a href="/todo/3" class="rounded-lg bg-slate-800 px-3 py-1.5 hover:bg-slate-700 transition-colors">"#3"</a>
            </div>
        </div>
    }
}

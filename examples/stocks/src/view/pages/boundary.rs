//! `<ErrorBoundary>` 演示页:子树出错 → 局部 fallback + reset 重试。
//! 机制:边界建一个 error signal,经 **Context** 下发给子树(`ErrorSink`);子树里的 `<RiskyPanel>`
//! 用 `error_reporter()` 取上报器,点按钮上报 → 错误冒泡到最近边界 → 渲 fallback;reset 清错 → children 重建。

use rui::view;

#[rui::page(csr, "/boundary")] // 纯客户端演示(无数据);ErrorBoundary 同构,SSR 也照常渲正常子树
pub fn view() -> rui::View {
    view! {
        <div class="flex flex-col gap-4">
            <div>
                <h1 class="text-3xl font-bold tracking-tight">"错误边界"</h1>
                <p class="mt-1 text-sm text-slate-400">
                    "<ErrorBoundary>:子树报错 → 局部 fallback + reset 重试(经 Context 下发,跨组件冒泡到最近边界)"
                </p>
            </div>

            <ErrorBoundary fallback={ |e: String, reset: std::rc::Rc<dyn Fn()>| view! {
                <div class="flex flex-col gap-2 rounded-lg border border-rose-500/40 bg-rose-500/10 p-4">
                    <p class="text-rose-300">{ format!("⚠ 子树出错:{}", e) }</p>
                    <button class="self-start rounded-lg bg-rose-500/20 px-3 py-1.5 text-sm text-rose-200 hover:bg-rose-500/30"
                        on:click={ move || reset() }>
                        "重试(reset)"
                    </button>
                </div>
            } }>
                <RiskyPanel />
            </ErrorBoundary>
        </div>
    }
}

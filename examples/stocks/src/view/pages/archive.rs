use rui::reactive::Signal;
use rui::{paginated, query, resource, view};

#[rui::page("/archive")] // ssr
pub fn view() -> rui::View {
    // query!:全部条数(首屏 SSR 注入)
    let count = query!(todos { id });
    // paginated!:Relay 游标分页,每页 5 条,load_next 累积追加
    let (rows, load_next, has_next, loading) = paginated!(todo_page(first: 5) { id text done });

    // 搜索:query 参数驱动 —— ?q= 变就自动重取(服务端按 text 过滤)。可分享 / 可后退 / SSR 首屏带结果。
    let q = rui::query_param("q");
    let qs = q.clone();
    let (results, searching, search_err) = resource!(search(q: qs.get()) { id text done });
    // 输入框本地草稿:从 URL 的 q 起步;Enter 提交 → rui::go 把 ?q= 推进历史(URL 变 → 同步输入框)。
    let draft = Signal::new(q.get());
    {
        let (d, qq) = (draft.clone(), q.clone());
        rui::reactive::effect(move || d.set(qq.get()));
    }

    view! {
        <div class="flex flex-col gap-4">
            <div>
                <h1 class="text-3xl font-bold tracking-tight">"归档"</h1>
                <p class="mt-1 text-sm text-slate-400">"resource! 搜索(输入即重取)+ paginated! 分页 + query! 计数"</p>
            </div>

            <Panel title="搜索 · query 参数(?q=,可分享 / 可后退)">
                <form class="px-4 py-3"
                    on:submit={ let d = draft.clone(); move || rui::go(crate::route, &format!("/archive?q={}", rui::query_encode(&d.get()))) }>
                    <input class="w-full rounded-lg bg-slate-800 px-3 py-2 outline-none placeholder:text-slate-500"
                        placeholder="输入关键字回车搜索…" bind:value={draft} />
                </form>
                <div class="flex items-center gap-2 px-4 pb-1 text-xs text-slate-400">
                    <span>"快捷:"</span>
                    <a href="/archive?q=rui" class="rounded bg-slate-800 px-2 py-1 hover:bg-slate-700 transition-colors">"rui"</a>
                    <a href="/archive?q=tailwind" class="rounded bg-slate-800 px-2 py-1 hover:bg-slate-700 transition-colors">"tailwind"</a>
                    <a href="/archive" class="rounded bg-slate-800 px-2 py-1 hover:bg-slate-700 transition-colors">"清空"</a>
                </div>
                <p class={ let e = search_err.clone(); move || if e.get().is_some() { "px-4 pb-2 text-xs text-rose-400" } else { "px-4 pb-2 text-xs text-slate-500" } }>
                    { let (qq, s, r, e) = (q.clone(), searching.clone(), results.clone(), search_err.clone()); move ||
                        if let Some(m) = e.get() { format!("搜索出错:{}", m) }
                        else if qq.get().trim().is_empty() { "?q 为空 —— 试试上面的快捷或回车搜索".to_string() }
                        else if s.get() { "搜索中…".to_string() }
                        else { format!("?q = \"{}\" · 匹配 {} 条", qq.get(), r.get().len()) } }
                </p>
                <ul>
                    <For list=results item=t>
                        <li class="flex items-center gap-3 border-t border-slate-800/70 px-4 py-3">
                            <span class={ if t.done { "text-emerald-400" } else { "text-slate-600" } }>{ if t.done { "✓" } else { "○" } }</span>
                            <span class="text-slate-100">{ t.text.clone() }</span>
                        </li>
                    </For>
                </ul>
            </Panel>
            <Panel title="全部待办(分页)">
                <p class="px-4 py-2 text-xs text-slate-500">{ let c = count.clone(); move || format!("共 {} 条", c.get().len()) }</p>
                <ul>
                    <For list=rows item=e>
                        <li class="flex items-center gap-3 border-t border-slate-800/70 px-4 py-3">
                            <span class={ if e.node.done { "text-emerald-400" } else { "text-slate-600" } }>{ if e.node.done { "✓" } else { "○" } }</span>
                            <span class={ if e.node.done { "text-slate-500 line-through" } else { "text-slate-100" } }>{ e.node.text.clone() }</span>
                            <span class="ml-auto text-xs text-slate-600">{ format!("#{}", e.node.id) }</span>
                        </li>
                    </For>
                </ul>
                <div class="flex items-center gap-3 border-t border-slate-800 px-4 py-3">
                    <button class="rounded-lg bg-slate-800 px-3 py-1.5 text-sm hover:bg-slate-700 transition-colors"
                        on:click={ move || load_next() }>"加载更多"</button>
                    <span class="text-sm text-slate-500">
                        { let h = has_next.clone(); move || if h.get() { "还有更多" } else { "已全部加载" } }
                    </span>
                    <span class="text-sm text-slate-600">{ let l = loading.clone(); move || if l.get() { "加载中…" } else { "" } }</span>
                </div>
            </Panel>
        </div>
    }
}

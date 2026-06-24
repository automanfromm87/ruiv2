use rui::view;

#[rui::page(static, "/about")] // 渲一次后缓存复用(内容不变)
pub fn view() -> rui::View {
    view! {
        <div class="flex flex-col gap-5">
            <div>
                <h1 class="text-3xl font-bold tracking-tight">"关于"</h1>
                <p class="mt-1 text-sm text-slate-400">"一个用 rui 全部能力搭的 todolist · 此页是 #[rui::page(static)](渲一次缓存)"</p>
            </div>
            <div class="grid grid-cols-3 gap-3">
                <Stat label="数据层" value="GraphQL" />
                <Stat label="渲染" value="细粒度无 VDOM" />
                <Stat label="交付" value="SSR + 真水合" />
            </div>
            <Panel title="这个 app 用到的 feature">
                <ul class="px-5 py-3 text-sm text-slate-300 leading-7">
                    <li>"· 待办列表 = subscription!(实时,服务端写操作广播 → 自动反映增删改)"</li>
                    <li>"· 新增 = form on:submit + bind:value 双向绑定"</li>
                    <li>"· 勾选 / 删除 = 动态 GraphQL mutation(按 id)"</li>
                    <li>"· 全部完成 = mutation! + 乐观更新(瞬时勾上)"</li>
                    <li>"· 过滤 = memo 派生 + 响应式属性高亮 tab"</li>
                    <li>"· 状态横幅 = <Switch>/<Match>;空状态 = if 表达式条件渲染"</li>
                    <li>"· 每行 = #[rui::component](片段 data masking + children 槽 + 闭包 props)"</li>
                    <li>"· 列表 = keyed <For>(勾选只重建那一行,增删不重建整列)"</li>
                    <li>"· 归档 = paginated! 游标分页 + query! 计数 + 规范化缓存"</li>
                    <li>"· 页面策略 = #[rui::page] ssr / csr(草稿)/ static(本页)"</li>
                </ul>
            </Panel>
        </div>
    }
}

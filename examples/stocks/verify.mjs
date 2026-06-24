// 无浏览器验证(todo 应用):路由渲染 + 订阅列表 + 增删改 + 过滤 + 组件 + keyed For + 数据交接。
const enc = new TextEncoder();
const dec = new TextDecoder();

async function fresh() {
  const nodes = [null];
  let fetchReq = null;
  const fetches = []; // 记录全部请求(一页多个并发请求时按 query 子串挑)
  let memory;
  let root = 0;
  let clearAppCalls = 0; // SPA 换页才 clear_app;同页换参数不应触发(据此验证"不重建")
  const focused = []; // on_mount 命令式聚焦记录
  const intervals = []; // set_interval 记录({ms,h})
  const cleared = []; // clear_interval 记录(on_cleanup 验证)
  let timerSeq = 0;
  const jsRun = []; // run_js / run_js_on 记录(逃生舱)
  const evals = []; // eval_js 记录({code,handler})
  const str = (p, l) => dec.decode(new Uint8Array(memory.buffer, p, l));
  const reg = (node) => (nodes.push(node), nodes.length - 1);
  const detach = (id) => { for (const n of nodes) if (n && n.children) { const i = n.children.indexOf(id); if (i >= 0) n.children.splice(i, 1); } };
  const env = {
    create_element: (p, l) => reg({ tag: str(p, l), attrs: {}, children: [], text: null }),
    create_text: (p, l) => reg({ tag: "#text", text: str(p, l), children: [] }),
    claim_element: () => 0,
    claim_text: () => 0,
    set_text: (id, p, l) => { nodes[id].text = str(p, l); },
    append_child: (par, ch) => { detach(ch); nodes[par].children.push(ch); }, // children 存 id;移动语义
    remove_child: (par, ch) => detach(ch),
    set_attr: (id, np, nl, vp, vl) => { (nodes[id].attrs ||= {})[str(np, nl)] = str(vp, vl); },
    set_value: (id, p, l) => { nodes[id].value = str(p, l); },
    add_event: () => {},
    clear_children: (id) => { if (nodes[id]) nodes[id].children = []; },
    gql_query: (qp, ql, h) => { fetchReq = { query: str(qp, ql), handler: h }; fetches.push(fetchReq); },
    gql_subscribe: (qp, ql, h) => { fetchReq = { query: str(qp, ql), handler: h }; fetches.push(fetchReq); },
    mount: (id) => { root = id; },
    clear_app: () => { clearAppCalls++; root = 0; }, // 换页:清容器(随后 mount 新根)
    push_url: () => {}, // 程序化导航:浏览器历史(无头环境无需实现)
    focus: (id) => { focused.push(id); }, // on_mount 命令式聚焦
    scroll_into_view: () => {},
    set_interval: (ms, h) => { intervals.push({ ms, h }); return ++timerSeq; }, // 返回 timer id
    clear_interval: (t) => { cleared.push(t); }, // on_cleanup 清定时器
    run_js: (p, l) => { jsRun.push(str(p, l)); }, // 逃生舱:即发即弃
    run_js_on: (id, p, l) => { jsRun.push(str(p, l)); },
    eval_js: (p, l, h) => { evals.push({ code: str(p, l), handler: h }); }, // 取返回:测试里手动回传
  };
  const bytes = await Bun.file(new URL("./web/app.wasm", import.meta.url)).arrayBuffer();
  const { instance } = await WebAssembly.instantiate(bytes, { env });
  const X = instance.exports;
  memory = X.memory;
  const write = (s) => { const b = enc.encode(s); const ptr = X.alloc(b.length); new Uint8Array(memory.buffer, ptr, b.length).set(b); return [ptr, b.length]; };
  const render = (path) => { const [p, l] = write(path); X.render_route(p, l); };
  const navigate = (path) => { const [p, l] = write(path); X.navigate(p, l); }; // SPA 导航(同页换参数 vs 换页)
  const onFetch = (json) => { const [p, l] = write(json); X.on_fetch(fetchReq.handler, p, l); };
  // 按 query 子串挑某个请求并喂数据给它的 handler(一页多请求时用)
  const fetchFor = (s) => fetches.find((r) => r.query.includes(s));
  const countFor = (s) => fetches.filter((r) => r.query.includes(s)).length;
  const feed = (s, json) => { const r = fetchFor(s); const [p, l] = write(json); X.on_fetch(r.handler, p, l); };
  const fire = (id, value = "") => { const [p, l] = write(value); X.dispatch(id, p, l); };
  const texts = () => nodes.filter(Boolean).map((n) => n.text).filter(Boolean);
  const has = (s) => texts().some((t) => t.includes(s));
  // 从挂载根遍历「活树」数 <li>(脱离的旧节点不算 → 反映 keyed For / 过滤的当前结果)
  const liCount = () => {
    let n = 0;
    const walk = (id) => { const x = nodes[id]; if (!x) return; if (x.tag === "li") n++; (x.children || []).forEach(walk); };
    walk(root);
    return n;
  };
  const runInterval = (h) => X.run_interval(h); // 手动触发一次定时器回调
  // eval 结果回传:首字节 \x00=ok / \x01=err(与 router.js eval_js 一致)
  const evalReturn = (codeSub, result, okFlag = true) => { const e = evals.find((x) => x.code.includes(codeSub)); const [p, l] = write((okFlag ? "\x00" : "\x01") + result); X.on_fetch(e.handler, p, l); };
  return { X, render, navigate, onFetch, fire, fetchFor, countFor, feed, texts, has, liCount, runInterval, evalReturn, focused, intervals, cleared, jsRun, evals, get fetchReq() { return fetchReq; }, get clearAppCalls() { return clearAppCalls; } };
}

let ok = true;
const check = (c, m) => { console.log((c ? "✓ " : "✗ ") + m); ok = ok && c; };

const TODOS = '{"data":{"todo_updates":[' +
  '{"__typename":"Todo","__id":"a","id":"a","text":"写代码","done":false},' +
  '{"__typename":"Todo","__id":"b","id":"b","text":"喝咖啡","done":true}]}}';

// 1) 各路由渲染
for (const [path, title] of [["/", "待办清单"], ["/archive", "归档"], ["/about", "关于"], ["/draft", "草稿"]]) {
  const f = await fresh();
  f.render(path);
  check(f.has(title), `${path.padEnd(9)} 渲染含 "${title}"`);
}

// 2) 首页:subscription 列表 + memo 统计 + 组件 + keyed For
{
  const f = await fresh();
  f.render("/");
  check(f.fetchReq && f.fetchReq.query.includes("todo_updates"), "/         发起 subscription todo_updates");
  check(f.fetchReq.query.includes("id text done"), "/         订阅内联了 ...TodoView 片段字段");
  f.onFetch(TODOS);
  check(f.has("写代码") && f.has("喝咖啡"), "/         订阅数据 → 组件列表渲染");
  check(f.liCount() === 2, `/         2 个 <li>(keyed For),实际 ${f.liCount()}`);
  check(f.has("共 2 项 · 1 未完成 · 1 已完成"), "/         memo 统计正确");
}

// 3) 首页:增 / 改 / 批量(事件 + 动态 mutation + mutation! 乐观)
{
  const f = await fresh();
  f.render("/");
  f.onFetch(TODOS);
  // handler:0=form submit,1=input,2/3/4=过滤 tab,5=全部完成,6=清除,7=首行 toggle
  f.fire(7); // 勾选第一行
  check(f.fetchReq.query.includes("toggle_todo") && f.fetchReq.query.includes('"a"'), "/         勾选 → 动态 toggle_todo(id=a)");
  f.fire(1, "买牛奶"); // 输入(bind:value)
  f.fire(0); // 提交表单(on:submit)
  check(f.fetchReq.query.includes("add_todo") && f.fetchReq.query.includes("买牛奶"), "/         表单提交 → add_todo(text=买牛奶)");
  f.fire(5); // 全部完成(mutation! + 乐观)
  check(f.fetchReq.query.includes("complete_all"), "/         全部完成 → mutation! complete_all");
}

// 4) 首页:过滤(memo 派生 + keyed For 增删行)
{
  const f = await fresh();
  f.render("/");
  f.onFetch(TODOS); // a 未完成, b 已完成
  check(f.liCount() === 2, "/         过滤前 2 行");
  f.fire(3); // 未完成 tab
  check(f.liCount() === 1, `/         过滤「未完成」→ 1 行,实际 ${f.liCount()}`);
  f.fire(4); // 已完成 tab
  check(f.liCount() === 1, `/         过滤「已完成」→ 1 行,实际 ${f.liCount()}`);
  f.fire(2); // 全部
  check(f.liCount() === 2, "/         过滤「全部」→ 2 行");
}

// 5) 归档:paginated! 游标分页(一页多请求,按 query 子串挑)
{
  const f = await fresh();
  f.render("/archive");
  check(!!f.fetchFor("todo_page(first: 5"), "/archive  发起分页 todo_page");
  f.feed("todo_page", '{"data":{"todo_page":[{"__typename":"TodoConnection","__id":null,"edges":[' +
    '{"__typename":"TodoEdge","__id":null,"node":{"__typename":"Todo","__id":"a","id":"a","text":"归档项A","done":true},"cursor":"a"}' +
    '],"page_info":{"has_next_page":true,"end_cursor":"a"}}]}}');
  check(f.has("归档项A"), "/archive  分页数据渲染");
}

// 5b) 归档:query 参数(?q=)驱动 resource! 搜索 —— 与 path 参数同机制、独立一条线
{
  const f = await fresh();
  f.render("/archive?q=todolist"); // 首屏即带 ?q
  check(!!f.fetchFor('search(q: "todolist")'), "/archive?q=todolist  query_param 驱动 search(q=todolist)");
  f.feed('search(q: "todolist")', '{"data":{"search":[{"__typename":"Todo","__id":"x","id":"x","text":"todolist 命中","done":false}]}}');
  check(f.has("todolist 命中"), "/archive?q=todolist  query 搜索结果渲染");
  // 同页换 query(同 key archive)→ 不重建,只 ?q 变 → resource! 重取
  const before = f.clearAppCalls;
  f.navigate("/archive?q=rui");
  check(f.clearAppCalls === before, "/archive  换 ?q 不重建(同 key,未 clear_app)");
  check(!!f.fetchFor('search(q: "rui")'), "/archive  ?q 变 → resource! 重取 search(q=rui)");
  // 换 path(归档→todo,key 不同)→ clear_app 重建
  f.navigate("/todo/9");
  check(f.clearAppCalls === before + 1, "/todo/9   换 path(key 不同)→ clear_app 重建");
}

// 6) 草稿:csr 页 + bind:value 双向
{
  const f = await fresh();
  f.render("/draft");
  f.fire(0, "你好"); // textarea bind:value 的 input handler
  check(f.has("2 字"), "/draft    bind:value → 实时字数(2 字)");
  check(f.has("你好"), "/draft    条件渲染 → 预览出现");
}

// 7) 数据交接:首页订阅响应预置 → 首屏命中缓存,不发请求
{
  const f0 = await fresh();
  f0.render("/");
  const q = f0.fetchReq && f0.fetchReq.query; // 订阅查询串
  const f = await fresh();
  { const [p, l] = (() => { const b = enc.encode(JSON.stringify({ [q]: TODOS })); const ptr = f.X.alloc(b.length); new Uint8Array(f.X.memory.buffer, ptr, b.length).set(b); return [ptr, b.length]; })(); f.X.hydrate_data(p, l); }
  f.render("/");
  check(f.has("写代码"), "/         命中 SSR 注入的订阅初值 → 首屏直接出列表");
}

// 8) 路由参数即 signal:/todo/:id 用 rui::param(1) + resource!;同页换参数不重建、reactive 重取
{
  const f = await fresh();
  f.render("/todo/1");
  check(!!f.fetchFor('detail(id: "1")'), "/todo/1   resource! 发起 detail(id=1)");
  f.feed('detail(id: "1")', '{"data":{"detail":[{"__typename":"Todo","__id":"1","id":"1","text":"第一条待办","done":true}]}}');
  check(f.has("第一条待办"), "/todo/1   详情渲染(第一条待办)");

  // 同页换参数:导航到 /todo/2 —— key 相同 → 不 clear_app(页面不重建),只 param 变 → resource! 重取
  const before = f.clearAppCalls;
  f.navigate("/todo/2");
  check(f.clearAppCalls === before, "/todo/2   同页换参数:未 clear_app(页面不重建)");
  check(!!f.fetchFor('detail(id: "2")'), "/todo/2   param signal 变 → resource! 重取 detail(id=2)");
  f.feed('detail(id: "2")', '{"data":{"detail":[{"__typename":"Todo","__id":"2","id":"2","text":"第二条待办","done":false}]}}');
  check(f.has("第二条待办"), "/todo/2   重取后内容原地更新(第二条待办)");

  // 换页:导航到 /archive —— key 不同 → clear_app 一次(整页重建)
  f.navigate("/archive");
  check(f.clearAppCalls === before + 1, "/archive  换页(key 不同)→ clear_app 重建一次");
  check(f.has("归档"), "/archive  换页后新页渲染");
}

// 9) 同 URL 导航去抖:再次导航到当前路径不重取(set_path 值不变不写 → param memo 不重通知)
{
  const f = await fresh();
  f.render("/todo/5");
  f.feed('detail(id: "5")', '{"data":{"detail":[]}}');
  const before = f.countFor('detail(id: "5")');
  f.navigate("/todo/5"); // 同路径
  check(f.countFor('detail(id: "5")') === before, `/todo/5   同 URL 再导航:不重取(set_path 去抖),仍 ${before} 次`);
  f.navigate("/todo/6"); // 换参数
  check(f.countFor('detail(id: "6")') >= 1, "/todo/6   换参数:正常重取");
}

// 11) query memo 去抖 + percent 解码
{
  const f = await fresh();
  f.render("/archive?q=a&sort=x");
  const n = f.countFor('search(q: "a")');
  check(n === 1, `/archive?q=a&sort=x  q 初次 search(q=a),${n} 次`);
  f.navigate("/archive?q=a&sort=y"); // 只 sort 变、q 不变
  check(f.countFor('search(q: "a")') === n, "/archive  无关 key(sort)变 → q 不重取(memo 值相等去抖)");

  const g = await fresh();
  g.render("/archive?q=hello%20world");
  check(!!g.fetchFor('search(q: "hello world")'), "/archive?q=hello%20world  query 值 percent 解码为空格");

  const h = await fresh();
  h.render("/archive?q=a+b");
  check(!!h.fetchFor('search(q: "a b")'), "/archive?q=a+b  query 值 + 解码为空格");
}

// 12) 错误处理:resource! 失败 → error 态;重取成功 → 渲染数据
{
  const f = await fresh();
  f.render("/todo/1");
  f.feed('detail(id: "1")', '{"errors":[{"message":"boom"}]}'); // 模拟 GraphQL 失败
  check(f.has("出错了:boom"), "/todo/1   resource! 失败 → error 态显示(boom)");
  f.feed('detail(id: "1")', '{"data":{"detail":[{"__typename":"Todo","__id":"1","id":"1","text":"恢复正常","done":false}]}}');
  check(f.has("恢复正常"), "/todo/1   重取成功 → 渲染数据(error 清除)");
}

// 13) mutation! on_error:失败 → 回调(示例里写错误横幅)
{
  const f = await fresh();
  f.render("/");
  f.onFetch(TODOS);
  f.fire(6); // 「清除已完成」按钮 → clear_done mutation
  f.feed('clear_done', '{"errors":[{"message":"nope"}]}');
  check(f.has("操作失败:nope"), "/         mutation! 失败 → on_error 回调写错误横幅(nope)");
}

// 14) 生命周期:on_mount 聚焦 + 定时器 tick + on_cleanup 清定时器
{
  const f = await fresh();
  f.render("/");
  f.onFetch(TODOS);
  check(f.focused.length > 0, "/         on_mount → focus(自动聚焦输入框)");
  check(f.intervals.length > 0, "/         uptime on_mount → set_interval 启动定时器");
  f.runInterval(f.intervals[0].h); // 手动 tick 一次
  check(f.has("⏱ 1s"), "/         interval tick → 时钟 reactive 更新(1s)");
  f.navigate("/about"); // 换页(不同 key)→ scope dispose
  check(f.cleared.length > 0, "/about    换页 → on_cleanup → clear_interval(定时器不泄漏)");
}

// 15) 嵌套路由组:/dash ↔ /dash/settings 共享 dash_shell(同组 key 不重建,outlet 按 path 换内容)
{
  const f = await fresh();
  f.render("/dash");
  f.feed("todos", '{"data":{"todos":[{"__typename":"Todo","__id":"a","id":"a","done":true}]}}');
  check(f.has("共享左侧"), "/dash           组 outlet 渲 overview(总览)");
  check(f.has("总览") && f.has("设置"), "/dash           dash_shell 侧栏(总览/设置)在");
  const before = f.clearAppCalls;
  f.navigate("/dash/settings"); // 同组 key("group:/dash")→ 不重建
  check(f.clearAppCalls === before, "/dash/settings  同组导航:dash_shell 不重建(未 clear_app)");
  check(f.has("显示名"), "/dash/settings  outlet 换成 settings 内容");
  f.navigate("/"); // 离开组(不同 key)→ 重建
  check(f.clearAppCalls === before + 1, "/               离开组(不同 key)→ clear_app 重建");
}

// 16) JS 逃生舱:eval 读浏览器值(回传 signal)+ run_js 调剪贴板
{
  const f = await fresh();
  f.render("/todo/1");
  f.feed('detail(id: "1")', '{"data":{"detail":[{"__typename":"Todo","__id":"1","id":"1","text":"x","done":false}]}}');
  check(f.evals.some((e) => e.code.includes("navigator.language")), "/todo/1  on_mount eval 读 navigator.language");
  f.evalReturn("navigator.language", "zh-CN"); // 模拟浏览器成功返回(ok)
  check(f.has("zh-CN"), "/todo/1  eval Ok 结果回传 signal → 显示(🌐 zh-CN)");
  // 错误通道:err 不会被当成正常值(回退 en,不显示 ERROR: 文本)
  const g = await fresh();
  g.render("/todo/1");
  g.feed('detail(id: "1")', '{"data":{"detail":[{"__typename":"Todo","__id":"1","id":"1","text":"x","done":false}]}}');
  g.evalReturn("navigator.language", "TypeError: boom", false); // err
  check(g.has("🌐 en"), "/todo/1  eval Err → 走错误分支(回退 en),不把错误当值");
  f.fire(0); // 「复制链接」按钮(detail 唯一 on:click)
  check(f.jsRun.some((c) => c.includes("clipboard")), "/todo/1  复制按钮 → run_js(navigator.clipboard)");
}

console.log(ok ? "✅ 全部通过" : "❌ 有失败");
process.exit(ok ? 0 : 1);

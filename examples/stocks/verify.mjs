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
  let creates = 0; // create_element/create_text 计数 → 同页换参数应为 0 新建(INV-1 直接证明页面体未重跑)
  const focused = []; // on_mount 命令式聚焦记录
  const intervals = []; // set_interval 记录({ms,h})
  const timeouts = []; // set_timeout 记录({ms,h})—— 过渡的延时移除,测试手动触发
  const cleared = []; // clear_interval 记录(on_cleanup 验证)
  let timerSeq = 0;
  const jsRun = []; // run_js / run_js_on 记录(逃生舱)
  const evals = []; // eval_js 记录({code,handler})
  const str = (p, l) => dec.decode(new Uint8Array(memory.buffer, p, l));
  const reg = (node) => (nodes.push(node), nodes.length - 1);
  const detach = (id) => { for (const n of nodes) if (n && n.children) { const i = n.children.indexOf(id); if (i >= 0) n.children.splice(i, 1); } };
  const env = {
    create_element: (p, l) => (creates++, reg({ tag: str(p, l), attrs: {}, children: [], text: null })),
    create_text: (p, l) => (creates++, reg({ tag: "#text", text: str(p, l), children: [] })),
    claim_element: () => 0,
    claim_text: () => 0,
    set_text: (id, p, l) => { nodes[id].text = str(p, l); },
    append_child: (par, ch) => { detach(ch); nodes[par].children.push(ch); }, // children 存 id;移动语义
    remove_child: (par, ch) => detach(ch),
    set_attr: (id, np, nl, vp, vl) => { (nodes[id].attrs ||= {})[str(np, nl)] = str(vp, vl); },
    set_value: (id, p, l) => { nodes[id].value = str(p, l); },
    set_checked: (id, on) => { if (nodes[id]) nodes[id].checked = !!on; }, // 受控复选框/单选
    console_error: (p, l) => console.error(str(p, l)), // panic hook
    add_class: (id, p, l) => { const n = nodes[id]; if (n) (n.cls ||= new Set()).add(str(p, l)); }, // 过渡 enter/leave 类
    remove_class: (id, p, l) => { const n = nodes[id]; if (n && n.cls) n.cls.delete(str(p, l)); },
    set_timeout: (ms, h) => { timeouts.push({ ms, h }); }, // 记录,测试手动 runTimeouts 触发(控制时序)
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
  // 编码完整事件(与 router.js encodeEvent 对应)→ dispatch。opts: {value,checked,key,code,ctrl,shift,alt,meta,clientX,clientY,button,deltaY,files:[{name,size,type}]}
  const encodeEvt = (o = {}) => {
    const US = "\x1f", GS = "\x1d", RS = "\x1e";
    const clean = (s) => String(s == null ? "" : s).replace(/[\x1d\x1e\x1f]/g, ""); // 同 router.js:剥分隔符
    const files = (o.files || []).map((f) => `${clean(f.name)}${RS}${f.size || 0}${RS}${clean(f.type)}`).join(GS);
    return [clean(o.value ?? ""), o.checked ? "1" : "", clean(o.key ?? ""), clean(o.code ?? ""), o.ctrl ? "1" : "", o.shift ? "1" : "", o.alt ? "1" : "", o.meta ? "1" : "", o.clientX ?? "", o.clientY ?? "", o.button ?? "", o.deltaY ?? "", files].join(US);
  };
  const fireEvent = (id, opts) => { const [p, l] = write(encodeEvt(opts)); X.dispatch(id, p, l); };
  const fire = (id, value = "") => fireEvent(id, { value }); // 便捷:只带 value(文本/单选)
  const texts = () => nodes.filter(Boolean).map((n) => n.text).filter(Boolean);
  const has = (s) => texts().some((t) => t.includes(s));
  // 从挂载根遍历「活树」收集文本(脱离的旧节点不算 → 负向断言可靠,如"X 已被替换/撤下")。
  const liveTexts = () => {
    const out = [];
    const walk = (id) => { const x = nodes[id]; if (!x) return; if (x.text) out.push(x.text); (x.children || []).forEach(walk); };
    walk(root);
    return out;
  };
  const liveHas = (s) => liveTexts().some((t) => t.includes(s));
  // 任一节点是否有某属性(可选指定值)—— 验证关键字属性名(type/for)未被 r# 污染。
  const hasAttr = (k, v) => nodes.some((n) => n && n.attrs && (v === undefined ? n.attrs[k] !== undefined : n.attrs[k] === v));
  // 任一节点的 .checked 状态(受控复选框/单选,set_checked 写入)
  const anyChecked = () => nodes.some((n) => n && n.checked === true);
  // 任一活树节点带某 class:既查 add_class/remove_class 的动态 .cls 集,也查静态 class 属性(set_attr)。
  const anyClass = (cls) => {
    let r = false;
    const walk = (id) => {
      const x = nodes[id];
      if (!x) return;
      if (x.cls && x.cls.has(cls)) r = true;
      if (x.attrs && x.attrs.class && x.attrs.class.split(/\s+/).includes(cls)) r = true;
      (x.children || []).forEach(walk);
    };
    walk(root);
    return r;
  };
  // 触发所有挂起的 set_timeout 回调(过渡出场动画结束 → 真正移除)
  const runTimeouts = () => { const ts = timeouts.splice(0); for (const t of ts) X.run_oneshot(t.h); };
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
  return { X, render, navigate, onFetch, fire, fireEvent, fetchFor, countFor, feed, texts, has, liveHas, hasAttr, anyChecked, anyClass, runTimeouts, liCount, runInterval, evalReturn, focused, intervals, timeouts, cleared, jsRun, evals, get fetchReq() { return fetchReq; }, get clearAppCalls() { return clearAppCalls; }, get creates() { return creates; } };
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
  check(f.has("共 2") && f.has("未完成 1") && f.has("已完成 1"), "/         memo 统计(stat 徽章)正确");
  check(f.anyClass("todo-enter"), "/         列表行带进场动画类 .todo-enter");
  check(f.anyChecked(), "/         已完成行复选框 set_checked(bind:checked 反映 done)");
}

// 3) 首页:增 / 改 / 批量(事件 + 动态 mutation + mutation! 乐观)
{
  const f = await fresh();
  f.render("/");
  f.onFetch(TODOS);
  // handler 注册序(shell 无 on:):0=提示×,1=submit,2=输入,3=输入 keydown.escape,4/5/6=过滤,
  //   7=全部完成,8=清除,9=改名✎,然后每行 3 个:行a 10=复选框bind/11=toggle/12=删,行b 13/14/15。
  f.fire(11); // 勾选第一行(toggle 真请求;复选框 bind 是 10,只动视觉)
  check(f.fetchReq.query.includes("toggle_todo") && f.fetchReq.query.includes('"a"'), "/         勾选(复选框 on:change)→ 动态 toggle_todo(id=a)");
  f.fire(2, "买牛奶"); // 输入(bind:value)
  f.fire(1); // 提交表单(on:submit)
  check(f.fetchReq.query.includes("add_todo") && f.fetchReq.query.includes("买牛奶"), "/         表单提交 → add_todo(text=买牛奶)");
  f.fire(7); // 全部完成(mutation! + 乐观)
  check(f.fetchReq.query.includes("complete_all"), "/         全部完成 → mutation! complete_all");
}

// 4) 首页:过滤(memo 派生 + keyed For 增删行)
{
  const f = await fresh();
  f.render("/");
  f.onFetch(TODOS); // a 未完成, b 已完成
  check(f.liCount() === 2, "/         过滤前 2 行");
  f.fire(5); // 未完成 tab(id 5)
  check(f.liCount() === 1, `/         过滤「未完成」→ 1 行,实际 ${f.liCount()}`);
  f.fire(6); // 已完成 tab(id 6)
  check(f.liCount() === 1, `/         过滤「已完成」→ 1 行,实际 ${f.liCount()}`);
  f.fire(4); // 全部(id 4)
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
  const detailMounts = () => f.evals.filter((e) => e.code.includes("navigator.language")).length;
  const before = f.clearAppCalls;
  const mountsBefore = detailMounts(); // detail 页 on_mount(eval navigator.language)= 页面体执行一次的"once"副作用
  f.navigate("/todo/2");
  check(f.clearAppCalls === before, "/todo/2   同页换参数:未 clear_app(页面不重建)");
  // INV-1 直接断言:页面体的 on_mount-once 副作用未再触发 → 页面体未重新执行(真·不重建,非仅"未 clear_app"的代理)。
  // 注:内层 reactive_block 因 resource! 进 loading 态会增 create_element,那是细粒度更新、不算页面体重跑。
  check(detailMounts() === mountsBefore, `/todo/2   同页换参数:页面体未重新执行(detail on_mount 未再触发),${mountsBefore}→${detailMounts()}`);
  check(!!f.fetchFor('detail(id: "2")'), "/todo/2   param signal 变 → resource! 重取 detail(id=2)");
  f.feed('detail(id: "2")', '{"data":{"detail":[{"__typename":"Todo","__id":"2","id":"2","text":"第二条待办","done":false}]}}');
  check(f.has("第二条待办"), "/todo/2   重取后内容原地更新(第二条待办)");

  // 换页:导航到 /archive —— key 不同 → clear_app 一次(整页重建)
  const createsBeforeCross = f.creates;
  f.navigate("/archive");
  check(f.clearAppCalls === before + 1, "/archive  换页(key 不同)→ clear_app 重建一次");
  check(f.creates > createsBeforeCross, "/archive  换页:有新建节点(整页重建)—— 与同页零新建形成 INV-1 对照");
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
  f.fire(8); // 「清除已完成」按钮 → clear_done mutation(handler 8)
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

// 14b) INV-5:on_mount 在「动态重建」后仍运行 —— 点 ✎ 动态显示的编辑输入子树(GreetingBadge,reactive_block
//   事件中重建)其 on_mount→focus 必须经 dispatch 尾部 flush_mounts 跑到(否则只在首屏静态渲染时挂载)。
//   强制来源:runtime.rs flush_mounts + client! 在 dispatch 尾部调它。
{
  const f = await fresh();
  f.render("/");
  f.onFetch(TODOS);
  const focusedBefore = f.focused.length; // 首屏 AddForm on_mount 已聚焦一次(静态子树)
  f.fire(9); // 点 ✎ 进入编辑(动态显示带 on_mount 聚焦的编辑输入子树)
  check(f.focused.length === focusedBefore + 1, `/         INV-5 动态子树 on_mount 在 dispatch 后运行(编辑框聚焦),focused ${focusedBefore}→${f.focused.length}`);
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

// 17) Context:页面 provide(Greeting),深层 <GreetingBadge>(在 Panel 里)inject —— 无 props 传递
{
  const f = await fresh();
  f.render("/");
  f.onFetch(TODOS);
  check(f.has("👤 当前用户(来自 context):rui"), "/         深层组件经 context 取到页面 provide 的 Greeting");
}

// 18) ErrorBoundary:子树上报错误(error_reporter,跨组件经 context 冒泡)→ fallback;reset → 恢复
{
  const f = await fresh();
  f.render("/boundary");
  check(f.liveHas("正常子树内容"), "/boundary 初始渲正常子树(<RiskyPanel>)");
  // RiskyPanel 的「触发错误」按钮是本页首个 on:click(shell/navbar 无 on:)→ handler id 0
  f.fire(0);
  check(f.liveHas("RiskyPanel 主动上报的错误"), "/boundary 上报 → 渲 fallback(经 ErrorSink context 冒泡)");
  check(!f.liveHas("正常子树内容"), "/boundary fallback 时正常子树已被替换(活树)");
  // fallback 渲染后注册了「重试」on:click → handler id 1;reset 清错 → children 重建
  f.fire(1);
  check(f.liveHas("正常子树内容"), "/boundary reset → children 重建,恢复正常子树");
  check(!f.liveHas("子树出错"), "/boundary reset 后 fallback 已撤下(活树)");
}

// 19) 换页清理事件处理器:HANDLERS 不随导航无界增长 → 换页后 handler id 从 0 重启
{
  const f = await fresh();
  f.render("/");           // 首页注册若干 handler(id 0=表单提交…)
  f.onFetch(TODOS);
  f.navigate("/boundary"); // 换页(不同 key)→ clear_app + clear_handlers
  f.fire(0);               // 已清理则 id 0 = /boundary 首个 on:click(RiskyPanel「触发错误」)
  check(f.liveHas("RiskyPanel 主动上报的错误"), "/boundary 换页后 fire(0) 命中新页按钮 → HANDLERS 已回收(否则 id 0 仍指旧页)");
}

// 20) 表单完整度:bind:value(文本/数字)、bind:checked、bind:group、<select>、memo 校验、关键字属性
{
  const f = await fresh();
  f.render("/forms");
  check(f.hasAttr("type", "checkbox"), "/forms 关键字属性 type 正确(parse_any 未污染成 r#type → 浏览器认 checkbox)");
  check(f.hasAttr("type", "radio") && f.hasAttr("type", "number"), "/forms type=radio/number 都正确渲出");
  check(f.liveHas("姓名必填"), "/forms 初始 name 空 → memo 校验错误显示");
  check(f.liveHas("姓名= · 年龄=18 · 订阅=false · 套餐=free · 主题=blue"), "/forms 初始受控值回显(数字/bool/单选/select)");
  // handler id(shell 无 on:):0=name input,1=age input,2=checkbox,3=radio free,4=radio pro,5=select
  f.fire(0, "Alice");
  check(f.liveHas("姓名=Alice"), "/forms bind:value 文本 → 回显更新");
  check(!f.liveHas("姓名必填"), "/forms 填了 name → 校验错误消失");
  f.fire(1, "25");
  check(f.liveHas("年龄=25"), "/forms bind:value 数字 → parse 回 i64");
  f.fire(1, "abc"); // 非法 → parse 失败不写
  check(f.liveHas("年龄=25"), "/forms 非法数字输入被忽略(parse 失败不写 signal)");
  f.fireEvent(2, { checked: true }); // 复选框 change:event().checked=true
  check(f.liveHas("订阅=true"), "/forms bind:checked → Signal<bool>(读 event().checked)");
  check(f.anyChecked(), "/forms set_checked 回写 .checked(受控复选框)");
  f.fire(4, "pro");
  check(f.liveHas("套餐=pro"), "/forms bind:group 单选 → Signal<String>");
  f.fire(5, "green");
  check(f.liveHas("主题=green"), "/forms <select> bind:value(change)→ Signal<String>");
}

// 21) 过渡:<Transition> 进出场 —— enter/leave 类 + 出场延时移除(代际防抖)
{
  const f = await fresh();
  f.render("/transitions");
  check(f.liveHas("我会淡入"), "/transitions 初始 show → child 在 DOM");
  check(f.anyClass("fade-enter"), "/transitions 初始进场 → 加 fade-enter 类");
  // 切换按钮是本页首个 on:click → id 0
  f.fire(0); // show → false:出场
  check(f.anyClass("fade-leave"), "/transitions 切换关 → 加 fade-leave(出场动画)");
  check(f.liveHas("我会淡入"), "/transitions 出场动画期间 child 仍在 DOM(延时移除,非立即)");
  f.runTimeouts(); // 出场动画结束 → 真正移除
  check(!f.liveHas("我会淡入"), "/transitions 延时后 child 真正从 DOM 移除");
  f.fire(0); // show → true:再进场
  check(f.liveHas("我会淡入") && f.anyClass("fade-enter"), "/transitions 再切换开 → child 重新进场(fade-enter)");
}

// 22) 首页新特性:提示条 <Transition> 关闭 · AddForm 校验 · GreetingBadge context 内联改名
{
  // 22a 提示条:Transition 默认显示(SSR 安全),× 关闭 → 离场动画 + 延时移除
  const f = await fresh();
  f.render("/");
  check(f.liveHas("本页用上了"), "/         提示条默认显示(<Transition> when=true)");
  f.fire(0); // 点 ×(handler 0)
  check(f.anyClass("fade-leave"), "/         关提示 → fade-leave 离场动画");
  check(f.liveHas("本页用上了"), "/         离场动画期间提示条仍在(延时移除)");
  f.runTimeouts();
  check(!f.liveHas("本页用上了"), "/         延时后提示条真正移除");
}
{
  // 22b AddForm 校验:实时字数 + 空提交被拦
  const f = await fresh();
  f.render("/");
  check(f.has("0/80"), "/         AddForm 字数计数初始 0/80");
  f.fire(1); // 空提交(submit handler 1)
  check(!(f.fetchReq && f.fetchReq.query.includes("add_todo")), "/         空白提交被 memo 校验拦下(未发 add_todo)");
  f.fire(2, "买牛奶"); // 输入(bind:value handler 2)
  check(f.has("3/80"), "/         输入后字数 3/80");
  f.fire(1); // 有效提交
  check(f.fetchReq.query.includes("add_todo"), "/         有效提交 → add_todo");
}
{
  // 22c GreetingBadge:深层组件经 context 内联改名(写回 context signal,无 prop-drill)
  const f = await fresh();
  f.render("/");
  check(f.liveHas("当前用户(来自 context):rui"), "/         改名前显示 context 初值 rui");
  f.fire(9); // 点 ✎ 进入编辑(handler 9)
  check(f.liveHas("改名"), "/         点 ✎ → 进入编辑态(bind:value 输入框)");
  f.fire(10, "小明"); // 编辑输入(bind:value handler 10)
  f.fire(11); // 失焦保存(on:blur handler 11)→ 写回 context signal
  check(f.liveHas("当前用户(来自 context):小明"), "/         on:blur 写回 context → 各处显示更新为 小明");
}

// 23) 事件系统:完整事件数据(rui::event() 取键盘/修饰键/文件)+ on:keydown.escape
{
  // 23a /forms 文件输入:on:change 读 event().files(name/size/type)
  const f = await fresh();
  f.render("/forms");
  f.fireEvent(6, { files: [{ name: "report.csv", size: 2048, type: "text/csv" }, { name: "pic.png", size: 99, type: "image/png" }] });
  check(f.liveHas("report.csv(2048B)") && f.liveHas("pic.png(99B)"), "/forms 文件输入 → event().files(name/size 解码)");
  // 23b /forms 按键探测:event().key + 修饰键
  f.fireEvent(7, { key: "k", ctrl: true });
  check(f.liveHas("最近按键:Ctrl+k"), "/forms event() → key + ctrl 修饰键");
  f.fireEvent(7, { key: "Enter" });
  check(f.liveHas("最近按键:Enter"), "/forms event().key=Enter(完整键盘事件,过去只能拿 value)");
}
{
  // 注:修饰符的「按键过滤 / prevent / stop / capture / self」是 router.js add_event 的逻辑(浏览器路径);
  // 本无头 harness 的 add_event 是 no-op、fire 直触 handler,故只覆盖 handler 效果 + 事件数据解码,
  // 不覆盖过滤本身(过滤逻辑由 review 读码确认 + 浏览器实测)。
  // 23c AddForm:Esc 清空草稿(on:keydown.escape;此处直触 handler 验证清空效果)
  const f = await fresh();
  f.render("/");
  f.fire(2, "未提交的草稿"); // 输入(bind:value,6 字)
  check(f.has("6/80"), "/         输入草稿 → 字数 6/80");
  f.fire(3); // on:keydown.escape 的 handler → 清空草稿
  check(f.has("0/80") && !f.liveHas("6/80"), "/         Esc(on:keydown.escape)→ 清空草稿(字数回 0/80)");
  // 分隔控制符剥除:value "x\x1fy" 应被剥成 "xy"(2 字),而非截断成 "x"(1 字)
  f.fireEvent(2, { value: "x\x1fy" });
  check(f.has("2/80") && !f.liveHas("1/80"), "/         输入含分隔控制符 → 被剥除不截断(x\\x1fy→xy,2 字)");
}

// 24) 可选/默认 props(typed builder)+ panic hook init
{
  const f = await fresh();
  f.render("/");
  f.onFetch(TODOS);
  check(f.liveHas("(可选 prop:其它页的 Panel 省略它)"), "/         Panel 提供可选 subtitle → 渲染(其它页省略它仍编译+渲染 = 可选 prop 生效)");
  // panic hook:init 导出存在且可调(装 hook;真 panic 日志由浏览器侧验证)
  check(typeof f.X.init === "function", "         client! 导出 init(panic hook 安装入口)");
  f.X.init();
  check(true, "         调用 init() 安装 panic hook 不报错");
}

console.log(ok ? "✅ 全部通过" : "❌ 有失败");
process.exit(ok ? 0 : 1);

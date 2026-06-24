// 无浏览器验证(全 view! 版):路由渲染 + 表格 fetch/响应式 + 计数器响应式。
const enc = new TextEncoder();
const dec = new TextDecoder();

async function fresh() {
  const nodes = [null]; // {tag, text, children}
  let fetchReq = null;
  let memory;
  const str = (p, l) => dec.decode(new Uint8Array(memory.buffer, p, l));
  const reg = (tag, text = "") => (nodes.push({ tag, text, children: [] }), nodes.length - 1);
  const env = {
    create_element: (p, l) => reg(str(p, l)),
    create_text: (p, l) => reg("#text", str(p, l)),
    claim_element: () => 0,
    set_text: (id, p, l) => { nodes[id].text = str(p, l); },
    append_child: (par, ch) => nodes[par].children.push(ch),
    set_attr: () => {},
    add_event: () => {},
    clear_children: (id) => { nodes[id].children = []; },
    gql_query: (qp, ql, h) => { fetchReq = { query: str(qp, ql), handler: h }; },
    gql_subscribe: (qp, ql, h) => { fetchReq = { query: str(qp, ql), handler: h }; },
    mount: () => {},
  };
  const bytes = await Bun.file(new URL("./web/app.wasm", import.meta.url)).arrayBuffer();
  const { instance } = await WebAssembly.instantiate(bytes, { env });
  const X = instance.exports;
  memory = X.memory;
  const write = (s) => { const b = enc.encode(s); const ptr = X.alloc(b.length); new Uint8Array(memory.buffer, ptr, b.length).set(b); return [ptr, b.length]; };
  const render = (path) => { const [p, l] = write(path); X.render_route(p, l); };
  const onFetch = (json) => { const [p, l] = write(json); X.on_fetch(fetchReq.handler, p, l); };
  const texts = () => nodes.filter(Boolean).map((n) => n.text).filter(Boolean);
  const deepText = (id) => { const n = nodes[id]; if (n.tag === "#text") return n.text; return n.children.length ? deepText(n.children[0]) : ""; };
  const tbodyRows = () => { const tb = nodes.findIndex((n) => n && n.tag === "tbody"); return nodes[tb].children.filter((c) => nodes[c].tag === "tr"); };
  return { nodes, X, render, onFetch, texts, deepText, tbodyRows, get fetchReq() { return fetchReq; } };
}

let ok = true;
const check = (c, m) => { console.log((c ? "✓ " : "✗ ") + m); ok = ok && c; };

// 1) 各路由渲染对应标题
for (const [path, title] of [
  ["/", "欢迎来到 rui"],
  ["/counter", "计数器"],
  ["/about", "关于"],
  ["/macro", "view! 宏"],
  ["/stock/AAPL", "AAPL"],
  ["/nope", "404 · 页面不存在"],
]) {
  const f = await fresh();
  f.render(path);
  check(f.texts().some((t) => t.includes(title)), `${path.padEnd(12)} 渲染含 "${title}"`);
}

// 2) <StatCard/> 组件
{
  const f = await fresh();
  f.render("/");
  check(f.texts().some((t) => t.includes("共享 struct")), "/            <StatCard/> 组件渲染");
}

// 3) 表格:fetch + 响应式 <For> + 倒序
{
  const f = await fresh();
  f.render("/table");
  check(f.fetchReq && f.fetchReq.query.includes("stocks {"), `/table       发起 GraphQL query: ${f.fetchReq && f.fetchReq.query}`);
  check(f.tbodyRows().length === 0, "/table       初始 0 行(待 fetch)");
  f.onFetch('{"data":{"stocks":[{"__typename":"Stock","__id":"X1","symbol":"X1","name":"One","price":10.0,"change":1.0},{"__typename":"Stock","__id":"X2","symbol":"X2","name":"Two","price":20.0,"change":-2.0},{"__typename":"Stock","__id":"X3","symbol":"X3","name":"Three","price":30.0,"change":3.0}]}}');
  check(f.tbodyRows().length === 3, `/table       fetch 后 3 行,实际 ${f.tbodyRows().length}`);
  check(f.deepText(f.tbodyRows()[0]) === "X1", "/table       首行 = X1");
  f.X.dispatch(0); // 倒序(首个 on:click)
  check(f.deepText(f.tbodyRows()[0]) === "X3", "/table       倒序后首行 = X3");
}

// 3b) 规范化缓存:同 entity 再次写入(新价)→ memo 视图自动重算
{
  const f = await fresh();
  f.render("/table");
  f.onFetch('{"data":{"stocks":[{"__typename":"Stock","__id":"AAPL","symbol":"AAPL","name":"Apple","price":1.0,"change":0.0}]}}');
  check(f.texts().some((t) => t.includes("$1")), "/table       缓存初值 price=1");
  f.onFetch('{"data":{"stocks":[{"__typename":"Stock","__id":"AAPL","symbol":"AAPL","name":"Apple","price":999.0,"change":0.0}]}}');
  check(f.texts().some((t) => t.includes("$999")), "/table       同 entity 更新后视图自动重算 → $999(规范化缓存)");
}

// 4) subscription:每次推送都更新(同一 handler 反复调用)
{
  const f = await fresh();
  f.render("/live");
  check(f.fetchReq && f.fetchReq.query.includes("subscription"), `/live        发起 subscription: ${f.fetchReq && f.fetchReq.query}`);
  f.onFetch('{"data":{"price_updates":[{"__typename":"Stock","__id":"X","symbol":"X","name":"n","price":10.0,"change":0.0}]}}');
  check(f.tbodyRows().length === 1 && f.deepText(f.tbodyRows()[0]) === "X", "/live        首推后 1 行 X");
  f.onFetch('{"data":{"price_updates":[{"__typename":"Stock","__id":"X","symbol":"X","name":"n","price":11.0,"change":0.0},{"__typename":"Stock","__id":"Y","symbol":"Y","name":"m","price":20.0,"change":0.0}]}}');
  check(f.tbodyRows().length === 2, "/live        二次推送后 2 行(订阅反复更新)");
}

// 5) 计数器:响应式文本
{
  const f = await fresh();
  f.render("/counter");
  let id = f.nodes.findIndex((n) => n && n.tag === "#text" && n.text === "0");
  check(id > 0, "/counter     初始计数 0");
  f.X.dispatch(1); // +1(-1 是 0,+1 是 1)
  check(f.nodes[id].text === "1", `/counter     +1 后 = 1,实际 ${f.nodes[id].text}`);
}

// 6) 分页 connection:首屏取第一页 + load_next 累积追加(Relay 游标分页)
{
  const f = await fresh();
  f.render("/feed");
  check(f.fetchReq && f.fetchReq.query.includes("stock_page(first: 3"), `/feed        发起分页查询: ${f.fetchReq && f.fetchReq.query}`);
  const p1 = '{"data":{"stock_page":[{"__typename":"StockConnection","__id":null,"edges":[' +
    '{"__typename":"StockEdge","__id":null,"node":{"__typename":"Stock","__id":"X1","symbol":"X1","name":"One","price":1.0},"cursor":"X1"},' +
    '{"__typename":"StockEdge","__id":null,"node":{"__typename":"Stock","__id":"X2","symbol":"X2","name":"Two","price":2.0},"cursor":"X2"}' +
    '],"page_info":{"has_next_page":true,"end_cursor":"X2"}}]}}';
  f.onFetch(p1);
  check(f.tbodyRows().length === 2, `/feed        首页 2 行,实际 ${f.tbodyRows().length}`);
  f.X.dispatch(0); // 点「加载更多」→ load_next(用游标 X2 取下一页)
  check(f.fetchReq.query.includes('after: "X2"'), `/feed        load_next 用游标 X2: ${f.fetchReq.query}`);
  const p2 = '{"data":{"stock_page":[{"__typename":"StockConnection","__id":null,"edges":[' +
    '{"__typename":"StockEdge","__id":null,"node":{"__typename":"Stock","__id":"X3","symbol":"X3","name":"Three","price":3.0},"cursor":"X3"}' +
    '],"page_info":{"has_next_page":false,"end_cursor":"X3"}}]}}';
  f.onFetch(p2);
  check(f.tbodyRows().length === 3, `/feed        追加后 3 行(累积),实际 ${f.tbodyRows().length}`);
  check(f.deepText(f.tbodyRows()[0]) === "X1" && f.deepText(f.tbodyRows()[2]) === "X3", "/feed        累积顺序 X1→X3(append 不替换)");
}

// 7) 乐观更新:点 mutation 按钮 → 视图立即 $200(未等网络)→ 响应回来用真值 $555 覆盖
{
  const f = await fresh();
  f.render("/table");
  f.onFetch('{"data":{"stocks":[{"__typename":"Stock","__id":"AAPL","symbol":"AAPL","name":"Apple","price":1.0,"change":0.0}]}}');
  check(f.texts().some((t) => t.includes("$1")), "/table       乐观前 AAPL = $1");
  f.X.dispatch(2); // 第三个 on:click(0 倒序 / 1 删一行 / 2 = AAPL→$200 mutation)
  check(f.texts().some((t) => t.includes("$200")), "/table       乐观:点击后立即 $200(未等网络响应)");
  f.onFetch('{"data":{"set_price":[{"__typename":"Stock","__id":"AAPL","symbol":"AAPL","name":"Apple","price":555.0,"change":0.0}]}}');
  check(f.texts().some((t) => t.includes("$555")), "/table       响应回来用真值 $555 覆盖乐观值");
}

// 8) 片段 fragment + data masking:...StockCard 把片段字段(symbol name price)内联进查询 + 组件渲染
{
  const f = await fresh();
  f.render("/cards");
  check(f.fetchReq && f.fetchReq.query.includes("symbol name price"), `/cards       ...StockCard 内联片段字段: ${f.fetchReq && f.fetchReq.query}`);
  f.onFetch('{"data":{"stocks":[{"__typename":"Stock","__id":"AAPL","symbol":"AAPL","name":"Apple","price":7.0,"change":0.0}]}}');
  check(
    f.texts().some((t) => t.includes("AAPL")) && f.texts().some((t) => t.includes("Apple")) && f.texts().some((t) => t.includes("$7")),
    "/cards       片段组件渲染(symbol/name/price)"
  );
}

console.log(ok ? "✅ 全部通过" : "❌ 有失败");
process.exit(ok ? 0 : 1);

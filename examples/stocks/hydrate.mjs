// 真水合验证(两趟法):Pass A(CSR)模拟 SSR 建树并按 create 顺序记录(= hid 序);
// Pass B(hydrate)set_hydrate(1) 渲染,claim_* 返回 Pass A 的节点;断言水合期零 create + 反应式命中认领节点。
const enc = new TextEncoder();
const dec = new TextDecoder();

async function loadWasm(env) {
  const bytes = await Bun.file(new URL("./web/app.wasm", import.meta.url)).arrayBuffer();
  const { instance } = await WebAssembly.instantiate(bytes, { env });
  return instance.exports;
}

async function run(path, responses, claimFrom) {
  const nodes = [null];
  const created = [];
  let mem, lastQuery = null, creates = 0;
  const str = (p, l) => dec.decode(new Uint8Array(mem.buffer, p, l));
  const reg = (o) => (nodes.push(o), nodes.length - 1);
  const mk = (o) => (created.push(o), creates++, reg(o));
  const detach = (id) => { for (const n of nodes) if (n && n.children) { const i = n.children.indexOf(id); if (i >= 0) n.children.splice(i, 1); } };
  const env = {
    create_element: (p, l) => mk({ tag: str(p, l), attrs: {}, children: [], text: null }),
    create_text: (p, l) => mk({ tag: "#text", text: str(p, l), children: [] }),
    claim_element: (hid) => reg(claimFrom[hid]),
    claim_text: (hid) => reg(claimFrom[hid]),
    set_text: (id, p, l) => { nodes[id].text = str(p, l); },
    append_child: (par, ch) => { detach(ch); nodes[par].children.push(ch); },
    remove_child: (par, ch) => detach(ch),
    set_attr: () => {},
    set_value: () => {},
    set_checked: () => {},
    console_error: () => {},
    add_class: () => {},
    remove_class: () => {},
    set_timeout: () => {},
    add_event: () => {},
    clear_children: (id) => { if (nodes[id]) nodes[id].children = []; },
    gql_query: (qp, ql) => { lastQuery = str(qp, ql); },
    gql_subscribe: (qp, ql) => { lastQuery = str(qp, ql); },
    mount: () => {},
    clear_app: () => {},
    push_url: () => {},
    focus: () => {},
    scroll_into_view: () => {},
    set_interval: () => 0,
    clear_interval: () => {},
    run_js: () => {},
    run_js_on: () => {},
    eval_js: () => {},
  };
  const X = await loadWasm(env);
  mem = X.memory;
  const w = (s) => { const b = enc.encode(s); const p = X.alloc(b.length); new Uint8Array(mem.buffer, p, b.length).set(b); return [p, b.length]; };
  if (responses) { const [p, l] = w(JSON.stringify(responses)); X.hydrate_data(p, l); }
  if (claimFrom) X.set_hydrate(1);
  creates = 0;
  { const [p, l] = w(path); X.render_route(p, l); }
  const rendered = creates;
  if (claimFrom) X.set_hydrate(0);
  return { X, w, nodes, created, creates: rendered, lastQuery, fire: (id, v = "") => { const [p, l] = w(v); X.dispatch(id, p, l); } };
}

let ok = true;
const check = (c, m) => { console.log((c ? "✓ " : "✗ ") + m); ok = ok && c; };

// 1) /about(static 页,纯文本/元素,无数据)—— 文本节点认领是水合最难点
try {
  const A = await run("/about", null, null);
  const B = await run("/about", null, A.created);
  check(B.creates === 0, `/about   水合期零 create(全部认领,含文本节点),实际 ${B.creates}`);
  check(A.created.some((n) => n.text && n.text.includes("关于")), "/about   SSR 树含文本「关于」(被认领)");
} catch (e) { check(false, "/about   水合抛错: " + e.message); }

// 2) /(首页,subscription 数据 + 组件 + keyed For)—— 数据行也认领
try {
  const TODOS = '{"data":{"todo_updates":[' +
    '{"__typename":"Todo","__id":"a","id":"a","text":"写代码","done":false},' +
    '{"__typename":"Todo","__id":"b","id":"b","text":"喝咖啡","done":true}]}}';
  const probe = await run("/", null, null);   // 拿订阅查询串
  const resp = { [probe.lastQuery]: TODOS };
  const A = await run("/", resp, null);        // 模拟 SSR(带数据,渲出列表)
  const B = await run("/", resp, A.created);    // 水合
  check(A.created.some((n) => n.text === "写代码"), "/        SSR 树含订阅数据行「写代码」");
  check(B.creates === 0, `/        水合期零 create(订阅列表/组件/keyed For 全认领),实际 ${B.creates}`);
} catch (e) { check(false, "/        水合抛错: " + e.message); }

// 3) /todo/1(路由参数页:param(1) + resource!)—— 首屏按 param SSR + 水合零 create
try {
  const DETAIL = '{"data":{"detail":[{"__typename":"Todo","__id":"1","id":"1","text":"第一条待办","done":true}]}}';
  const probe = await run("/todo/1", null, null);          // 拿 detail(id:"1") 查询串
  check(probe.lastQuery && probe.lastQuery.includes('detail(id: "1")'), "/todo/1  首屏按 param 发起 detail(id=1)");
  const resp = { [probe.lastQuery]: DETAIL };
  const A = await run("/todo/1", resp, null);               // 模拟 SSR(带数据)
  const B = await run("/todo/1", resp, A.created);          // 水合
  check(A.created.some((n) => n.text === "第一条待办"), "/todo/1  SSR 树含详情数据「第一条待办」");
  check(B.creates === 0, `/todo/1  水合期零 create(param 页全认领),实际 ${B.creates}`);
} catch (e) { check(false, "/todo/1  水合抛错: " + e.message); }

// 4) /dash(嵌套路由组:shell > dash_shell > reactive outlet > overview)—— 嵌套也水合零 create
try {
  const TODOS = '{"data":{"todos":[{"__typename":"Todo","__id":"a","id":"a","done":true}]}}';
  const probe = await run("/dash", null, null);
  const resp = { [probe.lastQuery]: TODOS };
  const A = await run("/dash", resp, null);
  const B = await run("/dash", resp, A.created);
  check(A.created.some((n) => n.text && n.text.includes("共享左侧")), "/dash    SSR 树含组内容(overview)");
  check(B.creates === 0, `/dash    水合期零 create(shell>dash_shell>outlet 全认领),实际 ${B.creates}`);
} catch (e) { check(false, "/dash    水合抛错: " + e.message); }

console.log(ok ? "✅ 水合全部通过" : "❌ 水合有失败");
process.exit(ok ? 0 : 1);

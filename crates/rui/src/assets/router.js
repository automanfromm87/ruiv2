// 文件路由的客户端 glue:按 location.pathname 渲染对应页,拦截 <a> 做 SPA 导航。
let wasm, mem;
const nodes = [null];
const hidx = {}; // hid -> SSR 节点(水合认领用)
const dec = new TextDecoder();
const encoder = new TextEncoder();
const str = (p, l) => dec.decode(new Uint8Array(mem.buffer, p, l));
const reg = (node) => (nodes.push(node), nodes.length - 1);

const env = {
  create_element: (p, l) => reg(document.createElement(str(p, l))),
  create_text: (p, l) => reg(document.createTextNode(str(p, l))),
  claim_element: (hid) => reg(hidx[hid]), // 认领 SSR 元素(按 data-h)
  claim_text: (hid) => reg(hidx[hid]), // 认领 SSR 文本节点(按 <!--h:N--> 标记)
  set_text: (id, p, l) => { nodes[id].textContent = str(p, l); },
  append_child: (par, ch) => nodes[par].appendChild(nodes[ch]), // appendChild 移动已在 DOM 的节点(重排保焦点)
  remove_child: (par, ch) => { const p = nodes[par], c = nodes[ch]; if (c && c.parentNode === p) p.removeChild(c); },
  set_attr: (id, np, nl, vp, vl) => nodes[id].setAttribute(str(np, nl), str(vp, vl)),
  set_value: (id, p, l) => { nodes[id].value = str(p, l); }, // 受控输入:写 .value property
  // 事件:把 target.value(无则空串)随 dispatch 传回 wasm;表单 submit 默认 preventDefault。
  add_event: (id, ep, el, h) => {
    const ev = str(ep, el);
    nodes[id].addEventListener(ev, (e) => {
      if (ev === "submit") e.preventDefault();
      const v = e.target && e.target.value != null ? String(e.target.value) : "";
      const b = encoder.encode(v);
      const ptr = wasm.alloc(b.length);
      new Uint8Array(mem.buffer, ptr, b.length).set(b);
      wasm.dispatch(h, ptr, b.length);
    });
  },
  clear_children: (id) => { const e = nodes[id]; while (e.firstChild) e.removeChild(e.firstChild); },
  // GraphQL query/mutation:POST /graphql,响应回 on_fetch。网络失败 → 注入 errors 让 UI 进 error 态(否则 loading 永真)。
  gql_query: (qPtr, qLen, handler) => {
    const query = str(qPtr, qLen);
    fetch("/graphql", { method: "POST", body: query })
      // 非 2xx(500/502/404 等,体可能是 HTML/纯文本)→ 合成 errors,别当成功响应(否则会把垃圾当数据)。
      .then((r) => (r.ok ? r.text() : r.text().then((t) => JSON.stringify({ errors: [{ message: "HTTP " + r.status + ":" + t.slice(0, 200) }] }))))
      .then((text) => deliver(handler, text))
      .catch((e) => deliver(handler, JSON.stringify({ errors: [{ message: "网络请求失败:" + e }] })));
  },
  // GraphQL subscription:开 SSE,每次推送都回 on_fetch(同一 handler 反复调用)。
  gql_subscribe: (qPtr, qLen, handler) => {
    const q = str(qPtr, qLen);
    const es = new EventSource("/graphql/subscribe?q=" + encodeURIComponent(q));
    es.onmessage = (e) => deliver(handler, e.data);
    // 连接彻底断开(非瞬时重连)才报错,避免 EventSource 自动重连的瞬时 error 刷屏。
    es.onerror = () => { if (es.readyState === EventSource.CLOSED) deliver(handler, JSON.stringify({ errors: [{ message: "订阅连接中断" }] })); };
  },
  mount: (id) => document.getElementById("app").appendChild(nodes[id]),
  clear_app: () => { const a = document.getElementById("app"); while (a.firstChild) a.removeChild(a.firstChild); }, // SPA 换页清容器
  push_url: (p, l) => history.pushState({}, "", str(p, l)), // 程序化导航:更新地址栏 + 历史
  // 命令式 DOM(on_mount 用):聚焦 / 滚动。容错(节点可能无该方法)。
  focus: (id) => { const n = nodes[id]; if (n && n.focus) n.focus(); },
  scroll_into_view: (id) => { const n = nodes[id]; if (n && n.scrollIntoView) n.scrollIntoView(); },
  // 定时器:每 ms 回调进 wasm 的 run_interval(handler);返回 timer id 供 clear_interval(on_cleanup)。
  set_interval: (ms, handler) => setInterval(() => wasm.run_interval(handler), ms),
  clear_interval: (timer) => clearInterval(timer),
  // JS 逃生舱:直接执行任意 JS / 浏览器 API(无 wasm-bindgen 时的通用出口)。
  run_js: (p, l) => { try { (0, eval)(str(p, l)); } catch (e) { console.error("run_js:", e); } }, // 间接 eval = 全局作用域
  run_js_on: (id, p, l) => { const el = nodes[id]; try { eval(str(p, l)); } catch (e) { console.error("run_js_on:", e); } }, // 直接 eval:code 里可用 el
  eval_js: (p, l, h) => { // 取返回值(同步值 / Promise 都行)→ 回调一次。首字节 \x00=ok / \x01=err(带外错误通道)
    const ok = (r) => deliver(h, "\x00" + (r == null ? "" : String(r)));
    const err = (e) => deliver(h, "\x01" + e);
    try { Promise.resolve((0, eval)(str(p, l))).then(ok).catch(err); }
    catch (e) { err(e); }
  },
};

const bytes = await fetch("/app.wasm").then((r) => r.arrayBuffer());
const { instance } = await WebAssembly.instantiate(bytes, { env });
wasm = instance.exports;
mem = wasm.memory;

// 把响应文本写进 wasm 内存,回调 on_fetch(handler)
function deliver(handler, text) {
  const b = encoder.encode(text);
  const ptr = wasm.alloc(b.length);
  new Uint8Array(mem.buffer, ptr, b.length).set(b);
  wasm.on_fetch(handler, ptr, b.length);
}

// 首屏 CSR(空 #app 从零渲染):alloc + 写内存 + 调 render_route(全量)。
function renderPath(path) {
  document.getElementById("app").innerHTML = ""; // 清掉上一页
  const b = encoder.encode(path);
  const ptr = wasm.alloc(b.length);
  new Uint8Array(mem.buffer, ptr, b.length).set(b);
  wasm.render_route(ptr, b.length);
  console.log("route →", path);
}

// SPA 导航:交给 wasm.navigate —— 同页换参数只更新路径 signal(不重建,resource! 重取);换页才清空重渲。
function navigate(path) {
  const b = encoder.encode(path);
  const ptr = wasm.alloc(b.length);
  new Uint8Array(mem.buffer, ptr, b.length).set(b);
  wasm.navigate(ptr, b.length);
  console.log("nav →", path);
}

// SPA:拦截内部 <a>(以 / 开头、非 .html),pushState 后导航。传完整 URL(含 ?query)。
document.addEventListener("click", (e) => {
  const a = e.target.closest("a");
  if (!a) return;
  const href = a.getAttribute("href") || "";
  if (href.startsWith("/") && !href.endsWith(".html")) {
    e.preventDefault();
    if (location.pathname + location.search !== href) history.pushState({}, "", href);
    navigate(location.pathname + location.search);
  }
});
window.addEventListener("popstate", () => navigate(location.pathname + location.search));

function writeStr(s) {
  const b = encoder.encode(s);
  const ptr = wasm.alloc(b.length);
  new Uint8Array(mem.buffer, ptr, b.length).set(b);
  return [ptr, b.length];
}

// 首屏:把 SSR 注入的 query 响应灌进客户端缓存(query! 命中即免重新联网)。
const dataEl = document.getElementById("__rui_data");
if (dataEl && dataEl.textContent) {
  const [p, l] = writeStr(dataEl.textContent);
  wasm.hydrate_data(p, l);
}

// 建 hid→节点索引:元素按 data-h;文本按 <!--h:N--> 注释标记的下一个文本节点(空文本则补一个)。
function buildHydrateIndex() {
  const app = document.getElementById("app");
  app.querySelectorAll("[data-h]").forEach((el) => { hidx[+el.getAttribute("data-h")] = el; });
  const it = document.createNodeIterator(app, NodeFilter.SHOW_COMMENT);
  let c;
  while ((c = it.nextNode())) {
    const m = /^h:(\d+)$/.exec(c.data);
    if (!m) continue;
    let t = c.nextSibling;
    if (!t || t.nodeType !== 3) { t = document.createTextNode(""); c.parentNode.insertBefore(t, c.nextSibling); }
    hidx[+m[1]] = t;
  }
}

// 首屏:有 SSR 内容(#app 下存在 data-h 节点)→ 水合(认领、不重建);
// 否则(csr 页:服务端只发了空壳)→ 纯 CSR。之后 SPA 导航一律 CSR。
if (document.querySelector("#app [data-h]")) {
  buildHydrateIndex();
  wasm.set_hydrate(1);
  { const [p, l] = writeStr(location.pathname + location.search); wasm.render_route(p, l); }
  wasm.set_hydrate(0);
  console.log("hydrated →", location.pathname + location.search);
} else {
  renderPath(location.pathname + location.search); // 纯 CSR(从零渲染进空 #app)
}

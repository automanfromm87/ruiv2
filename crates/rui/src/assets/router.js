// 文件路由的客户端 glue:按 location.pathname 渲染对应页,拦截 <a> 做 SPA 导航。
let wasm, mem;
const nodes = [null];
const dec = new TextDecoder();
const encoder = new TextEncoder();
const str = (p, l) => dec.decode(new Uint8Array(mem.buffer, p, l));
const reg = (node) => (nodes.push(node), nodes.length - 1);

const env = {
  create_element: (p, l) => reg(document.createElement(str(p, l))),
  create_text: (p, l) => reg(document.createTextNode(str(p, l))),
  claim_element: () => 0,
  set_text: (id, p, l) => { nodes[id].textContent = str(p, l); },
  append_child: (par, ch) => nodes[par].appendChild(nodes[ch]),
  set_attr: (id, np, nl, vp, vl) => nodes[id].setAttribute(str(np, nl), str(vp, vl)),
  add_event: (id, ep, el, h) => nodes[id].addEventListener(str(ep, el), () => wasm.dispatch(h)),
  clear_children: (id) => { const e = nodes[id]; while (e.firstChild) e.removeChild(e.firstChild); },
  // GraphQL query/mutation:POST /graphql,响应回 on_fetch
  gql_query: (qPtr, qLen, handler) => {
    const query = str(qPtr, qLen);
    fetch("/graphql", { method: "POST", body: query })
      .then((r) => r.text())
      .then((text) => deliver(handler, text));
  },
  // GraphQL subscription:开 SSE,每次推送都回 on_fetch(同一 handler 反复调用)
  gql_subscribe: (qPtr, qLen, handler) => {
    const q = str(qPtr, qLen);
    const es = new EventSource("/graphql/subscribe?q=" + encodeURIComponent(q));
    es.onmessage = (e) => deliver(handler, e.data);
  },
  mount: (id) => document.getElementById("app").appendChild(nodes[id]),
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

// 把路径字符串传进 wasm(JS → wasm:alloc + 写内存 + 调 render_route)
function renderPath(path) {
  document.getElementById("app").innerHTML = ""; // 清掉上一页
  const b = encoder.encode(path);
  const ptr = wasm.alloc(b.length);
  new Uint8Array(mem.buffer, ptr, b.length).set(b);
  wasm.render_route(ptr, b.length);
  console.log("route →", path);
}

// SPA:拦截内部 <a>(以 / 开头、非 .html),pushState 后重渲染
document.addEventListener("click", (e) => {
  const a = e.target.closest("a");
  if (!a) return;
  const href = a.getAttribute("href") || "";
  if (href.startsWith("/") && !href.endsWith(".html")) {
    e.preventDefault();
    if (location.pathname !== href) history.pushState({}, "", href);
    renderPath(location.pathname);
  }
});
window.addEventListener("popstate", () => renderPath(location.pathname));

renderPath(location.pathname);

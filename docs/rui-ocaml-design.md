# Rui-ML — `rui` 框架的 OCaml 同构重写设计

> Rui (OCaml package `rui`, top-level module `Rui`; ppx package `rui.ppx` / driver `ppx_rui`)

> 本文档由对真实 rui(Rust)源码逐子系统翻译而成,覆盖现有全部能力。代码为设计草图(OCaml + js_of_ocaml + Brr + ppxlib)。

## 目录

1. 设计原则与 Rust→OCaml 取舍
2. 目标技术栈
3. 模块与工程布局
4. Rust→OCaml 能力映射总表
5. 命名约定
6. Reactive Core
7. View & Rendering Engine
8. The DSL: jsx-ppx, page, component, router macros
9. DOM Abstraction, SSR & Hydration
10. GraphQL Data Layer
11. Routing: router!, params, nested groups, strategies
12. Lifecycle, Refs & JS Interop
13. Build, Tooling, Project Layout & Testing
14. 完整能力清单与覆盖(feature parity)
15. 风险、非目标与路线图

## Rui — an isomorphic, fine-grained-reactive, no-VDOM full-stack web framework for OCaml

Rui is a from-scratch OCaml port of the Rust `rui` framework. It keeps every capability of the original and re-expresses each one in idiomatic OCaml, leaning on the things OCaml does *better* than Rust for this domain (a GC instead of `Rc<RefCell>`/ownership ceremony; `js_of_ocaml` so the DOM and JS are first-class with zero FFI marshalling; `ppxlib` instead of `proc-macro`; rows / GADTs / first-class modules instead of trait projection).

**What it is.** A single OCaml codebase compiles two ways: native (SSR server, GraphQL execution, normalized-store pre-fetch) and `js_of_ocaml` (the reactive client). Applications write only: shared models (`[@@deriving gql]`), a backend schema (methods-as-schema), pages/components (`%view` / `[@page]`), and a route table. The framework supplies a fine-grained reactive core (signal / effect / memo with value-dedup), a direct-to-DOM renderer with **no virtual DOM and no diffing of static structure**, real SSR with **true hydration** (claim existing DOM nodes by `data-h` for elements and `<!--h:N-->` comment markers for text — never re-create, never `innerHTML=""`), SSR→client **data handoff** (dehydrate/rehydrate the GraphQL responses so the first paint does not re-fetch), per-page **render strategies** (`ssr` / `csr` / `static`), a router with **nested route groups + reactive outlet** (sibling navigation inside a group does not rebuild the layout), **path params as typed reactive signals** + **query params as a separate signal line**, and a **compile-time-checked GraphQL data layer** (`%query` / `%mutation` / `%subscription` / `%resource` / `%paginated` / `%fragment`) backed by a Relay-style normalized cache.

**Why it can be cleaner than the Rust original.**
- **GC erases an entire class of plumbing.** The Rust version is full of `Rc<RefCell<…>>`, manual `Scope` drop-order, a `try_with` guard against "cannot access TLS during destruction" on the per-connection-thread SSR, `take_parts`/`absorb_parts`, and explicit `dispose`. In OCaml the reactive graph is plain heap, disposal is *explicit but allocation-free* (we still need deterministic effect teardown for correctness, not for memory), and the TLS-destruction hazard simply does not exist.
- **`js_of_ocaml` deletes the FFI.** The Rust client is wasm with a hand-written two-way FFI (`extern "C"` imports + `#[no_mangle]` exports), a `nodes[]` registry, `u32` node handles, `alloc`/pointer/length marshalling, and a `router.js` glue file. In OCaml the client *is* JS: DOM nodes are real `Brr.El.t`/`Jv.t` values held directly in OCaml closures. No `u32` handles, no registry, no `alloc`, no glue file (the bootstrap is a tiny `Rui_client.start`). The "`u32` collides with `Display`" problem that forced the `IntoView` redesign never arises.
- **ppxlib gives real hygiene + error spans.** `%view` is a proper expression ppx producing typed OCaml; `[@page]`/`[@component]` are attribute ppxs. No `module_path!()`-as-key hack (we can mint a stable page key from the structure-item location/name).

**The one honest wash.** The `move ||` reactive boundary is fundamental to fine-grained reactivity and does not disappear: in OCaml a dynamic text/attribute/conditional is still `(fun () -> …)`. We choose the same trade-off the Rust version reasoned through (fine-grained + explicit thunks, not VDOM + zero-ceremony `if`), because VDOM would mean throwing away the whole fine-grained core, writing a reconciler, and regressing performance.

## Design principles + the concrete Rust→OCaml shifts

### P1. Fine-grained reactivity, no VDOM (kept identical in spirit)
Signal / effect / memo form a dynamic dependency graph; the DOM is mutated **in place** (`set_text`, `set_attr`), and structure is built once. Conditionals/lists re-run a thunk inside an `effect`, rebuilding only the affected sub-tree. This is the core and is preserved exactly.
- **Why a wash, not cleaner:** the `(fun () -> …)` thunk at every dynamic boundary is intrinsic. We do not pretend OCaml removes it. We *do* remove the secondary annoyances (see P5).

### P2. GC instead of ownership ceremony — but keep deterministic disposal
The Rust core needs `Rc<RefCell>` for shared mutable reactive nodes, dynamic-dependency cleanup (an effect drops itself from last run's signals before re-running), explicit `dispose`, owner stacks, cleanup stacks, and `Scope` whose `Drop` runs `on_cleanup` then disposes effects.
- **OCaml shift:** nodes are plain records on the GC heap; `Signal` is `{ mutable v; mutable subs }`. We **still** keep `Scope.dispose` and `on_cleanup` — not for memory, but because effects have *observable* side-effects (DOM listeners, `setInterval`, SSE subscriptions, normalized-store subscriptions) that must be torn down deterministically when a page/sub-tree goes away. The dynamic-dependency cleanup (an effect un-subscribing from stale signals before re-running) is **mandatory and ported verbatim** — it is a correctness property, not a memory optimization.
- **Cleaner:** the Rust `dispose_effect` `try_with` guard ("cannot access TLS during/after destruction" → process abort on per-connection-thread SSR) is **deleted**. OCaml SSR is single-runtime; there is no thread-local destructor race. The Rust `take_parts`/`absorb_parts` dance (re-parenting `on_mount`-created effects into the page scope) collapses to "open the page scope, run the mount callback inside it" because there is no borrow checker forbidding nested mutable access.

### P3. `js_of_ocaml` + Brr instead of wasm + hand-rolled FFI
The Rust client is a wasm module talking to a `router.js` env: `create_element`, `claim_element(hid)`, `append_child`, `add_event(node, ev, handler_id)`, `gql_query(ptr,len,handler_id)`, an integer `nodes[]` registry, `alloc`, and `dispatch(id, ptr, len)` to route events back by integer id.
- **OCaml shift:** the client compiles to JS via `js_of_ocaml`. DOM nodes are `Brr.El.t` held directly. Event handlers are OCaml closures attached with `Brr.Ev.listen` — **no handler-id registry, no `dispatch`, no payload marshalling** (the closure reads `Brr.Ev.target`/`.value` directly). `fetch` is `Brr_io.Fetch`; SSE is `Brr_io.Sse`/`EventSource`. The whole `assets/router.js` glue file disappears; what remains is `Rui_client.start route` which does data-rehydrate + hydrate-vs-CSR probe + nav interception, all in OCaml.
- **Cleaner:** every "`alloc` + write bytes + call export" trampoline (there were ~8 of them: `render_route`, `navigate`, `dispatch`, `on_fetch`, `hydrate_data`, `set_hydrate`, `run_interval`) becomes a direct OCaml call. Node handles are values, not `u32` indices, so the `From<View> for u32` / `impl Into<u32>` bridging and the "`u32` collides with `Display` in `view!` text dispatch" problem evaporate.

### P4. Tri-backend stays, but the split is a functor/dune-variant boundary
Rust used `#[cfg(target_arch = "wasm32")]` to pick the browser backend (create / hydrate) vs native string backend (SSR serialize). Hydrate vs create is a runtime flag (`HYDRATE` thread-local) on the client backend.
- **OCaml shift:** the DOM backend is a module signature `Rui_dom.S` with two implementations selected by dune: `rui.dom.client` (Brr, with a runtime `hydrating` ref toggling create-vs-claim, exactly mirroring the Rust flag) and `rui.dom.ssr` (native arena → HTML string). Application/view code depends only on `Rui_dom.S`; the framework wires the concrete one per target. Keep hydrate-as-runtime-flag (not a third module) because hydrate and CSR-create *share* the same render pass and must alternate within one client build — this is load-bearing and ported as-is.

### P5. ppxlib instead of proc-macro — and the type-level GraphQL re-expression (the deep shift)
This is where OCaml genuinely changes the design. The Rust data layer is a tour de force of **trait projection**: `Field<gqlf::name>` (one impl per field, projecting field name → type *without naming the object type*), `Scalar::Out`, `GqlElem::Elem` (extract element type of a list/object), `Reshape<S>::Out` (wrap an inner exact-fit struct back into the field's container shape), `Fragment::SELECTION`, all so that `%query`-generated code type-checks against `[@@derive(GqlObject)]`-generated impls **even though the two macros never see each other** (coherence + the type-checker as schema validator). It synthesizes an anonymous **exact-fit struct** per selection layer.

OCaml has no coherence and no orphan-rule blanket impls, so we re-express this with the three tools OCaml *does* have:
- **Rows (polymorphic variants / object types) for exact-fit selections.** A selection set maps to a **closed object/record type**: `%query todos { id text done }` produces a value of type `< id : string; text : string; done_ : bool >` (or a generated record). The "exact fit" (you can only read the fields you selected = data masking) is the row type itself; selecting a field that does not exist, or reading an unselected field, is a *plain type error* — no `PhantomData` field-existence checks needed.
- **First-class modules / generated modules for schema projection.** `[@@deriving gql]` on a model generates a module `Todo_gql` exposing `field : string -> Value.t -> Value.t`, `typename`, `id`, plus a **field-name → type** mapping the ppx consults at expansion time (the ppx has the model's field list in scope via a small registry generated alongside, replacing `Field<M>` trait projection). The "project field name to its Rust type" that `Field<gqlf::name>::Ty` did is done by the **ppx looking up the schema description**, emitting the correct OCaml type directly into the generated row.
- **GADTs where a value must carry its decoder.** `Rui_gql.Value.t` is a normal variant; the exact-fit decoder for a generated selection type is an emitted `of_value : Value.t -> t` (mirrors Rust `FromValue`). Where the original needed `Reshape`/`GqlElem` to reshape `Vec<T>`→`Vec<inner>` vs single-object→inner, the ppx simply emits `list` vs scalar mapping based on the schema arity it already knows — no `Reshape` trait, no `GqlElem` blanket-vs-specific impl juggling.
- **Net result:** `Scalar`, `GqlElem`, `Field<M>`, `Reshape`, the `gqlf` marker module, and the `const _: fn() = || { … PhantomData … }` field-existence checks all **disappear**, replaced by (a) the ppx's compile-time schema lookup and (b) ordinary OCaml type errors on the generated row/record. This is the single biggest *simplification* of the port. The one cost: the ppx must carry a schema model (a generated `_schema` value or a side table) rather than relying on the type-checker's global coherence; we accept that because it also gives better error messages.

### P6. Errors, async state, lifecycle, and store consistency are behavior, not types — ported 1:1
`errors_message` classification (only `{data, errors:[]}` or `{data,…}` is success; HTML error pages / non-list `errors` / missing `data&errors` are failures), `%resource` returning `(rows, loading, error)`, `%mutation` optimistic + `on_error` + rollback, query/sub **skip-merge on error** (never write garbage into the store), the normalized-store write order (merge all entities first, *then* bump versions, so no view sees a half-merged snapshot), connection cursor dedup, optimistic snapshot/restore — these are runtime invariants and are ported verbatim. The Rust `Rc<dyn Fn(String)>` trick for `on_error` (so the outer closure stays `Fn`) is unnecessary in OCaml (closures are GC values, captured-by-reference); we just store the callback.

### P7. Memory/handler hygiene becomes ordinary GC + explicit dispose
The Rust version fought handler-registry leaks: `FETCH_HANDLERS: Vec<Option<Rc>>` with `drop_fetch_handler` called via `on_cleanup`, slot-based `INTERVAL_HANDLERS`, `NAV_GEN` generation fencing so a flush interrupted by sync navigation drops the stale batch.
- **OCaml shift:** fetch/interval handlers are closures referenced by their `effect`/`scope`; when the scope disposes, the closures become unreachable → GC. We still keep **`drop`/cancel for external resources** (SSE `EventSource.close`, `clearInterval`) via `on_cleanup`, and we keep **`nav_gen` generation fencing** for `flush_mounts` (it is a correctness ordering concern — a mount callback that navigates must abandon the rest of the stale batch — not a memory concern). So the *mechanism* simplifies (no manual slot table for memory) but the *correctness fences* stay.

## Target stack (concrete)

- **Language/build:** OCaml (5.x recommended), **dune 3.x** as the only build system. opam package `rui` (+ `rui.ppx`). Use a dune **virtual library** for the DOM backend (`rui.dom` virtual; `rui.dom.client` / `rui.dom.ssr` implementations) so `Rui_view`/`Rui_runtime` are written once and the concrete backend is selected per target — the idiomatic dune answer to Rust's `#[cfg(target_arch="wasm32")]`.
- **Client → JS:** **js_of_ocaml** (the client compiles to JS, so DOM/JS are first-class; no FFI marshalling, no wasm, no `alloc`/ptr/len). Targets use `(modes js)`. This produces the `app.js` that replaces the Rust `app.wasm`.
- **Browser bindings:** **Brr** (typed) — `Brr.El` (elements), `Brr.Ev`/`Brr.Ev.listen` (events, payload read directly), `Brr.Document`/`Brr.G` (document, window, history.pushState), `Brr_io.Fetch` (the `/graphql` POST + non-2xx→errors handling), `Brr_io.Sse`/`EventSource` (subscriptions), `Jv` for the `run_js`/`run_js_on`/`eval` escape hatches and any unbound API. The whole `assets/router.js` glue disappears; `Rui_client.start` is OCaml.
- **DSL / metaprogramming:** **ppxlib** — one ppx_rewriter (`ppx_rui`) providing `%view`, `[@component]`, `[@page]`, `%router`, `%query`/`%mutation`/`%subscription`/`%resource`/`%paginated`/`%fragment`, `%gql_root`, and `[@@deriving gql]`. Prefer **regular OCaml-syntax markup parsed inside the `%view` payload** (a custom mini-parser over the ppx payload, mirroring the Rust `view!` parser) rather than HTML literals; this keeps spans/hygiene and avoids a separate jsx-ppx dependency. (If an off-the-shelf JSX-ppx is desired, `%view` can wrap it, but the recommendation is a self-contained payload parser for full control over the reactive-dispatch semantics.)
- **Signal library:** **designed in-house** (`Rui_reactive`) — a Solid-style fine-grained signal/effect/memo with dynamic-dependency cleanup, value-dedup memo (`?equal`), and scope-based disposal. Do NOT pull an external FRP lib; the store/router/view semantics depend on these exact behaviors (memo dedup, untrack, scope dispose order). It is pure OCaml and target-agnostic (compiles for both jsoo and native).
- **GraphQL:** **in-house** end-to-end (no external GraphQL lib) — `Rui_gql.Value` (variant + recursive-descent JSON parser/printer), `Rui_gql.Store` (Relay normalized cache), native `Rui_gql.Parser`/`Rui_gql.Exec` (document parser + selection-projection executor), and the ppx-generated exact-fit selection types / schema field-table. The compile-time checking is done by the ppx's schema lookup + ordinary OCaml row/record typing, NOT by an external `graphql_ppx` (which targets a remote schema SDL; here the schema is the OCaml `%gql_root` methods + `[@@deriving gql]` models, the single source of truth). If interop with a `.graphql` SDL is later wanted, a `graphql-ppx`-style import could feed the same field-table, but it is out of scope for the spine.
- **SSR server (native):** start with **stdlib + Unix** (thread-per-connection like the Rust original; same known-deferred hardening: bind addr/port via env, read/write timeouts, TLS/auth/logging). May later move to an Eio/effects-based loop; the `Rui_server` surface (`serve {route;resolve;sse}`, `page`, `doc`, SSE) is independent of the concurrency choice.
- **App build:** one shared app `lib/` (models/schema/views, preprocessed by `pps ppx_rui`) consumed by (a) a native `bin/ssr.ml` executable linking `rui.dom.ssr` + `rui.server`, and (b) a `(modes js)` target linking `rui.dom.client` + `rui.client` producing `app.js`. Static assets served by `Rui_server`: the OCaml client bootstrap output, `app.js`, `styles.css`.

## dune project layout — native(SSR)/jsoo(client) split + ppx libs

```
rui/                              (the framework — opam package "rui")
├── dune-project                  (lang dune 3.x; generate_opam_files)
├── lib/
│   ├── reactive/                 lib `rui.reactive`  (module Rui_reactive) — pure, target-agnostic
│   │   └── dune                  (no jsoo/native-specific deps)
│   ├── gql/                      lib `rui.gql`       (Rui_gql.{Value,Store}) — Value+Store+From/Into are target-agnostic
│   │   └── dune                  (Store uses rui.reactive)
│   ├── gql_native/               lib `rui.gql.native` (Rui_gql.{Parser,Exec}) — server GraphQL parser+executor
│   │   └── dune                  (only linked into native)
│   ├── dom/                      defines `module type S` (Rui_dom)            — the abstract DOM surface
│   │   ├── rui_dom.ml            (signature S + shared helpers)
│   │   └── dune                  lib `rui.dom`
│   ├── dom_client/               lib `rui.dom.client` (Rui_dom_client : Rui_dom.S)
│   │   └── dune                  (depends brr, brr.io, js_of_ocaml; modes: byte/js — the hydrate-flag backend)
│   ├── dom_ssr/                  lib `rui.dom.ssr`    (Rui_dom_ssr : Rui_dom.S)
│   │   └── dune                  (native arena→HTML string backend; depends rui.gql)
│   ├── view/                     lib `rui.view`       (Rui_view) — functor over Rui_dom.S
│   │   └── dune                  (depends rui.dom, rui.reactive)
│   ├── runtime/                  lib `rui.runtime`    (Rui_runtime) — functor over Rui_dom.S
│   │   └── dune                  (path/query signals, navigate, on_mount, flush_mounts)
│   ├── server/                   lib `rui.server`     (Rui_server) — native only: TCP/HTTP, SSE, page(), doc()
│   │   └── dune                  (depends rui.gql.native, rui.dom.ssr; native only)
│   ├── client/                   lib `rui.client`     (Rui_client) — jsoo only: replaces router.js
│   │   └── dune                  (depends rui.dom.client; modes js — start/hydrate-probe/nav-intercept/rehydrate)
│   └── rui/                      lib `rui` (module Rui) — umbrella re-export; thin per-target shims
│       └── dune                  (Rui ties Rui_view/Rui_runtime to a concrete Rui_dom.S per build)
└── ppx/
    ├── rui_ppx.ml                lib `rui.ppx` (driver `ppx_rui`) — ppxlib; all extensions/attributes
    └── dune                      (kind ppx_rewriter; depends ppxlib)
```

### The native/jsoo split (the crux)
- **Target-agnostic core** (`rui.reactive`, `rui.gql` Value+Store, `rui.view`-as-functor, `rui.runtime`-as-functor) compiles for both targets. These are functors over `Rui_dom.S` so they never name a concrete backend.
- **Two DOM backends are separate libraries**: `rui.dom.client` (Brr) and `rui.dom.ssr` (arena). Mirrors Rust's `#[cfg(target_arch="wasm32")]` split on `dom.rs` — but here it is *dune library selection* instead of cfg, which is cleaner (no `#[cfg]` sprinkled through one file; each backend is a self-contained module implementing `Rui_dom.S`).
- **`hydrate` is NOT a third backend** — `rui.dom.client` carries an internal `hydrating : bool ref` and `el`/`text` branch on it (create vs claim-by-hid), exactly as Rust's `HYDRATE` thread-local. This is load-bearing: hydrate and CSR share one render pass within the client build.
- **App project** has two dune executables/targets from one source tree:
  - a **native bin** (`bin/ssr` → `rui.server` + app `route`/`resolve`/`sse`), linking `rui.dom.ssr`.
  - a **jsoo target** (`(modes js)`) → `rui.client` + app `route`, linking `rui.dom.client`, producing `app.js` (the analog of `app.wasm`).
  Both depend on the same `lib/` of the app (models, schema, views) — that shared lib is functor-instantiated per target via the umbrella `Rui`.

### How Rui binds the functor per target (so app authors write `Rui.*` plainly)
The umbrella `rui` library has two implementations of a tiny `Rui` module (selected by dune `(modes …)` / a virtual library): one instantiates `Rui_view.Make`/`Rui_runtime.Make` with `Rui_dom_client`, the other with `Rui_dom_ssr`. **Recommendation: use a dune _virtual library_** `rui.dom` with `(virtual_modules rui_dom)` and two implementations `rui.dom.client` / `rui.dom.ssr`; the consumer (app's native bin vs js target) picks the implementation via `(implements)` / the `default_implementation`. This is the idiomatic dune answer to Rust's cfg-backend and keeps `Rui_view`/`Rui_runtime` non-functorized at the app surface.

### App project layout (mirrors the example, OCaml-ized)
```
myapp/
├── lib/  (dune: shared, deps rui, preprocess (pps ppx_rui))
│   ├── model.ml            (types [@@deriving gql])
│   ├── api/schema.ml       (%gql_root impls + aggregate resolver, native parts under a guard)
│   ├── api/todos.ml        (resolver bodies — native)
│   ├── view/components.ml  ([@component]s, %fragment)
│   ├── view/layout.ml      (shell, dash_shell)
│   ├── view/pages/*.ml     ([@page] funcs)
│   └── routes.ml           (%router → let route)
├── bin/ssr.ml              (native: Rui.Server.serve {route;resolve;sse})  (dune: (modes exe))
└── web/dune                (js target: Rui.Client.start route → app.js)    (dune: (modes js))
```

### Parsable module (replaces Rust `FromStr+Default+Clone+PartialEq` bound on `param_as`)
`module type Parsable = sig type t val parse : string -> t option val default : t val equal : t -> t -> bool end`. `param_as`/`query_param_as` take `(module Parsable with type t = 'a)`. Provide `Rui_runtime.{string; int; float; bool}` ready-made. The `[@page "/todo/:id"]` ppx, given `id : int signal`, emits `param_as (module Rui_runtime.Int) idx` by mapping the annotated type to a known Parsable (or requires an explicit `[@parse Module]` for custom types).

## Master Rust→OCaml feature mapping

| Rust `rui` mechanism | OCaml `Rui` mechanism | Cleaner / wash / cost |
|---|---|---|
| `Rc<RefCell<T>>` reactive nodes | plain mutable records on GC heap | **Cleaner** — no interior-mutability ceremony |
| `Signal<T>` `get`/`set`, `subs: Rc<RefCell<Vec<usize>>>` | `'a signal = { mutable v; mutable subs : effect_id list }` | Cleaner |
| effect arena `Vec<Option<EffectNode>>` + `usize` ids (no reuse) | same arena-of-options + int ids (keep stable ids) | Wash (ported as-is for the same dynamic-dep-cleanup invariant) |
| dynamic-dep cleanup (drop self from last-run signals) | **ported verbatim** (correctness, not memory) | Wash — mandatory |
| `memo` value-dedup via `T: PartialEq` + `untrack` self-read | `memo ?equal` (default `(=)`) + `untrack` self-read | Cleaner (`?equal` chooses structural/physical/custom) |
| `Scope` `Drop` → run cleanups then dispose; `take_parts`/`absorb_parts` | `scope`/`Scope.dispose` explicit; mount cb runs *inside* page scope | **Cleaner** — no Drop-order subtlety; reparenting trivial |
| `dispose_effect` `try_with` TLS-destruction guard (SSR abort fix) | **deleted** — single-runtime SSR, no TLS destructor race | **Cleaner** (a whole bug class gone) |
| `on_cleanup` CLEANUPS stack tied to scope | `on_cleanup` tied to current scope | Wash |
| wasm + `extern "C"` FFI + `router.js` env | `js_of_ocaml` + Brr; closures hold real `Brr.El.t` | **Cleaner** (no glue file, no marshalling) |
| `nodes[]` registry + `u32` node handles | `Brr.El.t` values directly (SSR: arena int, but abstract) | **Cleaner** (kills `u32`↔`Display` clash, `Into<u32>` bridges) |
| `alloc`/ptr/len + `dispatch(id,ptr,len)` + `on_fetch(id,…)` | direct OCaml calls; event closures read `Brr.Ev` payload | **Cleaner** (~8 trampolines deleted) |
| `add_event(node,ev,handler_id)` + `HANDLERS: Vec<Rc<Fn(&str)>>` | `Brr.Ev.listen` with OCaml closure | **Cleaner** (no handler-id table) |
| `dom.rs` `#[cfg(target_arch=wasm32)]` two backends | `module type Rui_dom.S` + two libs `dom.client`/`dom.ssr` (dune virtual lib) | **Cleaner** (self-contained backends) |
| hydrate as `HYDRATE` thread-local flag, claim by `data-h` / `<!--h:N-->` | `hydrating : bool ref` in `dom.client`, same claim scheme | Wash (ported as-is) |
| `IntoView` trait (`build`/`rebuild`) | `module type Into_view` (`build`/`rebuild`) + instances | Wash |
| `View(pub u32)` + `From<View> for u32` | `Rui_view.t` abstract over node | **Cleaner** |
| `reactive_block` / `keyed_for` | `Rui_view.reactive_block` / `keyed_for` | Wash |
| `proc-macro` `view!`/`#[component]`/`#[page]`/`router!` | `ppxlib`: `%view` / `[@component]` / `[@page]` / `%router` | Cleaner (real spans/hygiene; stable page key from loc, no `module_path!()`) |
| `Strategy::{Ssr,Csr,Static}` + `Page{key,strategy,render}` | `type strategy` + `page` record | Wash |
| path param via `module_path!()` key + `param_as::<T>` | page key from ppx (structure-item loc/name); `param_as (module Parsable)` | Cleaner key; cost: explicit Parsable module vs `FromStr` |
| `PATH`/`QUERY` thread-local signals, `split_url`, dedup `set_path` | same two-signal design (one ref each), `split_url`, dedup on set | Wash (ported design) |
| nested route group + reactive outlet (`reactive_block` over `path()`) | same: group = one `page`, outlet = `reactive_block` selecting leaf by `path` | Wash |
| `on_mount` MOUNT_QUEUE + flush at every wasm entry + `NAV_GEN` fence | same queue + flush at every client entry + `nav_gen` fence | Wash (fences kept for correctness) |
| `INTERVAL_HANDLERS`/`FETCH_HANDLERS` slot tables (leak fix) | closures held by scope → GC; keep `on_cleanup` for `EventSource.close`/`clearInterval` | **Cleaner** (GC handles memory; only external-resource teardown stays) |
| `run_js`/`run_js_on`/`eval` with `\x00/\x01` status byte | `Rui_dom.run_js/run_js_on/eval` → `(string,string) result` | Cleaner (`eval` callback gets `result`; OCaml has Promise via Brr.Fut / Jv) |
| `#[derive(GqlObject)]` (`Field<M>`,`GqlElem`,`Reshape`,`IntoValue`,`FromValue`,`gql_id`) | `[@@deriving gql]`: generates `Todo_gql` module + From/Into_value + a **schema field-table** the ppx reads | **Cleaner** (no coherence dance) |
| `gqlf` marker module via `gql_fields!` (manual field list) | **eliminated** — ppx derives markers from schema/derive; authors write no marker list | **Cleaner** |
| `Field<gqlf::name>::Ty` trait projection of field type | ppx looks up field type in the generated schema table → emits OCaml type | **Cleaner** (better errors) |
| exact-fit anonymous selection struct per layer (`gen_sel_struct`) | generated **record type / object (row) type** per selection layer | **Cleaner** (data masking = row type; unselected field = type error) |
| `Scalar::Out` | direct scalar type from schema table | Cleaner (no trait) |
| `Reshape`/`GqlElem` list-vs-object reshape | ppx emits `list` vs scalar based on schema arity | Cleaner |
| `Fragment::SELECTION` const + `...Name` spread | generated `Frag.selection : string` + `...Name` spread in `%query` | Wash |
| `mutation!` `on_error` `Rc<dyn Fn>` (keep outer `Fn`) | store the callback directly (GC closure) | Cleaner |
| query/sub skip-merge on error; resource `(rows,loading,error)` | identical runtime behavior | Wash (ported 1:1) |
| normalized store: merge-all-then-bump, `$ref`, denormalize, versions | identical Store with `Value.t` | Wash |
| `paginated!` Relay cursor + dedup + `merge_connection` | identical | Wash |
| `#[gql_root]` methods-as-schema + dispatch resolver | `%gql_root` on a module/struct → field-table + native `resolve` dispatch | Wash (ppx instead of proc-macro) |
| `FromArg` arg extraction by type | `Exec.Args` + a `From_arg` module/typed getters | Wash |
| native SSR server (thread-per-conn, arena, dehydrate, doc()) | `Rui_server` (can keep thread/`Unix` or use an effects-based loop) | Wash (server hardening still deferred, as in Rust) |
| SSR `dom::gql` local pre-fetch → `SSR_RESP` → `<script id=__rui_data>` | `Rui_dom_ssr.gql` local-exec → dehydrate → same `__rui_data` script | Wash |
| client `seed_responses` consume-once cache; sub seeds initial then SSE | identical | Wash |

## 命名约定

## Canonical OCaml API names — ALL 8 section authors MUST use these exactly

### Top-level packaging
| Concept | Name |
|---|---|
| Library (umbrella) | `rui` → module `Rui` (re-exports the public surface) |
| ppx | package `rui.ppx`, driver `ppx_rui` |
| Reactive core | `Rui_reactive` (re-exported as `Rui.Reactive`) |
| View / IntoView | `Rui_view` (`Rui.View`) |
| DOM backend sig | `Rui_dom` with `module type S`; impls `Rui_dom_client`, `Rui_dom_ssr` |
| Runtime / router | `Rui_runtime` (`Rui.Runtime`) |
| GraphQL data layer | `Rui_gql` (`Rui.Gql`), submodules `Rui_gql.Value`, `Rui_gql.Store`, `Rui_gql.Exec`, `Rui_gql.Parser` |
| SSR server | `Rui_server` (`Rui.Server`, native only) |
| Client bootstrap | `Rui_client` (`Rui.Client`, jsoo only) — replaces `router.js` |

### Reactive (`Rui_reactive` / `Rui.Reactive`)
- `type 'a signal` ; `Signal.make : 'a -> 'a signal` ; `Signal.get : 'a signal -> 'a` ; `Signal.set : 'a signal -> 'a -> unit` ; `Signal.update : 'a signal -> ('a -> 'a) -> unit`
- `effect : (unit -> unit) -> effect_handle` ; `Effect.dispose : effect_handle -> unit`
- `memo : ?equal:('a -> 'a -> bool) -> (unit -> 'a) -> 'a signal` (default `equal = (=)`; **value-dedup**: do not notify downstream when the recomputed value is equal — ports the Rust `PartialEq` dedup, `?equal` lets authors pick physical/custom equality)
- `untrack : (unit -> 'a) -> 'a`
- `scope : (unit -> 'a) -> 'a * scope` ; `Scope.dispose : scope -> unit`
- `on_cleanup : (unit -> unit) -> unit`
- (internal, for ppx/runtime) `Scope.take_parts` / `Scope.absorb_parts` — keep names for parity though OCaml impl is simpler.

### View (`Rui_view` / `Rui.View`)
- `type t` (a built view = handle to a DOM node, holds `Brr.El.t` on client / arena id on SSR — but exposed abstractly)
- `module type Into_view` with `type state; build : unit -> node * state; rebuild : state -> unit` (ports `IntoView`); instances `Into_view.of_text`, `of_view`, `of_option`, `of_list`, `of_unit`
- `text : string -> t` ; `node : t -> node`
- `reactive_block : (unit -> 'a) -> node` where `'a` is `Into_view` (ports `reactive_block`; the `%view` `{ fun () -> … }` runtime)
- `keyed_for : parent:node -> 'a list signal -> key:('a -> 'k) -> build:('a -> node) -> unit` (ports `keyed_for`)
- `node_ref : unit -> node_ref` ; `Node_ref.get` / `Node_ref.set`
- `type strategy = Ssr | Csr | Static`
- `type page = { key : string; strategy : strategy; render : unit -> t }` ; `Page.make : key:string -> strategy:strategy -> (unit -> t) -> page`

### DOM (`Rui_dom.S`) — the tri-backend surface (names match Rust `dom::*`)
`el`, `text`, `set_text`, `append`, `remove_child`, `attr`, `set_value`, `clear`, `mount`, `clear_app`, `push_url`, `on` (`node -> event:string -> (string -> unit) -> unit`), `on_click`, `focus`, `scroll_into_view`, `set_interval`, `clear_interval`, `run_js`, `run_js_on`, `eval` (`string -> ((string, string) result -> unit) -> unit`), `gql`, `subscribe`, `set_hydrate`, `seed_responses`, `dehydrate_responses` (ssr), `reset`/`take_html` (ssr). Client also exposes `hydrating`.

### Runtime / router (`Rui_runtime` / `Rui.Runtime`)
- `path : unit -> string signal` ; `param : int -> string signal` ; `param_as : (module Parsable with type t = 'a) -> int -> 'a signal` (OCaml has no `FromStr`+`Default` bound; pass a first-class parser module — see module_layout)
- `query_string : unit -> string signal` ; `query_param : string -> string signal` ; `query_param_as : (module Parsable with type t='a) -> string -> 'a signal`
- `matches : pattern:string -> path:string -> bool`
- `navigate : route -> string -> unit` ; `go : route -> string -> unit` ; `render_path : route -> string -> unit`
- `query_encode : string -> string`
- `on_mount : (unit -> unit) -> unit` ; `flush_mounts : unit -> unit` (jsoo) / no-op (native)
- `type route = string -> Rui_view.page`

### GraphQL (`Rui_gql` / `Rui.Gql`)
- `Value.t` variant (`Null | Bool of bool | Int of int | Float of float | Str of string | List of t list | Obj of (string*t) list`) with `get`, `field`, `as_str/as_int/as_float/as_bool/as_list/is_null`, `to_json`, `parse`, `errors_message`
- `module type From_value = sig type t val of_value : Value.t -> t end` ; `module type Into_value = sig type t val to_value : t -> Value.t end` (replace `FromValue`/`IntoValue` traits)
- `Store`: `read_entity`, `merge_all`, `bump_all`, `normalize_list`, `merge_connection`, `read_connection`, `keys_of`, `snapshot`, `restore`, `reset`
- `to_gql_arg` via `module type To_gql_arg` / generated; `Exec.execute`, `Exec.Args`, `Exec.resolver` (native)

### ppx surface (extension/attribute names section authors emit)
- `%view { … }` (expression extension; the JSX-ish markup)
- `[@page "ssr"/"csr"/"static", "/path/:id"]` on a `let view … = …` structure item (attribute ppx)
- `[@component]` on a `let name ~props = …` (generates the named-props record + slot)
- `[@@deriving gql]` (replaces `#[derive(GqlObject)]`; takes `[@gql.id]` on a field)
- `%query`, `%mutation`, `%resource`, `%subscription`, `%paginated`, `%fragment`, `%gql_root` (expression/structure extensions; `%gql_fields` is **not needed** — see mapping table, the ppx derives field markers from the schema, so authors do NOT write a marker list)
- `%router { … }` (generates `let route : Rui_runtime.route`)

### Conventional module paths the ppx assumes (mirror Rust's "directory-is-convention")
`Model` (shared models, `[@@deriving gql]`), `Api.Schema` (`%gql_root`), `View.Components`, `View.Layout`, `View.Pages` — the ppx emits references to `View.Components.<snake>` for `<Component>` tags and to `Model.<T>` for fragment/derive bases.

## Reactive Core

The reactive core is the engine the rest of Rui is built on: `Signal` (state), `effect` (auto-subscribing side-effects that re-run when their dependencies change), and `memo` (derived, re-subscribable values). It is deliberately DOM-agnostic — a pure dynamic dependency graph, Solid-style. The View layer, the router (`path`/`param`/`query_param` are all `memo`s over two root signals), and the GraphQL normalized cache (a query view is a `memo` over store entities) are *all* expressed in terms of these three primitives plus `scope`/`on_cleanup`. This section ports `crates/rui/src/reactive.rs` (307 lines) faithfully to OCaml — translating the existing design, not inventing new behavior — and shows where OCaml's GC, js_of_ocaml, variants, and labels make it cleaner, and where the `(fun () -> …)` boundary remains an irreducible wash.

### 1. What rui does today (Rust)

The Rust core lives entirely in thread-local arenas (one set of TLS per thread; the SSR server is thread-per-connection, so each request gets its own clean graph):

- **`Signal<T>`** = `{ inner: Rc<RefCell<T>>, subs: Rc<RefCell<Vec<usize>>> }`. `get()` clones the value out and, *if* an effect is currently running, registers that effect's id into `subs` (and pushes `subs` onto the effect's `deps`). `set(v)` writes, snapshots `subs`, then re-runs each subscribed effect.
- **`effect(f)`** allocates an `EffectNode { f: Rc<dyn Fn()>, deps: Vec<SubList> }` into a TLS `EFFECTS: Vec<Option<EffectNode>>` (slot-indexed by a stable, non-reused `usize` id; `None` = disposed tombstone), registers the id with the current owner, and runs it once. Returns an `EffectHandle { id }`.
- **`run_effect(id)`** is the crux of *dynamic dependency cleanup*: before running, it `drain`s the node's `deps` and removes `id` from each of those signals' `subs`. Then it sets `CURRENT = Some(id)`, runs `f` (which re-`get`s → re-subscribes → re-populates `deps`), and restores `CURRENT`.
- **`memo(f)`** seeds a `Signal` with `untrack(&f)` (so the initial compute doesn't pollute the *enclosing* effect's dependency set), then wraps `f` in an `effect` that recomputes and, **only if the new value differs (`PartialEq`)**, calls `sig.set(v)` to notify downstream — value-equality dedup. It reads its own current value via `untrack(|| sig.get())` to compare without self-subscribing into a cycle.
- **`untrack(f)`** temporarily sets `CURRENT = None` around `f`, restoring it after.
- **`scope(f)`** pushes a fresh frame onto an `OWNER: Vec<Vec<usize>>` stack and a `CLEANUPS: Vec<Vec<Box<dyn FnOnce()>>>` stack, runs `f`, pops both, and returns `(R, Scope { ids, cleanups })`. `Scope`'s `Drop` runs the cleanups first (node still alive, signals still readable) then `dispose_effect`s every id. `on_cleanup(f)` pushes onto the top cleanup frame.
- `Scope::take_parts` / `absorb_parts` move effect-ids + cleanups between scopes (used so `on_mount`-created effects get re-parented into the page scope).

The owner/scope machinery exists because effects have **observable** side-effects (DOM listeners, `setInterval`, SSE subscriptions, normalized-store subscriptions) that must be torn down deterministically when a page or sub-tree is removed — it is *not* a memory concern.

### 2. OCaml design

GC removes all of `Rc<RefCell>`, the `Vec<Option<_>>` arena, the stable-id indirection, and the `usize`-id bookkeeping: a signal *is* a record, an effect *is* a record, and subscription edges are direct record references. We keep `Cell`-of-current-effect, the owner stack, the cleanup stack, dynamic-dependency cleanup, value-dedup, and explicit disposal — those are correctness/observable-behavior properties, not memory ones.

#### 2.1 Internal node types

We need one cycle-free wrinkle: a signal holds its subscribers, and each subscriber (effect) holds the signals it depends on, so we can un-subscribe before re-running. In Rust this is two `Rc`s pointing at the *same* `SubList`. In OCaml we make the effect's subscriber-set abstract over the element type using an existential-free trick: signals are heterogeneous, so an effect's `deps` is a list of **unsubscribe thunks** (each closes over the specific signal and removes this effect from it). That sidesteps `'a signal` heterogeneity without GADTs.

```ocaml
(* Rui_reactive — internal (not exposed in .mli) *)

type effect_node = {
  run         : unit -> unit;          (* the user thunk *)
  mutable deps : (unit -> unit) list;  (* unsubscribe-me thunks from last run *)
  mutable alive : bool;                (* tombstone flag (replaces None slot) *)
}

type 'a signal = {
  mutable v    : 'a;
  mutable subs : effect_node list;     (* current subscribers *)
  equal        : 'a -> 'a -> bool;     (* for set-time short-circuit on signals too? see §4 *)
}

(* Solid-style "who is tracking right now" — a single mutable cell, like Rust's CURRENT. *)
let current : effect_node option ref = ref None

(* Owner / cleanup stacks — lists used as stacks. *)
let owner_stack   : effect_node list ref list ref = ref []
let cleanup_stack : (unit -> unit) list ref list ref = ref []
```

Note `effect_node` is *not* parameterized, so it can sit in `'a signal.subs` for any `'a`. The unsubscribe-thunk in `deps` is what carries the per-signal type.

#### 2.2 `Signal` — `.mli`

```ocaml
type 'a signal

module Signal : sig
  val make   : ?equal:('a -> 'a -> bool) -> 'a -> 'a signal
  val get    : 'a signal -> 'a            (* tracks if an effect is running *)
  val set    : 'a signal -> 'a -> unit    (* notifies subscribers *)
  val update : 'a signal -> ('a -> 'a) -> unit
  val peek   : 'a signal -> 'a            (* = untrack (fun () -> get s); see §4 *)
end
```

Implementation sketch (note: no value clone needed — OCaml `get` returns the value directly; Rust had to `.clone()` because it handed back an owned `T` out of the `RefCell`):

```ocaml
let make ?(equal = (=)) v = { v; subs = []; equal }

let get s =
  (match !current with
   | None -> ()
   | Some eff ->
     (* subscribe only once per run; physical de-dup of the effect node *)
     if not (List.memq eff s.subs) then begin
       s.subs <- eff :: s.subs;
       (* record how to remove *this* effect from *this* signal next run *)
       eff.deps <- (fun () -> s.subs <- List.filter (fun e -> e != eff) s.subs)
                   :: eff.deps
     end);
  s.v

let set s v =
  s.v <- v;
  (* snapshot first (a re-run may mutate s.subs), exactly like the Rust .clone() snapshot *)
  List.iter run_effect (List.rev s.subs)
```

The `List.rev` preserves first-subscribed-first order to match the Rust `Vec` iteration; subscriber lists are tiny in practice so list ops are fine (a `Hashtbl`/`Ptset` is available if profiling demands it).

#### 2.3 `Effect` — `.mli`

```ocaml
type effect_handle

val effect : (unit -> unit) -> effect_handle

module Effect : sig
  val dispose : effect_handle -> unit
end
```

The handle is just the node (GC keeps it alive as long as any signal still references it; `dispose` flips `alive` and severs edges):

```ocaml
let run_effect (eff : effect_node) =
  if eff.alive then begin
    (* dynamic-dependency cleanup: drop self from last run's signals first *)
    List.iter (fun unsub -> unsub ()) eff.deps;
    eff.deps <- [];
    let prev = !current in
    current := Some eff;
    (* exceptions must not strand `current` — restore in a finally *)
    Fun.protect ~finally:(fun () -> current := prev) eff.run
  end

let effect (f : unit -> unit) : effect_handle =
  let eff = { run = f; deps = []; alive = true } in
  register_owned eff;     (* push into top OWNER frame, if any *)
  run_effect eff;
  eff                     (* effect_handle = effect_node (abstract in .mli) *)

let dispose_effect eff =
  if eff.alive then begin
    eff.alive <- false;
    List.iter (fun unsub -> unsub ()) eff.deps;   (* sever subscriptions *)
    eff.deps <- []
  end
```

`Fun.protect` is the OCaml answer to Rust's manual `CURRENT.set(prev)` restore — and it is strictly *better*: in Rust, if a user effect panicked, `CURRENT` would be left pointing at the dead effect (Rust's code does not unwind-protect the restore). In OCaml a raised exception still restores `current`, so a throwing effect can't corrupt subsequent tracking. This is a small but real correctness win.

#### 2.4 `Memo` — `.mli` and dedup

```ocaml
val memo : ?equal:('a -> 'a -> bool) -> (unit -> 'a) -> 'a signal
```

```ocaml
let memo ?(equal = (=)) f =
  let sig_ = make ~equal (untrack f) in   (* seed without polluting outer effect deps *)
  let _h = effect (fun () ->
    let v = f () in                        (* runs tracked → subscribes to f's reads *)
    (* value-equality dedup: don't notify downstream if unchanged.
       read own value untracked to avoid self-subscription cycle. *)
    if not (equal (untrack (fun () -> get sig_)) v) then set sig_ v)
  in
  sig_
```

The default `equal = (=)` is OCaml's *structural* equality, the direct analogue of Rust's `PartialEq` derive. The `?equal` label is the cleaner-than-Rust part: a Rust `memo` is hard-bound to whatever `PartialEq` the type derives, whereas an OCaml author can pass `~equal:(==)` (physical, e.g. for cache-shared records), a custom comparator (e.g. compare only an `id` field), or even `~equal:(fun _ _ -> false)` to force notify-always. `param_as`/`query_param_as` etc. all flow through here, so they inherit the dedup that prevents "unrelated `?sort` change re-notifying the `?q` memo → redundant `%resource` refetch" (gaps progress 13).

#### 2.5 `untrack`

```ocaml
val untrack : (unit -> 'a) -> 'a
```

```ocaml
let untrack f =
  let prev = !current in
  current := None;
  Fun.protect ~finally:(fun () -> current := prev) f
```

Same finally-restore upgrade as `run_effect`.

#### 2.6 `Scope` / `Owner` — disposal and `on_cleanup` without `Drop`

This is the one place the Rust design leaned hard on RAII (`impl Drop for Scope`), and the OCaml port must reproduce the *observable* lifecycle without it.

```ocaml
type scope

module Scope : sig
  val dispose : scope -> unit
  (* internal, for ppx/runtime parity with Rust *)
  val take_parts   : scope -> effect_handle list * (unit -> unit) list
  val absorb_parts : scope -> effect_handle list -> (unit -> unit) list -> unit
end

val scope      : (unit -> 'a) -> 'a * scope
val on_cleanup : (unit -> unit) -> unit
```

```ocaml
type scope = {
  mutable effects  : effect_node list;     (* owned effects/memos *)
  mutable cleanups : (unit -> unit) list;  (* on_cleanup callbacks *)
  mutable disposed : bool;
}

let register_owned eff =
  match !owner_stack with
  | frame :: _ -> frame := eff :: !frame     (* push to top frame, ignore if none *)
  | [] -> ()

let on_cleanup f =
  match !cleanup_stack with
  | frame :: _ -> frame := f :: !frame
  | [] -> ()                                 (* no owner ⇒ ignored, like Rust *)

let scope (f : unit -> 'a) : 'a * scope =
  let oframe = ref [] and cframe = ref [] in
  owner_stack   := oframe :: !owner_stack;
  cleanup_stack := cframe :: !cleanup_stack;
  let r =
    Fun.protect
      ~finally:(fun () ->
        owner_stack   := List.tl !owner_stack;
        cleanup_stack := List.tl !cleanup_stack)
      f
  in
  (* frames captured in declaration order; reverse to first-registered-first *)
  (r, { effects = List.rev !oframe; cleanups = List.rev !cframe; disposed = false })

let dispose_scope sc =
  if not sc.disposed then begin
    sc.disposed <- true;
    (* cleanups FIRST (nodes alive, signals readable), then effects — mirrors Rust Drop order *)
    List.iter (fun c -> c ()) sc.cleanups;
    List.iter dispose_effect sc.effects;
    sc.effects <- []; sc.cleanups <- []
  end
```

**Why we still need an owner even with GC.** GC reclaims *unreferenced* memory; it cannot know that a `setInterval`/SSE-subscription/DOM-listener effect is *logically* dead while a closure somewhere (the browser's timer table, the SSE handler registry) still holds it. The owner/scope is what makes "this page/sub-tree is gone" an explicit, deterministic event that fires the teardown. Concretely (gaps progress 11, 15, 16): on SPA navigation the runtime does `Scope.dispose page_scope` *before* writing the new `PATH`/`QUERY` signals, so the outgoing page's `param`/`query_param` memos un-subscribe before they could "ghost-recompute" against the new route and fire a wasted fetch. That ordering is a correctness property the GC will never give us for free.

**Nested disposal without `Drop` recursion.** In Rust, a parent effect's closure owns a child `Scope`; when the parent effect is disposed its closure is dropped and the child `Scope`'s `Drop` recurses. In OCaml there is no `Drop`, so nested scopes must be disposed *explicitly* by whoever owns them. The clean place to hang this is the View layer's `reactive_block`/`keyed_for`: each holds the child `scope` it produced and calls `Scope.dispose` on it before rebuilding (and registers `on_cleanup (fun () -> Scope.dispose child)` so a parent teardown cascades). This is more explicit than Rust's "drop cascades for free," and is the one place GC is a mild wash — we trade implicit RAII recursion for one explicit `dispose` call per dynamic boundary. It is *not* more error-prone in practice because there are exactly two such boundaries (`reactive_block`, `keyed_for`), both framework-owned.

**`take_parts` / `absorb_parts`** stay for parity and serve the same `on_mount` re-parenting need (gaps progress 15, edge ②): mount callbacks run *after* the page `scope` frame has popped, so an effect created inside one would have no owner frame → ghost effect. The runtime runs each mount callback in its own child `scope`, then `Scope.absorb_parts page_scope ids cleanups` to fold the products into the page scope so they die on navigation.

### 3. Feature → OCaml mechanism mapping

| rui (Rust) | OCaml mechanism | Cleaner / wash / harder |
|---|---|---|
| `Signal { Rc<RefCell<T>>, Rc<RefCell<Vec<usize>>> }` | `{ mutable v; mutable subs; equal }` plain GC record | **Cleaner** — no `Rc`, no `RefCell`, no clone-out-of-cell on `get`, no double-borrow class of bugs at all |
| `EFFECTS: Vec<Option<EffectNode>>` arena + stable `usize` ids + tombstones | direct `effect_node` references + `alive : bool` | **Cleaner** — GC owns lifetime; ids and the `Vec<Option<_>>` slot machine vanish entirely |
| `subs: Vec<usize>` ↔ `deps: Vec<SubList>` (shared `Rc`) | `subs: effect_node list` + `deps: (unit -> unit) list` of unsubscribe thunks | **Wash, slightly cleaner** — the thunk-closure trick replaces shared-`Rc` aliasing and also dodges `'a signal` heterogeneity without GADTs |
| dynamic dependency cleanup (`drain deps; retain != id`) | `List.iter (fun u -> u ()) eff.deps; eff.deps <- []` at top of `run_effect` | **Exact port** — same algorithm, mandatory correctness property |
| `CURRENT: Cell<Option<usize>>` tracking discipline | `current : effect_node option ref` | **Wash** — same single-cell Solid-style design |
| restore `CURRENT`/`prev` after run | `Fun.protect ~finally` | **Cleaner** — exception-safe; Rust leaves `CURRENT` dangling on panic |
| `memo` with `PartialEq` dedup | `memo ?equal` defaulting to `(=)` | **Cleaner** — `?equal` chooses structural / physical / custom / always-notify; Rust is locked to the derived `PartialEq` |
| `untrack` | `untrack` (cell swap + `Fun.protect`) | Wash (+ exception-safe) |
| owner stack, cleanup stack | two `_ list ref list ref` stacks | **Wash** — identical shape, no `Box<dyn FnOnce>` boxing needed |
| `Scope` via `impl Drop` (RAII) | explicit `Scope.dispose`; nested boundaries dispose children explicitly | **Harder (mildly)** — no automatic `Drop` cascade; framework calls `dispose` at the two dynamic boundaries |
| `Box<dyn FnOnce()>` cleanups, `Rc<dyn Fn()>` effect bodies | plain `unit -> unit` closures | **Cleaner** — first-class functions, no trait-object boxing |
| TLS-during-`Drop` abort guarded by `try_with` | **does not exist** in OCaml | **Cleaner** — hazard structurally absent (see §4) |

The honest "no free lunch" line (per the spine's P1): the `(fun () -> …)` thunk at every dynamic boundary — conditionals, lists, dynamic attributes — is intrinsic to fine-grained reactivity and is **not** removed by OCaml. We do not pretend otherwise. OCaml removes the *secondary* ceremony (Rc/RefCell, ids, boxing, the Drop/TLS hazard), not the thunk.

### 4. Edge-cases rui solved, and how OCaml handles each

1. **Dynamic dependency cleanup (stale subscriptions grow unbounded).** rui's `dynamic_deps_cleanup` test: an effect that reads `a` under `if cond` then switches to `b` must stop firing on `a`. *OCaml:* `run_effect` clears `eff.deps` and un-subscribes from every signal before re-running, exactly as Rust drains `deps`. Ported verbatim; covered by a direct translation of the test.

2. **Memo value-equality dedup (`?q` unchanged but `?sort` changed → redundant refetch).** Fixed in gaps progress 13 by adding `T: PartialEq` + `if untrack(|| sig.get()) != v { sig.set(v) }`. *OCaml:* `memo ?equal` does the same compare via `untrack (fun () -> get sig_)` before `set`; default `(=)` matches the derived `PartialEq`, and `?equal` is a strict superset of the Rust capability. All downstream memos (`param_as`, `query_param_as`) inherit it.

3. **`memo` seed must not pollute the enclosing effect's deps.** rui seeds with `untrack(&f)`. *OCaml:* identical — `make ~equal (untrack f)`. Without this, building a `memo` *inside* an effect would silently subscribe that outer effect to the memo's inputs.

4. **`memo` self-subscription cycle.** Reading the memo's own signal to compare would re-subscribe the memo's effect to itself. *OCaml:* read via `untrack (fun () -> get sig_)`, same as Rust's `untrack(|| sig.get())`.

5. **Dispose ordering on navigation: dispose old page *before* writing route signals.** gaps progress 14 edge ②: writing `PATH`/`QUERY` first lets the about-to-die page's memos/resources ghost-recompute and waste fetches (doubled for query). *OCaml:* the runtime calls `Scope.dispose page_scope` first, then `Signal.set path …`. The disposed scope's effects have flipped `alive=false` and severed `deps`, so they cannot re-run. This is a runtime-ordering invariant, not a core change, but the core must make `dispose` synchronous and complete — which it is (`alive` flip + `deps` sever happen eagerly).

6. **`on_cleanup` runs before effect disposal, while nodes are still alive.** Rust `Scope::drop` runs cleanups first, then `dispose_effect`. *OCaml:* `dispose_scope` runs `cleanups` then `dispose_effect`s, same order — so a cleanup that reads a signal or touches a still-mounted node is safe.

7. **`on_mount`-created effects must be re-parented or they leak as ghosts.** gaps progress 15 edge ②: mount callbacks run after the page scope frame popped (empty owner stack → `register_owned` no-ops → ghost effect that never gets disposed). *OCaml:* run each mount callback in a child `scope`, then `Scope.absorb_parts page_scope` its parts. `take_parts`/`absorb_parts` are retained exactly for this.

8. **The TLS-during-Drop abort.** This is the subtlest rui bug (gaps "踩坑+修复" + `dispose_effect`'s `try_with`): after `Scope` became dispose-on-Drop, an SSR worker thread *ending* would destroy its thread-locals; residual `Scope`s (held by `reactive_block` effect closures inside `EFFECTS`) would be dropped during TLS teardown, re-entering `dispose_effect`, which then tried to access the *already-destructing* `EFFECTS` TLS → "cannot access TLS during/after destruction" → process **abort**. The fix was `EFFECTS.try_with(..).ok().flatten()` (if the arena is gone, skip). 

   *OCaml: this hazard is structurally impossible.* There is no destructor that runs during interpreter teardown, and — critically — the OCaml SSR backend will **not** use thread-locals at all. Each SSR request creates its reactive graph as an *ordinary value* threaded explicitly (or via a per-request `Domain`/`Effect`-handler-scoped context), and the graph is simply abandoned to the GC when the request handler returns; nothing re-enters disposal during shutdown. The `current`/`owner_stack`/`cleanup_stack` globals shown in §2.1 are fine for the **single-threaded js_of_ocaml client** (which is genuinely single-threaded). For native SSR we make those three a per-request record passed down (or bound with OCaml 5 effect handlers / a `Domain.DLS` key), so concurrent requests never share the tracking cell. Either way, there is no "access a half-destroyed arena" path, so the defensive `try_with`/`ok().flatten()` has no OCaml counterpart — the bug class is designed out rather than guarded. The associated testing lesson from rui (a single-threaded wasm harness can't catch a multi-thread TLS-teardown abort) maps to: exercise the native SSR path under concurrent connections in CI, since the js_of_ocaml test harness likewise can't surface a native-only context-isolation bug.

9. **`set` re-entrancy / iterating subscribers that mutate the list.** Rust snapshots `subs` (`.clone()`) before iterating because a re-run may add/remove subscribers. *OCaml:* `set` iterates `List.rev s.subs` over the list value captured at call time; because OCaml lists are immutable, the iteration is over a stable snapshot even though `s.subs` (the mutable field) may be reassigned mid-iteration by a nested `get`/cleanup. So the snapshot is free — no clone needed.

10. **Effect re-run order determinism.** Rust pushes new subscribers to the end of a `Vec` and iterates front-to-back. *OCaml:* we prepend to `subs` (O(1)) and `List.rev` at notify time to reproduce first-subscribed-first order, keeping observable ordering identical to Rust (matters for tests and for predictable DOM update order at a boundary with several effects).

### 5. Open questions / risks

- **Native SSR context model:** pick *one* of (a) explicit context record threaded through the View/Runtime API, (b) `Domain.DLS`-keyed globals, or (c) OCaml 5 effect-handler-scoped dynamic binding for `current`/owner/cleanup. (a) is most faithful to "no TLS hazard" and most testable but touches every signature; (b) keeps signatures identical to the client and is the smallest diff but reintroduces a (benign, no-destructor) thread-local flavor; (c) is elegant but couples the core to effects-syntax. Leaning (a) for the core globals being module-internal with a `with_context` entry point. **Decision needed before the View/Runtime authors freeze their signatures.**
- **Subscriber set data structure:** `effect_node list` with `List.memq`/`List.filter` is O(n) per subscribe/unsubscribe. Fine for typical fan-out (a handful of effects per signal), but a signal read by a large `keyed_for` could degrade. Risk is low (each row is its own effect on its own signals); revisit with a `Hashtbl`-of-physical-identity only if profiling shows it.
- **Physical identity for dedup:** `List.memq`/`!=` rely on physical equality of `effect_node`, which is sound for heap records. Confirm js_of_ocaml preserves physical identity semantics for these records under all optimization levels (it does for boxed records; the risk is only if a future change makes `effect_node` unboxable — it can't be, it has mutable fields).
- **Glitch/diamond consistency:** like Rust rui (and Solid without batching), this core is *not* glitch-free — a diamond dependency can transiently observe an inconsistent intermediate during a `set` cascade, and the same effect can run more than once per logical update. rui explicitly deferred batching/scheduling to "P4 引擎 (batch/memo 短路/重入保护)". We should mirror that: ship the synchronous, un-batched core (behavior-identical to today), and leave a documented seam (`batch : (unit -> unit) -> unit` that defers `run_effect` into a dedup'd queue flushed at the end) for a later pass. **Out of scope for v1, but the `set` notify path should be written so a queue can be inserted without API change.**
- **Exception escaping a subscriber during `set`:** if one subscribed effect raises, the remaining subscribers won't run (the exception propagates out of `set`). Rust has the same exposure (a panicking effect aborts the cascade). We get `current` restored for free via `Fun.protect`, but the partial-cascade semantics should be documented; an optional "isolate each subscriber" mode is a possible later hardening, not a v1 requirement.
- **`Signal.peek` vs `untrack`:** we expose `peek` as sugar for `untrack (fun () -> get s)`. Confirm with the other authors that `peek` is wanted (the spine lists `get`/`set`/`update` but not `peek`); it is a pure convenience and can be dropped to stay minimal if the API surface must match the spine exactly.

## View & Rendering Engine

This section ports `crates/rui/src/view.rs` — the layer that sits between the reactive core (`Rui_reactive`) and the tri-backend DOM (`Rui_dom.S`). It is the layer that turns "a value you want to show" into "a built DOM node that updates in place," and that turns the `%view` markup's `{ … }` blocks into reactive boundaries.

### 1. What rui does today (self-contained recap)

rui has **no virtual DOM**. The DOM is built once and then mutated in place by effects; structure changes (conditionals, lists) re-run a thunk inside an `effect` and rebuild only the affected subtree. The view layer is four things:

1. **`View(pub u32)`** — an opaque handle to *one already-built node*, identified by an engine node id (`u32`). It is the return type of `view!{}`, of every component fn, and of `#[rui::page]`. `view.node()` hands the raw id to `dom::*`, and `impl From<View> for u32` lets `View` flow into `dom::append`/`dom::mount` (which take `impl Into<u32>`).

2. **`trait IntoView { type State; fn build(self) -> (u32, State); fn rebuild(self, &mut State); }`** — "render a value to a node, and later update that node *in place*." It is implemented for `View`, `()`, `Option<T>`, `Vec<T>`, and a macro-generated set of `Display`-ish scalars (`&str`, `String`, all ints/floats, `bool`, `char`). The split between `build` and `rebuild` is the whole point: text rebuilds with `dom::set_text` (no wrapper element, no node churn), subtrees rebuild by swapping content under a `rui-slot` anchor.

3. **`reactive_block::<V: IntoView, F: Fn() -> V>(f) -> u32`** — the runtime for `view!`'s `{ move || … }` block. It runs `f` inside an `effect` (so it subscribes to whatever signals `f` reads), `build`s a node the first time, and `rebuild`s in place every time a dependency changes. Each evaluation happens in a child `scope` so the inner effects built last round are disposed next round.

4. **`keyed_for(parent, list: Signal<Vec<T>>, key_of, build)`** — the keyed `<For key={…}>` reconciler. It appends rows *directly under the real parent* (no wrapper element, so `<tbody><tr>` stays valid), and on every change diffs by key: drop vanished keys (`remove_child` + dispose the row's scope), reuse a kept row whose item is unchanged (move it with `append`, which moves an already-in-DOM node — preserving focus/selection/animation), rebuild a row whose item changed, and build new keys.

Plus two small pieces: **`NodeRef(Rc<Cell<u32>>)`** (an imperative escape hatch — `ref={r}` writes the element id into it; `0` means "not yet mounted"), and **`Strategy` / `Page`** (`#[rui::page(ssr|csr|static, "/path")]` wraps the page fn into a `Page { key; strategy; render: Box<dyn FnOnce() -> View> }`; `route()`'s match is the path→Page table).

Crucially, **conditional rendering is native Rust `if`/`match`**, not `<Show>`/`<Switch>` tags. Because `view!{}` and components both return `View`, and because the `{ }` block dispatches *by return type* (`View` → mount/swap subtree; `&str`/number/`String` → in-place text; `()` → nothing; `Option<T>` → if-without-else; `Vec<T>` → inline list), you write `{ move || if c.get() > 0 { view!{..} } else { view!{..} } }` and it is reactive for free, with reactive *text* still going through in-place `set_text` (it does not degrade into a wrapper).

> Edge-cases the rui history records that this design must preserve (cited in §5): in-place reactive text must not regress to a wrapper node (gaps progress #2); `reactive_block`'s per-round child scope must dispose last round's inner effects (no ghost effects, view.rs:166–192); `keyed_for` must reuse the *same DOM node* on reorder so focus survives (view.rs:208–214, progress #4); the per-connection-thread SSR teardown TLS hazard (reactive.rs `dispose_effect` `try_with`, "踩坑+修复"); hydration must claim *both* elements and text nodes and must turn `clear` into a no-op during hydration (dom.rs, progress #5 Stage 2).

### 2. OCaml design

#### 2.1 Node identity is the backend's, not a `u32`

In Rust a "node" is a `u32` (an arena index on SSR, an id into a JS-side handle table on wasm). In OCaml we keep the *abstraction* but let each backend pick its own concrete representation, via the existing `Rui_dom.S` virtual library: `Rui_dom_client.node = Brr.El.t` (typed JS element; `Brr.Jv` handle, no FFI marshalling, no ptr/len), `Rui_dom_ssr.node = int` (arena id, like today). `Rui_view` is written once against `Rui_dom.S` and the implementation is selected per dune target. This is the idiomatic answer to Rust's `#[cfg(target_arch="wasm32")]`.

```ocaml
(* rui_dom.mli — the tri-backend surface (only the view-relevant subset) *)
module type S = sig
  type node
  val el          : string -> node
  val text        : string -> node
  val set_text    : node -> string -> unit
  val append      : node -> node -> unit        (* moves an in-DOM child (appendChild semantics) *)
  val remove_child: node -> node -> unit
  val clear       : node -> unit                (* no-op while hydrating *)
  val attr        : node -> string -> string -> unit
  (* hydration: claim SSR-rendered nodes in build order *)
  val set_hydrate : bool -> unit
  (* … on, on_click, set_value, mount, focus, … as in the NAMING surface … *)
end
```

#### 2.2 The `View.t` handle

```ocaml
(* rui_view.mli *)
type node                       (* = backend node, kept abstract here *)
type t                          (* a built view: a handle to one DOM node *)

val text : string -> t          (* build a text node (static) *)
val node : t -> node            (* hand the underlying node to Dom.* *)
```

`t` is abstract and wraps a single `node`. Unlike Rust's `View(pub u32)` it does **not** need `Clone`/`Copy` derivation or a `From<View> for u32` impl — in OCaml `t` is a boxed record on the GC heap and we expose `node : t -> node` directly. *Cleaner:* the whole "make `View` cheaply copyable, give it `Into<u32>` so it can feed `append`" dance (view.rs:57–72) evaporates.

#### 2.3 The `Into_view` protocol — first-class modules, not a trait

Rust's `IntoView` is a trait with an associated `type State` and two methods. The blocker for a faithful OCaml port is that the associated type differs per instance (`Vec`'s state is a `u32` anchor, scalars' state is the text node id), yet `reactive_block` must store it existentially. We model it as a **module type plus a first-class-module value**, which existentially packs `state`:

```ocaml
module type Into_view = sig
  type state
  val build   : unit -> node * state           (* first build: returns (node, update state) *)
  val rebuild : state -> unit                  (* in-place update of the existing node *)
end

type into_view = (module Into_view)            (* existential pack — hides [state] *)
```

The Rust instances become smart constructors that each *capture the value to render in the closure* (so `build`/`rebuild` take no argument — the value is closed over, exactly mirroring `self`):

```ocaml
(* of_text — Display-ish scalar. rebuild = in-place set_text, NO wrapper. *)
val of_text   : string -> into_view
(* of_view — a built subtree, mounted under a rui-slot anchor. *)
val of_view   : t -> into_view
(* of_unit — renders nothing (empty text node). *)
val of_unit   : into_view
(* of_option — Some → subtree under anchor, None → empty (if-without-else). *)
val of_option : t option -> into_view
(* of_list — inline list under an anchor (use keyed_for for reuse). *)
val of_list   : t list -> into_view
```

Concrete implementations, one-to-one with view.rs lines 83–160:

```ocaml
let of_text (s : string) : into_view =
  (module struct
    type state = Dom.node                       (* the text node *)
    let build () = let n = Dom.text s in (n, n)
    let rebuild n = Dom.set_text n s            (* ← in-place; no rebuild, no wrapper *)
  end)

let of_view (v : t) : into_view =
  (module struct
    type state = Dom.node                       (* the rui-slot anchor *)
    let build () =
      let anchor = Dom.el "rui-slot" in
      Dom.append anchor (node v); (anchor, anchor)
    let rebuild anchor =
      Dom.clear anchor; Dom.append anchor (node v)
  end)

let of_option (o : t option) : into_view =
  (module struct
    type state = Dom.node
    let build () =
      let anchor = Dom.el "rui-slot" in
      Option.iter (fun v -> Dom.append anchor (node v)) o;
      (anchor, anchor)
    let rebuild anchor =
      Dom.clear anchor;
      Option.iter (fun v -> Dom.append anchor (node v)) o
  end)
```

`of_list` mirrors `Vec<T>` (clear anchor, append each). `of_unit` builds an empty text node and `rebuild` is `()`.

> *Wash, not cleaner, but type-safe:* the first-class-module pack is more ceremony than a Rust trait at the *definition* site. The win is at the *use* site (`reactive_block`), where OCaml's existential is one `(module Into_view)` instead of Rust's `Box<dyn>` + the `V::State: 'static` bound gymnastics — and the value is captured in the closure rather than threaded as `self`, so `build`/`rebuild` are nullary, which reads better.

#### 2.4 `reactive_block` — the `{ move || … }` runtime

```ocaml
val reactive_block : (unit -> into_view) -> node
```

```ocaml
let reactive_block (f : unit -> into_view) : node =
  let slot : (Obj.t * Reactive.scope) option ref = ref None in   (* (state, child-scope) *)
  let out  : node option ref = ref None in
  ignore (Reactive.effect (fun () ->
    (* evaluate f in a CHILD scope so the inner effects it builds are owned by
       this round and disposed next round — verbatim port of view.rs:178–192 *)
    let (iv : into_view), sc = Reactive.scope (fun () -> f ()) in
    let module IV = (val iv : Into_view) in
    match !slot with
    | Some (state, old_sc) ->
        slot := Some (state, sc);
        Reactive.Scope.dispose old_sc;          (* tear down last round's inner effects *)
        IV.rebuild (Obj.obj state)              (* in-place: set_text / swap subtree *)
    | None ->
        let (n, st) = IV.build () in
        out := Some n;
        slot := Some (Obj.repr st, sc)));
  Option.get !out
```

The shape is identical to the Rust runtime: an `effect` re-evaluates the thunk (subscribing to read signals), `build`s once, `rebuild`s thereafter, and a fresh child `scope` per round disposes the previous round's inner effects. Two notes:

- **The existential `state` across rounds.** Because `into_view` hides `state`, but `build` and `rebuild` of *the same module value* agree on it, the stored state and the `rebuild` we call must be the *same* `IV`. There is a real subtlety: each round produces a *new* `iv` (the closure captured a new value), but its `state` type is the same as last round's (the structure of the branch is stable — a text block stays a text block). We reconcile this with one `Obj.repr`/`Obj.obj` at the boundary (the state is always a `node`, or a small record, owned entirely by this module — never inspected by callers), keeping the public API fully typed. *Alternative considered:* make `into_view` carry a GADT witness so no `Obj` is needed; rejected as over-engineering since `state` is internal and homogeneous within a branch. This is the **one** place OCaml is a wash-to-slightly-uglier than Rust's associated type.

- **No `Rc<RefCell<…>>`, no `Rc<Cell<u32>>`.** Rust needs `Rc<RefCell<Option<(State, Scope)>>>` and `Rc<Cell<u32>>` purely to share the cell with the effect closure (view.rs:173–174). OCaml uses plain `ref`s captured by the closure — GC owns them. *Cleaner.*

**Return-type dispatch — the OCaml way.** In Rust the `view!` macro inspects the `{ }` block's static return type and selects the right `IntoView` impl. The ppx (`ppx_rui`) does the same, but OCaml gives us a *variant-based* alternative that is more honest. The ppx wraps `{ e }` into `reactive_block (fun () -> <coerce e>)`, where `<coerce e>` is chosen by the *expression's type*, which ppxlib cannot see — so we instead emit a call to an overloaded family resolved by an open variant the ppx already knows from the syntax:

| `view!` block syntax | OCaml emitted | `Into_view` instance |
|---|---|---|
| `{ fun () -> some_view }` (a `view!{}` / component / `if/match` returning views) | `reactive_block (fun () -> of_view (e ()))` | `of_view` |
| `{ fun () -> "..." }` / numeric | `reactive_block (fun () -> of_text (string_of_… e))` | `of_text` |
| `{ fun () -> () }` | `reactive_block (fun () -> of_unit)` | `of_unit` |
| `{ fun () -> opt_view }` | `reactive_block (fun () -> of_option (e ()))` | `of_option` |
| `{ fun () -> view_list }` | `reactive_block (fun () -> of_list (e ()))` | `of_list` |

The ppx decides which wrapper to emit from a small set of syntactic cues + an optional type ascription the author can give (`{ (e : View.t) }`). When ambiguous (e.g. `if` whose branches are `view!{}`), it defaults to `of_view`. This is exactly rui's design intent ("`view!` 里的 `{ }` 块按返回类型分派") rendered through the ppx. *Where OCaml is genuinely cleaner:* conditional rendering. Rust's `<Show>`/`<Switch>` tags were kept only as sugar that also compiles to `reactive_block` (progress #2). In OCaml the same `reactive_block` is fed a *native* `if`/`match` whose branches are `View.t`, and the polymorphic-variant/match exhaustiveness checker guarantees you handled every case — no `<Match>` tag, no fallthrough bug. We recommend the ppx **not** ship `<Show>`/`<Switch>` at all; `match` is the idiom.

#### 2.5 Reactive interpolation in the markup (signal-valued child)

A `{ count }`-style interpolation where `count : int signal` desugars to a reactive text boundary:

```ocaml
(* view!{ <span>{count}</span> }  where count : int signal  *)
let span = Dom.el "span" in
Dom.append span (reactive_block (fun () -> of_text (string_of_int (Signal.get count))));
View.of_node span
```

`Signal.get count` *inside* the `reactive_block` thunk subscribes the boundary's effect to `count`; on change the effect re-runs and `of_text.rebuild` does `Dom.set_text` on the same text node. No wrapper element is introduced for a pure-text interpolation — this is the "responsive text does not degrade" property (progress #2, view.rs:143). Reactive *attributes* (`class={ fun () -> … }`) compile the same way but call `Dom.attr` instead of building a node, inside their own effect (mirrors progress #1 "闭包属性自动包 effect").

#### 2.6 `keyed_for` — keyed reconciliation

```ocaml
val keyed_for :
  parent:node ->
  'a list signal ->
  key:('a -> 'k) ->
  build:('a -> node) ->
  unit
```

```ocaml
type ('a, 'k) keyed_row = { key : 'k; node : node; item : 'a; scope : Reactive.scope }

let keyed_for ~parent items ~key ~build =
  let rows : ('a, 'k) keyed_row list ref = ref [] in
  ignore (Reactive.effect (fun () ->
    let items = Signal.get items in
    let new_keys = List.map key items in
    let old = !rows in
    (* ① drop rows whose key vanished: remove_child + dispose row scope (Scope.dispose
       runs on_cleanup then disposes the row's inner effects) *)
    let keep =
      List.filter (fun r ->
        if List.exists (fun k -> k = r.key) new_keys then true
        else (Dom.remove_child parent r.node; Reactive.Scope.dispose r.scope; false))
        old
    in
    let keep = ref keep in
    (* ② walk new order: reuse / rebuild / build, appending to the right position.
       append() moves an already-in-DOM node, so reordering keeps the SAME node. *)
    let next =
      List.map (fun item ->
        let k = key item in
        match List.partition (fun r -> r.key = k) !keep with
        | (r :: _), rest ->
            keep := rest;
            if r.item = item then (Dom.append parent r.node; r)   (* reuse: move only *)
            else begin                                            (* item changed: rebuild row *)
              Dom.remove_child parent r.node; Reactive.Scope.dispose r.scope;
              let n, sc = Reactive.scope (fun () -> build item) in
              Dom.append parent n; { key = k; node = n; item; scope = sc }
            end
        | [], _ ->                                                (* new key *)
            let n, sc = Reactive.scope (fun () -> build item) in
            Dom.append parent n; { key = k; node = n; item; scope = sc })
        items
    in
    (* ③ clear leftovers (e.g. duplicate keys) *)
    List.iter (fun r -> Dom.remove_child parent r.node; Reactive.Scope.dispose r.scope) !keep;
    rows := next));
```

This is a line-for-line port of view.rs:215–266, with the four key behaviors intact:

- **key vanished** → `remove_child` + dispose the row's scope (tears down per-row effects/cleanups). In Rust this happened implicitly via `KeyedRow`'s `Drop` running `Scope::drop`; OCaml has no `Drop`, so we call `Scope.dispose` **explicitly** at every drop site (the three branches above). This is *the* place GC-vs-RAII bites: Rust's `std::mem::take` + dropping the old `Vec` auto-disposed vanished rows; we must remember the explicit `dispose`. We mitigate by routing *all* row removal through one helper.
- **key kept, item equal** → `append` the existing node to its new position. `append`/`appendChild` *moves* a node already in the DOM, so the node identity is preserved → focus, text selection, in-flight CSS animation, and `<input>` caret survive a reorder. (This is the marquee correctness property, view.rs:208–214.)
- **key kept, item changed** → rebuild that one row (dispose old scope, remove old node, build new).
- **new key** → build a new row.

*Where OCaml is cleaner:* `T: Clone + PartialEq` and `K: PartialEq + Clone` bounds disappear — OCaml's structural `=` works on the captured values, and we never `clone` the item (we store it by value, GC-shared). *Where it's a wash:* the explicit `Scope.dispose` at each removal (Rust got it from `Drop`); and `List.partition`-based key lookup is O(n²) just like the Rust `position` scan — for large keyed lists we note a hashtable index as a future optimization (open question §5).

#### 2.7 `NodeRef`, `Strategy`, `Page`

```ocaml
type node_ref
val node_ref : unit -> node_ref
module Node_ref : sig
  val get : node_ref -> node option   (* None = not yet mounted (Rust's 0 sentinel) *)
  val set : node_ref -> node -> unit
end

type strategy = Ssr | Csr | Static
type page = { key : string; strategy : strategy; render : unit -> t }
module Page : sig
  val make : key:string -> strategy:strategy -> (unit -> t) -> page
end
```

`node_ref` is a plain `node option ref` — GC, no `Rc<Cell<u32>>`. The Rust `0 = unmounted` sentinel becomes a proper `None` (*cleaner, no magic value*). `render` is `unit -> t`, a one-shot thunk — no `Box<dyn FnOnce>` needed (OCaml closures are first-class). The ppx for `[@page "ssr"/..., "/path/:id"]` emits `Page.make ~key:(module path) ~strategy (fun () -> <body>)`, and `%router { … }` builds the `route : string -> page` table.

### 3. Feature → OCaml mechanism, with the cleaner/wash verdict

| rui feature (view.rs) | OCaml mechanism | Verdict |
|---|---|---|
| `View(pub u32)` handle, `Copy`, `From<View> for u32` | abstract `View.t` over backend `node`; `node : t -> node` | **Cleaner** — no `Copy`/`Into` boilerplate, no `u32` collision with `Display` (the "u32 撞 Display" pain, progress #2) |
| backend node = `u32` (arena id / JS table id) | dune virtual lib: `Rui_dom_client.node = Brr.El.t`, `Rui_dom_ssr.node = int` | **Cleaner** — typed `Brr.El.t` on client, no ptr/len FFI; `#[cfg]` → dune impl selection |
| `IntoView` trait + assoc `type State` | `module type Into_view` + `(module Into_view)` existential; smart ctors `of_text`/`of_view`/`of_option`/`of_list`/`of_unit` | **Wash** at def site (FCM ceremony); type-safe existential at use site |
| `reactive_block` (`{move||..}` runtime) | `reactive_block : (unit -> into_view) -> node`, effect + per-round child `scope` | **Wash** — one `Obj.repr`/`obj` for the homogeneous internal `state`; but no `Rc<RefCell<Cell>>` (plain `ref`) |
| return-type dispatch of `{ }` block | ppx picks `of_*` wrapper by syntactic cue + optional ascription; **native `match` for conditionals** | **Cleaner for conditionals** (exhaustiveness-checked `match`, no `<Show>/<Switch>/<Match>`); wash for the dispatch itself |
| `<Show>`/`<Switch>`/`<Match>` (sugar over `reactive_block`) | dropped — use native `if`/`match` returning `View.t` | **Cleaner** (less surface, compiler-checked) |
| in-place reactive *text* (`set_text`, no wrapper) | `of_text.rebuild` calls `Dom.set_text`; interpolation `{sig}` wraps a bare text boundary | **Parity** — preserved exactly (no degrade) |
| `keyed_for` reconciliation | same algorithm, explicit `Scope.dispose` at each removal | **Wash** — GC removes clone bounds but adds explicit dispose (no `Drop`) |
| focus/selection preserved on reorder | `Dom.append` = `appendChild` moves the *same* node | **Parity** — identical mechanism |
| `KeyedRow.scope` auto-dispose via `Drop` | explicit `Scope.dispose` calls | **Harder** — must not forget; centralized in one removal helper |
| `NodeRef(Rc<Cell<u32>>)`, `0` sentinel | `node option ref`, `None` sentinel | **Cleaner** |
| `Page { render: Box<dyn FnOnce()->View> }` | `{ render : unit -> t }` | **Cleaner** — first-class closure |
| child-scope disposal of dynamic subtrees | `Reactive.scope` / `Scope.dispose` (ported verbatim — correctness, not memory) | **Parity** — mandatory, kept identical |

### 4. Real edge-cases rui solved, and how this design handles each

1. **Reactive text must not degrade into a wrapper element** (progress #2; view.rs:143–160). `of_text.rebuild` calls `Dom.set_text` on the *same* text node, and `{sig}` interpolation compiles to a bare text boundary with no `rui-slot`. Handled identically. (`of_view`, by contrast, *does* use a `rui-slot` anchor — that asymmetry is intentional and preserved.)

2. **`reactive_block`'s per-round child scope must dispose the previous round's inner effects** (view.rs:166–192). Without it, a branch that built effects (e.g. a nested `view!{}`) leaks "ghost effects" that keep firing after the branch is gone. Our `reactive_block` runs `f` in a fresh `Reactive.scope` each round and calls `Scope.dispose old_sc` before `rebuild`. Verbatim.

3. **Dynamic-dependency cleanup in effects** (reactive.rs:131–151; ported by the Reactive author). When a conditional branch changes which signals it reads, the effect must un-subscribe from the stale signals before re-running, or an old signal will wrongly retrigger it. The view layer relies on this being correct in `Rui_reactive` (it is mandatory, per principle P2) — `reactive_block` and `keyed_for` both depend on it.

4. **Focus/selection survive a keyed reorder** (view.rs:208–214, progress #4). `keyed_for` reuses the same DOM node and *moves* it with `append` (`appendChild` semantics), rather than rebuilding. On `Brr` this is `Brr.El.append_child` / `El.insert_*`, which moves the live element. Handled identically; the `~build`-vs-reuse decision turns on structural `item = item'`.

5. **`<For>` must not introduce a wrapper element** (progress #4). Rows are appended *directly* under the real `parent`, so `<tbody>` directly contains `<tr>` and the HTML stays valid. `keyed_for ~parent` appends to `parent` with no `rui-slot`. Handled identically.

6. **SSR per-connection-thread teardown** (reactive.rs `dispose_effect` `try_with`; "踩坑+修复"). In rui, each SSR request is a thread; when the thread's thread-locals are destroyed, leftover `Scope`/effect closures get dropped and re-enter `dispose_effect`, which could touch an already-destroying TLS and abort the process. In OCaml the SSR backend is **single shared runtime, no per-request thread-local arena teardown** (the `Rui_dom_ssr` arena is `reset`/`take_html` between renders, run on one thread or pooled), so this class of "access TLS during destruction" bug *does not exist* — there is no thread-local destructor racing the GC. *Cleaner by construction.* We still call `Scope.dispose` after each SSR render to run `on_cleanup` (which is `no-op`-ish on SSR for DOM, but tears down any subscriptions), via `Rui_dom_ssr.reset`.

7. **Hydration must claim both element and text nodes, and `clear` must be a no-op while hydrating** (dom.rs el/text/clear, progress #5 Stage 2). The OCaml `Rui_dom_client` carries the same `set_hydrate` flag and a build-order counter: during hydration `el`/`text` *claim* the SSR node (`Brr.El` walk by `data-h` for elements and the `<!--h:N-->` marker's following text node for text) instead of creating, and `clear`/`append`/`attr` are no-ops (the SSR children must be claimed, not wiped — otherwise `keyed_for`/`of_list` would erase the first paint). `reactive_block`'s first `build` therefore *claims* rather than *creates*, keeping the hydration build order in lockstep with SSR's emission order. The `View` layer is unchanged across hydrate/CSR — only the backend differs, which the virtual library already isolates. *Wash* — same flag-and-counter scheme, but it lives entirely in `Rui_dom_client`, not leaking into `Rui_view`.

8. **`of_option`'s if-without-else, `of_unit`'s render-nothing** (view.rs:96–122). Ported exactly: `of_option None` leaves the `rui-slot` empty (no else branch), `of_unit` builds an empty text node whose `rebuild` is `()`. Native `t option` makes this trivially clean (no `Option<T: IntoView>` trait bound needed).

### 5. Open questions / risks for this subsystem

- **The `state` existential boundary.** `reactive_block` stores `Into_view`'s hidden `state` across rounds and feeds it back to `rebuild`. The clean-but-internal solution uses one `Obj.repr`/`Obj.obj` (state is always a `node` or small record, never user-visible). The fully type-safe alternative is a GADT witness threaded through `into_view`; it removes `Obj` at the cost of more ppx machinery. **Decision needed** before locking the `Into_view` signature. Risk is low (the unsafety is contained to homogeneous internal state), but it is the one spot that isn't provably type-safe.

- **ppx return-type dispatch without types.** ppxlib runs before type inference, so the `{ }`-block dispatch (`of_text` vs `of_view` vs …) must be decided syntactically or via an author ascription `{ (e : View.t) }`. Misclassification (e.g. an `if` whose branches are strings, mistaken for views) would compile-error rather than misrender, but the error message could be poor. **Mitigation:** default to `of_view`, document the ascription escape hatch, and have `of_text` accept anything with a `to_string` via a small `Show`-like family. Needs a clear spec of the cues.

- **`keyed_for` key lookup is O(n²)** (inherited from rui's `position`/`partition` scan). Fine for the example-scale lists; a `Hashtbl` keyed index would make it O(n) for large tables. Deferred, but flagged — the signature (`key:('a -> 'k)`) already supports it.

- **Explicit disposal discipline (no `Drop`).** Every place that removes a keyed row or swaps a dynamic subtree must call `Scope.dispose`. We centralize this, but it is a standing footgun versus Rust's automatic `Drop`. A lint/test that asserts "every `remove_child` of a row is paired with a `Scope.dispose`" would de-risk it.

- **`reactive_block` returning a node before the effect's first run completes.** The Rust version reads `node.get()` *after* `effect` has run once (effects run immediately on registration). OCaml `Reactive.effect` must likewise run synchronously on creation so `Option.get !out` is populated — this is a hard dependency on the Reactive author's `effect` semantics (run-once-on-register). If batching is ever added (P4 in the roadmap), the "first build is synchronous" invariant must be preserved or `reactive_block` will `Option.get None`.

- **Hydration mismatch detection.** Today (and in this port) hydration trusts that the client build order exactly matches the SSR emission order; a divergence silently claims the wrong node. A debug-mode assertion (tag/`data-h` cross-check during `claim`) is worth adding but is out of scope for the View layer (belongs to `Rui_dom_client`).

Relevant source: `crates/rui/src/view.rs` (the whole subsystem), `crates/rui/src/reactive.rs` (the `scope`/`effect`/`dispose_effect` the view layer leans on), `crates/rui/src/dom.rs` (the `el`/`text`/`append`/`clear`/hydration primitives `Into_view` calls).

## The DSL: jsx-ppx, page, component, router macros

This section ports rui's whole compile-time surface — the `view!` JSX macro, `#[rui::component]`, `#[rui::page("/p/:id")]`, `router! { … }`, and the GraphQL-schema-as-types macros — to a single ppxlib rewriter, the `ppx_rui` driver shipped as package `rui.ppx`. The goal is a **faithful translation**: every parse rule, every codegen shape, and every edge-case the progress log records (events, reactive attrs, `bind:value`, keyed `<For>`, typed route params, nested groups, the schema-as-types projection) must reappear. Where OCaml's type system, GC, or js_of_ocaml make a feature cleaner, we say so; where the `(fun () -> …)` reactive boundary or the absence of a global type registry makes it a wash, we say that too.

### 0. What rui does today (so this is self-contained)

`crates/rui-macros/src/lib.rs` (~1800 LOC) is one proc-macro crate exporting:

- **`view!`** — a hand-written JSX parser (`syn::Parse`) over a token stream. It recognizes `Node::{El, For, Show, Switch, Text, Block}` and `Attr::{Static, Dyn, Event, Bind, Ref}`. Lowercase tags become DOM elements (`rui::dom::el`), capitalized tags become component calls (`crate::view::components::<snake>(<Pascal>Props { … })`). `{ expr }` blocks dispatch on whether the block ends in a closure (`is_reactive`): a closure → `reactive_block` (re-runs in an `effect`), otherwise → built once via `IntoView`. `<For>` lowers to `keyed_for` (with `key=`) or a clear-and-rebuild `effect` (without). `<Show>`/`<Switch>`+`<Match>` lower to `reactive_block` returning a `View`.
- **`#[rui::component]`** — rewrites `fn card(title: String, children: View) -> View { BODY }` into a `CardProps` record + `fn card(__props: CardProps) { let CardProps { … } = __props; BODY }`.
- **`#[rui::page(ssr|csr|static, "/todo/:id")]`** — rewrites `fn view(id: Signal<String>) -> View { BODY }` into `const __RUI_PATTERN`, `const __RUI_STRATEGY`, and `fn view() -> Page` that wires each `:id` segment to `param_as(idx)` by **looking up the segment index from the pattern string**, then `Page::new(module_path!(), strategy, move || { bindings; BODY })`.
- **`router! { layout=…, pages::index, group("/dash", layout=…){…}, fallback=… }`** — emits `pub fn route(path) -> Page` as an ordered `if matches(P::__RUI_PATTERN, path) { P::view() } else …` chain, with groups compiled to a single same-key `Page` whose outlet is a `reactive_block` selecting the leaf by path.
- **GraphQL macros** — `gql_schema!`, `query!`, `mutation!`, `subscription!`, `resource!`, `paginated!`, `fragment!`, `gql_fields!`, `#[derive(GqlObject)]`, `#[gql_root(query)]`. These synthesize exact-fit result structs from a selection set, projecting field types through `Field<gqlf::name>` associated-type witnesses (schema-as-types).

### 1. ppxlib mechanics vs. Rust proc-macros

The Rust design and the OCaml design share the same *shape* — parse a mini-DSL, emit calls into a runtime library — but the host machinery differs in four ways that matter:

| Concern | Rust proc-macro | OCaml ppxlib |
|---|---|---|
| Input | raw `TokenStream`; `view!` hand-rolls a `syn::Parse` over `<`, `>`, idents | a **parsed OCaml AST** (`Parsetree.expression`/`structure_item`). We do **not** get to re-tokenize freely. |
| Trigger | `view! { … }`, `#[attr]`, `#[derive]` | extension nodes `[%view …]`, attributes `[@page …]`/`[@component]`, deriver `[@@deriving gql]` |
| Hygiene | manual; rui uses fixed names `__n`, `__c`, `__h`, `__keys` and *relies* on them not colliding (it even renamed `PATTERN → __RUI_PATTERN` to dodge user collisions, see progress 12) | ppxlib offers `gen_symbol ()` for genuinely fresh names + `Ppxlib.Ast_builder` with location tracking. Hygiene is **easier and safer**. |
| Registration | one `#[proc_macro]` per macro | one driver `ppx_rui` registers many `Extension`/`Deriving`/context-free rules; `Driver.register_transformation` |
| Passes / order | the compiler expands macros outside-in, lazily | ppxlib runs **context-free rules in one fused bottom-up pass**, then whole-AST transformations. Order is explicit and we control it. |

**The big structural difference — and the central design choice for `%view`.** Rust's `view!` re-lexes its body: `<div class="x">` is *not* valid Rust, so syn reads raw `<`, `Ident`, `LitStr` tokens. OCaml's ppx receives an **already-parsed expression**, so `[%view <div>…</div>]` would be a *syntax error before the ppx ever runs*. We therefore choose the idiomatic ppxlib path: the JSX markup lives inside the extension payload as **OCaml-parseable syntax**, and the ppx rewrites that AST. Two viable encodings, we pick (B):

- **(A) String payload** — `[%view {jsx|<div class="x">{count}</div>|jsx}]`. The ppx lexes the string itself (closest to Rust, full JSX freedom), but we lose editor support and OCaml-level location precision inside the string.
- **(B) Applicative/list AST sugar (chosen)** — markup is written as ordinary OCaml that *parses* but is *rewritten*:

  ```ocaml
  let%view counter count =
    div ~cls:"card" [
      h1 [ text "Counter" ];
      button ~on_click:(fun () -> Signal.update count succ) [ text "+" ];
      span [ reactive (fun () -> Signal.get count) ];   (* {move || ...} *)
    ]
  ```

  Here `div`, `button`, `text`, `reactive`, `for_`, `show`, `switch`, `match_` are **not real functions** — they are markers the `%view` ppx pattern-matches on and lowers. This keeps everything inside the real OCaml grammar (so Merlin, indentation, and locations all work), while the ppx still performs the same AST surgery Rust does. The cost: the marker vocabulary is fixed by the ppx, exactly as rui's tag set is fixed by its parser. This is **a wash, not a clear win** over Rust — we trade "lex arbitrary `<tag>`" for "must parse as OCaml" — but it is the correct ppxlib idiom and it makes hygiene + locations strictly better.

> Throughout, `[%view e]` denotes the expression extension; the lowering targets the runtime names fixed by the spine: `Rui_view.text`, `Rui_view.reactive_block`, `Rui_view.keyed_for`, `Rui_dom.S` ops via the selected backend, `Rui_reactive.effect`, etc.

### 2. `%view`: the JSX ppx

#### 2.1 The lowered runtime surface (`.mli` sketches it targets)

```ocaml
(* Rui_view *)
type t                                   (* a built view: handle to a node *)
type node                                (* backend node id (Brr.El.t / arena id), abstract *)
val text  : string -> t
val node  : t -> node

module type Into_view = sig
  type state
  val build   : unit -> node * state     (* ports IntoView::build *)
  val rebuild : state -> unit            (* ports IntoView::rebuild *)
end

(* {move || expr} runtime: thunk re-run in an effect, dispatch on the value's Into_view *)
val reactive_block : (unit -> (module Into_view)) -> node

(* <For key=…> : keyed reconciliation, appends straight into parent (legal <tbody><tr>) *)
val keyed_for :
  parent:node -> 'a list Rui_reactive.signal ->
  key:('a -> 'k) -> build:('a -> node) -> unit

(* ref={r} support *)
type node_ref
val node_ref : unit -> node_ref
module Node_ref : sig val get : node_ref -> node option  val set : node_ref -> node -> unit end
```

The DOM ops the ppx emits come from the tri-backend signature `Rui_dom.S` (`el`, `text`, `set_text`, `append`, `attr`, `set_value`, `clear`, `on`, `on_click`, …), selected per target by the dune virtual library — the OCaml answer to rui's `#[cfg(target_arch="wasm32")]` split between `dom_client` and `dom_ssr`.

#### 2.2 Feature-by-feature lowering

For each element the ppx emits the same imperative "create node, set attrs, append children" block rui does. Rust uses a `{ let __n = el(tag); …; __n }` block with the magic name `__n`; OCaml uses a `let __n = … in` with a `gen_symbol ()`-fresh name, removing the collision hazard entirely.

**Elements + static/dynamic attrs.**

```ocaml
(* [%view div ~cls:"x" ~style:(some_expr) [ … ]] *)
let n = Dom.el "div" in
Dom.attr n "class" "x";                              (* Attr::Static *)
(let v = some_expr in Dom.attr n "style" v);         (* Attr::Dyn, non-reactive: set once *)
…children…; n
```

A dynamic attr whose argument is a **thunk** (`~cls:(fun () -> if active then "on" else "")`) is the reactive-attr case from progress 1; it wraps in an effect exactly like rui:

```ocaml
let af = (fun () -> …) in
Rui_reactive.effect (fun () -> Dom.attr n "class" (Printf.sprintf "%s" (af ())))
```

**Events `on:` → `~on_<event>`.** rui parses `on:<event>={...}` and wraps a zero-arg closure into `Fn(&str)` ignoring the payload, except it threads the payload for inputs. We expose two flavors so the *payload* (the C-root cause from the gaps audit — "only on:click worked, payload dropped") is first-class:

```ocaml
button ~on_click:(fun () -> …)              (* zero-arg, payload ignored *)
input  ~on_input:(fun (v : string) -> …)    (* payload threaded: target.value *)
input  ~on:("keydown", fun (v : string) -> …)  (* generic event name, no longer hardcoded *)
```

lowers to `Dom.on n "click" (fun _ -> h ())` and `Dom.on n "input" (fun v -> h v)`. Generic event names are passed through verbatim — the gaps fix "事件名透传" is structural here, since the event name is just a string argument.

**`bind:value` two-way binding.** rui restricts to `bind:value` over `Signal<String>` and emits a `signal→set_value` effect plus an `input→signal.set` listener. OCaml keeps the same two-line lowering but makes the type a *compile error at the binding site* rather than a `compile_error!` string:

```ocaml
input ~bind_value:(s : string Rui_reactive.signal)
(* lowers to: *)
(let s = s in Rui_reactive.effect (fun () -> Dom.set_value n (Signal.get s)));
(let s = s in Dom.on n "input" (fun v -> Signal.set s v))
```

Because `~bind_value` is a typed labelled argument of type `string signal`, passing an `int signal` is a normal type error with a real location — strictly better than emitting `compile_error!("only Signal<String>")`. A future `~bind_value_int`/`~bind_checked` can carry a parser, mirroring the deferred `bind:checked` gap.

**`ref={r}` → `~ref`.** rui special-cases `ref` (a keyword) and emits `{ let __rf = #h; __rf.set(__n); }`. In OCaml `ref` is also taken (it's the stdlib mutable cell), so the label is `~node_ref`:

```ocaml
div ~node_ref:r [ … ]   (*  →  Rui_view.Node_ref.set r n  *)
```

The progress-15 edge-case "ref must work during hydration" is preserved: `Node_ref.set` records the **claimed** node id (the id `claim_element` returned), not a freshly created one, because hydration is a property of `Dom.el`/`Dom.text` under `set_hydrate`, not of the ref.

**Text and `{ }` blocks (the `IntoView` dispatch).** This is the heart of progress 2. rui's `{ expr }` dispatches on the *return type*: `View` → mount/replace subtree, `&str`/number → in-place `set_text`, `()` → empty, `Option<T>` → no-else conditional, `Vec<T>` → inline list. In OCaml we cannot dispatch on a *return type* at a macro-syntactic level the way Rust does with traits, so we model `Into_view` as a **first-class module / GADT of cases** and let the ppx pick the constructor from syntactic shape, falling back to a typed `Into_view.t`:

```ocaml
(* Rui_view.Into_view instances *)
val of_text   : string -> (module Into_view)
val of_view   : t      -> (module Into_view)
val of_option : (module Into_view) option -> (module Into_view)
val of_list   : (module Into_view) list   -> (module Into_view)
val of_unit   : unit   -> (module Into_view)
```

- A bare string-typed block → `of_text`, emitting `Dom.set_text` on rebuild (no wrapping element — matching rui's "不退化、无包裹").
- A block ending in a `[%view …]` → `of_view`.
- The ppx emits `reactive_block (fun () -> <Into_view of the thunk body>)` when the block is a thunk (`reactive (fun () -> …)`), else builds once via the chosen `Into_view.build`.

**This is where OCaml is *cleaner than Rust*:** rui's progress-2 note records that the `View(pub u32)` newtype "clashed with `Display`" — a `u32` handle that also wanted to be text-formattable created ambiguity that drove the whole `IntoView` redesign and the `<Show>` tag. In OCaml `Rui_view.t` is **abstract** and disjoint from `string`/`int`, and conditionals are *native expressions* of type `(module Into_view)`, so the "u32 collides with Display" hazard simply cannot arise. Variants + first-class modules do for free what rui spent a refactor on.

**Conditionals — `Show`/`Switch`/`Match` AND native `if`/`match`.** Progress 2 records that after `IntoView`, rui made conditional rendering "regress to native Rust `if/else/match`" inside `{ move || … }`, while *keeping* `<Show>/<Switch>` compiling to `reactive_block`. We mirror both:

- Native: a `reactive` thunk may contain an ordinary OCaml `if … then [%view …] else [%view …]` or `match … with …`. This is the recommended path and is **strictly cleaner in OCaml** — `match` is exhaustive, variant-driven, and needs no `<Match>` tag.
- The `show`/`switch`/`match_` markers remain for parity:

  ```ocaml
  show ~when_:(fun () -> Signal.get n > 0) ~fallback:(fun () -> [%view text "none"]) [ … ]
  switch [ match_ ~when_:(fun () -> a ()) [ … ]; match_ ~when_:(fun () -> b ()) [ … ] ]
  ```

  Both lower to `reactive_block` returning a single `node`. rui requires the `when=`/`fallback=` blocks to *be closures* (it checks `is_reactive` and emits `compile_error!` otherwise — "Show 的 when 需要闭包"). In OCaml `~when_` is typed `unit -> bool`, so a non-thunk is a plain type error — the `compile_error!` strings disappear. `<Switch>` with no truthy arm renders `text ""`, ported verbatim.

**The single-node-vs-fragment branch rule (`emit_branch`).** rui's `emit_branch` is subtle: a branch with exactly one child (that isn't a `<For>`) emits that node directly; otherwise it wraps children in a `rui-frag` container (`display:contents`) so `dyn_node`/`reactive_block` always get **one** node id. We port this exactly:

```ocaml
let emit_branch children =
  match children with
  | [ one ] when not (is_for one) -> gen_node one
  | _ -> [%expr let n = Dom.el "rui-frag" in <emit_children>; n]
```

The SSR backend must inject the same `rui-slot, rui-frag { display:contents }` CSS into `<head>` (progress 1) so fragments and reactive slots don't disturb layout.

**`<For>` → `for_`.** rui dispatches on the presence of `key`:

```ocaml
(* keyed: ports keyed_for verbatim *)
ul [ for_ ~each:rows ~key:(fun r -> r.id) ~item:(fun r -> [%view li [ text r.title ]]) ]
(* non-keyed: clear-parent + rebuild effect, semantics preserved *)
ul [ for_ ~each:rows ~item:(fun r -> [%view li [ text r.title ]]) ]
```

The keyed form lowers to `Rui_view.keyed_for ~parent:n rows ~key ~build`, appending directly into the parent (legal `<tbody><tr>`), with the reconciliation contract from progress 4: key gone → `remove_child` + dispose that row's scope; key present and item `=` → reuse node (preserve focus); item changed → rebuild that row; new key → build; reorder via `append` (which moves an existing node). The non-keyed form keeps the clear-and-rebuild `effect` that rui deliberately left unchanged. Note: `for_` outside an element parent is a compile error in rui ("`<For>` 只能作为元素的子节点") — the ppx reproduces this by only recognizing `for_` inside a children list.

**Components — capitalized tag → named props record.** rui: `<Card title=.. sub={x}>kids</Card>` → `components::card(CardProps { title: "..".to_string(), sub: x, children: View(...) })`. In the OCaml marker grammar we distinguish components by an explicit form so the ppx need not guess casing:

```ocaml
component View.Components.card ~title:"Stocks" ~sub:x [ child; child ]
(* lowers to: *)
Rui_view.node
  (View.Components.card { title = "Stocks"; sub = x; children = Rui_view.of_view (emit_branch [child;child]) })
```

The progress-7 rules carry over: static attrs become strings, dynamic attrs become arbitrary typed expressions, missing fields are a record-construction type error (rui's "missing field" error becomes OCaml's "some record fields are undefined"), and `on:`/`bind:`/`ref` on a component is rejected — in OCaml simply because `card`'s props record has no such labels, again replacing a `compile_error!` string with a structural type error. **Cleaner than Rust here:** record field-by-name with the typechecker is exactly what rui hand-rolled with `CardProps { … }`, but we get it from the language.

#### 2.3 `%view` ppx implementation sketch (ppxlib)

```ocaml
open Ppxlib
let expand ~ctxt (e : expression) : expression =
  let loc = Expansion_context.Extension.extension_point_loc ctxt in
  let module B = Ast_builder.Make (struct let loc = loc end) in
  gen_node ~loc e               (* recursive, mirrors rui::gen_node *)

let view_ext =
  Extension.V3.declare "view" Extension.Context.expression
    Ast_pattern.(single_expr_payload __) expand

let () = Driver.register_transformation "rui.view"
    ~rules:[ Context_free.Rule.extension view_ext ]
```

`gen_node` matches on the marker application (`Pexp_apply (ident "div", args)`), splits `args` into attrs (labelled) and children (the final list), and recurses. Fresh binders come from `gen_symbol ()`; every emitted subexpression carries `~loc` from the source node so errors point at the user's markup — the location fidelity Rust gets from `Span` but which we get *per-AST-node for free*.

### 3. `[@page]`: typed route params wired transparently

rui's `#[rui::page(strategy, "/todo/:id")]` does three things: declares `__RUI_PATTERN`/`__RUI_STRATEGY`, rewrites the body into a `fn() -> Page` whose key is `module_path!()`, and binds each `:seg` to `param_as(idx)` by computing the segment index from the pattern. The OCaml form is an attribute on a `let`:

```ocaml
let%page[@page "ssr", "/todo/:id"] view (id : string Rui_reactive.signal) =
  let detail = [%resource detail (id = Signal.get id) { id title done_ }] in
  [%view div [ … ]]
```

Lowering (using the spine names `Rui_view.page`, `Page.make`, `Rui_runtime.param_as`):

```ocaml
let __rui_pattern_view  = "/todo/:id"
let __rui_strategy_view = Rui_view.Ssr
let view () : Rui_view.page =
  Page.make ~key:"App.View.Pages.Detail.view"      (* module-path-derived identity *)
    ~strategy:Rui_view.Ssr
    (fun () ->
       let (id : string Rui_reactive.signal) =
         Rui_runtime.param_as (module String_parser) 1   (* idx from ":id" position *)
       in
       [%view div [ … ]])
```

**Edge-cases ported (all from progress 11/12 + its adversarial review):**

- **Segment-index lookup.** The ppx parses the pattern, filters empty segments, and finds the position of `:id` (0-based) — identical to rui's `seg_index`. Each param `id : T signal` becomes `let id = param_as (module P_T) idx`.
- **Typed params without `FromStr + Default`.** rui's `param_as::<T: FromStr + Default + Clone>` cannot be expressed as an OCaml type bound. Per the spine, we pass a **first-class parser module**: `param_as : (module Parsable with type t = 'a) -> int -> 'a signal`. The ppx selects the parser module from the annotated type (`string → String_parser`, `int → Int_parser`, …) via a small built-in table, and authors can supply their own. This is *more explicit but arguably cleaner* — the failure mode is a missing parser-module instance, not a silent `Default::default()`.
- **Silent-default fallback (known limitation, ported honestly).** rui's `param_as` silently falls back to `Default` on parse failure. We keep that default behavior for parity, but because we pass an explicit parser module we can *also* offer `param_opt : … -> 'a option signal` — the "proper fix = Option化" the gaps log marked deferred. The doc should ship both and recommend `param_opt` for new code.
- **Reverse validation.** rui errors if a `:seg` in the pattern has no matching parameter, and if a parameter isn't in the pattern. The ppx reproduces both checks and raises `Location.raise_errorf ~loc` pointing at the offending parameter or pattern — better than rui's `name.span()` because the param's own location is available.
- **Rejected shapes.** rui rejects generics/where-clauses, `self` receivers, and non-identifier params (tuple/`_`/destructure). The OCaml `let%page` similarly rejects anything but `let view (p1 : ..) (p2 : ..) = …`: no type variables in the signature it can't place, no labelled/optional args, only simple value identifiers. Each is a `Location.raise_errorf`.
- **`__RUI_PATTERN`/`__RUI_STRATEGY` naming.** rui renamed these to a `__RUI_` prefix to avoid colliding with user items (progress 12 fix). We do the same, but additionally use `gen_symbol`-style suffixing (`__rui_pattern_<name>`) so two pages in one module can't collide.
- **Strategy.** `Rui_view.strategy = Ssr | Csr | Static` — a closed variant replaces rui's `Strategy` enum and its `attr.to_string()` keyword-matching hack (rui had to dodge `static` being a Rust keyword; OCaml just reads the string literal `"static"` from the attribute payload).
- **Query params are a *separate* line.** Per progress 13's "机制对称、声明分离": path params are structural (in the pattern + signature), query params are read on demand inside the body via `Rui_runtime.query_param "q"` / `query_param_as`. `[@page]` touches only path; the ppx does nothing with query strings — exactly rui's split of `PATH` vs `QUERY` signals.

### 4. `[@component]`: props record + children slot

rui rewrites `fn card(title: String, children: View) -> View { BODY }` into `pub struct CardProps { … }` + a function taking `__props` and destructuring it. The OCaml attribute does the same, generating a record type and a function over it:

```ocaml
let%component card ~title ~(sub : string) ~children =
  [%view section [ h2 [ text title ]; div [ Rui_view.node children ] ]]
```

lowers to:

```ocaml
type card_props = { title : string; sub : string; children : Rui_view.t }
let card (props : card_props) =
  let { title; sub; children } = props in
  [%view section [ … ]]
```

**Details and edge-cases:**

- **Props naming.** rui builds `<Pascal(fn)>Props`. We use `<snake>_props` (idiomatic OCaml type naming); the `%view` component lowering (§2.2) references it. Since OCaml records are nominal and module-scoped, the `View.Components` convention from the spine resolves `component View.Components.card` to `View.Components.card_props`.
- **Children slot.** A `~children` parameter (typed `Rui_view.t`) is the container/layout slot. The `%view` caller supplies it via `emit_branch`. Reproduces rui's `children: View(...)` exactly.
- **Reactive props.** Progress 8 records that components receive `Signal`/closure props (the "反应式 props" that let it delete `total2/active2` clones). In OCaml this is automatic — a prop typed `int signal` or `unit -> unit` is just a record field; GC means no `.clone()` ceremony at all. **Strictly cleaner than Rust:** every `.clone()` rui sprinkles to share a signal into a component vanishes.
- **Zero-arg components.** rui's progress 15 notes `#[component]` must generate an *empty* `Props` so `<Uptime/>` works. We special-case the no-label form to `type uptime_props = unit` and a `let uptime () = …`, so `component View.Components.uptime []` calls `uptime ()`.
- **All props required (v1 limitation, ported).** rui's record-literal call site makes every prop mandatory; optional/default props need a builder it didn't build. OCaml records have the same property — *but* OCaml's **optional labelled arguments** give us a clean upgrade path the Rust design lacked: `let%component card ?(sub="") ~title ~children` can lower to a function with an optional arg instead of a bare record, eliminating the limitation. We note this as the recommended OCaml-specific improvement over rui.
- **Rejecting events/bind/ref on components.** As in §2.2, structural: the props record has no such fields.

### 5. `%router`: route table + nested groups

rui's `router! { … }` emits `pub fn route(path) -> Page`. We mirror it as a structure-item extension generating `let route : Rui_runtime.route`:

```ocaml
let%router route =
  layout View.Layout.shell;
  pages [ Pages.Index.view; Pages.Detail.view ];
  group "/dash" ~layout:View.Layout.dash_shell [ Pages.Overview.view; Pages.Settings.view ];
  fallback not_found
```

generates:

```ocaml
let route (path : string) : Rui_view.page =
  let inner =
    if Rui_runtime.matches ~pattern:Pages.Index.__rui_pattern_view ~path then Pages.Index.view ()
    else if Rui_runtime.matches ~pattern:Pages.Detail.__rui_pattern_view ~path then Pages.Detail.view ()
    (* group: single same-key Page whose outlet picks the leaf by current path *)
    else if (Rui_runtime.matches ~pattern:("/dash" ^ Pages.Overview.__rui_pattern_view) ~path)
         || (Rui_runtime.matches ~pattern:("/dash" ^ Pages.Settings.__rui_pattern_view) ~path)
    then begin
      let gp = path in
      let strat =                     (* group strategy = strategy of the path-matched leaf *)
        if Rui_runtime.matches ~pattern:("/dash" ^ Pages.Overview.__rui_pattern_view) ~path
        then Pages.Overview.__rui_strategy_view else Pages.Settings.__rui_strategy_view in
      Page.make ~key:"group:/dash" ~strategy:strat (fun () ->
        View.Layout.dash_shell gp
          (Rui_view.node (Rui_view.reactive_block (fun () ->
             let lp = Signal.get (Rui_runtime.path ()) in
             if Rui_runtime.matches ~pattern:("/dash" ^ Pages.Overview.__rui_pattern_view) ~path:lp
             then (Pages.Overview.view ()).render ()
             else if Rui_runtime.matches ~pattern:("/dash" ^ Pages.Settings.__rui_pattern_view) ~path:lp
             then (Pages.Settings.view ()).render ()
             else of_view (Rui_view.text "")))))
    end
    else Page.make ~key:"not_found" ~strategy:Rui_view.Ssr not_found
  in
  match layout with
  | Some l -> { inner with render = (fun () -> l path (inner.render ())) }  (* preserve key/strategy *)
  | None -> inner
```

**Every router edge-case from progress 12/16 is ported:**

- **Ordered matching, first hit wins.** The `if … else if …` chain folds in declaration order, so literal routes must precede `:param` routes — same constraint, same note in docs.
- **Groups are one same-key `Page`** keyed `"group:<prefix>"`; same-key navigation does not rebuild the shell. The outlet is a `reactive_block` subscribing to `path()`. Ported verbatim, including: group strategy = the leaf the *current* path hits; empty group is a ppx error (`Location.raise_errorf` — rui's progress-16 fix); a call-form page item (`make_page ()`) is rejected with a clear diagnostic ("page entries must be page paths; use `group …`").
- **Global `layout` preserves the hit's key/strategy** and only wraps `render` — the `{ inner with render = … }` functional update is *cleaner than rui's* manual `Page { key; strategy; render: Box::new(…) }` reconstruction (no boxing, immutable record update).
- **`route` type.** `type route = string -> Rui_view.page` (spine). `let%router` produces a value of that type; `Rui_client`/`Rui_server` consume it instead of rui's `App.route` field and `router.js`.
- **Known limitations carried over:** group-member pages can't use their own `:param` (prefix offsets the absolute index); the global shell still rebuilds across cross-key navigation. Both are documented, not silently changed.

**Where OCaml is a wash:** like Rust proc-macros, ppxlib has **no compile-time global registry** (no `inventory`, no link-time collection). So `%router` still requires the author to *list* the pages explicitly — we cannot auto-discover `[@page]` functions across modules. This is the single most-felt limitation and it is identical in both languages; the gaps log calls it out ("proc-macro 无全局表故用 module_path"), and OCaml ppx has the same property.

### 6. Schema-as-types: `[@@deriving gql]`, `%gql_root`, and selection-set projection

This is the most type-system-heavy part of rui's DSL, and it's where the Rust→OCaml mapping is subtlest. rui's mechanism: a per-field marker type `gqlf::name`, an associated-type witness `impl Field<gqlf::name> for T { type Ty = … }`, and a `gen_sel_struct` that recursively synthesizes an exact-fit result struct by *projecting field types through `Field`*, peeling `Vec<_>` via `GqlElem::Elem` and reshaping via `Reshape`.

**The OCaml translation strategy.** OCaml has no associated types and no `inventory`, but it has GADTs, first-class modules, and (critically) `[@@deriving]`. The faithful port:

- **`gql_fields!` is eliminated.** Per the spine's mapping table, `%gql_fields` is *not needed*: the ppx derives field markers from the schema. Where rui makes the author write a marker list, the OCaml `[@@deriving gql]` on the model record *and* `%gql_root` on the schema impl together know every field name, so the selection macros (`%query`/`%fragment`/…) consult the derived metadata directly. This is a **DX win** — one fewer hand-maintained list.
- **`[@@deriving gql]` (replaces `#[derive(GqlObject)]`).** On a record type it generates: `typename`, `gql_id` (driven by `[@gql.id]` on a field; absent ⇒ `Value.Null`, i.e. a value object that inlines into its parent — ported exactly), `gql_field : string -> Value.t option`, and the `From_value`/`Into_value` module instances (replacing the `FromValue`/`IntoValue` traits). It also emits a **field-name set** the selection macros read to validate selections at expansion time.

  ```ocaml
  type todo = { id : string [@gql.id]; title : string; done_ : bool }
  [@@deriving gql]
  (* generates From_value/Into_value instances + a field registry "todo" → {id;title;done_} *)
  ```

- **`%gql_root` (replaces `#[gql_root(query)]`).** Annotates the schema module; generates the type-level `QueryRoot` field registry (each method's return type) used by selection validation, plus — native only — the `Exec.resolve` dispatcher built from method bodies. The `#[cfg(not(wasm32))]` split becomes dune's `Rui_gql.Exec` living only in the native/`ssr` implementation of the virtual library. Args are extracted via `From_value`/a `From_arg` instance, mirroring rui's `FromArg`.

- **Selection-set projection — the hard part.** rui's `gen_sel_struct` produces, per selection level, a `#[derive(Clone, PartialEq)] struct` whose scalar fields are `<<Elem as Field<gqlf::f>>::Ty as Scalar>::Out` and whose object fields recurse and reshape. OCaml has no associated-type projection, so `%query`/`%resource`/`%subscription`/`%fragment`/`%paginated` instead:
  1. parse the selection set (same `Sel { alias; name; args; children; spread }` grammar, including `alias: real`, literal vs. variable args, and `...Frag` spreads);
  2. **validate field names** against the derived registry for the element type at expansion time (this is where the "schema-as-types" exact-fit checking happens — an unknown field is a ppx error with a precise location, *replacing* rui's `PhantomData<<Elem as Field<gqlf::f>>::Ty>` compile checks);
  3. **synthesize a record type** for each level with fields named by alias-or-name, types looked up from the registry (scalar `Out` types, recursively-generated child records for object fields, wrapped in the container `list`/`option` the registry records);
  4. generate the `From_value` instance that reads `v.field key` per field — identical to rui's `parses` arm. Spreads inline the fragment's generated record (`Frag` field of type `Frag`), reading the same object (data masking) — ported verbatim.

  The query string is built at runtime exactly as rui does (`emit_args` + `emit_selection`): literal args inlined, variable args formatted via a `To_gql_arg` module (replaces `ToGqlArg`), aliases as `alias: name`, fragment spreads via the fragment's `SELECTION` constant.

- **`%query`/`%subscription`/`%resource` share one expander** (rui's `expand_fetch` with `Fetch = Query | Sub | Resource`). The lowering normalizes the response into `Rui_gql.Store`, records hit entity keys in a `string list signal`, and returns a `memo` reading the store and subscribing per-entity — ported exactly, including the **error/cleanup edge-cases** from progress 14/16:
  - On `errors_message` ⇒ skip merge, keep last-good (don't pollute the normalized cache);
  - `%resource` returns `(rows, loading, error)` and sets `error` *before* clearing `loading` (so the view's error branch wins, avoiding a flash of stale rows);
  - the fetch handler is registered with `Rui_runtime.on_cleanup (fun () -> Dom.drop_fetch_handler h)` so leaf/page disposal reclaims it (the handler-leak fix). With OCaml GC this is still *required* — it's observable-side-effect teardown (the spine's P2 principle), not memory hygiene.

- **`%mutation`** ports the optional trailing `optimistic:`/`on_error:` options (order-independent), the snapshot/restore optimistic flow, and the progress-14 fix that `on_error` must be held in an `Rc<dyn Fn>` so the outer closure stays `Fn`. In OCaml the outer thunk is just a `unit -> unit` and the callback is a captured `string -> unit` — **the whole `Rc<dyn Fn>` dance disappears** because OCaml closures share by reference under GC. This is a clean win: the single nastiest mutation bug in the log ("on_error 闭包破坏外层 Fn") is *unrepresentable* in OCaml.

- **`%paginated`** ports the Relay connection flow: `first` must be an integer literal (ppx error otherwise — rui's explicit check), the `@conn:<root>` store key, `merge_connection ~append`, the load-next throttle (`has_next && not loading`), and the `(edges, load_next, has_next, loading)` tuple. The generated edge/node records get structural equality automatically in OCaml (`=`), replacing rui's `#[derive(Clone, PartialEq)]` requirement — and the spine's `memo ?equal` value-dedup uses that same equality.

**Where this is a wash / harder:** rui's associated-type projection (`Field`/`GqlElem`/`Reshape`/`Scalar`) does the type plumbing *inside the type system*, so even hand-written code outside the macro gets exact-fit types. The OCaml design moves that projection *into the ppx* (registry lookup + record synthesis). The end-user experience is the same (exact-fit records, compile-time field validation), but a power user can't reuse the `Field` witnesses outside a `%query`. We accept this: GADT-encoding `Field<gqlf::f>` as a typed selection witness is possible but would be far heavier than the registry approach and buys little for the framework's actual usage.

### 7. Edge-cases the rui log solved → how this design handles each

| rui edge-case (progress entry) | OCaml mechanism |
|---|---|
| Generic event names dropped, only `on:click` worked (C) | `~on:(name, fn)` passes the event string straight to `Dom.on`; payload threaded for `~on_input` |
| Event payload (`target.value`) lost | `~on_input : (string -> unit)` typed handler; `Dom.on` callback receives the string |
| Dynamic attr computed once, not reactive (B) | thunk-valued attr (`~cls:(fun () -> …)`) wraps in `Rui_reactive.effect` |
| `<For>` cleared parent, no keyed diff | `for_ ~key` ⇒ `keyed_for` (reuse/remove/rebuild/move); non-keyed keeps clear-rebuild |
| `View(u32)` collided with `Display`, forced `<Show>` | `Rui_view.t` abstract; conditionals are native `if`/`match` returning `(module Into_view)` |
| `<Show>/<Switch>` `when` must be a closure (`compile_error!`) | `~when_ : unit -> bool` — a non-thunk is a normal type error with a location |
| `bind:value` limited to `Signal<String>` | `~bind_value : string signal` typed; wrong type = type error, not `compile_error!` string |
| Component missing prop = "missing field" | record construction = "some record fields are undefined" (typechecker) |
| Events/bind/ref rejected on components | structural: props record has no such labels |
| Zero-arg `<Uptime/>` needs empty Props | `type uptime_props = unit`, `let uptime () = …` |
| `:seg` index from pattern, typed param binding | ppx computes segment index, binds `param_as (module P) idx` |
| `param_as` silent `Default` on parse failure | parity default kept + `param_opt`/`param_as` over an explicit `Parsable` module; recommend `param_opt` |
| `PATTERN` collided with user items | `__rui_pattern_<name>`/`__rui_strategy_<name>` (prefixed + per-page suffix) |
| `#[page]` rejects generics/`self`/non-ident params; forwards attrs | `let%page` raises `Location.raise_errorf` on each; preserves doc/attrs |
| query string stripped at server entry (read-nothing) | `QUERY` signal + `query_param`; `[@page]` is path-only (symmetric mechanism, separate declaration) |
| memo re-notify on unrelated query-key change | `memo ?equal` value-dedup (default `=`), ports the `PartialEq` dedup |
| Empty `group(){}` → invalid code | ppx error at the group prefix location |
| Call-form router item (`make_page()`) | ppx diagnostic: page entries must be paths; use `group …` |
| Group strategy hardcoded `Ssr` | group strategy = strategy of the path-matched leaf (`__rui_strategy_<name>`) |
| Fetch handler leak across navigation + ghost writes | `on_cleanup (drop_fetch_handler h)` on every query/resource/sub; scope-driven teardown |
| `mutation on_error` closure broke outer `Fn` (`Rc<dyn Fn>` fix) | unrepresentable in OCaml — closures share under GC; plain `string -> unit` captured |
| query/mutation skip merge on errors; keep last-good | same `errors_message` guard before `Store.merge_all`/`normalize_list` |
| resource: set `error` before clearing `loading` | same ordering in the generated handler |
| `<Show>`/`<For>` on-mount not flushed in dynamic subtrees | `Rui_runtime.flush_mounts` called from the reactive-block/dispatch/fetch tails |
| SSR `<head>` needs `rui-slot,rui-frag{display:contents}` | `Rui_dom_ssr` injects the same CSS |
| `paginated! first` must be integer literal | ppx parses an int literal; non-literal = ppx error |
| `[@@deriving gql]` id optional ⇒ value object inlines | absent `[@gql.id]` ⇒ `gql_id = Value.Null`, not normalized |

### 8. Open questions / risks

1. **JSX encoding (B) vs. (A).** The marker-AST encoding keeps Merlin/locations but fixes the tag vocabulary inside the ppx; the string encoding (`{jsx|…|jsx}`) is closer to rui's free lexing but loses tooling. Decision affects every author. Recommend (B); revisit if the fixed marker set proves limiting (e.g. custom elements, SVG namespaces).
2. **Backend selection for `%view`-emitted DOM ops.** The ppx must emit calls that resolve to the *virtual library* `Rui_dom.S`. We need a stable module path (`Dom`) in scope at every expansion site. Risk: name capture if a user shadows `Dom`. Mitigation: emit fully-qualified `Rui_runtime.Dom.<op>` or a hidden alias bound by the ppx.
3. **Selection-set projection without `Field` witnesses.** Moving exact-fit typing into the ppx (registry + record synthesis) means cross-module schema references must resolve at ppx time. ppxlib runs per-file with no whole-program view, so the `[@@deriving gql]` registry must be *materialized into the generated module* (e.g. as a generated value/module the selection macros can reference by path: `Model.Todo` per the spine), not held in ppx memory. This is the riskiest piece — needs a concrete cross-file metadata convention.
4. **No global page registry (wash with Rust).** `%router` still requires an explicit page list. A dune-rule code-generator that scans `View.Pages.*` could close this, but it's outside the ppx and adds build complexity. Flag as future work, same status as rui's note.
5. **Typed params via parser modules.** Picking the `Parsable` module from a type annotation requires a built-in type→module table in the ppx; user-defined types need an extension point (e.g. `[@page.parser MyMod]`). Need to nail the syntax.
6. **`Into_view` dispatch on syntactic shape vs. type.** rui dispatches on the *return type* of `{ … }`; the ppx can only see syntax, so a block that *evaluates* to a `string` but is written opaquely (`let s = f () in s`) won't auto-select `of_text`. We require an explicit constructor (`text (f ())`) or rely on the typed `(module Into_view)` fallback. Document the rule clearly so authors aren't surprised.
7. **Optional/default component props.** OCaml optional labelled args offer a clean upgrade over rui's all-required record, but mixing optional args with the `~children` slot and the record-literal call path in `%view` needs a consistent lowering (function-with-optionals vs. record). Pick one before authors depend on it.
8. **Hydration ordering through fragments.** rui's hid numbering depends on a stable create-order between SSR and CSR (progress 5). The `%view` ppx must emit `el`/`text` in *exactly the same order* on both backends; any ppx-introduced reordering (e.g. evaluating attrs before children differently) would desync `claim_element`/`claim_text`. Needs a golden-order test in CI, the OCaml analogue of `hydrate.mjs`.

## DOM Abstraction, SSR & Hydration

### 1. What rui does today (the existing design)

rui has **one DOM API** (`el / text / set_text / append / remove_child / attr / set_value / clear / on / on_click / mount / clear_app / push_url / focus / scroll_into_view / set_interval / clear_interval / run_js / run_js_on / eval / gql / subscribe`) implemented by **three backends selected with `#[cfg(target_arch = "wasm32")]`** in `crates/rui/src/dom.rs`:

1. **browser · create (CSR):** `createElement` builds real DOM.
2. **browser · hydrate (SSR client):** *claims* the server-rendered DOM instead of rebuilding — `claim_element(hid)` / `claim_text(hid)`, attaches listeners, skips creation/append/attr.
3. **native · string (SSR server):** builds an **arena of `Node` records** (`tag / attrs / children / text / hid / is_text`) and serializes to an HTML string.

Components never know which backend is live — they call the same functions. A node is a **`u32` handle**. On wasm the handle is an index into a JS-side `nodes[]` array (`crates/rui/src/assets/router.js`); every call crosses a **hand-rolled FFI** (`extern "C"` imports + `wasm.alloc` + `TextEncoder`/`TextDecoder` over `memory.buffer` + `dispatch`/`on_fetch` exports). On native the `u32` indexes the arena `Vec<Node>`.

**Hydration** is two-counter-synchronized: SSR and the client walk the same render closure in the same order, so the *N*-th node created on the client maps to the *N*-th node emitted by SSR. SSR stamps every **element** with `data-h="N"` and every **text node** with a `<!--h:N-->` comment marker (text nodes can't carry attributes). The client `buildHydrateIndex()` scans `#app` for `[data-h]` elements and walks `SHOW_COMMENT` nodes to map `h:N → next text sibling` (synthesizing an empty text node if missing). During hydration `el()`/`text()` *increment the same `HID` counter* and call `claim_*`; `append`/`attr`/`mount`/`clear` become **no-ops** (`clear` especially: clearing would wipe the SSR children that `<For>` is about to claim).

**SSR server** (`crates/rui/src/server.rs`) is **std-only, zero-dep, thread-per-connection**. It serves `/graphql` (POST), `/graphql/subscribe` (SSE), the embedded `/router.js`, disk `web/app.wasm` + `web/styles.css`, and otherwise renders a page. `render_page` does `dom::reset()` + `gql::store::reset()` (per-request isolation), runs the page render closure inside a `scope` (so SSR-prefetch effects fire synchronously and fill signals), `mount`s, and `take_html()`s.

**Data handoff** closes the "client re-fetches everything" gap: during SSR, native `dom::gql` runs the query **locally** via the registered resolver (`server::local_execute`) and records `(query_string → response)` into `SSR_RESP`. `dehydrate_responses()` serializes that map; `ssr_doc` injects it into `<script id="__rui_data">` (with `</` → `<\/` to avoid `</script>` truncation). The client reads `#__rui_data`, calls `seed_responses`, and wasm `gql`/`subscribe` **synchronously deliver the cached response and skip the network** on first paint (consumed once; SPA navigation later does real requests).

The memory file records the **edge-cases that were fixed** (progress 5, 9, 14, 15, 16, 17 + the TLS-destruction trap) — they are the spec for the port (§5).

---

### 2. The big structural win: the FFI/handle/linear-memory layer EVAPORATES

The single most important fact for the OCaml port: **under js_of_ocaml the entire Rust FFI machinery disappears.** There is no wasm linear memory, so there is no `alloc`, no `ptr/len`, no `TextEncoder`/`TextDecoder` marshalling, no `nodes[]` registry, no `u32` handle, no `dispatch`/`on_fetch`/`run_interval` exports, and no `router.js` glue. jsoo compiles OCaml to JS that touches the DOM directly through **Brr**. A "node" is a real `Brr.El.t`, an event payload is read inline from the `Brr.Ev` object, and a `setInterval` callback is an OCaml closure passed straight to `Brr_io`/`G`.

Concretely, these Rust artifacts have **no OCaml equivalent**:

| Rust artifact (today) | Fate under jsoo |
|---|---|
| `ffi::create_element`, `add_event`, `dispatch`, `on_fetch`, `run_interval`, `wasm.alloc` | gone — direct Brr calls / OCaml closures |
| `nodes[] = [null]`, `reg(node)`, `u32` handle | gone — `Brr.El.t` is the node |
| `HANDLERS: Vec<Rc<dyn Fn(&str)>>`, `run_handler(id, value)` | gone — the closure is the listener (`Brr.Ev.listen`) |
| `assets/router.js` (140 lines of glue) | replaced by `Rui_client` (OCaml, jsoo-only) |
| `web/app.wasm` + `WebAssembly.instantiate` | replaced by `app.js` (`(modes js)`) |

What **stays** (now expressed in OCaml types instead of `u32`+linear-memory protocols):

- The **node abstraction itself** (`Rui_dom.S`) — still needed because two backends remain.
- The **hid two-counter hydration protocol** — a correctness invariant, ported verbatim.
- The **fetch-handler lifecycle** (`drop_fetch_handler` / `on_cleanup`) — still needed: an in-flight response for an abandoned page must become a no-op, not a "ghost write" to a normalized store.

So rui goes from **three cfg-gated backends + an FFI sidecar** to **two dune-selected backends + no sidecar**. That is the OCaml-cleaner headline of this subsystem.

---

### 3. OCaml design

#### 3.1 The backend signature `Rui_dom.S` (the bi-backend surface)

```ocaml
(* rui_dom.mli  — the virtual interface implemented by both backends *)
module type S = sig
  type node
  (** A built DOM node. Client: wraps [Brr.El.t]. SSR: an arena id (int).
      Exposed abstractly; [Rui_view]/[Rui_runtime] never inspect it. *)

  (* ── construction (hid-counted on both backends) ── *)
  val el        : string -> node
  val text      : string -> node
  val set_text  : node -> string -> unit
  val append    : parent:node -> node -> unit
  val remove_child : parent:node -> node -> unit          (* keyed <For> removal *)
  val attr      : node -> string -> string -> unit
  val set_value : node -> string -> unit                  (* controlled input: .value *)
  val clear     : node -> unit                            (* NO-OP while hydrating *)

  (* ── events ── *)
  val on        : node -> event:string -> (string -> unit) -> unit  (* payload = target.value | "" *)
  val on_click  : node -> (unit -> unit) -> unit

  (* ── app lifecycle / SPA ── *)
  val mount     : node -> unit                            (* into #app / set SSR doc root *)
  val clear_app : unit -> unit                            (* SPA page swap *)
  val push_url  : string -> unit                          (* history.pushState *)

  (* ── imperative DOM (on_mount) ── *)
  val focus            : node -> unit
  val scroll_into_view : node -> unit
  val set_interval     : ms:int -> (unit -> unit) -> int
  val clear_interval   : int -> unit

  (* ── JS escape hatch ── *)
  val run_js    : string -> unit
  val run_js_on : node -> string -> unit
  val eval      : string -> ((string, string) result -> unit) -> unit

  (* ── data layer ── *)
  val gql       : string -> (string -> unit) -> unit      (* query/mutation *)
  val subscribe : string -> (string -> unit) -> unit      (* SSE / SSR snapshot *)

  (* ── hydration / handoff ── *)
  val set_hydrate    : bool -> unit
  val seed_responses : string -> unit                     (* client: load injected cache *)

  (* ── SSR-only (no-op stubs on client for symmetric compilation) ── *)
  val dehydrate_responses : unit -> string
  val reset      : unit -> unit
  val take_html  : unit -> string
end
```

Two key API-shape upgrades over the Rust original, enabled by OCaml:

- **`on` carries a real closure, not a registry id.** The Rust `on` pushes into `HANDLERS` and passes a `u32`; the JS side later calls back `run_handler(id, value)`. Under jsoo the closure *is* the listener, so the whole `HANDLERS`/`run_handler`/`dispatch` round-trip is deleted. Same for `set_interval`, `eval`, `gql`/`subscribe` handlers.
- **`eval`'s result is a `(string, string) result`,** not an in-band `\x00`/`\x01` status byte. The Rust port had to encode ok/err as a leading byte over the FFI string channel (progress 17, fix ②); OCaml just uses a variant. This is strictly cleaner and impossible to get wrong.

`node` is **abstract** in the signature, so `Rui_view` (which holds it) compiles once against `S` and is reused for both targets — the OCaml answer to "components don't know the backend."

#### 3.2 Backend selection — dune virtual library (replaces `#[cfg]`)

```
rui.dom         (virtual library; exposes module type S, no impl)
 ├── rui.dom.client   (implementation; (modes js); depends on brr)
 └── rui.dom.ssr      (implementation; native)
```

`rui_view`/`rui_runtime` depend on the **virtual** `rui.dom`; the executable picks the implementation:

```scheme
; client target — dune
(executable (name app) (modes js)
 (libraries rui rui.dom.client) (preprocess (pps ppx_rui)))

; ssr server target — dune
(executable (name server) (modes exe)
 (libraries rui rui.dom.ssr) (preprocess (pps ppx_rui)))
```

This is the idiomatic dune mechanism for Rust's `#[cfg(target_arch=...)]`: one source of `Rui_view`, two link-time backends, **zero `cfg`-soup inside the code**. (A first-class-module functor `Make (D : Rui_dom.S)` is an alternative if a single binary must hold both — but the SSR server and the client are *separate* binaries, so the virtual-library route is simpler and matches the existing build split exactly.)

#### 3.3 SSR backend (`Rui_dom_ssr`) — arena + markers

A faithful port of the native `backend` module. The arena is a plain growable array on the GC heap; per-request `thread`-locals become a small mutable record (one per request thread — see §3.5).

```ocaml
(* rui_dom_ssr.ml *)
type kind = Element of { tag : string; mutable attrs : (string * string) list }
          | Text                                   (* <!--h:N--> + escaped text *)

type arena_node = {
  kind : kind;
  hid : int;
  mutable children : int list;                     (* reversed; flipped at serialize *)
  mutable text : string option;
}
type node = int                                    (* arena index *)

(* per-request state — see §3.5 on isolation *)
type ctx = {
  mutable arena : arena_node array; mutable len : int;
  mutable hid   : int;
  mutable root  : int option;
  mutable ssr_resp : (string * string) list;       (* query → response, for dehydrate *)
}

let el tag =
  let hid = next_hid () in
  push { kind = Element { tag; attrs = [] }; hid; children = []; text = None }

let text s =
  let hid = next_hid () in                          (* SAME counter as el — critical *)
  push { kind = Text; hid; children = []; text = Some s }

let clear node = let n = (cur ()).arena.(node) in n.children <- []; n.text <- None
let on _ ~event:_ _ = ()                            (* no events server-side *)
let set_hydrate _ = ()                              (* no hydration server-side *)

let serialize ctx id buf =
  let n = ctx.arena.(id) in
  match n.kind with
  | Text ->
    Buffer.add_string buf (Printf.sprintf "<!--h:%d-->" n.hid);   (* hydration anchor *)
    Buffer.add_string buf (esc_text (Option.value n.text ~default:""))
  | Element { tag; attrs } ->
    Buffer.add_string buf (Printf.sprintf "<%s data-h=\"%d\"" tag n.hid);
    List.iter (fun (k,v) -> Buffer.add_string buf
      (Printf.sprintf " %s=\"%s\"" k (esc_attr v))) attrs;
    Buffer.add_char buf '>';
    Option.iter (fun t -> Buffer.add_string buf (esc_text t)) n.text;
    List.iter (fun c -> serialize ctx c buf) (List.rev n.children);
    Buffer.add_string buf (Printf.sprintf "</%s>" tag)
```

Escaping matches the Rust source exactly: `esc_text` replaces `& < >`, `esc_attr` replaces `& "`. `set_value` finds-or-appends a `value` attribute (so the controlled value is visible in first paint). `gql`/`subscribe` call `Rui_server.local_execute`, record into `ssr_resp`, and synchronously deliver — so SSR renders **with data** for SEO. `dehydrate_responses` returns `Rui_gql.Value.(Obj ssr_resp |> to_json)`.

Where OCaml is **cleaner**: the Rust `serialize` had to `node.clone()` out of the `RefCell` before recursing to avoid a re-entrant borrow panic (`dom.rs:455`). OCaml has no `RefCell` borrow discipline — recursion over the arena is direct, no clone, no re-entrancy hazard. Variants (`kind`) also make `is_text` a proper sum type instead of a boolean flag.

#### 3.4 Client backend (`Rui_dom_client`) — real Brr DOM + claim-by-hid

```ocaml
(* rui_dom_client.ml *)
open Brr
type node = El.t                                   (* the node IS a Brr element *)

let hydrate = ref false                            (* exposed as Rui_client.hydrating *)
let hid = ref 0
let hidx : (int, El.t) Hashtbl.t = Hashtbl.create 256   (* hid → claimed SSR node *)

let set_hydrate on = hydrate := on
let next_hid () = let v = !hid in incr hid; v

let el tag =
  if !hydrate then (match Hashtbl.find_opt hidx (next_hid ()) with
                    | Some e -> e | None -> El.v (Jstr.v tag) [])  (* defensive *)
  else El.v (Jstr.v tag) []

let text s =
  if !hydrate then claim_text (next_hid ())        (* same counter as el *)
  else (* a Brr text node; rui wraps Dom.Text as El.t-compatible handle *)
    El.txt' s |> as_node

let append ~parent child = if not !hydrate then El.append_children parent [child]
let attr node k v        = if not !hydrate then El.set_at (Jstr.v k) (Some (Jstr.v v)) node
let set_value node s     = El.set_prop (El.Prop.value) (Jstr.v s) node   (* property! *)
let clear node           = if not !hydrate then El.set_children node []   (* no-op when hydrating *)
let mount node           = if not !hydrate then El.append_children app_el [node]

let on node ~event f =
  ignore (Ev.listen (Ev.Type.create (Jstr.v event)) (fun e ->
    let tgt = Ev.target e |> Ev.target_to_jv in
    if event = "submit" then Ev.prevent_default e;
    let payload =
      match Jv.find tgt "value" with Some v -> Jstr.to_string (Jv.to_jstr v) | None -> "" in
    f payload) (El.as_target node))
```

`buildHydrateIndex` (today in `router.js`) becomes an OCaml function in `Rui_client` using `Brr` document iteration: `querySelectorAll [data-h]` for elements, and a `NodeIterator` (via `Jv` if Brr lacks a typed binding) over `SHOW_COMMENT` to map `h:N → next text sibling`, synthesizing an empty text node when absent — byte-for-byte the same algorithm.

`set_value` uses the **property** (`El.set_prop value`), not the attribute, matching the Rust `set_value` / `router.js` `nodes[id].value = ...` — important for controlled inputs (the attribute only seeds the initial value).

#### 3.5 SSR server (`Rui_server`) — std-only http analog → OCaml http

The Rust server is deliberately dependency-free. The OCaml port keeps that spirit but is honest that "std-only http" in OCaml means either hand-rolling on `Unix` sockets (closest 1:1) or pulling a tiny dep. The design exposes the same `App` shape and the same per-request flow:

```ocaml
(* rui_server.mli — native only *)
type sse = { snapshot : unit -> string; subscribe : unit -> string Lwt_stream.t }
type app = {
  route   : string -> Rui_view.page;                 (* path → page (strategy + lazy render) *)
  resolve : Rui_gql.Exec.resolver;
  sse     : sse option;
}
val set_resolver  : Rui_gql.Exec.resolver -> unit
val local_execute : string -> string                 (* isomorphic SSR prefetch *)
val serve         : app -> unit                      (* blocks forever *)
```

Per-request flow (mirrors `handle` + `page` + `render_page`):

1. **Read the full request** (`read_request`): read to `\r\n\r\n`, parse `Content-Length`, drain the body — TCP segmentation handling, with the 1 MiB header cap. In OCaml this is straightforward `Unix.read` looping or an http library's request parser.
2. **Split target into `(path, query)`** dropping the `#fragment` first — ported exactly (progress 12 fix ①: `/todo/1?x=1` must give `id = "1"`, and `/about?utm=x` must not 404).
3. **Per-request isolation:** `Rui_dom_ssr.reset () ; Rui_gql.Store.reset ()` before rendering, so concurrent requests share nothing. With one render `ctx` per request (a value threaded through, or a `Domain.DLS` / per-thread slot — see below) this is naturally race-free.
4. **Render by strategy** (`Rui_view.strategy = Ssr | Csr | Static`):
   - `Csr` → `doc "" ""` (empty `#app`, no data) → client detects "no SSR content" and does pure CSR.
   - `Ssr` → render + inject `dehydrate_responses`.
   - `Static` → first render then cache by **normalized key** (path + sorted query params, so `?a=1&b=2 ≡ ?b=2&a=1`), with a **1024-entry cap** to stop `?utm=...` cache flooding (progress 13 fix ⑤).
5. **`render_page`** runs the page render closure inside a `Rui_reactive.scope`, mounts, takes HTML; the scope is disposed right after (SSR is one-shot — effects fire synchronously to fill signals, then tear down).
6. **Document skeleton** (`doc`) is identical: `<style>rui-slot,rui-frag{display:contents}</style>`, `<div id="app">…</div>`, `<script id="__rui_data" type="application/json">…</script>`, `<script type="module" src="/router.js">` → **replaced by `<script type="module" src="/app.js">`** (the jsoo bundle).

**Concurrency model — the one genuine divergence.** Rust uses `thread::spawn` per connection and `thread_local!` arenas, which gives free per-request isolation. OCaml's options:

- **Thread-per-conn + per-domain/per-thread state:** keep the same shape, store the render `ctx` in `Thread`-local or `Domain.DLS`. Closest port; works under OCaml 5 domains.
- **Explicit-context threading (preferred for cleanliness):** make the SSR arena a value created per request and threaded through (`Rui_dom_ssr` exposes `with_ctx : (unit -> 'a) -> 'a` that binds a fresh `ctx` for the dynamic extent). This removes *all* global mutable SSR state and is trivially safe under any concurrency model — but requires `Rui_view`/`Rui_runtime` to read "current ctx" the same way (a `ref` set inside `with_ctx`, restored on exit).
- **Lwt/Eio + a real http server** for production hardening (the Rust server is explicitly "demo-grade": no timeouts, no limits, hardcoded `127.0.0.1:8084` — root cause D / progress notes). This is the recommended production target; `serve` would run on Eio with proper read/write timeouts and a connection cap.

Either way the **SSE subscription** path is preserved: open `text/event-stream`, push `(snapshot)()` once, then stream `subscribe`d values as `data: <json>\n\n`, breaking on client disconnect (write error).

**The TLS-destruction trap → gone.** The memory file records a serious abort: after `Scope` became drop-immediately, the per-connection thread's `thread_local` arenas/effects were destroyed in random order, and `dispose_effect` touched an already-destroyed `EFFECTS` TLS → "cannot access TLS during/after destruction" → process abort. The fix was `EFFECTS.try_with(..)`. **OCaml has no `thread_local` destruction phase** — GC reclaims per-request records whenever; there is no "during/after destruction" window and no `try_with` guard needed. With the explicit-context approach there isn't even shared mutable global state to destroy. This entire class of bug evaporates.

#### 3.6 Client bootstrap (`Rui_client`) — replaces `router.js`

`Rui_client` is jsoo-only and does what `router.js` did, but in OCaml:

```ocaml
(* rui_client.mli — jsoo only *)
val hydrating : unit -> bool
val start     : route:Rui_runtime.route -> unit
(* installs <a> click interception (internal "/…" non-".html" links → SPA navigate),
   popstate handler, reads #__rui_data → seed_responses, then either hydrates or CSRs *)
```

The boot logic ports directly:

```ocaml
let start ~route =
  (* 1. data handoff *)
  (match Document.find_el_by_id G.document (Jstr.v "__rui_data") with
   | Some el -> Rui_dom_client.seed_responses (El.text_content el |> Jstr.to_string)
   | None -> ());
  (* 2. hydrate iff SSR content present, else pure CSR *)
  if has_ssr_content () then begin
    Rui_client.build_hydrate_index ();
    Rui_dom_client.set_hydrate true;
    Rui_runtime.render_path route (location_path_and_search ());
    Rui_dom_client.set_hydrate false
  end else
    Rui_runtime.render_path route (location_path_and_search ());
  (* 3. SPA: intercept internal <a>, popstate → navigate *)
  install_link_interception (); install_popstate route
```

Note `set_hydrate true … render … set_hydrate false`: first paint claims, **without** the old `innerHTML = ""` that used to nuke the SSR DOM (root cause A). SPA navigation thereafter is CSR; `navigate` compares the page `key` and only `clear_app`s on a real page change (progress 11).

---

### 4. Feature → OCaml mechanism map (and where OCaml wins / washes)

| rui feature (today) | OCaml mechanism | Cleaner / wash / harder |
|---|---|---|
| Tri-backend via `#[cfg]` | **Bi-backend** via dune virtual library (`rui.dom` + `.client`/`.ssr`) | **Cleaner** — link-time, no cfg in code, and the wasm-create backend folds into the client backend |
| `u32` node handle + `nodes[]` registry | `Brr.El.t` (client) / arena `int` (ssr), behind abstract `node` | **Much cleaner** — no registry, no marshalling |
| FFI `extern "C"` + `alloc`/ptr/len + encode/decode | **gone** (jsoo direct DOM) | **Cleaner** — whole layer deleted |
| `HANDLERS`/`run_handler`/`dispatch` event round-trip | OCaml closure as `Brr.Ev.listen` callback | **Cleaner** |
| `eval` ok/err status byte | `(string, string) result` variant | **Cleaner** |
| `serialize` RefCell-clone to dodge re-entrant borrow | direct recursion over arena | **Cleaner** (no borrow checker) |
| `is_text: bool` flag | `kind = Element … \| Text` variant | **Cleaner** (rows/variants) |
| hid two-counter hydration protocol | same protocol, OCaml `ref` counter | **Wash** — intrinsic to no-VDOM hydration; ported verbatim |
| `data-h` / `<!--h:N-->` markers + `buildHydrateIndex` | identical strings; Brr `querySelectorAll` + comment `NodeIterator` | **Wash** |
| `set_hydrate` gating `append`/`attr`/`clear`/`mount` | identical guards on `Rui_dom_client` | **Wash** |
| `seed_responses`/`dehydrate_responses`/`<script id=__rui_data>` | identical; `</` → `<\/` escape kept | **Wash** |
| `drop_fetch_handler` + `on_cleanup` one-shot fetch slots | OCaml: handler stored in a `ref option`; `on_cleanup` sets it to `None`; deliver flattens | **Slightly cleaner** (no `Vec<Option<Rc>>` slot index; just an `option ref`), same correctness obligation |
| thread-per-conn + `thread_local` arena | thread/domain + per-request `ctx` (or explicit threading) | **Wash, modest divergence** — see §3.5; OCaml needs `Domain.DLS` or explicit ctx where Rust got `thread_local` free |
| TLS-destruction abort (`EFFECTS.try_with`) | n/a — no TLS destruction phase | **Cleaner** — bug class removed |
| std-only http server | `Unix` sockets (1:1) or Eio/http lib (production) | **Wash → harder** — "zero deps" is less idiomatic in OCaml; production wants a real http stack |
| `web/app.wasm` + `WebAssembly.instantiate` | `app.js` via `(modes js)` | **Cleaner** (no wasm toolchain, no instantiation dance) |

The honest **wash** core: the hid-counter / marker / `set_hydrate`-no-op machinery is *intrinsic to fine-grained hydration without a VDOM*. OCaml does not make it disappear; it ports identically. The **win** is everything *around* it — the FFI/handle/memory plumbing — which is pure Rust-on-wasm incidental complexity that jsoo erases.

---

### 5. Edge-cases rui solved — and how the OCaml design handles each

Each is cited from `dom.rs` / `router.js` / `server.rs` or the progress log; the OCaml design must reproduce the behavior.

1. **`clear()` must be a no-op during hydration** (`dom.rs:212`, progress 5/Stage 2). Otherwise `<For>` clearing its parent wipes the SSR children it's about to claim → blank first paint. → `Rui_dom_client.clear` guards on `!hydrate`.
2. **Text nodes also consume a hid and get `<!--h:N-->`** (`dom.rs:339`, `text()` shares the `el()` counter). Empty text still gets a marker; the client synthesizes an empty text node if the comment has no text sibling (`router.js:139`). → `Rui_dom_ssr.text` increments the same counter; `build_hydrate_index` synthesizes the empty node.
3. **`append`/`attr`/`mount` are no-ops while hydrating** (`dom.rs:88/99/104`) — the node is already in the DOM with its attributes. → same guards.
4. **`set_value` is a property on the client but an attribute on SSR** (`dom.rs:203/368`, `router.js:19`). SSR's value attribute makes the controlled value visible in first paint; the client writes `.value` (find-or-replace the `value` attr server-side). → `El.set_prop value` vs find-or-append attribute.
5. **One-shot fetch handlers must be reclaimed; abandoned-page in-flight responses must become no-ops** (`dom.rs:229`/`280`, progress 16 fix ⑤). A handler that's been dropped → `run_fetch` flattens to `None` → no "ghost write" to the normalized store; without this, every navigation leaks a handler (pinning its captured signal). → OCaml: each `gql`/`subscribe`/`eval`/`resource` handler lives in a `string -> unit option ref`; the owning scope's `on_cleanup` sets it to `None`; deliver pattern-matches `Some f` else no-op.
6. **`eval` is one-shot, reclaimed on delivery *and* on scope teardown** (`dom.rs:172–188`, progress 17 fix ①), covering never-settling Promises. → `eval` registers via the same slot mechanism + `Rui_reactive.on_cleanup`.
7. **SSR data injection must not break out of `<script>`** — `</` → `<\/` (`server.rs:214`). JSON treats `\/` as a legal `/`. → kept exactly in `ssr_doc`.
8. **`gql` cache is consumed once; SPA navigation does real requests** (`dom.rs:258`, `r.remove(pos)`). `subscribe` delivers the SSR snapshot synchronously *then* opens SSE so hydration matches SSR, then live updates take over (`dom.rs:269`). → OCaml `seed_responses` builds an assoc list; `gql` `List.assoc`-removes on hit; `subscribe` delivers-then-streams.
9. **Per-request isolation:** `dom::reset()` + `gql::store::reset()` before each render (`server.rs:71/72`). → `Rui_dom_ssr.reset` + `Rui_gql.Store.reset` (or a fresh per-request `ctx`, which makes isolation structural rather than a reset call).
10. **Path/query split:** drop `#fragment`, split `?query`, empty path → `/` (`server.rs:122`, progress 12). Static cache key normalizes (sorts) query params and caps at 1024 (`server.rs:189`, progress 13). → ported in the request handler and the `Static` branch.
11. **SSE `onerror` only reports on `CLOSED`** (`router.js:48`) to avoid EventSource's auto-reconnect transient errors spamming the UI; non-2xx HTTP **synthesizes `errors[]`** so the UI leaves the loading state (`router.js:38`, progress 14 fix ③). → `Rui_client`'s `gql`/`subscribe` wrap `fetch`/`EventSource` (via Brr / `Jv`) with the same error synthesis: non-`ok` → `{"errors":[{"message":"HTTP …"}]}`, network catch → `{"errors":[…]}`, SSE error only when readyState = CLOSED.
12. **`submit` events `preventDefault` automatically; payload is `target.value` or `""`** (`router.js:25`, progress 3). → `Rui_dom_client.on` calls `Ev.prevent_default` for `"submit"` and reads `value` defensively.
13. **Interval handlers must be reclaimable to avoid unbounded growth** (`dom.rs:127–152`, progress 15 fix ③: `<Uptime/>` in the shell is rebuilt every navigation). The slot stores `(timer_id, callback)` and `clear_interval` nulls the slot to drop the captured signal. → OCaml: `set_interval` returns the JS timer id; the callback closure is held only by the JS timer, and `clear_interval` clears it (jsoo GC then reclaims the closure) — no manual `Vec<Option<…>>` table needed, since there's no FFI id indirection.
14. **`focus`/`scroll_into_view` tolerate missing methods** (`router.js:54`). → Brr typed bindings make `El.focus`/scroll total; if reaching through `Jv`, guard the method's presence.
15. **`run_js` (indirect eval = global scope) vs `run_js_on` (direct eval, `el` bound)** (`dom.rs:161/165`, `router.js:60/61`). → `run_js` uses indirect eval `(Jv.global ## eval)`; `run_js_on` evaluates with `el` injected (e.g. a small wrapper function taking the node). SSR no-ops both, like `on_mount` (only the client runs them).

---

### 6. Open questions / risks

- **SSR concurrency model.** The cleanest OCaml design (explicit per-request `ctx` threaded through `Rui_view`/`Rui_runtime`) requires those modules to read "current SSR node sink" through a `ref` set inside `with_ctx`. The literal port (`Domain.DLS`/thread-local) is closer to Rust but reintroduces global mutable state. **Decision needed:** pick explicit-ctx (safer, idiomatic) vs DLS (1:1). Recommended: explicit-ctx for SSR, since it also makes per-request isolation structural and kills any reset-ordering risk.
- **"std-only" is un-idiomatic in OCaml.** Rust got a zero-dep server from `std::net`. OCaml's stdlib `Unix` can do it, but production wants timeouts/limits/TLS (root cause D, never fixed in Rust). Risk: scope-creep into choosing an http stack (Eio vs Lwt vs Httpaf). The design should ship a minimal `Unix`-socket server for parity and a documented Eio path for hardening — but not block the port on it.
- **Brr coverage for the hydration walk.** `buildHydrateIndex` needs a comment-node iterator (`NodeIterator` with `SHOW_COMMENT`). If Brr lacks a typed binding, fall through to `Jv` (`document ## createNodeIterator`). Risk: thin `Jv` interop here; needs a small tested helper.
- **Brr text-node handle vs `node = El.t`.** rui's `node` mixes elements and text nodes under one `u32`. Brr distinguishes `El.t` from text/`Dom.Node`. The client backend must represent a text node as a `node`-compatible handle (wrap `Brr.El.txt'` / a `Dom.Node` behind the abstract `node`), and `set_text`/`claim_text` must operate on it. **Decision needed:** make `node` an abstract wrapper (`Element of El.t | Text of Jv.t`) rather than a bare `El.t`, at a small ergonomic cost — likely the right call.
- **Lost-response / out-of-order resource responses** (deferred in Rust, progress 14). The single-handler-per-resource model can't tell which in-flight request a response belongs to on rapid refetch. Same limitation persists in OCaml unless we add a request-generation token; flagged, not fixed, to keep parity.
- **`run_js`/`eval` security.** The escape hatch is `eval`. Same XSS surface as Rust; the design inherits it. Worth a doc warning, no mechanism change.
- **Hydration mismatch diagnostics.** The two-counter protocol silently corrupts if SSR and client render diverge (e.g. a non-deterministic render). Rust has no guard. OCaml could add a *debug-build* assertion (claimed node's tag vs requested tag) — low cost, high debugging value. Open: include it behind a build flag?

## GraphQL Data Layer

This is the most type-heavy subsystem in Rui, and the one where OCaml's row types, GADTs, first-class modules, and GC buy the largest wins over Rust. It is also where the port must be most careful: the Rust design leans hard on *trait coherence across proc-macro boundaries* (a `query!` macro and a `#[derive(GqlObject)]` derive that cannot see each other's output, reconciled only at type-check time). The OCaml ppx world is structurally similar (`%query` and `[@@deriving gql]` are separate ppx invocations that cannot see each other) so the same "defer everything to type-checking via generated type-level witnesses" strategy transfers almost intact — and gets *cleaner* because OCaml lets us express "exact-fit selection struct" as a real structural type rather than a tower of associated-type projections.

### 1. What Rui does today (ground truth)

The Rust `gql` module is five files:

- **`value.rs`** — a runtime `Value` enum (`Null | Bool | Int(i64) | Float(f64) | Str | List | Object(Vec<(String,Value)>)`), a hand-rolled recursive-descent JSON parser (int/float discrimination, `\uXXXX` escapes), `to_json`, accessor helpers (`field` returns `&Null` on miss so `FromValue` chains never `unwrap`), the `FromValue`/`IntoValue` traits with scalar+`Vec`+`Option` instances, and `errors_message` — the response classifier that decides success vs failure for the whole data layer.
- **`store.rs`** — a Relay-style normalized cache. Entities are keyed `"__typename:__id"`; nested objects carrying those two meta fields are extracted into independent entities and the parent keeps a `{"$ref": key}` placeholder; each entity has a version `Signal<u64>`; `read_entity` subscribes to versions and de-normalizes (`$ref` → inline, recursively), `merge_all`/`bump_all`/`normalize_list` write then publish, and there are connection records (`merge_connection`/`read_connection`) plus optimistic-update primitives (`keys_of`/`snapshot`/`restore`/`reset`).
- **`exec.rs`** (native only) — a schema-agnostic executor: parse document → call an injected `Resolver` callback per root field → `project` the resolver's full-field `Value` down to the selection. It never names a concrete type or the app's schema roots.
- **`parser.rs`** (native only) — a recursive-descent GraphQL *document* parser with **progress guards** (any malformed token advances at least one byte) so a hostile request can never pin a server thread.
- **`mod.rs`** — the type-system traits: `Scalar`, `GqlElem`, `Field<M>`, `Reshape<S>`, `Fragment`, `GqlObject`, plus `ToGqlArg`, `gql_escape`, and `decode_rows`.

On top of those, the `rui-macros` crate generates the user-facing surface: `#[derive(GqlObject)]`, `#[gql_root(query|mutation|subscription)]`, `gql_fields!`, and the five fetch macros `query!`/`subscription!`/`resource!`/`mutation!`/`paginated!`/`fragment!`. The fetch macros each synthesize an **exact-fit selection struct** by projecting field types through `Field<gqlf::name>` markers, then wire it to the normalized store via a fetch-handler indirection (`on_fetch_handler` returns a `u32` slot id; `drop_fetch_handler` nulls the slot; `run_fetch` no-ops a nulled slot).

The OCaml port keeps every one of these moving parts and every edge case the gaps log records.

---

### 2. `Rui_gql.Value` — runtime value model + JSON codec

OCaml's variants are a *direct, cleaner* port of the Rust enum. We keep the `Object of (string * t) list` association-list representation (not a `Map`) on purpose — field **order** is observable (`to_json` round-trips, the executor preserves selection order, and the store's `merge_into` does last-write-wins by scanning), so an ordered alist is the faithful structure.

```ocaml
(* rui_gql_value.mli — submodule Rui_gql.Value *)
type t =
  | Null
  | Bool  of bool
  | Int   of int          (* OCaml native int is 63-bit; see risks for i64 note *)
  | Float of float
  | Str   of string
  | List  of t list
  | Obj   of (string * t) list

val get      : t -> string -> t option
val field    : t -> string -> t          (* missing -> Null, mirrors Rust field() *)
val as_str   : t -> string               (* non-Str -> ""    *)
val as_int   : t -> int                  (* Float -> truncated, else 0 *)
val as_float : t -> float                (* Int  -> coerced,  else 0. *)
val as_bool  : t -> bool                 (* only Bool true is true *)
val as_list  : t -> t list               (* non-List -> [] *)
val is_null  : t -> bool

val to_json  : t -> string
val parse    : string -> t               (* tolerant recursive-descent, never raises *)
val errors_message : t -> string option  (* the response classifier *)
```

The accessors are *total* and never raise — `field` returning `Null` on a miss is load-bearing: the generated `of_value` decoders chain `field` calls and rely on "missing → Null → scalar default", so a partial response never crashes a render. This is a place where OCaml is a **wash** with Rust: the Rust `field` returns `&Null` (a borrow of a `const NULL`), OCaml returns the immutable `Null` value; both are zero-allocation in spirit.

**Parser.** A direct transcription of the Rust byte-walker (`{`/`[`/`"`/`t`/`f`/`n`/number dispatch, `\uXXXX` decoded to a UTF-8 codepoint, int-vs-float decided by seeing `.`/`e`/`E`). It is *tolerant by design* (mirrors Rust): an unterminated string or trailing garbage produces a best-effort `Value` rather than an error, because `parse` is also the fallback path for HTTP error pages — and `errors_message` is what then classifies the result as a failure. We do **not** swap in a `yojson`/`jsonm` dependency: keeping the hand-rolled parser preserves the exact int/float discrimination and the exact "garbage parses to *something*" behavior the classifier depends on, and keeps `Rui_gql` dependency-free so the same code compiles to both jsoo and native.

**`errors_message` — the classifier (ported verbatim, including the hardening from progress 14).** This single function decides success/failure for `%query`, `%resource`, `%mutation`, `%subscription`. The Rust version was hardened to treat *malformed* responses as failures so an HTTP error page or parse-garbage can never be mistaken for an empty success:

```ocaml
let errors_message (v : t) : string option =
  match v with
  | Obj _ ->
    (match get v "errors" with
     | None ->
       if get v "data" <> None then None                       (* {data, ...}     -> success *)
       else Some "invalid response (missing data/errors)"      (* {} etc.         -> failure *)
     | Some (List []) -> None                                  (* explicit errors:[] -> success *)
     | Some (List errs) ->
       Some (String.concat "; "
               (List.map (fun e ->
                  match as_str (field e "message") with
                  | "" -> "GraphQL error"                       (* placeholder: never drop the error signal *)
                  | m  -> m) errs))
     | Some _ -> Some "invalid response (errors field malformed)")  (* errors null/obj/str/num -> failure *)
  | _ -> Some "invalid response (not a JSON object)"            (* HTML page / bare value -> failure *)
```

Every branch is one of the cases the Rust test `errors_message_classification` asserts, including: bare `0` (non-object) → failure; `{}` (missing both) → failure; `{"errors":"boom"}` (errors not a list) → failure; `{"errors":[{}]}` (error with no message) → `Some "GraphQL error"` (placeholder, do not lose the "there was an error" signal). This is **as natural in OCaml as in Rust** — a `match` over the variant is the same shape.

**`From_value` / `Into_value`.** The Rust `FromValue`/`IntoValue` traits become the canonical module types from the spine, with named instances so the ppx and hand-written code can reference them:

```ocaml
module type From_value = sig type t val of_value : Value.t -> t end
module type Into_value = sig type t val to_value : t -> Value.t end
```

The scalar instances (`string`, `int`, `float`, `bool`) and the two combinators (`list`, `option`) are provided as functors / first-class-module values:

```ocaml
module Fv : sig
  val string : (module From_value with type t = string)
  val int    : (module From_value with type t = int)
  val float  : (module From_value with type t = float)
  val bool   : (module From_value with type t = bool)
  val list   : (module From_value with type t = 'a) -> (module From_value with type t = 'a list)
  val option : (module From_value with type t = 'a) -> (module From_value with type t = 'a option)
end
```

`option`'s decoder is exactly the Rust `Option` instance: `if is_null v then None else Some (T.of_value v)`. In practice the ppx does *not* thread these modules around — for a generated record it inlines `of_value`/`to_value` field-by-field (see §6), so first-class modules are only the public hand-written escape hatch. OCaml here is a **slight win**: `option`/`list` combinators are values, not blanket `impl`s, so there is no orphan-rule or coherence anxiety.

---

### 3. `Rui_gql.Store` — the normalized cache

The store is a global, per-render-context pair of mutable tables plus version signals. In Rust these are `thread_local!` `RefCell<HashMap>`; OCaml uses a plain module-level `Hashtbl` (the jsoo client is single-threaded; the native SSR server resets per render, see the TLS edge-case below). **GC is a real win** here: the Rust store stores `Signal<u64>` clones in a `HashMap` and clones `Value`s on every read; OCaml stores plain records and `Value`s are immutable so reads share structure freely.

```ocaml
(* rui_gql_store.mli — submodule Rui_gql.Store *)

val read_entity : string -> Value.t option
(* subscribes to this entity's version + every nested $ref'd entity, de-normalizes *)

val merge_all     : Value.t -> string list   (* merge top-level entities, return their keys; bump nested *)
val bump_all      : string list -> unit      (* publish: bump versions -> wake subscribed views *)
val normalize_list: Value.t -> string list   (* merge_all then bump_all (mutation convenience) *)

val merge_connection : conn_key:string -> Value.t -> append:bool -> unit
val read_connection  : string -> Value.t

val keys_of  : Value.t -> string list                       (* collect all entity keys (top + nested) *)
val snapshot : string list -> (string * Value.t option) list (* None = absent *)
val restore  : (string * Value.t option) list -> unit        (* undo optimistic write + bump *)

val reset    : unit -> unit                  (* SSR: clear between renders *)
```

**Entity key + `$ref` normalization.** Identical algorithm: an object is an entity iff it has a non-empty `__typename` and a scalar `__id` (`scalar_key` accepts `Str`(non-empty)/`Int`/`Float`/`Bool`). `normalize_value` recurses, replacing each entity object with `Obj [("$ref", Str key)]` and merging its (recursively normalized) fields into the table; objects without an id stay inline (value objects like `Connection`/`Edge`/`PageInfo`). `denormalize` is the inverse, and crucially calls `read_entity` recursively on each `$ref` so de-normalizing also **subscribes to nested entity versions** — that is the Relay-consistency soul: a `mutation` that rewrites `Item:A` wakes every view that read an `Order` containing it.

```ocaml
let rec normalize_value (v : Value.t) (touched : string list ref) : Value.t =
  match v with
  | List xs -> List (List.map (fun x -> normalize_value x touched) xs)
  | Obj _ ->
    let inner = normalize_fields v touched in
    (match entity_key v with
     | Some key -> merge_into key inner;
                   touched := key :: !touched;
                   Obj [("$ref", Str key)]
     | None -> inner)                         (* value object: fields normalized in place *)
  | scalar -> scalar
```

**Version bump uses `untrack`.** `bump` does `untrack (fun () -> Signal.set ver (Signal.get ver + 1))` — exactly the Rust `untrack(|| ver.set(ver.get()+1))` — so bumping inside a fetch handler (which may run inside an effect) does not accidentally create a dependency edge. The bump is what `read_entity`'s `version key |> Signal.get` subscription observes; combined with **`memo`'s value-dedup** (from the spine: `memo ?equal` does not notify when the recomputed value is `=` the old one, porting the Rust `PartialEq` dedup), an entity bump that produces an identical row re-projection does not cascade a re-render.

**Edge-case (gaps store.rs): write-then-bump for a consistent snapshot.** `merge_all` merges the *entire* batch into the table *before* publishing any top-level key, so any view woken by the bump sees a fully-merged snapshot, never a half-merged intermediate. Nested entities are bumped inside `merge_all` (they notify *other* views referencing them); the top-level keys are returned and bumped by the *caller* (`%query`'s handler does `let mk = merge_all payload in Signal.set keys mk; bump_all mk`). This split is preserved exactly — it is a correctness property, not an optimization.

**Connection records (Relay cursor pagination).** Stored under a caller-supplied key (`"@conn:<field>"`); shape `{ edges:[{node:{$ref}, cursor}], page_info:{has_next_page,end_cursor} }`. `merge_connection ~append:false` replaces edges (first page / refetch), `~append:true` appends. The edge node (which has an id) is extracted to its own entity, so a `mutation` on a node updates the paginated view too. **Edge-case: cursor de-dup.** Appending skips any edge whose non-empty `cursor` already exists — idempotent against double-clicked "load more" or a resent request. Ported verbatim:

```ocaml
let dup = c <> "" && List.exists (fun x -> as_str (field x "cursor") = c) !lst in
if not dup then lst := !lst @ [e]
```

**Optimistic primitives.** `keys_of` recursively collects every entity key (top + nested) of a predicted `Value`; `snapshot` records each key's current value (`None` = absent); `restore` writes them back (`Some` → insert, `None` → remove) and bumps. These back `%mutation`'s optimistic+rollback (§7). OCaml is a **wash** here — the logic is identical alist/hashtbl bookkeeping.

**Edge-case (the SSR TLS-destruction abort, from gaps progress "踩坑+修复").** In Rust the store is `thread_local!`, and `reset()` is called before each SSR render so per-connection threads stay isolated. OCaml's native SSR uses **one render context per request**, and `Rui_gql.Store.reset` is called at the top of each render. Because OCaml has no per-thread-local-on-thread-exit destructor running user code, the *specific* "cannot access TLS during/after destruction → process abort" bug that bit Rust **cannot occur** in the OCaml port — this is a clean GC/runtime win. (If the SSR server is later made multi-threaded with domains, the store must move behind a per-request value threaded explicitly or a `Domain.DLS` key; flagged in risks.)

---

### 4. `Rui_gql.Parser` + `Rui_gql.Exec` — server executor (native only)

These compile only into `Rui_dom_ssr` / `Rui_server` (the dune virtual-library backend split is the idiomatic answer to Rust's `#[cfg(not(target_arch="wasm32"))]`).

**Parser.** Direct port of the recursive-descent document parser: `OpKind = Query | Mutation | Subscription`, `aval = AStr | AInt | AFloat | ABool | ANull`, `field = { alias : string option; name : string; args : (string*aval) list; selection : field list }`, anonymous operations, aliases, nested args, multi-root, **variable-definition skipping** (`query Foo($x:T)` — the client inlines variable values, so the server only ever sees literals and skips the `(...)`), and commas-as-insignificant whitespace.

**Edge-case (gaps parser.rs `malformed_input_terminates`): progress guards = anti-DoS.** Every loop (`args`, `selection_set`, `value`, `skip_var_defs`) advances at least one byte on an unrecognized token. The OCaml port keeps a mutable index `i` and the identical guards (`if i = before then incr_i ()`), with a unit test asserting `{ @ }`, `{ stock(id: $v) {symbol} }`, `{ a(x: [) }`, `{ # garbage ! }`, `{{{{` all terminate and a valid query still parses. This is non-negotiable: without it a hostile request pins a server thread (remote DoS). OCaml is a **wash** — same byte-walker, same guards.

**Executor.** Schema-agnostic by construction. The Rust executor takes a `Resolver = fn(OpKind, &str, &Args) -> Value` injected by the app; OCaml uses a first-class function of the same shape, plus an `Args` accessor module:

```ocaml
(* rui_gql_exec.mli *)
module Args : sig
  type t
  val str   : t -> string -> string
  val int   : t -> string -> int
  val float : t -> string -> float
  val bool  : t -> string -> bool
end

type resolver = Parser.op_kind -> string -> Args.t -> Value.t
val empty_resolver : resolver                   (* every field -> Null; the `rui init` skeleton *)
val execute : string -> resolver -> string      (* document text -> {"data":..,"errors":[]} json *)
```

`execute` walks each op's selection, calls `resolve kind field args` for the full-field `Value`, then `project`s by the selection (recursive, list-aware, alias-aware), and assembles `{"data":{...},"errors":[]}` — note it **always** emits `errors:[]` (empty), which is why the client classifier's "`{data, errors:[]}` = success" branch is the common case.

**Edge-case (exec.rs `project`): meta fields are always preserved.** Even if the query doesn't select `__typename`/`__id`, projection injects them (so the client store can locate the entity), and dedupes them if the client *did* select them (skip non-aliased `__typename`/`__id` in the loop to avoid duplicate keys). Ported exactly:

```ocaml
let rec project (v : Value.t) (sel : Parser.field list) : Value.t =
  match v with
  | List xs -> List (List.map (fun x -> project x sel) xs)
  | Obj _ when sel <> [] ->
    let out = ref [] in
    (match get v "__typename" with Some tn -> out := ("__typename", tn) :: !out | None -> ());
    (match get v "__id"       with Some id -> out := ("__id", id)       :: !out | None -> ());
    List.iter (fun (f : Parser.field) ->
      if f.alias = None && (f.name = "__typename" || f.name = "__id") then ()
      else
        let fv = field v f.name in
        let pv = if f.selection = [] then fv else project fv f.selection in
        out := (Option.value f.alias ~default:f.name, pv) :: !out) sel;
    Obj (List.rev !out)
  | other -> other
```

The `gql_root` resolver dispatch (§5) feeds this. OCaml is a **wash** for the executor.

---

### 5. Exact-fit selection via type-level witnesses — Field/Scalar/GqlElem/Reshape

This is the conceptual heart, and where OCaml has the **most genuine room to be cleaner** — though it also forces a design choice. The Rust mechanism:

- `#[derive(GqlObject)]` emits, per field, `impl Field<gqlf::name> for Order { type Ty = FieldType; }` — a marker-keyed type-level map from field name to field type.
- `query!` only knows the *field name*; it writes `<Order as Field<gqlf::name>>::Ty` to recover the field type **without naming the object type**, then `<Ty as Scalar>::Out` for scalars, `<Ty as GqlElem>::Elem` to dig into a sub-object (with a blanket `impl GqlElem for Vec<T>` so lists and singletons unify), and `<orig as Reshape<InnerStruct>>::Out` to wrap the synthesized inner struct back into the original container shape (`Vec<T>` → `Vec<Inner>`, single → `Inner`).
- This makes "is this field present? what type? object or scalar?" a **type-check error if wrong**, even though `query!` never saw the derive output. Selecting an object as a scalar → no `Scalar` impl → error; a scalar as an object → no `GqlElem::Elem` → error.

**The OCaml port has two viable encodings; we pick (B) and offer (A) as the fallback witness.**

**(A) First-class-module witnesses (the faithful, mechanical port).** `[@@deriving gql]` generates, per field, a *field witness value* carrying the field name and a decoder, grouped into a generated `module Order_fields`:

```ocaml
(* generated by [@@deriving gql] for type Order *)
module Order_fields = struct
  let id      : (Order.t, string) field = { name = "id";    of_value = Value.as_str }
  let total   : (Order.t, float)  field = { name = "total"; of_value = Value.as_float }
  let items   : (Order.t, Item.t list, Item.t) obj_field = { name = "items"; elem = (module Item_witness) }
end
```

where `('obj,'ty) field` and `('obj,'ty,'elem) obj_field` are phantom-typed records. A `%query` then projects field types by *referencing the witness value*, and the OCaml type-checker rejects a misuse the same way Rust's coherence does: selecting `items` as a scalar fails because there is no scalar witness named `items`; selecting `total` with a sub-selection fails because `total`'s witness is a `field`, not an `obj_field`. This is a near-mechanical translation of `Field<M>`/`Scalar`/`GqlElem`, and it is a **wash with Rust** — same indirection, just values instead of associated types.

**(B) The synthesized selection struct is a real OCaml record / object — exact-fit and data-masking fall out *for free* (the genuine win).** In Rust, `gen_sel_struct` synthesizes a fresh `struct __Row0 { ... }` with a generated `FromValue`. OCaml does exactly the same thing, but the *consumer* of an exact-fit selection is far nicer than Rust because OCaml has **structural object types and polymorphic records**. A `%query`/`%fragment` selection compiles to an anonymous-ish record whose fields are precisely the selected ones, and **nothing else is reachable** — that *is* data masking, enforced by the type system with zero ceremony:

```ocaml
(* %query order(id: oid) { id total items { sku qty } }  generates: *)
type __row_items = { sku : string; qty : int }
type __row = { id : string; total : float; items : __row_items list }

let __row_items_of_value v =
  { sku = Value.as_str (Value.field v "sku");
    qty = Value.as_int (Value.field v "qty") }
let __row_of_value v =
  { id = Value.as_str (Value.field v "id");
    total = Value.as_float (Value.field v "total");
    items = List.map __row_items_of_value (Value.as_list (Value.field v "items")) }
```

The container-shape reconstruction that Rust needs `Reshape` for (`Vec<T>` → `Vec<Inner>`, single → `Inner`) is **just `List.map` vs. a direct call in OCaml** — the ppx knows from the field witness whether the field is a list (`obj_field` with a list `'ty`) or a single object and emits the right combinator. There is no `Reshape` trait, no blanket-impl overlap reasoning: OCaml expresses the same thing with a plain conditional in the code generator. This is the single biggest "OCaml is more natural" win in the subsystem.

**Data masking, concretely:** because `__row_items` only has `sku` and `qty`, a component handed a `__row_items` *physically cannot* read `price` even if the parent fetched it — exactly Relay masking, and exactly what the Rust `Fragment`-generated struct achieves, but here it is the ordinary OCaml record-field-visibility rule rather than a generated newtype.

**Why not pure row types?** OCaml object/row types (`< sku:string; qty:int >`) would also work and would let the *same* decoder serve any superset, but they (a) leak into inferred signatures verbosely and (b) don't give the masking guarantee as crisply (width subtyping would let a wider object flow where a narrower is expected). Generated **nominal records** give better error messages and exact masking, so the ppx emits records by default; rows remain available for advanced hand-written selections. GADTs are *not* needed for exact-fit (the field witnesses already carry the type), but a GADT *is* the right tool for the heterogeneous `Value` decoder dispatch if we ever want a single `decode : ('a sel) -> Value.t -> 'a` interpreter — noted as an open design alternative.

---

### 6. `[@@deriving gql]` and `%gql_root` — schema-as-types

**`[@@deriving gql]`** replaces `#[derive(GqlObject)]` + the `#[gql(id)]` field attribute, ported as a ppxlib deriver with a `[@gql.id]` field attribute (per the spine). For a model type it generates, mirroring `derive_gql_object` one-for-one:

- the `Order_fields` witness module (§5(A));
- `typename : string` constant (`= "Order"`);
- `gql_id : Order.t -> Value.t` (the `[@gql.id]` field via `Into_value`, or `Null` for value objects with no id — exactly the Rust "id optional → value object → not normalized → inlined" rule, which is how `Connection`/`Edge`/`PageInfo` work);
- `gql_field : Order.t -> string -> Value.t option` (name → `Into_value` of that field, the executor's runtime accessor);
- `to_value` that **injects `__typename` and `__id` first**, then the fields — this is what makes the store able to normalize anything a resolver returns;
- `of_value`.

```ocaml
type t = { id : string; [@gql.id]  total : float;  items : Item.t list }
[@@deriving gql]
(* generates typename/gql_id/gql_field/to_value/of_value + Order_fields witnesses *)
```

The `__typename`/`__id` injection in `to_value` is the exact Rust behavior (`("__typename", Str name); ("__id", gql_id self)` prepended). OCaml is a **wash** — ppxlib record introspection is as ergonomic as `syn` field iteration, and arguably cleaner (no `Fields::Named` matching boilerplate).

**`%gql_root` — "write the methods, that *is* the schema"** replaces `#[gql_root(query|mutation|subscription)]`. The Rust attribute turns an `impl Query { fn stocks(&self) -> Vec<Stock> {..} fn stock(&self, id: String) -> Vec<Stock> {..} }` into (a) a type-level schema visible to both ends (`struct QueryRoot;` + `impl Field<gqlf::stocks> for QueryRoot { type Ty = Vec<Stock>; }`) and (b) a native-only resolver that extracts args by type via `FromArg` and dispatches by field name. The OCaml `%gql_root` is a structure-item ppx over a module of functions:

```ocaml
module%gql_root Query = struct
  let stocks ()        : Stock.t list = Db.all ()
  let stock  ~(id:string) : Stock.t list = Db.by_id id
end
(* generates:
   - QueryRoot field witnesses: stocks -> Stock.t list, stock -> Stock.t list  (both ends)
   - native-only resolve : string -> Args.t -> Value.t  dispatching by field name,
     extracting labelled args via From_arg, returning (to_value of result) *)
```

The function's **return type is the field type** (the ppx reads it from the type annotation, the same way the Rust macro reads `m.sig.output`), and **labelled arguments are the GraphQL args** (`~id:string` → `Args.str args "id"`), replacing the `FromArg` trait with a small `From_arg` first-class-module set (`string`/`int`/`float`/`bool`, extensible for custom scalars). The app aggregates the per-root `resolve` into the single injected `resolver` (a 3-way `match` on `OpKind`), exactly as the Rust app does in `bin/ssr.rs`. This is a **wash to a slight win**: OCaml's labelled args map onto GraphQL named args more directly than Rust's positional `fn` params + name-string extraction, so there's no `pname_s`/`call_args` zip dance.

**The `gqlf` marker module is gone.** Rust needs `gql_fields!` to centrally declare zero-sized marker types because `Field<M>` is keyed by a nominal type and proc-macros can't share a global registry. OCaml keys field projection by the *witness value* in the generated `*_fields` module instead, so there is **no separate marker-declaration step** — consistent with the spine's "`%gql_fields` is not needed; the ppx derives field markers from the schema." This is a small **DX win**: one fewer thing the author writes and one fewer place to forget a field.

---

### 7. The fetch ppx surface — `%query` / `%resource` / `%subscription` / `%mutation` / `%paginated` / `%fragment`

All of these compile against the witnesses of §5–6 and wire into the store of §3 through the **fetch-handler indirection** of §8. The shared core (`expand_fetch`) generates: the exact-fit selection record(s), a `keys : string list signal`, a registered fetch handler, the transport call, and a `memo` that re-reads the store.

```ocaml
(* %query order(id: oid) { id total items { sku qty } }  ->  __row list signal *)
let rows : __row list Signal.t =
  let keys = Signal.make [] in
  let h = Dom.on_fetch_handler (fun text ->
    let v = Value.parse text in
    (* skip-merge on error: never write an errors object into the store *)
    match Value.errors_message v with
    | Some _ -> ()                                  (* keep last-good rows *)
    | None ->
      let payload =
        match Value.get v "data" with
        | Some d -> (match Value.get d "order" with Some p -> p | None -> v)
        | None -> v in
      let mk = Store.merge_all payload in
      Signal.set keys mk; Store.bump_all mk)
  in
  Reactive.on_cleanup (fun () -> Dom.drop_fetch_handler h);   (* reclaim on scope dispose *)
  Dom.gql (build_query_string ()) h;
  Reactive.memo (fun () ->
    List.filter_map Store.read_entity (Signal.get keys)
    |> List.map __row_of_value)
```

Key per-feature points and where OCaml lands:

- **`%query`** — one-shot fetch; returns `__row list signal` (a `memo` over the store). **Wash.** The query *string* is built at runtime (`build_query_string`) so runtime variable args work; literal args are baked, variable args (`order(id: oid)`) go through `to_gql_arg` (escaped) — same `emit_args` logic.
- **`%subscription`** — identical to `%query` but the transport is `Dom.subscribe` (opens an SSE stream and keeps calling the handler). The TodoList demo uses a `%subscription` on the full list as the *source of truth* to neatly sidestep list-invalidation. **Wash.**
- **`%resource`** — the reactive query (Solid `createResource` / Leptos `Resource`). Returns `(rows : __row list signal, loading : bool signal, error : string option signal)`. The fetch is wrapped in an `effect`: the query-string builder *reads the arg signals* (e.g. `search(q: Signal.get qs)`), so the effect subscribes to them; any arg change re-runs the effect → rebuilds the string → re-fetches (reusing the same handler slot `h`). **Edge-case (gaps progress 14): order of state writes on failure** — the handler sets `error := Some msg` *then* `loading := false` (error branch wins; never flash stale rows), and on success clears `error` *before* merging. **Edge-case: keep last-good rows on error** — the failure branch returns without touching `keys`, so the previous successful rows stay rendered. OCaml is a **wash** with Rust; the effect-wraps-fetch shape is identical.
- **`%mutation`** — returns a thunk `unit -> unit`. Supports optional, order-independent trailing options `~optimistic:(expr : Into_value)` and `~on_error:(string -> unit)`. The flow (ported exactly):
  1. `~optimistic` → `let opt = to_value pred in let snap = Store.snapshot (Store.keys_of opt) in Store.normalize_list opt` (predict + snapshot *before* sending → view updates instantly).
  2. On response: **always `restore snap` first** (undo the optimistic write), *then* decide. If `errors_message v = Some msg` → call `on_error msg` and **do not** normalize garbage (we stay in the rolled-back state). Else `normalize_list payload` (write real values).
  3. **Edge-case (gaps progress 14, the `Rc<dyn Fn>` fix): `on_error` must not break the outer closure.** Rust had to wrap `on_error` in `Rc` so the per-call handler can clone it without moving captured variables out (which would degrade the outer `Fn` to `FnOnce`). **OCaml has no ownership, so this entire class of bug evaporates** — `on_error` is just a `string -> unit` value captured by the handler closure; GC handles sharing. This is a clean **GC win**, and a place where the OCaml port is strictly *simpler* than Rust.
  4. The `target` first argument is purely syntactic in Rust (the store auto-updates all views referencing the entity); OCaml keeps it only as an optional doc-affordance and does not bind it (mirroring the Rust fix that *removed* the `let _ = &target` so it doesn't capture the signal).

  Compile-time field validation (`mutation_checks`) — every selected scalar must exist on the mutation-root element type, every object field must be an object — is enforced in OCaml simply by *using the field witnesses* in the generated decoder: a non-existent field has no witness → unbound-value error at compile time, a scalar-selected-as-object uses a `field` witness where an `obj_field` is required → type error. So OCaml needs **no separate `PhantomData` check pass** — the generated `of_value` *is* the check. Small **win**.

- **`%paginated`** — Relay connection cursor pagination. Returns `(edges, load_next, has_next, loading)` where `edges` is a `memo` over `read_connection "@conn:<field>"`. The ppx navigates types `QueryRoot.field → Connection`, `Connection.edges → Edge`, `Edge.node → node-elem` via witnesses (replacing the `Field<gqlf::edges>`/`Field<gqlf::node>`/`Field<gqlf::cursor>` projections), synthesizes `__row` (node) and `__edge { node; cursor }` records, fetches the first page immediately (`fetch "" ~append:false`), and `load_next` is **throttled** (`if has_next && not loading then fetch cursor ~append:true`) — the same dedup guard against double-clicks. `first:` must be an integer literal (the ppx errors clearly otherwise, mirroring the Rust `LitInt` requirement). **Wash.**
- **`%fragment`** — `[%fragment Name on Type { fields }]` generates the named exact-fit record `Name` (the masking type) plus a `selection : string` constant (replacing `Fragment::SELECTION`). A `%query` spread `...Name` inlines `Name.selection` into the query string at runtime and stores a `Name` sub-record in the parent (reading the *same* object — fragment fields are inlined in the parent object). As in §5(B), OCaml's record-field visibility *is* the masking. **Win** (no generated newtype-and-trait, just a record + a string constant).

**`to_gql_arg` / arg formatting.** A small module type `To_gql_arg` with `string` (quote+escape via `gql_escape` = replace `\`→`\\`, `"`→`\"`), `int`/`float`/`bool` (bare). The query-string builders call it for variable args. **Wash.**

---

### 8. One-shot fetch-handler reclaim + ghost-write protection

The transport indirection is a slot registry: `on_fetch_handler : (string -> unit) -> int` pushes a `Some handler` and returns its index; `drop_fetch_handler : int -> unit` sets the slot to `None`; `run_fetch : int -> string -> unit` looks up the slot and **no-ops if it is `None`**. This solves two real bugs the gaps log records (progress 16/17):

- **Handler leak:** every navigation used to leak a handler (and its captured signals) forever. Fix: each `%query`/`%resource`/`%subscription` registers `Reactive.on_cleanup (fun () -> Dom.drop_fetch_handler h)`, so the page/leaf scope dispose reclaims the slot.
- **Ghost writes:** an in-flight response for an abandoned page would write stale data into the store. Fix: because dispose nulled the slot, `run_fetch` flattens `None` and does nothing.

OCaml ports this verbatim — `int array`/`(string -> unit) option array` (or a growable `Buffer`-like vector) — and again **GC makes the cleanup correctness-only, not memory-mandatory**, matching the spine's P2 principle. The `eval`/`run_js` escape hatch (progress 17) reuses the same slot mechanism with a status-byte error channel mapped to `(string, string) result` — i.e. `Rui_dom.S.eval : string -> ((string,string) result -> unit) -> unit`.

**SSR data handoff (progress 5).** During SSR, native `Dom.gql`/`subscribe` deliver synchronously (the resolver runs in-process) *and* record `(query_string → response)` into `SSR_RESP`; `dehydrate_responses ()` serializes them into a `<script id=__rui_data>` (with `</` → `<\/` to prevent tag-truncation); on the client, `seed_responses json` seeds a `HYDRATE_RESP` table, and client `Dom.gql` first looks up the exact query string — **hit → deliver synchronously, consume once, skip the network POST**; subsequent SPA navigations re-fetch. `subscribe` delivers the seeded initial value first (so hydration matches SSR) then opens the live SSE. All of this is `Rui_dom.S` surface (`gql`, `subscribe`, `seed_responses`, `dehydrate_responses`, `set_hydrate`) — the dune virtual-library backends provide native vs jsoo implementations. **Wash** (the string-keyed dedup is identical), with the bonus that the native store-reset story is cleaner (§3).

---

### 9. Edge-case ledger (each real Rui fix → OCaml handling)

| Rui edge-case (cite) | OCaml handling |
|---|---|
| `errors_message` hardening: non-object / missing data&errors / errors-not-a-list / HTTP error page all = failure; `{data,errors:[]}` & `{data,...}` = success; error w/o message → placeholder (value.rs tests; progress 14) | Ported branch-for-branch as a `match` over `Value.t`; same unit tests. |
| Skip-merge on error for `%query`/`%subscription` (don't pollute store) (progress 14) | Handler early-returns on `errors_message <> None`; `keys` untouched → last-good rows kept. |
| Write-all-then-bump for a consistent snapshot (store.rs header; `merge_all`) | `merge_all` merges whole batch first; caller bumps top-level keys after `Signal.set keys`. |
| `bump` under `untrack` (no spurious dependency edges) | `untrack (fun () -> Signal.set ver (Signal.get ver + 1))`. |
| Nested entity extracted to its own entity; cross-query update visible (store.rs tests) | `normalize_value` extracts entities to `$ref`; `denormalize` recursively `read_entity`s → subscribes nested versions. |
| Value objects (no id) inlined, not normalized (Connection/Edge/PageInfo) | `entity_key = None` when `__typename`/`__id` absent → fields normalized in place; `[@@deriving gql]` emits `gql_id = Null` for id-less types. |
| Connection cursor de-dup on append (idempotent load-more) (store.rs `connection_append_and_node_update`) | `if c <> "" && already-present then skip`. |
| `%paginated` load_next throttle (no double-fetch with same cursor) | `if has_next && not loading then fetch`. |
| Optimistic predict → snapshot → restore-then-decide; failure stays rolled-back (mutation codegen) | `snapshot (keys_of opt)`; on response `restore snap` first, then merge or `on_error`. |
| `on_error` must not degrade outer closure to `FnOnce` (the `Rc<dyn Fn>` fix, progress 14) | **Eliminated by GC** — `on_error : string -> unit` is a captured value; no `Rc`, no move-out hazard. |
| `mutation` `target` must not capture the signal (the removed `let _ = &target`, progress 9) | OCaml ppx parses but does not bind `target`. |
| Fetch-handler leak + ghost write on abandoned page (progress 16) | `on_cleanup → drop_fetch_handler`; `run_fetch` no-ops a `None` slot. |
| Parser progress guards = anti-DoS (parser.rs `malformed_input_terminates`) | Same per-loop byte-advance guards; same termination test. |
| Executor always preserves `__typename`/`__id`, dedupes if client selected them (exec.rs `project`) | Ported `project` exactly. |
| SSR thread-local destruction abort (progress "踩坑+修复") | **Cannot occur**: OCaml SSR uses per-request render context + `Store.reset`; no thread-exit user destructor. |
| `memo` value-dedup so an entity bump producing an equal row doesn't cascade (progress 13 `PartialEq` dedup) | `Reactive.memo ?equal` (default `=`) ports the dedup; `%resource` arg memos benefit. |
| `resource` failure: set error before clearing loading; clear error before merge (progress 14) | Handler orders the signal writes identically. |
| SSR response handoff `</` → `<\/` to prevent `<script>` truncation (progress 5) | `dehydrate_responses` escapes the same; `seed_responses` parses + consumes-once. |

---

### 10. Open questions / risks

1. **`Int of int` is 63-bit, not i64.** OCaml's native `int` is 63-bit on 64-bit platforms, but Rust uses `i64`. GraphQL ids and large counters can exceed 2^62. Options: keep `int` (matches jsoo's JS-number reality, where it's actually *worse* — 53-bit safe), or use `Int of int64` (faithful but boxes and is awkward in jsoo). Recommendation: `int` for the client (parity with JS), document the 53-bit limit; revisit if a native-only large-int field appears. **Decision needed.**
2. **Exact-fit encoding (A vs B).** §5 picks generated nominal records (B) for masking + error quality, with field-witness modules (A) as the projection mechanism. The pure-row-type alternative is more flexible but leaks into signatures and weakens masking. Worth a spike to confirm ppxlib generates clean records for deeply nested selections without name collisions (the `__rowN` counter scheme from Rust ports directly).
3. **Multi-domain SSR.** The store is module-global + `reset` per render, fine for single-threaded jsoo and one-render-per-request native. If SSR moves to OCaml 5 domains for parallelism, the store/version tables must move behind `Domain.DLS` or be threaded as an explicit per-request value — a non-trivial refactor of every `Store.*` signature. Flag now.
4. **Out-of-order `%resource` responses** (deferred in Rust progress 14). A fast re-fetch's single shared handler can't tell which request a response belongs to; a stale response can clobber a fresh one. The faithful port inherits this. Proper fix needs a per-run request id on the transport or a per-run handler (which reintroduces the leak the slot mechanism solved) — same trade-off Rust deferred.
5. **`%subscription` error channel** (deferred in Rust). `%query`/`%subscription` share the fetch core; subscriptions currently skip-merge on error but surface no error signal. Adding one means giving subscription a 2- or 3-tuple return (breaking its current single-signal shape) — decide whether to unify `%subscription` with `%resource`'s tuple.
6. **Mutation/paginated one-shot handlers still lack `on_cleanup`** in Rust (built at click time, no live scope). OCaml could attach them to the page scope, but a click-time mutation has no enclosing reactive scope — same structural issue. Consider a global generation counter to invalidate stale mutation handlers on navigation.
7. **ppx vs Rust macro hygiene.** The Rust macros lean on `crate::api::schema::QueryRoot`, `crate::data::model::T`, `crate::gqlf::*` path conventions. The OCaml ppx must adopt the spine's conventional module paths (`Model.<T>`, `Api.Schema`) and emit hygienic generated names (`__row0`, `__keys`, `__h`) — ppxlib's `gen_symbol` covers this, but the cross-module witness references (`Order_fields.items`) need the deriver and the `%query` ppx to agree on the generated module name; a stable naming contract (`<Type>_fields`) must be pinned across the two ppx entry points exactly as Rust pins `Field<gqlf::name>`.

## Routing: router!, params, nested groups, strategies

### What rui does today (ground truth)

rui has no router object, no route tree, and no `match` table you hand-write. The whole router is two cooperating pieces in `crates/rui/src/runtime.rs` (the isomorphic runtime) and the `router!` proc-macro in `crates/rui-macros/src/lib.rs`, plus the SSR `page()` dispatcher in `crates/rui/src/server.rs`. The shape:

- **A page is a value, not a closure-only entry.** `#[rui::page(ssr|csr|static, "/todo/:id")]` rewrites `fn view(id: Signal<i64>) -> View` into a function returning `Page { key, strategy, render }` where `key = module_path!()` (page identity), `strategy` is the `Strategy` enum, and `render` is a `Box<dyn FnOnce() -> View>` that captures the typed param bindings and defers the page body. The attribute also emits two `#[doc(hidden)] pub const`s on the page module: `__RUI_PATTERN: &str` (the literal route pattern) and `__RUI_STRATEGY: rui::Strategy`.
- **`router! { ... }` generates `pub fn route(path: &str) -> Page`.** It is a candidate list, not a tree — the macro folds the declared pages into an `if rui::matches(P::__RUI_PATTERN, path) { P::view() } else if … else { Page::new("not_found", Ssr, fallback) }` chain, first-match wins. A proc-macro can't build a global compile-time table (no `inventory`, zero-dep constraint), so you still list the pages, but you never write the `match`/literals/matchers yourself.
- **Routing is purely *signal-driven*.** Two thread-local signals are the source of truth: `PATH: Signal<String>` (the `location.pathname`, drives matching + path params) and `QUERY: Signal<String>` (raw `k=v&k=v`, drives query params). They are completely independent — `PATH` never sees the query, `QUERY` never participates in matching.
- **Path params are derived signals.** `param(i)` is a `memo` over `PATH` that splits on `/`, drops empty segments, takes the i-th. `param_as::<T>(i)` parses to `T`, falling back to `T::default()`. The `#[page]` attribute wires the i-th `:name` segment of the pattern to the i-th typed argument transparently — the author writes `id: Signal<i64>`, never an index.
- **Query params are a parallel, independent line.** `query_param("k")` / `query_param_as::<T>("k")` are `memo`s over `QUERY`, with percent + `+` decoding; `query_encode` is the symmetric writer; `query_string()` exposes the raw string. The asymmetry is deliberate: path is *structural/positional* (declared in the pattern + signature), query is *optional/named* (read on demand in the body) — mirrors React Router `useParams` vs `useSearchParams`.
- **`navigate` distinguishes same-key from cross-key.** SPA navigation builds the candidate `Page` (cheap — only constructs the value, doesn't run the body), compares its `key` to `CUR_KEY`. **Same key** → page stays mounted, only `set_path`/`set_query` fire → live `param`/`query_param` memos recompute → `resource!`s re-fetch, no flicker, no rebuild. **Different key** → dispose old page scope, `set_path`/`set_query`, `dom::clear_app()`, re-render. `go(route, url)` is programmatic navigation: `dom::push_url(url)` (history.pushState) then `navigate`. Intercepted `<a>`/`popstate` already pushed state in JS, so they call `navigate` directly.
- **Nested route groups reuse the reactive outlet, not a level-stack.** `group("/dash", layout = dash_shell) { pages::overview, pages::settings }` compiles to **one** `Page` with a fixed `key = "group:/dash"`; member patterns are prefix-concatenated at runtime (`matches("/dash" + member::__RUI_PATTERN, path)`); the group `render` is `dash_shell(path, View(reactive_block(|| select-leaf-by-path)))`. Because the key is fixed across the group, sibling nav (`/dash` ↔ `/dash/settings`) takes the same-key branch → `dash_shell` and its sidebar persist; only the `reactive_block` outlet (which subscribes to `path()`) swaps the leaf.
- **Three render strategies, dispatched server-side.** `page(route, path, query)` in `server.rs`: `Csr` → empty shell `doc("","")`; `Ssr` → render + inject dehydrated GraphQL responses; `Static` → first render via `ssr_doc`, then cache by a normalized `path?sorted-query` key in a 1024-entry `OnceLock<Mutex<HashMap>>`. The group's strategy is the strategy of whichever leaf the current path hits.

### OCaml design

Everything lives in `Rui_runtime` (`Rui.Runtime`), with the backend-dependent calls going through `Rui_dom.S`. The two source-of-truth signals are module-level `ref`s holding `Rui_reactive.signal`s, lazily created (mirrors the Rust `thread_local` `Signal::new`). Native and jsoo share the *same* `Rui_runtime` source; only the `Rui_dom` implementation differs (dune virtual library `rui.dom` with `rui.dom.client` / `rui.dom.ssr`), which is the idiomatic answer to Rust's `#[cfg(target_arch="wasm32")]`.

#### Core types and signatures (`rui_runtime.mli`)

```ocaml
(* A page = strategy + identity key + deferred render thunk. Ported from view::Page. *)
type route = string -> Rui_view.page
(* Rui_view.page = { key : string; strategy : strategy; render : unit -> Rui_view.t }
   Rui_view.strategy = Ssr | Csr | Static *)

(* ── path: the routing source of truth ── *)
val path        : unit -> string Rui_reactive.signal
val param       : int -> string Rui_reactive.signal
val param_as    : (module Parsable with type t = 'a) -> int -> 'a Rui_reactive.signal
val matches     : pattern:string -> path:string -> bool

(* ── query: a separate, independent source of truth ── *)
val query_string   : unit -> string Rui_reactive.signal
val query_param    : string -> string Rui_reactive.signal
val query_param_as : (module Parsable with type t = 'a) -> string -> 'a Rui_reactive.signal
val query_encode   : string -> string

(* ── navigation ── *)
val render_path : route -> string -> unit   (* first paint / full render *)
val navigate    : route -> string -> unit   (* SPA: same-key no-rebuild, cross-key rebuild *)
val go          : route -> string -> unit   (* programmatic: push_url + navigate *)

(* ── SSR seeding (native): set signals before the first paint so param()/query_param() read right ── *)
val set_current_path  : string -> unit
val set_current_query : string -> unit

(* ── on_mount queue + nav generation (jsoo real, native no-op) ── *)
val on_mount     : (unit -> unit) -> unit
val flush_mounts : unit -> unit
```

`param_as` / `query_param_as` take a first-class **parser module** rather than relying on a `FromStr + Default` bound (OCaml has no such ad-hoc trait surface). This is the single biggest mechanical shift in this subsystem:

```ocaml
module type Parsable = sig
  type t
  val of_string : string -> t option   (* None on parse failure *)
  val default   : t                     (* fallback, ports T::default() *)
end
```

The `#[rui::page]` → `[@page]` ppx emits these module references for the author. Stock instances ship in `Rui.Parsable` (`Int`, `Int64`, `Float`, `Bool`, `String`).

#### param / matches implementation (faithful port)

```ocaml
let split_segments s =
  String.split_on_char '/' s |> List.filter (fun x -> x <> "")

let param i =
  let p = path () in
  Reactive.memo (fun () ->
    match List.nth_opt (split_segments (Signal.get p)) i with
    | Some s -> s | None -> "")

let param_as (type a) (module P : Parsable with type t = a) i : a Signal.t =
  let p = path () in
  Reactive.memo ~equal:( = ) (fun () ->
    match List.nth_opt (split_segments (Signal.get p)) i with
    | Some s -> (match P.of_string s with Some v -> v | None -> P.default)
    | None -> P.default)

let matches ~pattern ~path =
  let ps = split_segments pattern and xs = split_segments path in
  List.length ps = List.length xs
  && List.for_all2
       (fun p x -> String.length p > 0 && p.[0] = ':' || String.equal p x)
       ps xs
```

`memo`'s value-dedup (`?equal`, defaulting to `(=)`) is the OCaml analogue of the Rust `PartialEq` short-circuit added in progress-13's review — it is load-bearing here, see edge cases below.

#### query: independent line, decode/encode

```ocaml
let lookup_query qs key =
  String.split_on_char '&' qs
  |> List.find_map (fun kv ->
       let k, v =
         match String.index_opt kv '=' with
         | Some i -> String.sub kv 0 i, String.sub kv (i+1) (String.length kv - i - 1)
         | None   -> kv, ""
       in
       if String.equal (pct_decode k) key then Some v else None)

let query_param key =
  let qs = query_string () in
  Reactive.memo (fun () ->
    match lookup_query (Signal.get qs) key with
    | Some v -> pct_decode v | None -> "")
```

`pct_decode` (handles `%XX` and `+` → space) and `query_encode` (RFC-3986-unreserved passthrough, everything else `%XX`) are straight ports of the Rust byte loops. In OCaml they're cleaner because we use `Buffer.t` and `Char.code`/`Printf.sprintf "%%%02X"` directly, with no `String::from_utf8_lossy` ceremony — but the algorithm is identical so the round-trip stays symmetric.

#### split_url and dispose-before-set ordering

```ocaml
let split_url full =
  let no_frag = match String.index_opt full '#' with
    | Some i -> String.sub full 0 i | None -> full in
  match String.index_opt no_frag '?' with
  | Some i -> String.sub no_frag 0 i,
              String.sub no_frag (i+1) (String.length no_frag - i - 1)
  | None   -> no_frag, ""

let set_path p =
  let s = path () in
  (* value-unchanged ⇒ don't write: avoids redundant param recompute / resource re-fetch
     when navigating to the same URL. Ports runtime.rs set_path's untrack guard. *)
  if not (String.equal (Reactive.untrack (fun () -> Signal.get s)) p) then Signal.set s p
```

#### navigate / go (the no-rebuild core)

```ocaml
let cur_key  : string option ref = ref None
let page_scope : Rui_reactive.scope option ref = ref None

let navigate route full =
  let p, q = split_url full in
  let page = route p in                       (* builds Page value; render thunk NOT run *)
  let same = (match !cur_key with Some k -> String.equal k page.key | None -> false) in
  if same then begin
    (* same page: stays mounted. set_path/set_query → live param/query_param memos
       recompute → resource! re-fetch. Group sibling nav lands here too: set_path
       synchronously drives the outlet's reactive_block (it subscribes to path). *)
    bump_nav_gen ();
    set_path p; set_query q;
    flush_mounts ()                            (* new leaf's on_mount must run now *)
  end else begin
    (* cross-key: dispose FIRST, then write signals (old memos already unsubscribed,
       new page not yet built ⇒ no ghost recompute), then clear + re-render. *)
    (match !page_scope with Some sc -> Reactive.Scope.dispose sc; page_scope := None | None -> ());
    set_path p; set_query q;
    Dom.clear_app ();
    cur_key := Some page.key;
    bump_nav_gen ();
    let node, sc = Reactive.scope (fun () -> page.render ()) in
    Dom.mount (Rui_view.node node);
    page_scope := Some sc;
    flush_mounts ()
  end

let go route full = Dom.push_url full; navigate route full
```

`render_path` is the same as the cross-key branch but unconditional (first paint / full render), and it disposes-then-sets before calling `route p` so the freshly-disposed old memos can't fire a phantom recompute against `PATH`/`QUERY`.

#### `%router { … }` ppx → `let route`

The ppx is the OCaml `router!`. It emits the same first-match `if/else` chain, but in OCaml the conditions read better because module access is `M.__rui_pattern` and the leaf selection in groups is an `if` expression, not a `quote!`-built token tree.

```ocaml
let%router () = {
  layout = View.Layout.shell;            (* optional global outer shell *)
  pages  = [ Pages.index; Pages.todo_detail; Pages.archive ];
  group ("/dash", View.Layout.dash_shell) [ Pages.overview; Pages.settings ];
  fallback = Pages.not_found;
}
```

generates:

```ocaml
let route (path : string) : Rui_view.page =
  let inner =
    if Rui_runtime.matches ~pattern:Pages.Index.__rui_pattern ~path then Pages.Index.view ()
    else if Rui_runtime.matches ~pattern:Pages.Todo_detail.__rui_pattern ~path then Pages.Todo_detail.view ()
    else if Rui_runtime.matches ~pattern:Pages.Archive.__rui_pattern ~path then Pages.Archive.view ()
    (* group("/dash") folds to one Page with a fixed key + reactive outlet *)
    else if
      Rui_runtime.matches ~pattern:("/dash" ^ Pages.Overview.__rui_pattern) ~path
      || Rui_runtime.matches ~pattern:("/dash" ^ Pages.Settings.__rui_pattern) ~path
    then begin
      let gp = path in
      let strat =
        if Rui_runtime.matches ~pattern:("/dash" ^ Pages.Overview.__rui_pattern) ~path
        then Pages.Overview.__rui_strategy
        else if Rui_runtime.matches ~pattern:("/dash" ^ Pages.Settings.__rui_pattern) ~path
        then Pages.Settings.__rui_strategy
        else Rui_view.Ssr
      in
      Rui_view.Page.make ~key:"group:/dash" ~strategy:strat (fun () ->
        View.Layout.dash_shell ~path:gp
          (Rui_view.View (Rui_view.reactive_block (fun () ->
             let lp = Rui_reactive.Signal.get (Rui_runtime.path ()) in
             if Rui_runtime.matches ~pattern:("/dash" ^ Pages.Overview.__rui_pattern) ~path:lp
             then (Pages.Overview.view ()).render ()
             else if Rui_runtime.matches ~pattern:("/dash" ^ Pages.Settings.__rui_pattern) ~path:lp
             then (Pages.Settings.view ()).render ()
             else Rui_view.text ""))))
    end
    else Rui_view.Page.make ~key:"not_found" ~strategy:Rui_view.Ssr Pages.not_found
  in
  (* optional global layout: keep inner's key/strategy, wrap render *)
  { inner with render = (fun () -> View.Layout.shell ~path (inner.render ())) }
```

The `[@page "ssr", "/todo/:id"]` attribute ppx (companion section) emits `__rui_pattern`, `__rui_strategy`, the `param_as` bindings, and the `Page.make` wrapper — so by the time `%router` runs, each page module exposes exactly the three symbols `view`, `__rui_pattern`, `__rui_strategy`.

#### How the outlet / no-rebuild works under the OCaml reactive model

The mechanism is identical to Rust and depends on exactly two reactive properties that `Rui_reactive` already guarantees:

1. **`reactive_block` (the `%view { fun () -> … }` runtime) is an `effect` over a slot anchor.** Inside the group render, `Rui_view.reactive_block (fun () -> select_leaf_by (Signal.get (path ())))` reads `path()` *inside the thunk*, so the effect subscribes to `PATH`. When sibling nav fires `set_path`, the effect re-runs, disposing the previous leaf's sub-scope (dynamic-dependency cleanup + `on_cleanup`) and building the new leaf into the same anchor. `dash_shell` — built once, outside the block — is untouched.
2. **same-key nav never disposes the page scope.** Because `key = "group:/dash"` is constant across the group, `navigate`'s `same` branch runs, so `page_scope` (which owns `dash_shell`, the sidebar, its `Uptime` interval, etc.) survives. Only `set_path` fires, and only the inner effect reacts.

This is the GC-vs-ownership shift made concrete: in Rust the leaf sub-scope must be explicitly disposed (`Scope::drop` runs `on_cleanup` then disposes effects) to tear down DOM listeners / `setInterval` / SSE subs; in OCaml the *memory* is GC'd, but we **keep `Scope.dispose` and `on_cleanup` verbatim** because those side-effects are observable and must die deterministically when the outlet swaps. The dynamic-dependency cleanup (an effect unsubscribing from stale signals before re-running) is a correctness property and is ported exactly.

#### SSR `page()` dispatch + static cache key (port)

```ocaml
let page route path query =
  Rui_runtime.set_current_path path;     (* first-paint param() reads correct segment *)
  Rui_runtime.set_current_query query;   (* first-paint query_param() reads correct value *)
  let pg = route path in
  match pg.strategy with
  | Csr    -> doc "" ""                                   (* empty shell, no data *)
  | Ssr    -> ssr_doc pg                                  (* render + inject dehydrated responses *)
  | Static ->
    let key =
      if query = "" then path
      else
        let parts = String.split_on_char '&' query |> List.filter ((<>) "")
                    |> List.sort String.compare in           (* ?a=1&b=2 ≡ ?b=2&a=1 *)
        Printf.sprintf "%s?%s" path (String.concat "&" parts)
    in
    match Hashtbl.find_opt static_cache key with
    | Some html -> html
    | None ->
      let html = ssr_doc pg in
      if Hashtbl.length static_cache < 1024 then Hashtbl.replace static_cache key html;  (* flood guard *)
      html
```

The HTTP entry splits the request target into `(path, query)` the same way `split_url` does — strip `#fragment`, split on first `?`, with empty pathname normalized to `/`. On native there is no `Mutex` needed unless the server is multi-domain/parallel; with OCaml 5 effects-based or domain-parallel serving, `static_cache` becomes a `Mutex`-guarded `Hashtbl` (or `Domain.DLS` per-domain), which we call out as a risk below.

### Feature → OCaml mechanism map (cleaner / wash / harder)

| rui feature | OCaml mechanism | Verdict |
|---|---|---|
| `router!` route table | `%router` ppx → `if/else` chain over `matches` | **Wash.** Still a candidate list (no global table in either language). OCaml reads slightly cleaner (`M.__rui_pattern`, real `if` not `quote!`). |
| `Page { key; strategy; render }` | record with `render : unit -> Rui_view.t` | **Cleaner.** No `Box<dyn FnOnce>`; the closure is a first-class GC value. |
| `param`/`param_as` typed at page | `param`, `param_as (module P)` + `[@page]` wiring | **Harder/wash.** Rust uses `FromStr + Default` inference from the signature; OCaml needs an explicit `Parsable` module per type (the ppx generates it). More principled (no silent `Default`), but more machinery. |
| `matches` | `List.for_all2` over split segments | **Cleaner.** `for_all2` + a length guard is one expression; Rust needs `zip` + `len()==len()` separately. |
| query params (independent signal) | second module-level signal `QUERY` + `query_param*` memos | **Wash.** Same two-line design; the path/query split is identical. |
| decode/encode | `Buffer.t` ports of the byte loops | **Cleaner.** No `from_utf8_lossy`; `Char`/`Buffer`/`Printf` are direct. |
| navigate same-key vs cross-key | `key` compare + same/cross branches | **Wash.** Logic ported verbatim; GC removes the `take()`/drop ordering boilerplate but the dispose-before-set discipline stays. |
| `go` (pushState) | `Dom.push_url` then `navigate` | **Cleaner on client.** jsoo + Brr call `History.push_state` typed and directly — no FFI marshalling, no `alloc`/`ptr`/`len` dance, so the entire `alloc`/`render_route`/`navigate_route`/`dispatch` C-ABI surface from `client!` simply disappears (replaced by `Rui_client` registering Brr event listeners). |
| nested groups (reactive outlet) | one `Page` (`key="group:<prefix>"`) + `reactive_block` over `path()` | **Wash.** Identical design; variants/records make the leaf-selection `if`-chain marginally nicer. |
| strategies ssr/csr/static | `Rui_view.strategy = Ssr \| Csr \| Static` variant | **Cleaner.** A closed variant the `match` in `page()` is exhaustive over — the compiler enforces all three arms, which Rust gets too but here `Static`-keyword-vs-`Ident` parsing pain in the page attribute disappears. |
| per-group strategy | `if matches … then leaf.__rui_strategy else …` | **Wash.** Same fold. |
| SSR `page()` + static cache key | `Hashtbl` + sorted-query key + 1024 cap | **Wash, slightly harder on native.** Rust's `thread_local` request isolation is free; OCaml multicore needs explicit `Mutex`/DLS (see risks). |
| `on_mount` queue + nav generation | `(unit -> unit) Queue.t` + `int ref` generation counter | **Cleaner.** GC removes `Box<dyn FnOnce>` and `Scope::take_parts/absorb_parts` is simpler (still kept for parity). |

### Edge cases rui solved — and how the OCaml design handles each

These are the subtle bugs the rui gap-log (`rui-framework-gaps.md`, progress 11–16) recorded fixing. The OCaml design must address every one:

1. **`?query`/`#fragment` leaking into `param`/match (progress 12).** rui's first cut let `/todo/1?x=1` make `param(1) == "1?x=1"` and `/about?utm=x` fall to 404. **Fix ported:** `split_url` (client) and the HTTP target split (server) strip `#fragment` and split on the first `?` *before* anything touches `PATH`; `matches`/`param` never see the query. The SSR entry normalizes empty pathname to `/`.

2. **Same-URL navigation causing redundant re-fetch (progress 12).** Signals/memos do no built-in dedup, so navigating to the identical URL would rewrite `PATH`, recompute the param memo, re-notify, and re-run every subscribing `resource!`. **Fix ported:** `set_path`/`set_query` guard with `untrack (fun () -> Signal.get s)` and skip the write when unchanged.

3. **Unrelated query key changing triggers a re-fetch (progress 13 review).** `?q` unchanged but `?sort` changed still re-notified the `q` memo → `resource!` re-fetched needlessly. **Fix ported:** `Rui_reactive.memo` does **value-equality dedup** (`?equal`, default `(=)`): on recompute, if the new value equals the cached one, downstream is *not* notified. This is the OCaml port of the Rust `PartialEq` short-circuit and is exactly why `param_as`/`query_param_as` pass `~equal`. All call-site types (`string`, `int`, derived-PartialEq rows) support structural `=`; `~equal` lets an author pick physical/custom equality where `=` would loop or be wrong (e.g. functional values).

4. **Ghost recompute when writing signals before disposing the old page (progress 13 review).** Originally `navigate`/`render_path` wrote `PATH`/`QUERY` first, so the *about-to-die* old page's memos/resources recomputed and wasted fetches (doubled once query was added). **Fix ported:** cross-key `navigate` and `render_path` **dispose the old page scope first**, *then* write signals — at that instant old memos have unsubscribed and the new page isn't built, so nothing phantom-recomputes.

5. **Query value percent/`+` not decoded, encode missing (progress 13 review).** `?q=hello%20world` searched the literal `"hello%20world"` and never matched. **Fix ported:** `pct_decode` runs in `query_param`/`query_param_as` on read; `query_encode` is the symmetric writer the app calls when building `?q=`. The round-trip is value-for-value identical to rui's.

6. **Static cache key not normalized / unbounded (progress 13 review).** `?a=1&b=2` and `?b=2&a=1` rendered/cached twice, and arbitrary `?utm=…` strings let the cache grow without bound. **Fix ported:** the cache key sorts query parts (`?a=1&b=2 ≡ ?b=2&a=1`) and the `Hashtbl` is capped at 1024 entries; once full it stops caching and keeps rendering on demand.

7. **`on_mount` not firing on same-key (group) navigation, and dynamic-subtree on_mount being dropped (progress 15/16).** A group sibling-nav takes the same-key branch which returns early; without flushing, the new leaf's `on_mount` (focus / `setInterval` / 3rd-party init) would never run, leaking to the next dispatch. **Fix ported:** the same-key branch calls `bump_nav_gen (); … ; flush_mounts ()`. `flush_mounts` also runs at the tail of every event/fetch/interval entry so `%view` dynamic rebuilds get their mounts.

8. **`flush_mounts` re-entrancy when a callback navigates mid-flush (progress 16).** If an `on_mount` callback calls `go`/`navigate`, the old page is disposed but the outer `flush` would keep running remaining callbacks against the dead page. **Fix ported:** a `nav_gen` generation counter; `flush_mounts` snapshots the generation and aborts the remaining batch if it changed.

9. **Effects created inside `on_mount` callbacks leaking with no owner (progress 16).** `flush` runs after the page scope is popped, so an effect created in a callback had no owner. **Fix ported:** each callback runs inside a child `Reactive.scope`, whose parts are `absorb`ed into `page_scope` (the `Scope.take_parts`/`absorb_parts` names are kept for parity) so they die when the page changes.

10. **Native TLS-destruction abort on SSR thread teardown (the "important" pitfall).** In rui, when an SSR per-connection thread exits, `thread_local` destruction order could make `dispose_effect` touch an already-destructing `EFFECTS` TLS → process abort. **OCaml status:** with OCaml 5 the natural SSR model is per-request *scope* on a thread pool or per-domain, not per-thread `thread_local`s, so this exact abort doesn't arise — but the design must still ensure each request gets an isolated reactive scope (and store reset) and disposes it in a `Fun.protect`/`finally`, never relying on GC finalizer ordering for disposal. This is the OCaml analogue of rui's `try_with` guard: dispose explicitly at end-of-request, never from a finalizer.

11. **Known limitation carried over: typed param parse-failure silently falls back to default.** rui's `param_as` returns `T::default()` on garbage (e.g. `/todo/abc` for `Signal<i64>`). The OCaml `Parsable` keeps the same behavior (`default` on `None`) for parity, but because `of_string` returns `option`, a stricter page can use a `Parsable` whose `t = int option` to surface the failure — the proper-fix path rui deferred is *available* in OCaml without changing the core.

12. **Known limitation carried over: route overlap is declaration-order.** First-match wins, so literal routes must be listed before `:param` routes. The `%router` ppx documents this and (optionally, see open questions) can warn when a later pattern is shadowed by an earlier `:param` of equal arity.

13. **Known limitation carried over: group member pages can't use their own `:param`.** Param indices are absolute over the full path, so a `:param` inside a grouped page is offset by the prefix. Ported as-is; called out in open questions as the main place an OCaml redesign could improve (a relative-param API).

### Open questions / risks

- **Multicore static cache & request isolation.** rui leans on `thread_local` for per-request `PATH`/`QUERY`/store isolation. Under OCaml 5 domains we must decide: (a) keep a single-domain SSR loop (simplest, matches rui's per-conn-thread-but-serialized feel), (b) `Domain.DLS` for the runtime signals + a `Mutex`-guarded `static_cache`, or (c) thread a request context record explicitly instead of using globals. Option (c) is the cleanest long-term but diverges most from the rui design — needs a call.
- **`Parsable` ergonomics.** Passing `(module Rui.Parsable.Int)` at every `param_as` is verbose. The `[@page]` ppx hides it for declared params, but ad-hoc `query_param_as` calls in page bodies are noisy. Worth considering a small set of named helpers (`param_int`, `query_param_int`) as sugar.
- **Relative params in groups.** Should `%router` rebase param indices for grouped pages (subtract the prefix segment count), fixing limitation #13? Doable in OCaml because the ppx knows the prefix at expansion time; needs a design for how the page declares "my params are relative."
- **Shadowed-route lint.** Can the `%router` ppx statically detect a later literal route shadowed by an earlier `:param` route of equal arity and emit a warning? Patterns are literals known at expansion, so a best-effort check is feasible; full overlap analysis is not (patterns can differ in arity dynamically — they can't here, since arity is fixed by the literal).
- **Global outer shell still rebuilds on cross-key nav.** Like rui, only the *group's* inner layout persists; the global `layout = shell` is re-rendered on every cross-key navigation. A persistent-global-shell (Rust's deferred "level-stack") would need a larger redesign (an outlet at the top level too) — out of scope but noted.
- **`go` history/back semantics with the value-dedup guard.** Because `set_path` skips unchanged writes, pressing back to a URL whose pathname is identical but whose query differs must still update — verified the design splits and writes `QUERY` independently, but the back/forward + scroll-restoration interplay (Brr `History`/`popstate`) needs an integration test in `Rui_client`.
- **Lost-update / out-of-order responses on fast re-nav.** rui deferred the out-of-order `resource!` response problem (a single fetch handler can't tell which nav's response arrived). The OCaml port inherits it; if we add a per-run request id at the `Rui_dom.gql` layer it's cleanly solvable, but that's a data-layer change, flagged here because rapid same-key param nav is the scenario that triggers it.

## Lifecycle, Refs & JS Interop

This section ports rui's lifecycle hooks (`on_mount` / `on_cleanup`), element refs (`node_ref` + `ref={…}`), the imperative-DOM commands (`focus`, `scroll_into_view`, `set_interval`/`clear_interval`), and the JS escape hatch (`run_js` / `run_js_on` / `eval`) to OCaml on js_of_ocaml + Brr. It is the "keys to the outside world" subsystem: the places where the otherwise-declarative reactive core has to touch the real DOM imperatively, run third-party JS, and tear those side-effects down deterministically. It is where js_of_ocaml + Brr pay off the most — a large fraction of the hand-written two-way FFI in rui simply *disappears*.

### 1. What rui does today (ground truth)

rui has **no `wasm-bindgen`**. Every JS capability is a hand-written two-way FFI: Rust→JS via `extern "C"` imports resolved by `router.js`'s instantiate env (`dom.rs` `mod ffi`), and JS→Rust via the `#[no_mangle]` exports generated by `client!` (`runtime.rs`). Because of that, the lifecycle/ref/interop surface is deliberately small and string-oriented.

**`on_mount` (`runtime.rs:157-206`)** — register a callback to run *after this render's nodes are in the DOM*. It is **queued**, not run inline: callbacks go into a `thread_local MOUNT_QUEUE: Vec<Box<dyn FnOnce()>>`, and `flush_mounts()` drains them once the synchronous build/rebuild has finished (so the nodes referenced via `node_ref` actually exist). `flush_mounts` is called at the tail of *every* client entry point: `render_path`, `navigate` (both the same-key fast path and the re-render path), `dispatch` (events), `on_fetch` (query/resource/subscription results), and `run_interval` (timer ticks) — so dynamic sub-trees (`<Show>` opening, `<For>` adding rows) rebuilt by an event/fetch/timer get their `on_mount` too. On **SSR (native) it is a whole-cargo no-op** (`#[cfg(not(wasm32))]`), because the server has no DOM and runs no imperative side-effects. Each callback runs inside a child `scope()`; the resulting effect-ids + cleanups are `absorb_parts`'d into the page `Scope` so anything created in `on_mount` is disposed on page teardown. A `NAV_GEN` generation counter guards the drain loop so that if a callback navigates mid-flush, the remaining (now-orphaned) callbacks are dropped.

**`on_cleanup` (`reactive.rs:31-39`)** — register an unmount callback on the *current* `Scope`. `reactive.rs` keeps a `CLEANUPS` stack pushed/popped by `scope()`; on `Scope::drop` the cleanups run **before** the effects are disposed (so the node still exists and signals are still readable), then each owned effect is disposed. Outside any scope it is silently ignored. This is what tears down `set_interval`, removes listeners, and disposes third-party widgets.

**`node_ref` + `ref={…}` (`view.rs:39-53`)** — `NodeRef(Rc<Cell<u32>>)` holds the engine node-id (`0` = not mounted). The ppx's `ref={r}` on an element emits `{ let __rf = r; __rf.set(__n); }`, where `__n` is the id returned by `el()` — which works whether the node was *created* (CSR) or *claimed* (hydration via `claim_element`). `ref` on a component is a compile error.

**Imperative DOM (`dom.rs:117-158`)** — `focus`, `scroll_into_view`, and the interval pair. `set_interval(ms, f)` registers a Rust closure in an `INTERVAL_HANDLERS: Vec<Option<(timer_id, Rc<dyn Fn()>)>>` table, calls the JS `setInterval` with the slot id, and JS calls back into `run_interval(hid)`. `clear_interval(timer)` clears the JS timer **and nulls the slot** so the closure (and the signals it captured) is released. All of these are no-ops on SSR.

**JS escape hatch (`dom.rs:159-188`)** — `run_js(code)` (fire-and-forget, indirect `eval`, global scope), `run_js_on(node, code)` (`el` in the code is bound to that node), and `eval(code, f)` which returns a value through the fetch-handler machinery, supports Promises, and delivers a **`Result<&str, &str>`**: the JS side prefixes a status byte (`0x00` = ok, `0x01` = err), and the Rust handler strips it and dispatches `Ok`/`Err`. The handler is one-shot (reclaimed on delivery) and is also reclaimed via `on_cleanup` if its scope dies before the Promise settles. Values are stringified (`String(r)`); objects must be `JSON.stringify`'d by the caller. SSR: all three are no-ops.

### 2. OCaml design

The whole subsystem lives behind the `Rui_dom.S` virtual library (so `Rui_view`/`Rui_runtime` are written once), with `Rui_dom_client` (jsoo + Brr) and `Rui_dom_ssr` (native, no-op) implementations selected per dune target. Lifecycle queueing/flushing lives in `Rui_runtime` and `Rui_reactive`; refs live in `Rui_view`.

#### 2a. `on_cleanup` and the scope machinery (`Rui_reactive`)

```ocaml
(* rui_reactive.mli — lifecycle-relevant surface *)
type scope
val scope      : (unit -> 'a) -> 'a * scope
val on_cleanup : (unit -> unit) -> unit          (* attach to the current scope; no-op if none *)

module Scope : sig
  val dispose      : scope -> unit                (* run cleanups, then dispose owned effects *)
  (* ppx/runtime-internal, kept for spine parity though the impl is plain lists *)
  val take_parts   : scope -> effect_handle list * (unit -> unit) list
  val absorb_parts : scope -> effect_handle list -> (unit -> unit) list -> unit
end
```

```ocaml
(* impl sketch — note: no Rc<RefCell>, no Drop, no TLS-destruction hazard *)
type scope = {
  mutable effects  : effect_handle list;   (* owned effects/memos, in creation order *)
  mutable cleanups : (unit -> unit) list;  (* on_cleanup callbacks, in creation order *)
  mutable disposed : bool;
}

let owner_stack   : scope list ref = ref []   (* replaces Rust OWNER + CLEANUPS stacks, unified *)

let on_cleanup f =
  match !owner_stack with
  | s :: _ -> s.cleanups <- f :: s.cleanups    (* prepended; run order reverses below *)
  | []     -> ()                               (* no owner → ignored, same as rui *)

let scope thunk =
  let s = { effects = []; cleanups = []; disposed = false } in
  owner_stack := s :: !owner_stack;
  let r = Fun.protect ~finally:(fun () -> owner_stack := List.tl !owner_stack) thunk in
  (r, s)

let dispose s =
  if not s.disposed then begin
    s.disposed <- true;
    List.iter (fun c -> c ()) (List.rev s.cleanups);   (* cleanups BEFORE effect disposal *)
    List.iter Effect.dispose (List.rev s.effects)        (* nested child scopes recurse here *)
  end
```

Key fidelity points: cleanups run **before** effect disposal (rui's invariant), in registration order (`List.rev` of the prepended list), and `dispose` is idempotent (`disposed` flag) — OCaml has no `Drop`, so disposal is an explicit call from `Rui_runtime` on page/subtree teardown, exactly where rui's `Scope::drop` fires today.

#### 2b. `on_mount` (queued, flushed everywhere) — `Rui_runtime`

```ocaml
(* rui_runtime.mli *)
val on_mount     : (unit -> unit) -> unit   (* client: queue; native: no-op *)
val flush_mounts : unit -> unit             (* client: drain queue; native: no-op *)
```

```ocaml
(* rui_runtime_client.ml (jsoo target) *)
let mount_queue : (unit -> unit) Queue.t = Queue.create ()
let nav_gen = ref 0
let bump_nav_gen () = incr nav_gen

let on_mount f = Queue.add f mount_queue

let flush_mounts () =
  let gen = !nav_gen in
  let rec loop () =
    if not (Queue.is_empty mount_queue) then begin
      (* snapshot this batch; on_mount called *inside* a callback re-queues for the next loop *)
      let batch = Queue.fold (fun acc f -> f :: acc) [] mount_queue in
      Queue.clear mount_queue;
      let batch = List.rev batch in
      let rec run = function
        | [] -> loop ()
        | f :: rest ->
          if !nav_gen <> gen then ()            (* callback navigated → drop the orphaned tail *)
          else begin
            (* run in a child scope; absorb its effects/cleanups into the page scope so a memo
               created in on_mount is disposed on page teardown (no ghost effects) *)
            let (), child = Rui_reactive.scope f in
            let ids, cls = Rui_reactive.Scope.take_parts child in
            (match Rui_runtime_state.page_scope () with
             | Some p -> Rui_reactive.Scope.absorb_parts p ids cls
             | None   -> ());
            run rest
          end
      in run batch
    end
  in loop ()
```

On native (`Rui_runtime_ssr.ml`) both are `let on_mount _ = ()` / `let flush_mounts () = ()`. `flush_mounts ()` is invoked at the tail of `render_path`, `navigate` (both branches), and at every JS→OCaml re-entry — but in the jsoo port those re-entries are **Brr event callbacks / Promise continuations / `setInterval` ticks**, not `client!`'s `#[no_mangle]` exports. Concretely, the runtime wraps each such callback so `flush_mounts ()` runs after the user handler returns (see 2d/2e).

#### 2c. `node_ref` + `ref` (`Rui_view`)

```ocaml
(* rui_view.mli *)
type node_ref
val node_ref : unit -> node_ref
module Node_ref : sig
  val get : node_ref -> node option       (* None until mounted; replaces the `0 = unmounted` sentinel *)
  val set : node_ref -> node -> unit
end
```

```ocaml
(* impl — a plain mutable option cell on the GC heap, no Rc<Cell> *)
type node_ref = node option ref
let node_ref () = ref None
module Node_ref = struct
  let get r = !r
  let set r n = r := Some n
end
```

The ppx (`ppx_rui`) lowers `ref={r}` on an element to `Node_ref.set r __n` after the element id `__n` is bound — identical placement to rui, and it runs on both the create and hydrate paths (the claimed id is what gets stored). `ref` on a `<Component>` tag is rejected at expansion with a located error, matching rui. The `Some`/`None` typing is the first cleanliness win: the "not yet mounted" state is a real `None` instead of a magic `0`, so `Node_ref.get r |> Option.iter Dom.focus` can't accidentally focus node 0.

#### 2d. Imperative DOM (`Rui_dom.S`, client impl over Brr)

```ocaml
(* Rui_dom.S — the tri-backend signature; native impls are no-ops *)
val focus            : node -> unit
val scroll_into_view : node -> unit
val set_interval     : int -> (unit -> unit) -> interval_id
val clear_interval   : interval_id -> unit
```

```ocaml
(* Rui_dom_client.ml — Brr gives these directly; no FFI table, no JS→OCaml id round-trip *)
let focus n            = Brr.El.to_jv n |> fun jv -> ignore (Jv.call jv "focus" [||])
                         (* or: Brr.El.set_has_focus true n in newer brr *)
let scroll_into_view n = ignore (Jv.call (Brr.El.to_jv n) "scrollIntoView" [||])

type interval_id = Brr.G.timer_id
let set_interval ms f =
  (* wrap so on_mount queued by f's reactive rebuilds get flushed — replaces run_interval's tail flush *)
  Brr.G.set_interval ~ms (fun () -> f (); Rui_runtime.flush_mounts ())
let clear_interval id = Brr.G.stop_timer id
```

The big structural change: rui needs an `INTERVAL_HANDLERS` slot table because the timer must call *back into Rust* by integer id (the FFI can't pass a Rust closure to JS). Under jsoo, `Brr.G.set_interval` takes an **OCaml closure directly** and returns a `timer_id` we hold. So the entire `INTERVAL_HANDLERS` table, the `run_interval` export, and the "null the slot to release the closure" reclamation **all disappear** — `clear_interval` just stops the timer, and the GC reclaims the closure (and its captured signals) once the timer and the registering scope are gone. We keep `on_cleanup (fun () -> Rui_dom.clear_interval id)` as the idiom (the timer must still be stopped deterministically when the page unmounts), but the leak-prevention bookkeeping is gone.

#### 2e. The JS escape hatch — and why most of it is unnecessary under jsoo

```ocaml
(* Rui_dom.S *)
val run_js    : string -> unit                                   (* fire-and-forget, global scope *)
val run_js_on : node -> string -> unit                           (* `el` bound to node in the code *)
val eval      : string -> ((string, string) result -> unit) -> unit  (* value/Promise → Result *)
```

```ocaml
(* Rui_dom_client.ml *)
let run_js code     = ignore (Jv.call Jv.global "eval" [| Jv.of_string code |])
let run_js_on n code =
  (* bind `el` then eval, mirroring run_js_on's contract *)
  let f = Jv.callable (Jv.call Jv.global "Function" [| Jv.of_string "el"; Jv.of_string code |]) in
  ignore (Jv.apply f [| Brr.El.to_jv n |])

let eval code k =
  let cleaned = ref false in
  let reclaim () = cleaned := true in
  Rui_reactive.on_cleanup reclaim;                 (* scope dies before settle → drop, no ghost write *)
  let settle r = if not !cleaned then (cleaned := true; k r) in   (* one-shot: fire at most once *)
  let v = (try Jv.call Jv.global "eval" [| Jv.of_string code |]
           with Jv.Error e -> Jv.throw e) in
  match Jv.find v "then" with
  | Some _ ->                                       (* a Promise *)
    let fut = Fut.of_promise ~ok:(fun jv -> Ok (Jv.to_string (Jv.call Jv.global "String" [| jv |]))) v in
    Fut.await fut (function
      | Ok r              -> settle r
      | Error e           -> settle (Error (Jv.Error.message e)))
  | None ->
    settle (Ok (Jv.to_string (Jv.call Jv.global "String" [| v |])))
```

The critical point for the OCaml port: **under jsoo the escape hatch is mostly redundant.** rui needs `run_js("navigator.clipboard.writeText(...)")` etc. *because it has no typed bindings* — string eval is the only door to the platform. Brr gives typed, direct access, so the idiomatic rui interop string becomes a typed call with no `eval`:

| rui (string eval) | OCaml + Brr (typed, direct) |
|---|---|
| `run_js("navigator.clipboard.writeText(x)")` | `Brr_io.Clipboard.(write_text (of_navigator G.navigator) x)` (returns `unit Fut.or_error`) |
| `eval("navigator.language", …)` | `Brr.Navigator.languages G.navigator` (typed `string list`, **no callback, no Result-of-string**) |
| `eval("localStorage.getItem('k')", …)` | `Brr_io.Storage.get_item (Brr.Window.local_storage G.window) (Jstr.v "k")` (`Jstr.t option`) |
| `run_js("scrollTo(0,0)")` | `Brr.Window.scroll_to G.window ~x:0. ~y:0.` |
| `run_js_on(node, "el.querySelector(...)")` | `Brr.El.find_first_by_selector ~root:n …` (typed `El.t option`) |
| `eval("fetch(u).then(r=>r.text())", …)` | `Brr_io.Fetch.(url (Jstr.v u))` → `Fut.or_error` |

So the recommendation is: **prefer Brr for everything that has a binding**, and keep `eval` only as the genuinely untyped escape (a third-party global, an inline snippet) implemented over `Js_of_ocaml.Js.Unsafe` / `Jv`. The `(string, string) result` channel and the stringify caveat are preserved verbatim for `eval`'s untyped value, because that contract still matters when you reach a binding-less corner.

### 3. Feature → OCaml mechanism map (and where OCaml wins / washes / hurts)

| rui feature | OCaml mechanism | Cleaner / wash / harder |
|---|---|---|
| `on_mount` (queue + multi-site flush) | `Queue.t` + `flush_mounts` at the same sites, but sites are Brr callbacks/Promises/timers, not `#[no_mangle]` exports | **Wash** — the queue+flush discipline is intrinsic; OCaml just doesn't need the `alloc`/`dispatch`/`on_fetch`/`run_interval` export shims to reach them. |
| `on_mount` SSR no-op | `let on_mount _ = ()` in the native impl (dune virtual lib selects it) | **Cleaner** — `#[cfg(target_arch="wasm32")]` becomes a dune implementation choice; no cfg-gating in the source. |
| `on_cleanup` (scope-tied, before dispose) | `owner_stack` + `scope.cleanups`, run before `Effect.dispose` in `Scope.dispose` | **Wash** — same algorithm; OCaml folds rui's separate `OWNER` and `CLEANUPS` thread_locals into one `scope` record. |
| Scope teardown | explicit `Scope.dispose` (idempotent) instead of `Drop` | **Wash, and safer** — see edge-case 6 (the TLS-during-destruction abort goes away entirely). |
| `node_ref` / `ref` | `node option ref`; `Node_ref.get : node_ref -> node option` | **Cleaner** — `None`/`Some` instead of the `0`-means-unmounted sentinel; no `Rc<Cell<u32>>`. |
| `focus` / `scroll_into_view` | direct Brr / `Jv.call` | **Cleaner** — typed, no FFI declaration. |
| `set_interval` / `clear_interval` + handler reclaim | `Brr.G.set_interval` takes the closure directly; `Brr.G.stop_timer` | **Much cleaner** — the whole `INTERVAL_HANDLERS` slot table, `run_interval` export, and manual slot-nulling are deleted; GC + `on_cleanup`-driven `stop_timer` suffice. |
| `run_js` / `run_js_on` | `Jv.call …"eval"` / `Function("el", code)` | **Wash** (still untyped eval) — but rarely needed; Brr replaces most callers. |
| `eval` value + Result channel + Promise | `Fut.of_promise` + `Jv.to_string ∘ String(...)`; `Result` preserved | **Cleaner channel, same contract** — `Result` is native to OCaml (rui hand-rolled it from a status byte across the FFI); the stringify caveat remains. |
| Most platform APIs (clipboard/storage/navigator/scroll) | typed Brr / Brr_io modules | **Strictly cleaner** — these eval-strings *disappear* (table in 2e). |
| Where it's *harder* | a binding-less global, or a snippet that must run in true global scope | `Jv.Unsafe`/`eval` is slightly more ceremony than a raw FFI string, and you lose Brr's typing — but this is the genuinely-untyped corner, which is exactly where rui also had no safety. |

FFIs that **disappear** entirely in the jsoo port: `create_element`/`create_text`/`set_text`/`append_child`/`remove_child`/`set_attr`/`add_event`/`set_value`/`clear_children`/`mount`/`focus`/`scroll_into_view`/`set_interval`/`clear_interval`/`push_url` become typed Brr calls; the JS→Rust exports `alloc`/`render_route`/`navigate`/`dispatch`/`on_fetch`/`hydrate_data`/`set_hydrate`/`run_interval` and the `ptr/len` marshalling vanish because OCaml values cross the boundary directly (no wasm linear memory). What **remains as untyped escape**: `run_js`/`run_js_on`/`eval` over `Jv`, plus `gql`/`subscribe` (which call out to `fetch`/`EventSource` via Brr_io rather than a hand-written `gql_query` import).

### 4. Edge-cases rui solved → how the OCaml design handles each

These are the subtle bugs rui fixed (progress entries 15, 16, 17, and the reactive/DOM hardening). The OCaml design must address every one.

1. **`on_mount` not flushed in dynamic sub-trees** (rui progress 15 ①). `<Show>` opening or `<For>` adding a row is driven by an event/fetch/timer; if `flush_mounts` only ran at `render_path`/`navigate`, those `on_mount`s would be missed or fire on the *next* navigation. → OCaml runs `flush_mounts ()` at the tail of every Brr event callback, every resolved data Promise (`gql`/`resource`/`subscription` continuation), and every `set_interval` tick (the wrapper in 2d). Same coverage, different (cleaner) call sites.

2. **Owner-less effects created in an `on_mount` callback leak** (rui progress 15 ②). `flush_mounts` runs after `scope()` has popped, so the owner stack is empty and a memo created in `on_mount` would have no owner ("ghost effect"). → Each callback runs inside its own `Rui_reactive.scope`, and `Scope.take_parts` / `Scope.absorb_parts` fold its effects+cleanups into the current `page_scope`, so they're disposed on page teardown (2b/2b). Names kept for spine parity (`take_parts`/`absorb_parts`).

3. **`INTERVAL_HANDLERS` unbounded growth** (rui progress 15 ③). A `<Uptime/>` in the shell registers an interval on every navigation; rui's `Vec<Rc>` was push-only and pinned the captured `secs` signal forever, fixed by slot-nulling on `clear_interval`. → In OCaml this class of bug **cannot occur**: `set_interval` holds no global table; the closure is owned by the JS timer and the registering scope, and `on_cleanup (fun () -> clear_interval id)` stops the timer on unmount, after which the GC reclaims the closure and its captured signals. The fix is structural, not bookkeeping.

4. **Mid-flush synchronous navigation** (rui progress 15 ④). A callback that calls `go`/`navigate` disposes the current page; the outer flush loop must not keep running the remaining (orphaned) callbacks against a disposed page. → Ported verbatim: `nav_gen` is bumped on navigation, captured at the top of `flush_mounts`, and re-checked before each callback; a mismatch abandons the tail (2b).

5. **`eval` one-shot handler reclaim + ghost write** (rui progress 17 ①, ②, ③). rui's `eval` reclaimed its fetch-handler on delivery, also reclaimed it via `on_cleanup` if the scope died before the Promise settled (else a late resolve writes into a dead page), used a status byte instead of an in-band `"ERROR:"` prefix, and stringified values. → OCaml's `eval` (2e) keeps a `cleaned` flag making the continuation fire at most once; registers `on_cleanup reclaim` so a Promise that settles after the scope is disposed is a no-op (no ghost write, and a never-settling Promise's continuation is dropped when its scope dies); delivers a real `(string, string) result` (the status byte is unnecessary — `Fut.or_error` already carries success/failure); and preserves the `String(...)`/`JSON.stringify` stringify caveat. **Known carried-over limitation:** `eval` invoked from a top-level event handler with no live scope, whose Promise never settles, still cannot be reclaimed (no scope to hang `on_cleanup` on) — same as rui.

6. **`Scope` disposal during TLS destruction → process abort** (rui "踩坑+修复"). With `Scope: Drop`, an SSR worker thread ending would drop residual scopes whose effect closures held child scopes, re-entering `dispose_effect` while the `EFFECTS` thread_local was already being torn down → "cannot access TLS during destruction" → abort. rui fixed it with `EFFECTS.try_with(..)`. → In the OCaml port this **does not exist**: SSR runs on the native (`Rui_dom_ssr`) backend with no thread_local-Drop interplay, disposal is an explicit `Scope.dispose` call at well-defined points (not a destructor running at thread teardown), and the dispose loop is idempotent (`disposed` flag) so re-entry from a nested child scope is harmless. The whole `try_with` defensiveness is unnecessary. (The corresponding testing lesson — single-threaded harnesses can't catch the multi-thread TLS-destruction bug — also goes away, since the failure mode is gone.)

7. **`ref` under hydration** (rui progress 15, rejected-but-verified). `Node_ref.set r __n` must store the *claimed* node, not skip on the hydrate path. → Preserved: the ppx emits the `set` after `el`/`claim_element` returns, on both paths; `__n` is the claimed id during hydration, so a ref to an SSR-rendered node still resolves and `focus`/`scroll_into_view` work post-hydration.

8. **`on_mount` effects join the page scope, not the subtree** (rui progress 15, known limitation). Effects created in `on_mount` are disposed on page teardown, not on subtree removal — not a leak (they're owned), just retained longer. → Carried over identically; documented as the same accepted trade-off (most `on_mount`s are `focus`/timers that create no effects).

### 5. Open questions / risks

- **`focus` API surface in Brr.** Brr's `El.set_has_focus` covers the common case but `scrollIntoView` options, `select()` on inputs, etc. may need a small `Jv.call` shim or a few typed wrappers in `Rui_dom_client`. Decide whether to widen `Rui_dom.S` (e.g. `select`, `scroll_into_view ~block`) or expose them only through the `eval` escape. Widening keeps the typed-first story but grows the tri-backend signature (each addition needs a native no-op).
- **`run_js`/`Function("el", code)` global-scope fidelity.** `Jv.call Jv.global "eval"` is indirect eval (global scope), matching rui's `run_js`; but `run_js_on` via `new Function` does *not* see local app state the way an inline `eval` would. Confirm `new Function("el", code)` is an acceptable equivalent to rui's contract, or use `eval` with a pre-bound `el` via a small wrapper. Low stakes (rui's own `run_js_on` is "init a chart/editor on this node").
- **CSP / `eval` availability.** Both rui and this port rely on `eval`/`new Function`; under a strict Content-Security-Policy these throw. Since Brr replaces most callers, the practical risk is smaller than in rui (fewer eval sites), but `eval` should fail through its `Result`/`Error` channel rather than throwing — verify the `Jv.Error` path is caught for the synchronous-eval-throws case, not just Promise rejection.
- **`Fut` vs the one-shot guarantee.** Using `Fut`/`Fut.await` for `eval`'s Promise path is idiomatic, but `Fut` continuations can in principle be scheduled after the `on_cleanup` ran; the `cleaned` flag guards delivery, but confirm there's no path where `await` is itself the leak (it shouldn't be — `Fut` is GC'd with its scope). This is the OCaml analogue of rui's "never-settling Promise" limitation and should be load-tested.
- **`on_cleanup` ordering under nested scopes.** rui relies on `Scope::drop` recursing into child scopes held by effect closures. In OCaml, child scopes are disposed when their owning effect is disposed (`Effect.dispose`), so the runtime must ensure a reactive_block/keyed_for that creates child scopes registers them for disposal (via `on_cleanup` or by the effect owning them). Verify the recursion is preserved by construction, since OCaml has no `Drop` to lean on.
- **Handler/closure reclamation parity for one-shot mutation handlers.** rui notes `mutation!`/`paginated!` one-shot handlers built at click time (no live scope) still leak. The OCaml port's GC reclaims a closure once nothing references it, but a Promise pending on a `fetch` does reference its continuation; decide whether to require all such call sites to run inside a scope (so `on_cleanup` can drop them) or accept the same edge leak. This straddles the data-layer section but the lifecycle contract (`on_cleanup` is the only reclaim hook) is set here.

## Build, Tooling, Project Layout & Testing

This section ports rui's *build & developer experience* — not a feature of the framework so much as the machinery that lets one source tree compile to **two artifacts** (an SSR native binary and a client bundle), serve them, and be tested headlessly. It is the part that most directly benefits from leaving Rust+wasm behind for OCaml+js_of_ocaml, and it is also where rui's nastiest verified bug lives (the SSR thread-local-destruction abort).

### 1. What rui does today (ground truth)

rui is a ~4200-LOC from-scratch isomorphic full-stack framework. The build subsystem, as it exists in the tree, is:

- **A Cargo workspace** (`Cargo.toml`) with four members: `crates/rui` (the framework), `crates/rui-macros` (proc-macros), `crates/rui-cli` (the `rui` CLI), and `examples/stocks` (the demo app — a TodoList despite the name).
- **Two targets from one app crate.** `examples/stocks/Cargo.toml` declares `crate-type = ["cdylib", "rlib"]` plus `[[bin]] name = "ssr"`. The same `lib.rs` is compiled **twice**:
  - to `wasm32-unknown-unknown` as a `cdylib` → `app.wasm` (the client), and
  - to the native host as the `ssr` binary (the server), which links the `rlib` and calls `rui::serve(App { route, resolve, sse })`.
- **A cfg-split backend.** `crates/rui/src/dom.rs` selects the DOM implementation with `#[cfg(target_arch = "wasm32")]` (browser FFI backend) vs the native string/SSR backend; `crates/rui/src/lib.rs` and `src/server.rs` gate the whole server module behind `#[cfg(not(target_arch = "wasm32"))]`. So `Rui_view`/`Rui_runtime`/`Rui_gql` are written once and one of two `dom` impls is linked per target.
- **A zero-dependency CLI** (`crates/rui-cli/src/main.rs`) with three commands that replace the original `build.mjs`:
  - `rui init <name>` — scaffolds a project from `include_str!`-embedded templates (`crates/rui-cli/templates/*.tpl`), creating the `data`/`api`/`view` directory skeleton that the proc-macros assume.
  - `rui dev` — `cargo build --target wasm32-unknown-unknown --lib` → copy `target/.../{pkg}.wasm` to `web/app.wasm` → run Tailwind if present (else write an empty `web/styles.css` to avoid a 404) → `cargo run --bin ssr` (blocks).
  - `rui build` — same but `--release` and `--minify`.
- **An embedded client glue script.** There is **no bundler**. `crates/rui/src/assets/router.js` is the entire client runtime (`WebAssembly.instantiate`, the FFI `env` import object, SPA `<a>` interception, hydration index, data rehydration). It is baked into the server with `include_str!("assets/router.js")` and served at `/router.js`. `web/app.wasm` and `web/styles.css` are read off disk.
- **The SSR server** (`crates/rui/src/server.rs`) is pure `std`, zero-dependency, thread-per-connection, hardcoded `127.0.0.1:8084`. It serves `/graphql` (POST), `/graphql/subscribe` (SSE), `/router.js`, `/app.wasm`, `/styles.css`, and otherwise renders a page per the `#[rui::page]` strategy (Ssr/Csr/Static), injecting prefetched GraphQL responses into a `<script id="__rui_data">`.
- **Two headless test harnesses, no browser.** `examples/stocks/verify.mjs` and `hydrate.mjs` instantiate `app.wasm` under **Bun/Node** with a hand-written mock `env` (a plain JS object array standing in for the DOM tree), then drive `render_route`/`navigate`/`dispatch`/`on_fetch` and assert on the mock tree. `hydrate.mjs` is the famous **two-pass zero-create** test (see §7).

The canonical verification flow recorded in the gaps memo is: ① `cargo build` ② wasm build ③ `cargo test -p rui` ④ `bun verify.mjs` ⑤ `bun hydrate.mjs` ⑥ run the SSR server and hammer all routes ≥5 rounds (each connection = one thread teardown — this is the only thing that catches the TLS-destruction abort; see §8).

### 2. OCaml design — overview of the mapping

| rui (today) | Rui (OCaml) |
|---|---|
| Cargo workspace | `dune-project` + per-package `dune` files; opam packages `rui`, `rui.ppx` |
| `crates/rui` + `crates/rui-macros` | library `rui` + ppx `rui.ppx` (driver `ppx_rui`, via `ppxlib`) |
| `crate-type = ["cdylib","rlib"]` two-target compile | one library, **two executables**: native `ssr.exe` (`(modes exe)`) + client `app.bc.js` (`(modes js)`) |
| `#[cfg(target_arch="wasm32")]` in `dom.rs` | a **dune virtual library** `rui.dom` with implementations `rui.dom.client` (jsoo+Brr) and `rui.dom.ssr` (native) |
| `app.wasm` | `app.bc.js` (js_of_ocaml output) — no wasm, no `alloc`/ptr/len, no manual FFI marshalling |
| `router.js` (the client glue + FFI env) | **deleted**; folded into `Rui_client` — jsoo *is* the client, Brr *is* the DOM binding |
| `rui-cli` (`init`/`dev`/`build`) | `rui` CLI (still useful) **or** plain dune targets + watch mode; init scaffolds the same dirs |
| `verify.mjs` / `hydrate.mjs` | OCaml tests under `dune runtest`: native tests drive `Rui_dom_ssr`; a Node+`app.bc.js` harness drives the real client backend |
| `cargo test -p rui` | `dune runtest` (unit tests with `alcotest`/inline tests) |

The headline wins: **the client glue largely disappears** (jsoo + Brr replace `router.js` and the entire hand-rolled FFI), the cfg split becomes a first-class dune *virtual library* instead of `#[cfg]` smeared through one file, and the SSR concurrency model can use a real lightweight scheduler instead of thread-per-connection (which is also what fixes the TLS bug structurally).

### 3. Packaging: `dune-project`, opam, and the virtual-library backend split

```lisp
; dune-project
(lang dune 3.16)
(name rui)
(generate_opam_files true)

(package
 (name rui)
 (synopsis "From-scratch isomorphic OCaml full-stack framework")
 (depends
  (ocaml (>= 5.1))
  dune
  (js_of_ocaml (>= 5.8))      ; client → JS
  js_of_ocaml-compiler
  (brr (>= 0.0.6))            ; typed browser bindings (client backend only)
  (ppxlib (>= 0.33))))        ; ppx infra

(package
 (name rui_ppx)               ; opam pkg `rui.ppx`, lib `rui.ppx`, driver `ppx_rui`
 (synopsis "ppx for rui (%view, %query, [@page], [@@deriving gql], ...)")
 (depends (ocaml (>= 5.1)) ppxlib (rui (= :version))))
```

The DOM backend is the crux. Rust's `#[cfg(target_arch="wasm32")]` toggles two bodies of one `mod backend`. The idiomatic dune answer is a **virtual library**: `Rui_dom` declares `module type S` (and exposes it via a virtual module `Rui_dom_impl`), and two libraries *implement* it. `Rui_view`/`Rui_runtime`/`Rui_gql` depend on the virtual `rui.dom` and are compiled exactly once; the executable picks the implementation.

```lisp
; src/dom/dune  — the virtual backend interface
(library
 (name rui_dom)
 (public_name rui.dom)
 (virtual_modules rui_dom_impl))   ; rui_dom_impl.mli present, no .ml here

; src/dom_client/dune  — jsoo + Brr implementation
(library
 (name rui_dom_client)
 (public_name rui.dom.client)
 (implements rui.dom)              ; provides rui_dom_impl.ml
 (libraries brr js_of_ocaml)
 (modes byte))                     ; consumed by a (modes js) executable

; src/dom_ssr/dune  — native string/arena implementation
(library
 (name rui_dom_ssr)
 (public_name rui.dom.ssr)
 (implements rui.dom)
 (modes native byte))

; src/view/dune, src/runtime/dune, src/gql/dune — backend-agnostic core
(library
 (name rui_view)
 (public_name rui.view)
 (libraries rui.dom))              ; the VIRTUAL lib, not an impl
```

`Rui_dom.S` is the tri-backend surface from the spine (`el`, `text`, `set_text`, `append`, `remove_child`, `attr`, `set_value`, `clear`, `mount`, `clear_app`, `push_url`, `on`, `on_click`, `focus`, `scroll_into_view`, `set_interval`, `clear_interval`, `run_js`, `run_js_on`, `eval`, `gql`, `subscribe`, `set_hydrate`, `seed_responses`, `dehydrate_responses`, plus ssr-only `reset`/`take_html` and client-only `hydrating`):

```ocaml
(* rui_dom.mli — the interface every backend satisfies *)
module type S = sig
  type node                                   (* Brr.El.t on client, arena id on ssr *)

  val el        : string -> node
  val text      : string -> node
  val set_text  : node -> string -> unit
  val append    : node -> node -> unit
  val remove_child : node -> node -> unit
  val attr      : node -> string -> string -> unit
  val set_value : node -> string -> unit
  val clear     : node -> unit
  val mount     : node -> unit
  val clear_app : unit -> unit
  val push_url  : string -> unit

  val on        : node -> event:string -> (string -> unit) -> unit
  val on_click  : node -> (unit -> unit) -> unit
  val focus     : node -> unit
  val scroll_into_view : node -> unit
  val set_interval   : ms:int -> (unit -> unit) -> int
  val clear_interval : int -> unit

  val run_js    : string -> unit
  val run_js_on : node -> string -> unit
  val eval      : string -> ((string, string) result -> unit) -> unit

  val gql       : string -> (string -> unit) -> unit
  val subscribe : string -> (string -> unit) -> unit

  val set_hydrate : bool -> unit
  val seed_responses    : (string * string) list -> unit   (* SSR-injected data → client *)
  val dehydrate_responses : unit -> (string * string) list (* ssr: collected query→resp *)
end
```

**Where this is dramatically cleaner.** rui carries three parallel `mod backend` blocks plus a hand-written FFI table (`extern "C"`) plus a *third* hand-written copy of that table in JS (`router.js`'s `env`) *and a fourth* in each test harness's mock `env`. That is the same surface written four times, kept in sync by discipline (the memo notes the recurring bug "verify/hydrate's env mock **must** provide `clear_app` — otherwise wasm instantiate fails"). In OCaml the interface is declared **once** as `module type S`, dune mechanically enforces that each `(implements)` library provides every module, and `eval`'s error channel is a real `(string, string) result` instead of rui's out-of-band first-status-byte hack (`\x00`=ok/`\x01`=err). No `alloc`/ptr/len, no `TextEncoder`, no string marshalling — Brr passes OCaml strings straight through.

### 4. The two-target build in dune

```lisp
; examples/todo/dune  (the app, both targets from one source)

; shared app library (the rlib analog) — backend-agnostic
(library
 (name todo)
 (libraries rui rui.view rui.runtime rui.gql)
 (preprocess (pps rui_ppx)))

; NATIVE SSR binary  ←→  Cargo [[bin]] ssr  +  crate-type rlib
(executable
 (name ssr)
 (modes exe)
 (libraries todo rui.server rui.dom.ssr)   ; picks the SSR backend impl
 (preprocess (pps rui_ppx)))

; CLIENT bundle  ←→  Cargo crate-type cdylib  +  app.wasm
(executable
 (name app)
 (modes js)                                 ; js_of_ocaml output: app.bc.js
 (libraries todo rui.client rui.dom.client) ; picks the jsoo/Brr backend impl
 (js_of_ocaml (flags (:standard --no-source-map)))
 (preprocess (pps rui_ppx)))

; copy the client bundle to where the server serves it (build.mjs's copy step)
(rule
 (target (dir web))
 (deps app.bc.js styles.css)
 (action (progn
           (copy app.bc.js web/app.js)
           (copy styles.css web/styles.css))))
```

This is a near-exact structural map of rui's `Cargo.toml`: the `(library todo)` is the `rlib`, `(executable ssr (modes exe))` is `[[bin]] ssr`, and `(executable app (modes js))` is the `cdylib`→`app.wasm` line — except the artifact is `app.js`, not `app.wasm`. The backend choice that Rust expresses with `#[cfg(target_arch=...)]` is here expressed by **which `rui.dom.*` impl each executable links**, which is strictly better: the core libraries never see a cfg, and you cannot accidentally call an ssr-only function from client code (it is not in the linked impl).

**Crate-type note.** rui's `crate-type = ["cdylib","rlib"]` exists precisely to get two artifacts from one crate. OCaml does not need a per-target crate-type because dune's `(modes js)` vs `(modes exe)` on two `(executable)` stanzas does the same job over a shared `(library)`.

### 5. Dev server, asset pipeline, and the `router.js` analog (or lack thereof)

rui's `router.js` does five jobs: (1) load+instantiate the wasm, (2) define the FFI `env`, (3) build the hydration index and rehydrate injected data, (4) intercept `<a>` clicks / `popstate` for SPA navigation, (5) wire DOM events back into wasm. In OCaml **jobs 1–2 vanish** (jsoo *is* the loaded program; Brr *is* the DOM binding), and jobs 3–5 move into `Rui_client` as real OCaml. `Rui_client` is the `(modes js)` executable's `main`:

```ocaml
(* rui_client.ml — the entry point, replaces router.js *)
let () =
  (* (3) rehydrate SSR-injected query responses *)
  (match Brr.El.find_first_by_selector (Jstr.v "#__rui_data") with
   | Some el -> Rui_dom_client.seed_responses (parse_data_script el)
   | None -> ());

  (* (3) hydrate iff the server sent SSR content; else pure CSR *)
  let path = current_location () in
  (match Brr.El.find_first_by_selector (Jstr.v "#app [data-h]") with
   | Some _ ->
     build_hydrate_index ();          (* data-h elements + <!--h:N--> text markers *)
     Rui_dom_client.set_hydrate true;
     Rui_runtime.render_path !App.route path;
     Rui_dom_client.set_hydrate false
   | None ->
     Rui_runtime.render_path !App.route path);

  (* (4) SPA <a> / popstate interception *)
  Rui_client.intercept_links ~navigate:(fun p -> Rui_runtime.navigate !App.route p);
  Rui_runtime.flush_mounts ()
```

For job (5), Brr's `Ev.listen` reads the event payload directly (`Brr.El.value`, `Ev.target`), so rui's "encode `target.value` into wasm memory and `dispatch(id, ptr, len)`" pipeline collapses to a closure call. `on:submit` still needs `Ev.prevent_default`, matching `router.js` line 24.

**Dev server.** The native `ssr.exe` stays the dev server (we keep rui's design: server serves `/graphql`, `/graphql/subscribe`, `/app.js`, `/styles.css`, and pages). But two improvements drop out for free:
- The hardcoded `127.0.0.1:8084` becomes `Rui_server.serve ?addr ?port` reading `PORT`/`ADDR` env (the memo's recurring "port 走 env" quick-win, and the painful "old SSR process still on 8084, curl hit the old binary" debugging story).
- The client glue is no longer `include_str!`'d JS; the `(modes js)` build produces `app.js` directly and the server serves it from `web/`. There is no `router.js` to embed.

**Asset pipeline / watch mode.** rui's `rui dev` shells out to `cargo build` then copies. The dune equivalent is one command, `dune build @all`, and crucially **`dune build -w` (watch mode)** rebuilds *both* `ssr.exe` and `app.js` on save — replacing the manual rebuild loop. Tailwind stays exactly as in `crates/rui-cli/src/main.rs::tailwind`: a `(rule)` that runs `tailwindcss -i tailwind.css -o web/styles.css` if an input exists, else writes an empty `styles.css` to avoid the 404 (rui's exact fallback behavior, preserved).

### 6. The `rui` CLI (init / dev / build)

The CLI is still worth keeping — `dune` alone does not scaffold the `data`/`api`/`view` convention the ppx assumes, nor does it know to copy `app.js` into `web/`. The OCaml CLI mirrors `crates/rui-cli`:

- `rui init <name>` — validates the name as a legal OCaml/opam package name (rui's check rejects names that become a bare `_` because `ssr.rs` would emit `_::route`; the OCaml analog rejects names that aren't valid module/lib identifiers, since the generated `Ssr` module references `Todo.route`). Writes the template tree from embedded templates: `dune-project`, `dune` files, `lib.ml`/`bin/ssr.ml`, and the empty `data/`/`api/`/`view/` skeleton with guiding comments — exactly the `crates/rui-cli/templates/*.tpl` set. (`init`'s `framework_crates()` path-walking trick is unneeded; opam resolves `rui` normally.)
- `rui dev` — `dune build @all && dune build -w` in the background + spawn `ssr.exe`, or simply `dune exec ssr` after a build. The build orchestration that rui hand-codes (cargo → wasm → copy → tailwind → run) becomes the dune dependency graph plus the copy `(rule)`.
- `rui build` — `dune build --profile release`; jsoo release flags (`--opt 3`) replace the wasm `opt-level="s"`/`lto`/`panic="abort"` profile.

### 7. Headless test harness — `verify.mjs` / `hydrate.mjs` ported

This is the most interesting part to port, because rui's harnesses are clever: they avoid a browser entirely by instantiating `app.wasm` with a **mock `env`** that models the DOM as a JS array of `{tag, attrs, children, text}` records (`verify.mjs` lines 21–45), then assert on that mock tree after driving `render_route`/`navigate`/`dispatch`/`on_fetch`.

In OCaml we get **two complementary, cheaper test paths**, because the backend is a real `module type S`:

**(a) Native tests against `Rui_dom_ssr` (the bulk of the suite).** Most of `verify.mjs`'s assertions (routing renders the right title, subscription emits the right query, `dispatch` fires the right mutation, memo stats, filtering, error states) are backend-agnostic — they only need *a* DOM. We run them natively against `Rui_dom_ssr`, which already maintains an arena tree (it has to, to produce SSR HTML). This is `dune runtest`, no Bun, no instantiate dance, and it can assert on the arena directly:

```ocaml
(* test/test_routing.ml *)
let test_home_subscription () =
  let dom = Rui_dom_ssr.create () in
  let q = Rui_test.render_and_capture_query dom App.route "/" in
  Alcotest.(check bool) "subscribes to todo_updates"
    true (String.length q > 0 && contains q "todo_updates");
  Alcotest.(check bool) "inlines ...TodoView fields"
    true (contains q "id text done");
  Rui_test.feed dom todos_json;
  Alcotest.(check int) "2 <li> (keyed For)" 2 (Rui_test.li_count dom)
```

This is strictly more honest than `verify.mjs`: there the mock `env`'s `claim_element`/`claim_text` just `return 0` (lines 24–25) and `gql_query`/`gql_subscribe` only record the last query — the OCaml `Rui_dom_ssr` is the *real* SSR backend, so the same code under test runs against production server logic.

**(b) Node + `app.js` for the genuinely client-only assertions.** A few things are inherently client-side: real `seed_responses` rehydration, SPA `navigate` vs `clear_app` accounting, and most importantly hydration claiming. For these we keep rui's instantiate-with-mock-env spirit but trivially simpler: instead of a wasm `env` import object and `TextEncoder` marshalling, we expose the entrypoints from `app.js` (jsoo can export OCaml functions to JS via `Js.export`) and feed them a **mock `Rui_dom_client`**. Cleanest of all: we don't even need Node — we can swap in a *third* `(implements rui.dom)` library, `rui.dom.mock`, an in-memory arena identical to `verify.mjs`'s node array, and run the "client" tests natively too. The mock then becomes ordinary OCaml that the compiler checks against `module type S` — so it can never drift out of sync (the recurring `verify.mjs` bug of a missing `clear_app` in the mock env is structurally impossible).

**Two-pass zero-create hydration assertion (`hydrate.mjs`).** This is the load-bearing test and must be ported faithfully. The assertion: render with SSR data **without** hydration (Pass A) to capture the create-order node list = the hid sequence; then render the *same* path **with** `set_hydrate(true)` and a `claim_*` that returns Pass A's nodes (Pass B), and assert **zero `create` calls happened during Pass B** — every element and text node was claimed, not built. The OCaml mock backend instruments this directly:

```ocaml
(* test/hydrate.ml — two-pass zero-create *)
let run path ~responses ~claim_from =
  let m = Rui_dom_mock.create () in
  Option.iter (Rui_dom_mock.seed_responses m) responses;
  (match claim_from with
   | Some src -> Rui_dom_mock.set_claim_source m src; Rui_dom_mock.set_hydrate m true
   | None -> ());
  Rui_dom_mock.reset_create_count m;
  Rui_runtime.render_path App.route path;
  if claim_from <> None then Rui_dom_mock.set_hydrate m false;
  m

let test_about_zero_create () =
  let a = run "/about" ~responses:None ~claim_from:None in
  let b = run "/about" ~responses:None ~claim_from:(Some (Rui_dom_mock.created a)) in
  Alcotest.(check int) "/about hydration zero create" 0 (Rui_dom_mock.create_count b);
  Alcotest.(check bool) "/about SSR tree contains text 关于"
    true (Rui_dom_mock.has_text a "关于")
```

The crucial subtlety `hydrate.mjs` encodes (line 139) — text nodes are claimed via `<!--h:N-->` comment markers, and an empty text node must be *synthesized* if the comment has no following text sibling — is a property of the **server's serializer + the client backend's `claim_text` counter**, both of which we port verbatim into `Rui_dom_ssr` (emit `<!--h:N-->` before text, `data-h` on elements) and `Rui_dom_client` (parallel hid counter shared between `el`/`text`). The test exercises `/about` (static, text-node-heavy — the hardest case), `/` (subscription data + components + keyed For), `/todo/1` (param page + resource), and `/dash` (nested route group), exactly as `hydrate.mjs` does.

### 8. The verified edge-cases rui solved, and how Rui handles each

The gaps memo records several build/runtime edge-cases that were found and fixed. The OCaml design must address the same ones:

1. **SSR thread-local destruction abort (the big one).** rui's `Scope` was changed to dispose-on-`Drop`. Because the server is thread-per-connection and reactive state lives in `thread_local!` (`EFFECTS`, `PAGE_SCOPE`), at thread teardown the destruction *order* of TLS slots is nondeterministic: a `Scope` dropping during teardown would call `dispose_effect`, which touches the already-being-destroyed `EFFECTS` TLS → `"cannot access TLS during/after destruction"` → process **abort**. The fix was defensive (`EFFECTS.try_with(..).ok().flatten()`). **OCaml fix:** there is no per-thread TLS-destruction hazard if SSR rendering does not spread reactive state across thread-local storage destroyed at thread exit. The OCaml SSR backend holds the arena and reactive scope in an explicit per-request *value* threaded through `render_page` (or in domain-local storage with an explicit `reset`), not in implicitly-destroyed TLS — and `render_page` already does an explicit `Scope.dispose` after producing HTML (rui's `let (node, _sc) = scope(...)` then drops; we keep that as `let v, sc = Rui_reactive.scope render in ...; Rui_reactive.Scope.dispose sc`). The memo's hard-won testing lesson stands and is ported: **single-threaded harnesses cannot catch this** — so the OCaml CI must also run the SSR server and hammer all routes across many connections (each connection = one scope lifecycle), step ⑥ of the flow.

2. **CSR vs SSR vs Static page strategy detection.** The server must emit an empty `#app` shell for `csr` pages and full HTML+data for `ssr`; the client must *probe* (`#app [data-h]` present → hydrate, else pure CSR) so a pure-CSR page does not crash. Ported exactly in `Rui_client` (§5) — the probe is one `Brr.El.find_first_by_selector`.

3. **`</script>` truncation in injected data.** `ssr_doc` replaces `</` → `<\/` before embedding the JSON in `<script id="__rui_data">`. `Rui_server`'s data-injection does the identical escape (JSON allows `\/`).

4. **Static cache key normalization + 1024 cap.** Static pages cache by `path?sorted(query)` so `?a=1&b=2` ≡ `?b=2&a=1`, with a 1024-entry cap to prevent `?utm=...` cache flooding. Ported into `Rui_server`'s static cache (an in-memory `Hashtbl` guarded by a mutex/domain-safe map, with the same sort+cap).

5. **Path/query split at the request boundary.** The server strips `#fragment` and splits `?query` so `/todo/1?x=1` doesn't make the path param `"1?x=1"` and `/about?utm=x` doesn't 404. `Rui_server` performs the same `split_on '#'` then `split_on '?'` with the empty-path→`"/"` normalization.

6. **HTTP non-2xx and network failures must synthesize `errors`.** `router.js`'s `gql_query` turns a non-2xx response (HTML/text body) and network rejection into a synthetic `{errors:[...]}` so the UI enters its error state instead of treating garbage as data (or hanging in `loading` forever). `Rui_dom_client.gql` does the same with `Fut`/`Brr.Fetch` — match on `Response.ok`, else wrap the status+truncated body as a GraphQL error; `catch` network errors similarly. The SSE `onerror` only reports when the connection is truly `CLOSED` (avoid EventSource auto-reconnect spam) — ported on `Brr_io.Ev`.

7. **Tailwind optional, empty `styles.css` fallback.** Ported as the dune `(rule)` behavior in §5.

### 9. Example app layout (the TodoList) in OCaml

rui's `examples/stocks` is the convention the ppx hard-codes (`crate::data::model`, `crate::api::schema` with `#[gql_root]`, `crate::view::{components,layout,pages}`, `gql_fields!` at crate root, `client!`, `router!`). The OCaml layout mirrors it one-to-one — and per the spine the ppx assumes module paths `Model`, `Api.Schema`, `View.Components`, `View.Layout`, `View.Pages`:

```
examples/todo/
  dune-project
  dune                      ; library `todo` + ssr exe (modes exe) + app exe (modes js) [§4]
  tailwind.css              ; optional input
  web/                      ; build output: app.js, styles.css (served by ssr)
  lib/
    todo.ml                 ; top-level: ties modules together
    model.ml                ; [@@deriving gql] types (was crate::data::model)
    api/
      schema.ml             ; %gql_root (was #[gql_root])           — type layer: both targets
      todos.ml              ; resolvers + SSE broadcast (native only via dune (modes ...))
    view/
      components.ml         ; [@component] AddForm, Toolbar, TodoItem, ... (View.Components)
      layout.ml             ; shell, dash_shell (View.Layout)
      pages/
        index.ml            ; [@page "ssr", "/"]      %view { ... }
        archive.ml          ; [@page "ssr", "/archive"]
        detail.ml           ; [@page "ssr", "/todo/:id"]   (typed param via param_as)
        about.ml            ; [@page "static", "/about"]
        draft.ml            ; [@page "csr", "/draft"]
        overview.ml         ; [@page ..., "/"]    (group /dash)
        settings.ml         ; [@page ..., "/settings"]
  bin/
    ssr.ml                  ; Rui_server.serve { route; resolve; sse }  (was src/bin/ssr.rs)
  test/
    dune                    ; (test (libraries todo rui.dom.mock alcotest))
    test_routing.ml         ; verify.mjs's CSR/data assertions [§7a]
    hydrate.ml              ; two-pass zero-create [§7b]
```

`bin/ssr.ml` is the exact analog of the 12-line `examples/stocks/src/bin/ssr.rs`:

```ocaml
let () =
  Rui_server.serve {
    route   = Todo.route;
    resolve = Api.Schema.resolve;
    sse     = Some { snapshot = Api.Todos.snapshot_json;
                     subscribe = Api.Todos.add_subscriber };
  }
```

and `lib/todo.ml` carries the `%router { ... }` (replacing `rui::router!`) and the `Rui_client` wiring. Note one structural simplification: rui needs `gql_fields!(...)` at the crate root (a hand-maintained marker list, because proc-macros can't see a global symbol table) — the spine eliminates this; the ppx derives field markers from the schema, so the OCaml app does **not** write a `gql_fields` list. The native-only `api/todos.ml` is gated not by `#[cfg(not(target_arch="wasm32"))]` but by the dune fact that only `ssr.exe` links the resolver-bearing libraries (the client links `rui.dom.client` + the view/runtime, not the server resolvers), so a missing-on-client resolver is a *link-time* selection, not a cfg.

### 10. Where OCaml is cleaner / a wash / harder

**Cleaner (clear wins):**
- **The FFI quadruplication is gone.** One `module type S`, dune-enforced implementations; no `extern "C"` table, no `router.js` `env`, no `TextEncoder`/`alloc`/ptr/len in any harness. Brr passes strings and events through directly.
- **`eval` gets a real error channel** (`(string, string) result`) instead of the in-band `\x00`/`\x01` status-byte hack rui had to invent because the FFI return is a single string.
- **The cfg split becomes structural** (virtual library) rather than `#[cfg]` woven through `dom.rs`, `lib.rs`, `server.rs`, and every `api/mod.rs` — and the test mock becomes a *compiler-checked* third implementation that cannot drift.
- **No bundler, no wasm toolchain, no `wasm32-unknown-unknown` target install, no manual copy step** — `dune build` produces both artifacts; `dune build -w` is the dev loop.
- **The SSR TLS-destruction abort is removed by construction** (explicit per-request scope value instead of TLS destroyed at thread exit), and a real concurrency model (e.g. `eio`) can replace unbounded thread-per-connection (also closing rui's DoS gap D, though that's the server section's remit).

**A wash:**
- **Two-target compile.** dune's `(modes js)`/`(modes exe)` is about as much config as Cargo's `crate-type` + `[[bin]]`. Neither is meaningfully simpler.
- **Hydration's intrinsic complexity.** The hid counter, `data-h`/`<!--h:N-->` markers, the synthesize-empty-text-node edge case, the two-pass zero-create test — these are *protocol* invariants between server serializer and client claimer, independent of language. We port them verbatim; OCaml doesn't make them disappear.
- **The Tailwind/asset-copy step.** A `(rule)` vs CLI shell-out — equivalent.

**Harder / friction:**
- **jsoo output is bigger and the dead-code story is different.** rui's release wasm is `opt-level="s"` + `lto` + `panic="abort"`. jsoo bundles the OCaml runtime; `app.js` will be larger than a tightly-optimized wasm unless we lean on jsoo's `--opt 3` + dead-code elimination. This is a size regression to budget for.
- **Native↔client API parity is now a compile-time obligation in both directions.** Rust's `#[cfg]` lets ssr-only functions simply not exist on the wasm side. With the virtual library, every function in `module type S` must be implemented by *every* backend including the mock — which is the cleanliness win, but it does mean adding one DOM primitive touches three impls (vs rui's two `mod backend` blocks + JS env, so really a wash in edit count, but now type-checked).
- **Domain/concurrency choice for SSR.** Replacing thread-per-connection with `eio`/`lwt` adds a dependency and a programming-model decision rui didn't have to make (it used raw `std::thread` + `std::net`). The payoff is the TLS-bug fix and request bounding, but it's net new surface.

### 11. Open questions / risks

1. **jsoo bundle size vs rui's wasm.** Need a concrete measurement and a budget; may require `js_of_ocaml` separate-compilation / `--opt 3` / shrinking the linked surface. Risk that `app.js` is multiples of `app.wasm`.
2. **Exporting client entrypoints for the Node harness.** If we keep a Node+`app.js` test path (rather than only the native mock backend), we must decide how the test drives jsoo-exported functions (`Js.export` surface) and whether that's worth maintaining alongside the cleaner native-mock path. Leaning toward native mock backend as primary.
3. **SSR concurrency model.** `eio` (OCaml 5 effects) vs `lwt` vs a bounded thread pool. This decision belongs partly to the server section but determines whether the per-request reactive scope lives in domain-local storage, a passed value, or fiber-local state — and that determines whether the TLS-abort class of bug can recur in a new form.
4. **`dune build -w` interaction with the running `ssr.exe`.** Watch mode rebuilds `app.js`, but the running server reads `web/app.js` off disk; we need a copy `(rule)` + possibly a server file-watch or just rely on the browser re-fetching. No hot-reload yet (rui never had it — it's P5 in the roadmap); HMR remains out of scope.
5. **Static page cache lifetime.** rui's static cache lives until process exit (no invalidation/revalidation; the memo flags this). The OCaml port inherits the gap; if we add an `eio` scheduler, a TTL/revalidate becomes feasible but is unspecified.
6. **Where the `app.js` ↔ `web/` copy and the data-injection script id (`__rui_data`) are owned.** These are a contract shared by `Rui_server` (emits) and `Rui_client` (reads); keeping the id/marker constants in one shared module (`Rui` umbrella) avoids the kind of drift that bit rui across `server.rs`/`router.js`.

## 完整能力清单与覆盖(feature parity)

下表是从 rui 进度日志 + 源码穷举出的能力清单(共 94 项),逐项应在上文对应子系统中有设计:

- [ ] Reactive Signal: make/get/set/update; get inside an effect subscribes; set notifies only subscribed effects (snapshot subs before running to avoid borrow re-entrancy)
- [ ] Reactive effect: register + run-once immediately; tracks signals read during the run
- [ ] Dynamic dependency cleanup: before each re-run an effect unsubscribes from all signals it read last time (so a conditional branch that changes its deps does not leave stale subscriptions) — correctness-critical, ported verbatim
- [ ] effect dispose handle: stop an effect, drop its closure, unsubscribe from all deps
- [ ] memo: derived signal recomputed when deps change; initial value computed under untrack (not polluting the outer effect's dep set)
- [ ] memo VALUE-DEDUP: do not notify downstream when the recomputed value equals the previous (untrack self-read to avoid self-subscribe cycle); ?equal to choose structural/physical/custom equality — fixes 'unrelated query-key change re-triggers resource refetch'
- [ ] untrack: run a thunk without subscribing
- [ ] Scope: collect all effects/memos created during a thunk; dispose them all at once (route change cleanup)
- [ ] Scope DISPOSE ORDER: run on_cleanup callbacks FIRST (node still present, signals still readable) THEN dispose the effects — and recursively for nested scopes
- [ ] No-Drop-TLS-hazard: deterministic disposal is explicit (not via Drop); the Rust per-connection-thread 'cannot access TLS during destruction' abort and its try_with guard do NOT exist in single-runtime OCaml SSR
- [ ] on_cleanup: register an unmount callback tied to the CURRENT scope (clearInterval / unbind / destroy 3rd-party / close EventSource); no-op outside any scope; on SSR runs after the one-shot render scope drops (DOM ops are no-ops)
- [ ] scope take_parts/absorb_parts: reparent effects/memos created inside an on_mount callback into the PAGE scope (so they die on page change); trivial in OCaml (run callback inside the page scope)
- [ ] Into_view (build/rebuild) protocol: build returns (node, state); rebuild updates in place
- [ ] Into_view instances: text/scalar -> text node (rebuild = in-place set_text, NO wrapper, NO node rebuild, does not degrade); View -> mount/replace sub-tree at a rui-slot anchor; unit -> empty text node; Option -> conditional (no else); list -> inline list (no keying)
- [ ] View handle: abstract node handle (Brr.El.t on client / arena id on SSR), replacing Rust View(u32) and the From<View> for u32 / Into<u32> bridging
- [ ] reactive_block: the %view '{ fun () -> ... }' runtime — an effect re-evaluating the thunk; build once then rebuild in place; each round runs in a child scope so inner effects of the previous sub-tree are disposed (no ghost effects)
- [ ] Expression-style conditional rendering: %view '{ }' blocks dispatch by RETURN TYPE (View -> sub-tree, scalar -> text, unit -> empty, option -> conditional, list -> inline) so native if/else/match works; OCaml type-based dispatch via Into_view
- [ ] Show / Switch+Match: compile to reactive_block (kept as sugar; native if/match also works)
- [ ] keyed For reconciliation: <For list item key={...}> appended directly to parent (no wrapper, so <tbody><tr> is legal); key gone -> remove_child + dispose row scope; key kept & item equal -> reuse node (append=move, preserves focus/selection/animation); key kept & item changed -> rebuild that row; new key -> build; reordering via append (appendChild moves existing nodes)
- [ ] Non-keyed For: clear-parent + full rebuild semantics preserved (distinct from keyed)
- [ ] Reactive attributes: closure attribute class={fun () -> ...} wrapped in an effect (re-set attr on dep change), mirroring reactive text
- [ ] Static vs dynamic attributes: name="x" static string; name={expr} dynamic
- [ ] Generic events: on:<event>={...} passes the real event name through (not hard-coded click); SSR binds no events
- [ ] Event payload: handler receives target.value (or "") — on client read directly from Brr.Ev (no handler-id registry, no dispatch marshalling)
- [ ] on:click convenience: zero-arg closure wrapped to ignore payload
- [ ] Two-way binding: bind:value={signal} -> effect signal->set_value + input event->signal.set (Signal<string>); set_value sets the .value PROPERTY (client) / value attribute (SSR first paint visible)
- [ ] submit default preventDefault on the client; on:submit handler
- [ ] node_ref / ref={r}: write the created-or-claimed element handle into a ref cell; used by on_mount for imperative DOM
- [ ] Components: [@component] generates a named-props record + a slot; <Card title=.. sub={x}>children</Card> -> named record literal call (order-independent, type-checked, missing field = error); children slot for container/layout components; static attr -> string, dynamic attr -> any type; events/bind/ref on a component = compile error
- [ ] Components carry reactive props: Signal/closure props (so a component owns its props, no per-page clones); zero-arg components generate an empty props record
- [ ] Render strategies: [@page ssr|csr|static]; default ssr; ssr = render + inject data + hydrate; csr = empty shell, client renders from scratch; static = render once then cache by path(+query)
- [ ] Page model: page = { key; strategy; render } with deferred render closure capturing params; route() is the path->page table
- [ ] Page key: stable identity (from ppx structure-item loc/name, replacing Rust module_path!()) — same page different params = same key (no rebuild), different page = different key (rebuild)
- [ ] Router %router: candidate-page list + optional global layout + fallback; matches by declaration order (literal routes before :param); generates let route
- [ ] matches: pattern vs path segment match (equal segment count, :name wildcard, root '/' or '' matches root); pathname only
- [ ] Nested route groups: group("/prefix", layout=...) { pages }; group = ONE page with fixed key group:<prefix>; member prefixes concatenated at runtime for matching
- [ ] Reactive outlet (no-rebuild on sibling nav): same-key navigation inside a group only sets PATH -> outlet (reactive_block subscribing path) swaps leaf content; group layout persists (sidebar stays, no flicker, no state loss)
- [ ] Group strategy: derived from the leaf matched by current path; in-group navigation is always client-side
- [ ] Path params as signals + typed: PATH signal is the source of truth; param(i) reactive segment; param_as parses to a type (OCaml via a Parsable first-class module); same-page param change -> resource refetch, no full rebuild
- [ ] Path param declaration: [@page "/todo/:id"] with id: int signal in the signature; ppx maps :name segment index -> param_as binding; reverse-check every :seg has a matching param (catch typos); reject self/non-identifier/generic params
- [ ] Query params SEPARATE signal line: QUERY signal independent of PATH (does not affect routing); query_param/query_param_as derive via memo; query_string raw signal
- [ ] Query path/query clean split: server entry and client nav split full URL into (pathname, query) via split_url (strip #fragment, split on first ?); two independent signal lines
- [ ] Query value percent/+ decode on read; query_encode on write (symmetric); fixes ?q=hello%20world matching literal
- [ ] set_path/set_query value-unchanged dedup (no write if same) to avoid redundant resource refetch on same-URL navigation
- [ ] navigate/render_path order: dispose old page FIRST, then write PATH/QUERY (so the dying page's memos/resources do not ghost-recompute and waste fetches); same-key branch is pure set
- [ ] go: programmatic navigation = push_url (history.pushState) + navigate
- [ ] resource!: reactive query; signal params read inside the query subscribe -> any param change re-runs the fetch effect (rebuild query string + refetch, reusing handler); returns (rows, loading, error)
- [ ] resource! error: GraphQL errors[] or HTTP/network error -> error := Some msg AND keep last rows (do not render garbage); success -> clear error; set error before clearing loading (error branch wins, no stale-rows flash)
- [ ] mutation!: optimistic update (merge predicted entity + snapshot before request, restore on response then write real value; empty real value = failure stays rolled back); on_error callback (order-independent tail option, parsed in a loop with optimistic); skip-merge garbage on failure; supports literal AND runtime args; target now syntactic-only (normalized store auto-updates all views referencing the entity)
- [ ] mutation! on_error keeps outer closure usable: Rust used Rc<dyn Fn>; OCaml just stores the GC closure
- [ ] query!/subscription! skip-merge on error: never write the errors object into the store (no cache pollution), keep last-good result
- [ ] errors_message classification: only {data, errors:[]} or {data,...} is success; non-JSON-object (HTML error page/bare value/parse fail) = failure; non-empty errors list = join messages; errors missing -> success only if data present; errors present but not a list (null/object/string/number) = failure; empty message -> placeholder so 'has error' signal not lost
- [ ] Client transport error injection: non-2xx HTTP (500/502/404 with HTML/text body) -> synthesize errors; network reject -> inject errors (so loading does not stay true forever); SSE onerror reports only on CLOSED (avoid auto-reconnect transient error spam)
- [ ] on_mount flush at ALL client entry points: render_path/navigate tails AND dispatch (event) AND on_fetch (fetch result) AND run_interval (timer) — so dynamic sub-trees rebuilt by events/fetch/timer also fire their on_mount
- [ ] on_mount queue + child-scope absorb: each callback runs in a child scope whose effects/memos are absorbed into PAGE scope (no owner-less ghost effects); on SSR on_mount is wholly no-op
- [ ] nav_gen generation fencing: if a mount callback navigates mid-flush, the remaining stale batch (belonging to the now-disposed old page) is abandoned
- [ ] node_ref/ref + imperative DOM in on_mount: focus / scroll_into_view / set_interval / clear_interval
- [ ] set_interval/clear_interval registry hygiene: timer slot freed on clear (release captured signal); OCaml relies on GC for the closure but keeps clearInterval via on_cleanup (e.g. <Uptime/> in shell must not leak across navigation)
- [ ] fetch-handler reclaim: query!/resource!/subscription!/eval register a fetch handler dropped via on_cleanup on scope dispose (avoid leak + ghost writes from in-flight responses of abandoned pages); OCaml: closures GC'd with scope, but in-flight EventSource/fetch must be cancelled/ignored via the generation/cleanup mechanism
- [ ] JS escape hatch run_js: fire-and-forget eval in global scope (clipboard / localStorage / scrollTo / 3rd-party libs)
- [ ] JS escape hatch run_js_on(node, code): eval with `el` bound to that node (init charts/editors via node_ref)
- [ ] JS escape hatch eval(code, cb): get return value (supports Promise) -> Ok value / Err message; Rust used a \x00/\x01 status byte -> OCaml returns (string,string) result; eval handler has on_cleanup reclaim (never-settling Promise safe)
- [ ] SSR true hydration Stage 2: client text() and el() CLAIM SSR-rendered nodes (el by data-h, text by <!--h:N--> comment marker -> following text node, empty text inserted if missing); clear() is no-op during hydration (else <For> clear-parent would wipe claimed DOM); set_hydrate(bool) toggles; subscribe also delivers SSR-injected initial value synchronously for consistent hydration
- [ ] Hydration claim index: client scans #app, builds hid->node index (elements by data-h, text by <!--h:N--> next text node); first paint set_hydrate(1)+render (no innerHTML="")+set_hydrate(0); later SPA nav is CSR
- [ ] Hydration vs CSR probe: client checks for #app [data-h]; present -> hydrate, absent (csr page empty shell) -> pure CSR; also fixes pure-client-no-SSR crash
- [ ] Data handoff dehydrate/rehydrate Stage 1: SSR native dom::gql records query-string->response into SSR_RESP; dehydrate_responses serializes; server injects <script id=__rui_data> with </ -> <\/ to prevent </script> truncation; client hydrate_data seeds the cache before first render; client gql hits HYDRATE_RESP -> synchronous delivery, skips POST (consume-once; SPA nav still does real requests) -> first paint does not re-fetch
- [ ] Normalized store: entity key = __typename:__id (derive injects these meta fields, executor projection always preserves them); responses normalize in; bump entity version signals; query view is a memo reading the store and subscribing relevant versions -> mutation writing the same entity auto-updates all views referencing it (Relay consistency across queries)
- [ ] Store write order: merge ALL entities first (merge_all, no bump) THEN publish key list / bump versions -> any woken view sees a fully-merged consistent snapshot (no half-merged intermediate)
- [ ] Store $ref + denormalize: objects with __typename/__id (even nested) extracted to independent entities, replaced in place by {$ref:key}; read_entity denormalizes recursively, subscribing nested entity versions (nested entity mutation re-runs referencing views)
- [ ] Entity keys: scalar id supports String/Int/Float/Bool; value objects without [@gql.id] (Connection/Edge/PageInfo) are not normalized (inline under parent)
- [ ] Store reset per SSR render (request isolation, not relying on per-connection thread incidentally)
- [ ] paginated! Relay cursor: field(first: N){ node selection }; connection shape edges[{node,cursor}] + page_info{has_next_page,end_cursor}; node extracted as independent entity (ref); load_next appends new page into connection record; cursor-dedup on append (idempotent re double-click/resend); load_next throttle (skip while loading); returns (rows, load_next, has_next, loading); node mutation re-runs pagination view (full Relay consistency)
- [ ] fragment! data masking: fragment!(Name on Type { fields }) generates a named exact-fit data structure + selection string; %query ...Name inlines the fragment selection; parent result holds the Name sub-struct so a component can only read fields the fragment declared (masking)
- [ ] %query / %subscription / %resource share expand_fetch: generate exact-fit selection type + Signal<Vec<Row>> view (memo from store) + transport (gql vs subscribe vs effect-wrapped)
- [ ] Exact-fit selection types: each selection layer becomes a generated record/row type; selecting a missing field or reading an unselected field is a plain OCaml type error (replaces Rust Field/Scalar/GqlElem/Reshape PhantomData checks)
- [ ] gql_root methods-as-schema: %gql_root(query|mutation|subscription) on a module/impl generates the type-level schema (root + per-field return type, both targets) for compile-time exact-fit checks AND a native resolve dispatch (method body reads store, args extracted by type via From_arg)
- [ ] [@@deriving gql] (replaces derive GqlObject): generates typename, gql_id (Null when no [@gql.id] = value object), gql_field, into_value (with __typename/__id meta), of_value, and a schema field-table the ppx consults at expansion (replacing Field<M> trait projection and the gql_fields! marker list which is ELIMINATED)
- [ ] to_gql_arg: format a runtime value as a GraphQL arg literal (string quoted+escaped, number/bool bare) for variable args field(arg: var)
- [ ] GraphQL Value model + recursive-descent JSON parser (nested, string escapes incl \uXXXX, bool/null, int/float distinction) + to_json; shared both targets
- [ ] Server GraphQL parser (native): operation query/mutation/subscription (anonymous ok), nested selection, field args, alias, multi-root; skip variable definitions; progress-guard on malformed input (avoid infinite loop = remote DoS)
- [ ] Server executor (native): parse doc -> resolver(kind, field, args) -> project by selection (recursive + list + alias) -> {data, errors:[]}; project always preserves __typename/__id meta; executor is type-agnostic (operates on Value + selection), resolver injected by serve (framework/app decoupled)
- [ ] Exec.Args + From_arg: per-type argument extraction (str/i64/f64/bool, extensible)
- [ ] SSR server: page() by strategy (Ssr render+inject / Csr empty shell / Static OnceLock cache keyed by path+normalized-sorted-query with a 1024 cap to prevent ?utm cache flooding); doc() HTML skeleton with rui-slot,rui-frag{display:contents} + __rui_data script + client bootstrap script; render_page resets dom + store per render; server entry strips ?query/#fragment, splits path/query, sets current path/query before render so first paint param()/query_param() are correct
- [ ] SSE subscription server: long-lived connection, push snapshot then stream broadcasts; client EventSource per subscription query
- [ ] Static asset serving: client bootstrap (was /router.js; now /app.js bootstrap), app.js (was app.wasm), styles.css; 404 for missing files
- [ ] App description: route (path->page), resolve (resolver), optional sse (snapshot + subscribe channel); rui::serve wires resolver for isomorphic local execute
- [ ] Isomorphic local execute: native dom::gql runs the SAME query through the registered resolver (SSR pre-fetch) so SSR renders with data (SEO-visible)
- [ ] Client bootstrap (Rui_client, replaces router.js): instantiate, rehydrate data, build hydrate index, hydrate-or-CSR probe, intercept internal <a> (starts with /, not .html) + popstate for SPA nav (passes pathname+search), preventDefault, programmatic push_url
- [ ] alloc/render_route/navigate/dispatch/on_fetch/hydrate_data/set_hydrate/run_interval client entry points (Rust #[no_mangle] exports) -> direct OCaml functions in Rui_client (no ptr/len marshalling)
- [ ] Tri-backend DOM: client(create) / client(hydrate, runtime flag) / native(SSR string serialize with data-h + <!--h:N-->) behind one module signature Rui_dom.S
- [ ] esc_text/esc_attr on SSR serialize; write_json_str escaping; gql_escape for arg literals
- [ ] decode_rows helper: decode transport response into a row list (tolerates {data:{root:[...]}} and bare [...])
- [ ] Parsable module surface for typed params (replaces FromStr+Default+Clone+PartialEq bound); ready-made string/int/float/bool
- [ ] Component/fragment/page convention paths the ppx assumes: Model (deriving gql), Api.Schema (gql_root), View.Components, View.Layout, View.Pages


各子系统自报覆盖(`covers`):


**Reactive Core** — signal/effect/memo with GC (no Rc<RefCell>); dynamic dependency cleanup; memo value-equality dedup; owner/scope without Rust Drop; on_cleanup under GC; untrack; Solid-style reading/tracking discipline; TLS-during-Drop hazard avoidance; Scope.take_parts/absorb_parts; module signatures Signal/Effect/Memo/Scope/Owner

**View & Rendering Engine** — View handle (built node); IntoView build/rebuild protocol; reactive_block ({move||..} runtime + return-type dispatch); keyed_for reconciliation (reuse/move/rebuild, focus preservation); conditional rendering as native if/match (no <Show>/<Switch>); reactive interpolation in JSX (signal-valued child); text-vs-subtree in-place update; NodeRef / refs; Strategy + Page; tri-backend node identity (Brr.El.t vs SSR arena id); Scope-driven disposal of dynamic subtrees; hydration claim of text + element nodes

**The DSL: jsx-ppx, page, component, router macros** — view! → %view jsx-ppx (elements/attrs/events/on:/bind:/ref/reactive {} blocks/For/Show/Switch/Match/components/fragments); #[rui::page] → let%page / [@page] (route pattern + typed signal params + __RUI_PATTERN/__RUI_STRATEGY); #[rui::component] → [@component] (props record + children slot); router! → %router (route table + nested group(...)); gql_fields/schema-as-types → ppx derivation; ppxlib mechanics vs proc-macro (passes, AST surgery, hygiene); edge-cases from progress log and how OCaml handles them

**DOM Abstraction, SSR & Hydration** — dom backend abstraction (tri-backend → bi-backend); native SSR string/arena builder with data-h + <!--h:N--> markers; jsoo+Brr client backend with claim_element/claim_text by hid; elimination of the FFI/u32-handle/linear-memory layer under jsoo; hydration model (set_hydrate, claim by data-h + comment-marked text, clear() no-op during hydrate); controlled value rendering (set_value attr vs property); SSR server (std-only http → OCaml http, per-request isolation, thread-per-conn, SSE); data handoff (dehydrate_responses → <script id=__rui_data> → seed_responses, skip first network); SPA navigation / push_url / clear_app; command-imperative DOM (focus/scroll/interval) and JS escape hatch (run_js/run_js_on/eval); one-shot fetch handler lifecycle (drop_fetch_handler / on_cleanup); backend selection via dune virtual library

**GraphQL Data Layer** — Value model + JSON codec (Rui_gql.Value); errors_message classification; FromValue/IntoValue (From_value/Into_value modules); Normalized store (Rui_gql.Store): entity key __typename:__id, version bump, read_entity, merge_all/bump_all/normalize_list, $ref denormalize, connection records, snapshot/restore, keys_of, reset; Server executor + parser (Rui_gql.Exec / Rui_gql.Parser): resolver dispatch, projection, meta-field preservation, progress-guard DoS protection; Trait projection (Field<M>/Scalar/GqlElem/Reshape/Fragment/GqlObject) -> OCaml first-class-module witnesses + GADT/row exact-fit selection; [@@deriving gql] (replaces #[derive(GqlObject)] + gql.id); %gql_root (schema-as-types, methods=schema); %query / %subscription / %resource (expand_fetch); %mutation optimistic + rollback + on_error; %paginated (Relay connection cursor pagination); %fragment (data masking); ToGqlArg / to_gql_arg arg formatting; one-shot fetch-handler reclaim (on_fetch_handler/drop_fetch_handler/run_fetch) + ghost-write protection; SSR dehydrate/seed responses + hydration data handoff; edge-cases: skip-merge on error, write-then-bump consistent snapshot, cursor dedup, nested cross-query entity update, value object (no id) inlining, errors_message hardening, memo value-dedup, out-of-order responses

**Routing: router!, params, nested groups, strategies** — router! route table; path params as derived signals (param/param_as); matches(); query params as a separate independent signal (query_param/query_param_as, url decode/encode); navigate (same-key no-rebuild vs cross-key rebuild); go (programmatic + pushState); nested route groups via reactive outlet (reactive_block keyed on path); render strategies ssr/csr/static + per-group strategy; SSR page() dispatch + static cache key normalization; render_path / split_url; on_mount flush + nav generation fence; dispose-before-set ordering for ghost-recompute avoidance

**Lifecycle, Refs & JS Interop** — on_mount (queued + flushed at all client entry points incl dynamic-subtree rebuilds; SSR no-op); on_cleanup (scope-tied, run-before-dispose); node_ref / ref; imperative DOM: focus / scroll_into_view / set_interval / clear_interval (handler reclaim); JS escape hatch: run_js / run_js_on / eval (Result error channel); Brr/jsoo replacement of the Rust string-eval FFI; which FFIs disappear; edge-cases: flush in dynamic subtrees, owner-less effects in on_mount callbacks, INTERVAL_HANDLERS unbounded growth, mid-flush navigation (NAV_GEN), eval one-shot handler reclaim + ghost-write, TLS-during-destruction

**Build, Tooling, Project Layout & Testing** — cargo-to-dune-opam; two-target-build-native-ssr-plus-jsoo-client; opam-deps-jsoo-brr-ppxlib; dev-server-and-asset-pipeline-router.js-analog; rui-cli-init-dev-build; headless-test-harness-verify.mjs-hydrate.mjs; two-pass-zero-create-hydration-assertion; example-app-layout-todo; dune-project-and-dune-file-sketches; tri-backend-cfg-to-virtual-library; ssr-tls-destruction-bug; crate-type-cdylib-rlib-to-modes-byte-js



## 风险、非目标与路线图

各子系统的「open questions / risks」见上文对应章节末尾。建议构建顺序:① 反应式核(signal/effect/memo + owner/dispose)→ ② DOM 抽象两后端(native 字符串 + jsoo/Brr)+ 一个手写同构页(验证 SSR+水合零 create)→ ③ jsx-ppx(%view)+ page/component ppx → ④ GraphQL 数据层(value/store + %query/%mutation/%subscription,exact-fit 用 row/GADT)→ ⑤ 路由(router! + 参数 signal + 嵌套组 outlet)→ ⑥ resource!/错误态/生命周期/ref/逃生舱 → ⑦ 工具链(dune 两目标 + 无头水合测试)。
非目标(v1):wasm 目标(jsoo→JS 已够;wasm_of_ocaml 留待后续)、可视化 devtools、SSR 流式渲染。

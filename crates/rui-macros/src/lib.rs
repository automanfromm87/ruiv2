//! rui 的 view! 宏:JSX 式标记在编译期展开成引擎调用。纯 Rust,无 .html/.types/compile.mjs。
//!
//! 语法:
//!   <div class="x" style={expr} on:click={move || ...}> 子节点 </div>   元素 / 静态属性 / 表达式属性 / 事件
//!   <StatCard label="x" value={v} />                                    组件(首字母大写 → 调 registry components::snake)
//!   <For list=rows item=r> <tr>...{ &r.symbol }...</tr> </For>          响应式列表(list 为 signal,变则重建)
//!   "文本" / { expr }(静态) / { move || expr }(响应式文本)
//!
//! 另含 GraphQL data 层宏:gql_schema! / gql_fields! / #[derive(GqlObject)] / query! / mutation! / subscription!
//!
//! 目录解耦:宏不再硬编码 crate::view::components / crate::data::model / crate::api::schema / crate::gqlf,
//! 而是统一引用 `crate::__rui_registry::{components,model,schema,fields}` 这层 re-export 间接。应用在 crate 根
//! 调一次 `rui::app! { .. }` 把四个键映射到实际路径(缺省走旧约定,也可指向别的目录甚至别的 crate)。
use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TS2};
use quote::quote;
use syn::ext::IdentExt; // Ident::parse_any:把 `type` / `for` 等关键字也当属性名解析
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{parse_quote, Data, DeriveInput, Expr, Fields, Ident, LitInt, LitStr, Stmt, Token, Type};

// ───────────────────────── rui::app!(应用 registry:解耦宏与目录结构)─────────────────────────
// 宏不再硬编码 crate::view::components / crate::data::model / crate::api::schema / crate::gqlf,而是统一引用
// 一个 re-export 间接层 `crate::__rui_registry::{components,model,schema,fields}`。app! 在 crate 根调用一次,
// 把这四个键映射到应用实际的模块路径(缺省走旧约定):
//   rui::app! {}                                              // 全用默认约定
//   rui::app! { components = crate::ui::widgets, schema = crate::gql::roots }  // 自定义目录
//   rui::app! { model = ::shared_types::model }               // 跨 crate(多 package / 多 domain)
// 路径解析在所有宏展开**之后**发生,故 re-export 模块与消费宏的展开顺序无关(paths resolve late)。
// 必须调用一次:否则消费宏引用的 crate::__rui_registry 不存在 → "unresolved module" 编译错。
struct AppRegistry {
    components: syn::Path,
    model: syn::Path,
    schema: syn::Path,
    fields: syn::Path,
}
impl Parse for AppRegistry {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let (mut components, mut model, mut schema, mut fields) = (None, None, None, None);
        while !input.is_empty() {
            let key: Ident = input.parse()?;
            input.parse::<Token![=]>()?;
            let path: syn::Path = input.parse()?;
            match key.to_string().as_str() {
                "components" => components = Some(path),
                "model" => model = Some(path),
                "schema" => schema = Some(path),
                "fields" => fields = Some(path),
                other => {
                    return Err(syn::Error::new(
                        key.span(),
                        format!("rui::app! 未知键 `{other}`(支持 components / model / schema / fields)"),
                    ))
                }
            }
            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
        }
        let dflt = |s: &str| syn::parse_str::<syn::Path>(s).expect("默认路径合法");
        Ok(AppRegistry {
            components: components.unwrap_or_else(|| dflt("crate::view::components")),
            model: model.unwrap_or_else(|| dflt("crate::data::model")),
            schema: schema.unwrap_or_else(|| dflt("crate::api::schema")),
            fields: fields.unwrap_or_else(|| dflt("crate::gqlf")),
        })
    }
}

#[proc_macro]
pub fn app(input: TokenStream) -> TokenStream {
    let r = syn::parse_macro_input!(input as AppRegistry);
    let (c, m, s, f) = (r.components, r.model, r.schema, r.fields);
    // re-export 间接层:消费宏统一引用 crate::__rui_registry::{components,model,schema,fields}。
    // pub use 零成本、类型透明(Field<gqlf::X> 仍解析到同一真实类型 → 编译期 exact-fit 校验不变)。
    quote! {
        #[doc(hidden)]
        #[allow(unused_imports)]
        pub(crate) mod __rui_registry {
            // pub(crate):消费宏只在本 crate 内引用 crate::__rui_registry,故无需对外 pub
            //(也避开"经私有模块 pub 再导出"的可见性坑;跨 crate 场景下各 crate 有自己的 registry)。
            pub(crate) use #c as components;
            pub(crate) use #m as model;
            pub(crate) use #s as schema;
            pub(crate) use #f as fields;
        }
    }
    .into()
}

enum Node {
    El { tag: String, attrs: Vec<Attr>, children: Vec<Node> },
    For { list: Ident, item: Ident, key: Option<syn::Block>, children: Vec<Node> },
    // 条件渲染:when 为返回 bool 的闭包;true→children,false→fallback(可选,返回节点的闭包)。
    Show { when: syn::Block, fallback: Option<syn::Block>, children: Vec<Node> },
    // 多分支:命中第一个 when 为真的 <Match>,都不命中则什么也不渲染。
    Switch { arms: Vec<(syn::Block, Vec<Node>)> },
    // 错误边界:子树出错 → 渲 fallback(闭包 |err: String, reset: Rc<dyn Fn()>| -> View)+ reset 重试。
    ErrorBoundary { fallback: syn::Block, children: Vec<Node> },
    // 过渡:单子元素进出场动画(when 为返回 bool 的闭包;name=CSS 类前缀;duration=出场后移除延时 ms)。
    Transition { name: String, duration: u32, when: syn::Block, children: Vec<Node> },
    Text(LitStr),
    Block(syn::Block),
}
enum Attr {
    Static { name: String, value: LitStr },
    Dyn { name: String, block: syn::Block },     // name={expr}
    Event { event: String, modifiers: Vec<String>, handler: syn::Block }, // on:<事件>[.修饰符]*={...}
    Bind { prop: String, expr: syn::Block },      // bind:<属性>={signal}(双向绑定)
    Ref { handle: syn::Block },                   // ref={node_ref}(把元素 id 写进句柄,配 on_mount)
}

fn parse_node(input: ParseStream) -> syn::Result<Node> {
    if input.peek(LitStr) {
        return Ok(Node::Text(input.parse()?));
    }
    if input.peek(syn::token::Brace) {
        return Ok(Node::Block(input.parse()?));
    }
    input.parse::<Token![<]>()?;
    let tag: Ident = input.parse()?;
    if tag == "For" {
        return parse_for(input);
    }
    if tag == "Show" {
        return parse_show(input);
    }
    if tag == "Switch" {
        return parse_switch(input);
    }
    if tag == "Match" {
        return Err(syn::Error::new(tag.span(), "<Match> 只能放在 <Switch> 内"));
    }
    if tag == "ErrorBoundary" {
        return parse_error_boundary(input);
    }
    if tag == "Transition" {
        return parse_transition(input);
    }
    let mut attrs = Vec::new();
    loop {
        if input.peek(Token![/]) {
            input.parse::<Token![/]>()?;
            input.parse::<Token![>]>()?;
            return Ok(Node::El { tag: tag.to_string(), attrs, children: vec![] });
        }
        if input.peek(Token![>]) {
            input.parse::<Token![>]>()?;
            break;
        }
        // ref={node_ref}:元素引用(ref 是关键字,单独 peek)。
        if input.peek(Token![ref]) {
            input.parse::<Token![ref]>()?;
            input.parse::<Token![=]>()?;
            attrs.push(Attr::Ref { handle: input.parse()? });
            continue;
        }
        let name = Ident::parse_any(input)?; // parse_any:允许 `type` / `for` 等关键字做属性名
        if input.peek(Token![:]) {
            // 前缀语法:on:<事件>={...} 事件处理;bind:<属性>={signal} 双向绑定。
            input.parse::<Token![:]>()?;
            let sub: Ident = input.parse()?;
            // 事件修饰符:on:keydown.enter.prevent → 解析 `.标识符`*(prevent/stop/capture/passive/self
            // + 按键过滤 enter/esc/space/up… 由 router.js 区分应用)。
            let mut modifiers: Vec<String> = Vec::new();
            while input.peek(Token![.]) {
                input.parse::<Token![.]>()?;
                modifiers.push(Ident::parse_any(input)?.to_string());
            }
            input.parse::<Token![=]>()?;
            let block: syn::Block = input.parse()?;
            match name.to_string().as_str() {
                "on" => attrs.push(Attr::Event { event: sub.to_string(), modifiers, handler: block }),
                "bind" => {
                    if !modifiers.is_empty() {
                        return Err(syn::Error::new(name.span(), "bind: 不支持修饰符(修饰符用于 on:<事件>)"));
                    }
                    attrs.push(Attr::Bind { prop: sub.to_string(), expr: block });
                }
                _ => return Err(syn::Error::new(name.span(), "前缀只支持 on:<事件> 或 bind:<属性>")),
            }
        } else {
            input.parse::<Token![=]>()?;
            if input.peek(LitStr) {
                attrs.push(Attr::Static { name: name.to_string(), value: input.parse()? });
            } else {
                attrs.push(Attr::Dyn { name: name.to_string(), block: input.parse()? });
            }
        }
    }
    let children = parse_children(input)?;
    Ok(Node::El { tag: tag.to_string(), attrs, children })
}

fn parse_for(input: ParseStream) -> syn::Result<Node> {
    let mut list = None;
    let mut item = None;
    let mut key = None;
    loop {
        if input.peek(Token![>]) {
            input.parse::<Token![>]>()?;
            break;
        }
        let name: Ident = input.parse()?;
        input.parse::<Token![=]>()?;
        // key={ 关于 item 的表达式 };list / item 为标识符。
        if name == "key" {
            key = Some(input.parse::<syn::Block>()?);
        } else {
            let val: Ident = input.parse()?;
            if name == "list" {
                list = Some(val);
            } else if name == "item" {
                item = Some(val);
            }
        }
    }
    let children = parse_children(input)?;
    Ok(Node::For {
        list: list.expect("<For> 需要 list="),
        item: item.expect("<For> 需要 item="),
        key,
        children,
    })
}

// <Show when={闭包} fallback={闭包}?> children </Show>
fn parse_show(input: ParseStream) -> syn::Result<Node> {
    let mut when: Option<syn::Block> = None;
    let mut fallback: Option<syn::Block> = None;
    loop {
        if input.peek(Token![>]) {
            input.parse::<Token![>]>()?;
            break;
        }
        let name: Ident = input.parse()?;
        input.parse::<Token![=]>()?;
        let blk: syn::Block = input.parse()?;
        match name.to_string().as_str() {
            "when" => when = Some(blk),
            "fallback" => fallback = Some(blk),
            _ => return Err(syn::Error::new(name.span(), "<Show> 只支持 when= / fallback=")),
        }
    }
    let children = parse_children(input)?;
    let when = when.ok_or_else(|| syn::Error::new(Span::call_site(), "<Show> 需要 when={ 闭包 }"))?;
    Ok(Node::Show { when, fallback, children })
}

// <Switch> <Match when={闭包}> .. </Match> .. </Switch>
fn parse_switch(input: ParseStream) -> syn::Result<Node> {
    input.parse::<Token![>]>()?; // <Switch> 不接受属性
    let mut arms = Vec::new();
    loop {
        // 结束:</Switch>
        if input.peek(Token![<]) && input.peek2(Token![/]) {
            input.parse::<Token![<]>()?;
            input.parse::<Token![/]>()?;
            let _close: Ident = input.parse()?;
            input.parse::<Token![>]>()?;
            break;
        }
        input.parse::<Token![<]>()?;
        let m: Ident = input.parse()?;
        if m != "Match" {
            return Err(syn::Error::new(m.span(), "<Switch> 内只能放 <Match>"));
        }
        let mut when: Option<syn::Block> = None;
        loop {
            if input.peek(Token![>]) {
                input.parse::<Token![>]>()?;
                break;
            }
            let name: Ident = input.parse()?;
            input.parse::<Token![=]>()?;
            let blk: syn::Block = input.parse()?;
            if name == "when" {
                when = Some(blk);
            } else {
                return Err(syn::Error::new(name.span(), "<Match> 只支持 when="));
            }
        }
        let children = parse_children(input)?;
        let when =
            when.ok_or_else(|| syn::Error::new(Span::call_site(), "<Match> 需要 when={ 闭包 }"))?;
        arms.push((when, children));
    }
    Ok(Node::Switch { arms })
}

// <ErrorBoundary fallback={ |err, reset| view!{..} }> children </ErrorBoundary>
fn parse_error_boundary(input: ParseStream) -> syn::Result<Node> {
    let mut fallback: Option<syn::Block> = None;
    loop {
        if input.peek(Token![>]) {
            input.parse::<Token![>]>()?;
            break;
        }
        let name: Ident = input.parse()?;
        input.parse::<Token![=]>()?;
        let blk: syn::Block = input.parse()?;
        match name.to_string().as_str() {
            "fallback" => fallback = Some(blk),
            _ => return Err(syn::Error::new(name.span(), "<ErrorBoundary> 只支持 fallback=")),
        }
    }
    let children = parse_children(input)?;
    let fallback = fallback.ok_or_else(|| {
        syn::Error::new(Span::call_site(), "<ErrorBoundary> 需要 fallback={ |err, reset| view!{..} }")
    })?;
    Ok(Node::ErrorBoundary { fallback, children })
}

// <Transition name="fade" duration=300 when={ 闭包 }> child </Transition>
// name:CSS 类前缀(必填);duration:出场动画后移除的延时 ms(可选,默认 300);when:返回 bool 的闭包(必填)。
fn parse_transition(input: ParseStream) -> syn::Result<Node> {
    let mut name: Option<String> = None;
    let mut duration: u32 = 300;
    let mut when: Option<syn::Block> = None;
    loop {
        if input.peek(Token![>]) {
            input.parse::<Token![>]>()?;
            break;
        }
        let attr: Ident = input.parse()?;
        input.parse::<Token![=]>()?;
        match attr.to_string().as_str() {
            "name" => name = Some(input.parse::<LitStr>()?.value()),
            "duration" => {
                if !input.peek(LitInt) {
                    return Err(syn::Error::new(attr.span(), "<Transition> duration 须是编译期整数字面量(如 duration=300)"));
                }
                duration = input.parse::<LitInt>()?.base10_parse()?;
            }
            "when" => when = Some(input.parse::<syn::Block>()?),
            _ => return Err(syn::Error::new(attr.span(), "<Transition> 只支持 name= / duration= / when=")),
        }
    }
    let children = parse_children(input)?;
    let name = name.ok_or_else(|| syn::Error::new(Span::call_site(), "<Transition> 需要 name=\"...\"(CSS 类前缀)"))?;
    let when = when.ok_or_else(|| syn::Error::new(Span::call_site(), "<Transition> 需要 when={ move || cond.get() }"))?;
    // 单一子元素约束:多子 / <For> 会被 emit_branch 包进 rui-frag(display:contents),CSS 动画作用其上不可见。
    // 故强制单子元素(需要包多个时用户自己套一个 <div>)。
    if children.len() != 1 || matches!(children[0], Node::For { .. }) {
        return Err(syn::Error::new(Span::call_site(), "<Transition> 需要单一根子元素(多个或 <For> 请自行套一个 <div>);否则动画作用在 display:contents 包裹上不可见"));
    }
    Ok(Node::Transition { name, duration, when, children })
}

fn parse_children(input: ParseStream) -> syn::Result<Vec<Node>> {
    let mut children = Vec::new();
    loop {
        if input.peek(Token![<]) && input.peek2(Token![/]) {
            input.parse::<Token![<]>()?;
            input.parse::<Token![/]>()?;
            let _close: Ident = input.parse()?;
            input.parse::<Token![>]>()?;
            break;
        }
        children.push(parse_node(input)?);
    }
    Ok(children)
}

struct View(Node);
impl Parse for View {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        Ok(View(parse_node(input)?))
    }
}

fn is_reactive(b: &syn::Block) -> bool {
    matches!(b.stmts.last(), Some(Stmt::Expr(Expr::Closure(_), _)))
}
// 单表达式块 { x } → x(避免 unused_braces 警告);多语句块原样保留
fn unwrap_block(b: &syn::Block) -> TS2 {
    if b.stmts.len() == 1 {
        if let Stmt::Expr(e, None) = &b.stmts[0] {
            return quote! { #e };
        }
    }
    quote! { #b }
}
fn to_pascal(s: &str) -> String {
    s.split('_')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let mut c = p.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect()
}
fn to_snake(s: &str) -> String {
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if c.is_uppercase() {
            if i != 0 {
                out.push('_');
            }
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

// 元素的子节点:For 直接对父发射(清父+重建);其余生成节点表达式后 append
fn emit_children(children: &[Node]) -> Vec<TS2> {
    children.iter().map(|c| {
        if let Node::For { list, item, key, children } = c {
            match key {
                // keyed:按 key reconcile,复用节点保焦点;直接挂父节点(表格 <tr> 合法)。
                Some(k) => {
                    let key_expr = unwrap_block(k);
                    let body = emit_branch(children); // 每行单根节点(多子 → rui-frag 包裹)
                    quote! {
                        rui::view::keyed_for(
                            __n,
                            #list.clone(),
                            move |#item: &_| #key_expr,
                            move |#item: &_| #body,
                        );
                    }
                }
                // 非 keyed:沿用清父+重建(live/table 的「值变即重建」语义不变)。
                None => {
                    let body: Vec<TS2> = children.iter().map(|gc| {
                        let cg = gen_node(gc);
                        quote! { let __c = #cg; rui::dom::append(__n, __c); }
                    }).collect();
                    quote! {
                        rui::reactive::effect({
                            let #list = #list.clone();
                            move || {
                                rui::dom::clear(__n);
                                for #item in &#list.get() {
                                    #(#body)*
                                }
                            }
                        });
                    }
                }
            }
        } else {
            let cg = gen_node(c);
            quote! { let __c = #cg; rui::dom::append(__n, __c); }
        }
    }).collect()
}

// 一个分支的内容 → 单个节点表达式:单子节点直接 gen_node;多子节点(或含 For)包进 rui-frag 容器。
// 供 <Show>/<Switch> 用 —— dyn_node 要求 f 返回单个节点 id。
fn emit_branch(children: &[Node]) -> TS2 {
    if children.len() == 1 && !matches!(children[0], Node::For { .. }) {
        gen_node(&children[0])
    } else {
        let cstmts = emit_children(children);
        quote! {{
            let __n = rui::dom::el("rui-frag");
            #(#cstmts)*
            __n
        }}
    }
}

fn gen_node(n: &Node) -> TS2 {
    match n {
        Node::Text(s) => quote! { rui::dom::text(#s) },
        Node::Block(b) => {
            // 块按返回**类型**分派(IntoView):返回 View → 挂载/替换子树;返回 &str/数字等 → 文本。
            // 响应式块(末尾是闭包)走 reactive_block(就地更新);静态块构建一次。
            if is_reactive(b) {
                let f = unwrap_block(b); // 单语句块 → 去掉多余花括号(闭包直接作实参)
                quote! { rui::view::reactive_block(#f) }
            } else {
                let v = unwrap_block(b);
                quote! { rui::view::IntoView::build(#v).0 }
            }
        }
        Node::For { .. } => quote! { compile_error!("<For> 只能作为元素的子节点") },
        Node::Show { when, fallback, children } => {
            if !is_reactive(when) {
                return quote! { compile_error!("<Show> 的 when 需要闭包,例: when={ move || cond.get() }") };
            }
            let pred = unwrap_block(when);
            let branch = emit_branch(children);
            let fb = match fallback {
                Some(b) => {
                    if !is_reactive(b) {
                        return quote! { compile_error!("<Show> 的 fallback 需要闭包,例: fallback={ move || view!{ .. } }") };
                    }
                    unwrap_block(b)
                }
                None => quote! { (|| rui::View(rui::dom::text(""))) },
            };
            quote! {
                rui::view::reactive_block({
                    let __pred = #pred;
                    let __fb = #fb;
                    move || -> rui::View { if __pred() { rui::View(#branch) } else { __fb() } }
                })
            }
        }
        Node::Switch { arms } => {
            let mut binds = Vec::new();
            let mut checks = Vec::new();
            for (i, (when, children)) in arms.iter().enumerate() {
                if !is_reactive(when) {
                    return quote! { compile_error!("<Match> 的 when 需要闭包,例: when={ move || cond.get() }") };
                }
                let p = Ident::new(&format!("__p{}", i), Span::call_site());
                let pred = unwrap_block(when);
                let branch = emit_branch(children);
                binds.push(quote! { let #p = #pred; });
                checks.push(quote! { if #p() { return rui::View(#branch); } });
            }
            quote! {
                rui::view::reactive_block({
                    #(#binds)*
                    move || -> rui::View {
                        #(#checks)*
                        rui::View(rui::dom::text(""))
                    }
                })
            }
        }
        Node::ErrorBoundary { fallback, children } => {
            // fallback 是闭包 |err: String, reset: Rc<dyn Fn()>| -> View;children 每次(含重试)按需重建。
            // 复用 is_reactive(末句为闭包)做检测,与 <Show>/<Switch> 的 when/fallback 一致。
            if !is_reactive(fallback) {
                return quote! { compile_error!("<ErrorBoundary> 的 fallback 需要闭包,例: fallback={ |err: String, reset: std::rc::Rc<dyn Fn()>| view!{ .. } }") };
            }
            let fb = unwrap_block(fallback);
            let branch = emit_branch(children);
            quote! {
                rui::view::error_boundary(
                    #fb,
                    move || -> rui::View { rui::View(#branch) },
                )
            }
        }
        Node::Transition { name, duration, when, children } => {
            if !is_reactive(when) {
                return quote! { compile_error!("<Transition> 的 when 需要闭包,例: when={ move || cond.get() }") };
            }
            let pred = unwrap_block(when);
            let branch = emit_branch(children); // child 只构建一次(transition 内部按需显隐)
            quote! {
                rui::view::transition(#name, #duration, #pred, move || -> rui::View { rui::View(#branch) })
            }
        }
        Node::El { tag, attrs, children } => {
            let is_component = tag.chars().next().map(|c| c.is_uppercase()).unwrap_or(false);
            if is_component {
                // 组件:具名 props(按名匹配、类型由编译器校验)+ children 槽。走 typed builder:
                // <Card title=.. sub={x}>子节点</Card> → card(CardProps::builder().title(..).sub(x).children(View(..)).build())
                // 只设提供了的 prop;漏设必填 → build() 不可用(编译错);漏设可选 → 取默认。
                let f = Ident::new(&to_snake(tag), Span::call_site());
                let props = Ident::new(&format!("{}Props", tag), Span::call_site());
                let mut setters: Vec<TS2> = Vec::new();
                for a in attrs {
                    match a {
                        Attr::Static { name, value } => {
                            let id = Ident::new(name, Span::call_site());
                            setters.push(quote! { .#id(#value.to_string()) });
                        }
                        Attr::Dyn { name, block } => {
                            let id = Ident::new(name, Span::call_site());
                            let v = unwrap_block(block);
                            setters.push(quote! { .#id(#v) });
                        }
                        Attr::Event { .. } | Attr::Bind { .. } | Attr::Ref { .. } => {
                            return quote! { compile_error!("组件属性只支持 名=\"值\" 或 名={表达式};事件 / 绑定 / ref 请在组件内部处理") };
                        }
                    }
                }
                if !children.is_empty() {
                    let b = emit_branch(children); // 子节点 → 单个 View,作为 children 槽传入
                    setters.push(quote! { .children(rui::View(#b)) });
                }
                return quote! {
                    crate::__rui_registry::components::#f(
                        crate::__rui_registry::components::#props::builder() #(#setters)* .build()
                    ).node()
                };
            }
            let astmts: Vec<TS2> = attrs.iter().map(|a| match a {
                Attr::Static { name, value } => quote! { rui::dom::attr(__n, #name, #value); },
                Attr::Dyn { name, block } => {
                    if is_reactive(block) {
                        // 闭包属性 → 包进 effect:依赖变化时重设属性(镜像响应式文本路径)。
                        let f = unwrap_block(block);
                        quote! {{
                            let __af = #f;
                            rui::reactive::effect(move || rui::dom::attr(__n, #name, &::std::format!("{}", __af())));
                        }}
                    } else {
                        let v = unwrap_block(block);
                        quote! { rui::dom::attr(__n, #name, &(#v)); }
                    }
                }
                Attr::Event { event, modifiers, handler } => {
                    // on:<事件>[.修饰符]*:把用户(零参)闭包包成忽略 payload 的 Fn(&str);
                    // 事件描述符 = "事件 修饰符*"(空格分隔)交给 dom::on → router.js 据此应用修饰符。
                    // handler 内可用 rui::event() 取键盘/鼠标/files 等完整事件数据。
                    let h = unwrap_block(handler);
                    let desc = if modifiers.is_empty() {
                        event.clone()
                    } else {
                        format!("{} {}", event, modifiers.join(" "))
                    };
                    quote! {{ let __h = #h; rui::dom::on(__n, #desc, move |_: &str| __h()); }}
                }
                Attr::Ref { handle } => {
                    // 把刚创建 / 认领的元素 id 写进句柄;on_mount 里据此取真实节点。
                    let h = unwrap_block(handle);
                    quote! {{ let __rf = #h; __rf.set(__n); }}
                }
                Attr::Bind { prop, expr } => {
                    let s = unwrap_block(expr);
                    match prop.as_str() {
                        // 受控文本 / 数字 / <select>:signal→.value(effect 回写)+ 事件→parse 回 signal。
                        // signal 类型 T 只需 Display + FromStr:String(parse 无损)/ i64 / f64 都行;parse 失败
                        // (非法 / 空)则不写 → 数字框输入垃圾被忽略。<select> 用 change,文本用 input(实时)。
                        "value" => {
                            let event = if tag.as_str() == "select" { "change" } else { "input" };
                            quote! {{
                                { let __s = (#s).clone(); rui::reactive::effect(move || rui::dom::set_value(__n, &::std::format!("{}", __s.get()))); }
                                { let __s = (#s).clone(); rui::dom::on(__n, #event, move |__v: &str| { if let ::std::result::Result::Ok(__x) = __v.parse() { __s.set(__x); } }); }
                            }}
                        }
                        // 受控复选框(Signal<bool>):signal→.checked(effect)+ change 事件读 event().checked。
                        "checked" => quote! {{
                            { let __s = (#s).clone(); rui::reactive::effect(move || rui::dom::set_checked(__n, __s.get())); }
                            { let __s = (#s).clone(); rui::dom::on(__n, "change", move |_: &str| __s.set(rui::dom::event().checked)); }
                        }},
                        // 单选组(Signal<String>):checked = (signal == 本项 value);change → signal.set(本项 value)。
                        // 需配套 value="..."(从本元素自身的 value 属性取本 radio 的取值)。
                        "group" => {
                            let val = attrs.iter().find_map(|a| match a {
                                Attr::Static { name, value } if name == "value" => Some(quote! { #value.to_string() }),
                                Attr::Dyn { name, block } if name == "value" => {
                                    let v = unwrap_block(block);
                                    Some(quote! { (#v).to_string() })
                                }
                                _ => None,
                            });
                            match val {
                                // __val 在 effect **内**求值:静态 value 是常量;动态 value={signal} 则订阅它 → 值变也响应。
                                Some(val) => quote! {{
                                    { let __s = (#s).clone(); rui::reactive::effect(move || { let __val = #val; rui::dom::set_checked(__n, __s.get() == __val); }); }
                                    { let __s = (#s).clone(); rui::dom::on(__n, "change", move |__v: &str| __s.set(__v.to_string())); }
                                }},
                                None => quote! { compile_error!("bind:group 的 <input type=\"radio\"> 需配套 value=\"...\"(本项取值)"); },
                            }
                        }
                        _ => quote! { compile_error!("bind: 支持 value(文本/数字/select)、checked(复选框)、group(单选组)"); },
                    }
                }
            }).collect();
            let cstmts = emit_children(children);
            quote! {{
                let __n = rui::dom::el(#tag);
                #(#astmts)*
                #(#cstmts)*
                __n
            }}
        }
    }
}

#[proc_macro]
pub fn view(input: TokenStream) -> TokenStream {
    let v = syn::parse_macro_input!(input as View);
    let n = gen_node(&v.0); // 内部仍以节点 id(u32)构建;出口包成 rui::View。
    quote! { rui::View(#n) }.into()
}

// ───────────────────────── #[rui::component] ─────────────────────────
// 把 `fn card(title: String, children: View) -> View { BODY }` 改写成:
//   pub struct CardProps { pub title: String, pub children: View }
//   pub fn card(__props: CardProps) -> View { let CardProps { title, children } = __props; BODY }
// 于是 view! 里 <Card title=.. >子节点</Card> 用具名结构体字面量调用(按名匹配 + 类型校验 + children 槽)。
#[proc_macro_attribute]
pub fn component(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let f = syn::parse_macro_input!(item as syn::ItemFn);
    let vis = &f.vis;
    let name = &f.sig.ident;
    let output = &f.sig.output;
    let body = &f.block;
    // 组件会被改写成 props 结构体 + builder + fn(Props),泛型 / where 无处安放 → 明确报错而非静默丢弃。
    if !f.sig.generics.params.is_empty() || f.sig.generics.where_clause.is_some() {
        return syn::Error::new_spanned(
            &f.sig.generics,
            "#[rui::component] 暂不支持泛型 / where 约束(props 走具名结构体 + builder);如需多态请用具体类型或 enum",
        )
        .to_compile_error()
        .into();
    }
    let pascal = to_pascal(&name.to_string());
    let props_name = Ident::new(&format!("{}Props", pascal), Span::call_site());
    let builder_name = Ident::new(&format!("{}PropsBuilder", pascal), Span::call_site());

    // 收集字段:id / 类型 / 默认表达式(Some = 可选;None = 必填)。可选由 `#[prop(default)]` /
    // `#[prop(default = <expr>)]` / `#[prop(optional)]` 标注(#[prop] attr 被本宏消费,不会泄漏)。
    let mut ids: Vec<Ident> = Vec::new();
    let mut tys: Vec<syn::Type> = Vec::new();
    let mut defaults: Vec<Option<TS2>> = Vec::new();
    for arg in &f.sig.inputs {
        if let syn::FnArg::Typed(pt) = arg {
            if let syn::Pat::Ident(pi) = &*pt.pat {
                let mut def: Option<TS2> = None;
                for attr in &pt.attrs {
                    if attr.path().is_ident("prop") {
                        let mut d: TS2 = quote! { ::core::default::Default::default() };
                        let _ = attr.parse_nested_meta(|meta| {
                            if meta.path.is_ident("default") {
                                if meta.input.peek(Token![=]) {
                                    let v: syn::Expr = meta.value()?.parse()?;
                                    d = quote! { #v };
                                }
                                Ok(())
                            } else if meta.path.is_ident("optional") {
                                Ok(())
                            } else {
                                Err(meta.error("#[prop] 只支持 default / default = <expr> / optional"))
                            }
                        });
                        def = Some(d);
                    }
                }
                ids.push(pi.ident.clone());
                tys.push((*pt.ty).clone());
                defaults.push(def);
            }
        }
    }

    let n = ids.len();
    // 每字段一个类型参数(builder 状态:Missing → Set<T>)。S0/S1… CamelCase 合法。
    let snames: Vec<Ident> = (0..n).map(|i| Ident::new(&format!("S{i}"), Span::call_site())).collect();
    // <...>:空则不带尖括号(零 prop 组件)。
    let glist = |xs: &[TS2]| -> TS2 {
        if xs.is_empty() {
            quote! {}
        } else {
            quote! { <#(#xs),*> }
        }
    };

    let snames_g = glist(&snames.iter().map(|s| quote! { #s }).collect::<Vec<_>>());
    let missing_g = glist(&(0..n).map(|_| quote! { rui::props::Missing }).collect::<Vec<_>>());
    let struct_fields: Vec<TS2> = ids.iter().zip(&tys).map(|(id, ty)| quote! { pub #id: #ty }).collect();
    let builder_fields: Vec<TS2> = ids.iter().zip(&snames).map(|(id, s)| quote! { #id: #s }).collect();
    let builder_init: Vec<TS2> = ids.iter().map(|id| quote! { #id: rui::props::Missing }).collect();

    // 每字段一个 setter:字段 i 为 Missing 时可调,调后变 Set<T_i>(其余字段状态不变)。
    let setters: Vec<TS2> = (0..n)
        .map(|i| {
            let id = &ids[i];
            let ty = &tys[i];
            let impl_params: Vec<TS2> = (0..n).filter(|&j| j != i).map(|j| { let s = &snames[j]; quote! { #s } }).collect();
            let impl_g = glist(&impl_params);
            let self_args: Vec<TS2> = (0..n).map(|j| if j == i { quote! { rui::props::Missing } } else { let s = &snames[j]; quote! { #s } }).collect();
            let ret_args: Vec<TS2> = (0..n).map(|j| if j == i { quote! { rui::props::Set<#ty> } } else { let s = &snames[j]; quote! { #s } }).collect();
            let self_g = glist(&self_args);
            let ret_g = glist(&ret_args);
            let body_fields: Vec<TS2> = (0..n).map(|j| { let fj = &ids[j]; if j == i { quote! { #fj: rui::props::Set(v) } } else { quote! { #fj: self.#fj } } }).collect();
            quote! {
                #[allow(non_snake_case, non_camel_case_types)]
                impl #impl_g #builder_name #self_g {
                    #vis fn #id(self, v: #ty) -> #builder_name #ret_g {
                        #builder_name { #(#body_fields),* }
                    }
                }
            }
        })
        .collect();

    // build():必填字段在 Self 类型里固定 Set<T>(漏设 → Missing → 本 impl 不匹配 → build() 不可用);
    // 可选字段泛型 + OrDefault bound,build 时 or_default(默认值)。
    let build_impl_params: Vec<TS2> = (0..n).filter(|&j| defaults[j].is_some()).map(|j| { let s = &snames[j]; quote! { #s } }).collect();
    let build_impl_g = glist(&build_impl_params);
    let build_self_args: Vec<TS2> = (0..n).map(|j| if defaults[j].is_some() { let s = &snames[j]; quote! { #s } } else { let ty = &tys[j]; quote! { rui::props::Set<#ty> } }).collect();
    let build_self_g = glist(&build_self_args);
    let build_wheres: Vec<TS2> = (0..n).filter(|&j| defaults[j].is_some()).map(|j| { let s = &snames[j]; let ty = &tys[j]; quote! { #s: rui::props::OrDefault<#ty> } }).collect();
    let where_clause = if build_wheres.is_empty() { quote! {} } else { quote! { where #(#build_wheres),* } };
    let build_field_init: Vec<TS2> = (0..n).map(|j| { let fj = &ids[j]; if let Some(def) = &defaults[j] { quote! { #fj: self.#fj.or_default(|| #def) } } else { quote! { #fj: self.#fj.0 } } }).collect();

    quote! {
        #[allow(non_snake_case)]
        #vis struct #props_name { #(#struct_fields),* }

        #[allow(non_snake_case, non_camel_case_types)]
        #vis struct #builder_name #snames_g { #(#builder_fields),* }

        impl #props_name {
            #[allow(non_snake_case)]
            #vis fn builder() -> #builder_name #missing_g {
                #builder_name { #(#builder_init),* }
            }
        }

        #(#setters)*

        #[allow(non_snake_case, non_camel_case_types)]
        impl #build_impl_g #builder_name #build_self_g #where_clause {
            #vis fn build(self) -> #props_name {
                #props_name { #(#build_field_init),* }
            }
        }

        #[allow(non_snake_case)]
        #vis fn #name(__props: #props_name) #output {
            let #props_name { #(#ids),* } = __props;
            #body
        }
    }
    .into()
}

// ───────────────────────── #[rui::page(ssr|csr|static, "/路由模式")] ─────────────────────────
// 把页面函数 `fn view(id: Signal<String>) -> View { BODY }` 改写成:
//   pub const PATTERN: &str = "/todo/:id";              // router! 据此分发(路由模式声明在 page 自身)
//   pub fn view() -> rui::Page { Page::new(module_path!(), 策略, move || { let id = param_as(idx); BODY }) }
// 即:路由参数在 page 签名命名 + 类型化,`:id` 的位置从模式串解析,param_as 接线对用户透明。
struct PageAttr {
    strategy: TS2,
    pattern: String,
    name: Option<String>, // 可选显式 RouteId(身份与 URL 解耦:如同模式两套策略,或想跨模式变更保持同 key)
}
impl Parse for PageAttr {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut strategy = quote! { rui::Strategy::Ssr };
        let mut pattern = String::new();
        let mut name = None;
        while !input.is_empty() {
            if input.peek(LitStr) {
                pattern = input.parse::<LitStr>()?.value(); // 路由模式串,如 "/todo/:id"
            } else if input.peek(Token![static]) {
                input.parse::<Token![static]>()?; // static 是关键字,单独 peek
                strategy = quote! { rui::Strategy::Static };
            } else {
                let id: Ident = input.parse()?;
                match id.to_string().as_str() {
                    "ssr" => strategy = quote! { rui::Strategy::Ssr },
                    "csr" => strategy = quote! { rui::Strategy::Csr },
                    "name" => {
                        input.parse::<Token![=]>()?;
                        name = Some(input.parse::<LitStr>()?.value()); // name = "todo_detail"
                    }
                    other => {
                        return Err(syn::Error::new(
                            id.span(),
                            format!("#[rui::page] 只支持 ssr / csr / static / name=\"..\" + 可选路由模式串,收到 `{other}`"),
                        ))
                    }
                }
            }
            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
        }
        Ok(PageAttr { strategy, pattern, name })
    }
}

#[proc_macro_attribute]
pub fn page(attr: TokenStream, item: TokenStream) -> TokenStream {
    let PageAttr { strategy: strat, pattern, name: route_name } = syn::parse_macro_input!(attr as PageAttr);
    let f = syn::parse_macro_input!(item as syn::ItemFn);
    emit_page(strat, pattern, route_name, f)
}

// 共享的页面发射逻辑(`#[page]` 与 `#[route(ssr|csr|static)]` 复用,产出 `fn() -> rui::Page`)。
fn emit_page(strat: TS2, pattern: String, route_name: Option<String>, f: syn::ItemFn) -> TokenStream {
    let vis = &f.vis;
    let name = &f.sig.ident;
    // 稳定 RouteId(取代 module_path!()):显式 name 优先;否则用路由模式串(顶层页即全路径,稳定、可读、跨重命名/跨 crate 不变);
    // 二者皆无(罕见的无模式页)才回退 module_path!() 以保唯一身份。组成员的 key 由 router! 的组接管(此 key 被丢弃)。
    let route_id = match &route_name {
        Some(n) => quote! { #n },
        None if !pattern.is_empty() => quote! { #pattern },
        None => quote! { module_path!() },
    };
    let attrs = &f.attrs; // 转发到生成的 fn(保留 #[doc] / #[cfg] / #[allow] 等)
    let body = &f.block; // 原函数体,返回 View

    // 页面会被改写成 `fn() -> rui::Page`,泛型 / where 无处安放 → 明确报错而非静默丢弃。
    if !f.sig.generics.params.is_empty() || f.sig.generics.where_clause.is_some() {
        return syn::Error::new_spanned(
            &f.sig.generics,
            "#[rui::page] 不支持泛型 / where 约束:页面函数会被改写为 fn() -> rui::Page",
        )
        .to_compile_error()
        .into();
    }

    // 模式里 `:name` 段 → 段索引(0 基,忽略空段),供签名参数按名定位。
    let seg_index = |pname: &str| -> Option<usize> {
        pattern
            .split('/')
            .filter(|s| !s.is_empty())
            .position(|s| s.strip_prefix(':') == Some(pname))
    };
    // 签名里的每个参数 `name: Signal<T>` → `let name: Signal<T> = rui::param_as(idx);`
    // (T 由注解推断,param_as 据模式段索引取值。)接收者 / 非标识符参数明确报错,不静默丢弃。
    let mut bindings: Vec<TS2> = Vec::new();
    let mut param_names: Vec<String> = Vec::new();
    for arg in &f.sig.inputs {
        let pt = match arg {
            syn::FnArg::Typed(pt) => pt,
            syn::FnArg::Receiver(r) => {
                return syn::Error::new_spanned(
                    r,
                    "#[rui::page] 不支持 self 接收者:页面是自由函数,参数只能是路由模式里的 `:name` 段",
                )
                .to_compile_error()
                .into();
            }
        };
        let pi = match &*pt.pat {
            syn::Pat::Ident(pi) => pi,
            other => {
                return syn::Error::new_spanned(
                    other,
                    "#[rui::page] 参数必须是简单标识符(如 `id: Signal<String>`),不支持 `_` / 元组 / 解构",
                )
                .to_compile_error()
                .into();
            }
        };
        let pname = &pi.ident;
        let ty = &pt.ty;
        match seg_index(&pname.to_string()) {
            Some(idx) => bindings.push(quote! { let #pname: #ty = rui::param_as(#idx); }),
            None => {
                let msg = format!(
                    "#[rui::page] 参数 `{}` 不在路由模式 `{}` 里(应为 `:{}` 段)",
                    pname, pattern, pname
                );
                return syn::Error::new(pname.span(), msg).to_compile_error().into();
            }
        }
        param_names.push(pname.to_string());
    }
    // 反向校验:模式里每个 `:name` 段都要在签名里有对应参数(防 dead 段 + 让拼写错指向真正的笔误)。
    for seg in pattern.split('/').filter(|s| !s.is_empty()) {
        if let Some(seg_name) = seg.strip_prefix(':') {
            if !param_names.iter().any(|p| p == seg_name) {
                let msg = format!(
                    "#[rui::page] 路由模式 `{}` 的 `:{}` 段在签名里没有对应参数(加 `{}: Signal<T>` 或修正拼写)",
                    pattern, seg_name, seg_name
                );
                return syn::Error::new(name.span(), msg).to_compile_error().into();
            }
        }
    }

    quote! {
        // 路由模式 + 策略:router! 据此分发 / 取策略(内部名,避免与用户同名项撞)。
        #[doc(hidden)]
        #vis const __RUI_PATTERN: &str = #pattern;
        #[doc(hidden)]
        #vis const __RUI_STRATEGY: rui::Strategy = #strat;
        #(#attrs)*
        #vis fn #name() -> rui::Page {
            // key = 稳定 RouteId(显式 name / 路由模式;见上)。同一页(不同参数)key 相同、不同页 key 不同
            // → 导航时据此判断「重建」or「仅换参数」。不再用 module_path!()(重命名 / 移动 / 跨 crate 复用都稳定)。
            rui::Page::new(#route_id, #strat, move || {
                #(#bindings)* // 路由参数:模式里的 :name → 对应 signal(reactive,同页换参数即变)
                #body
            })
        }
    }
    .into()
}

// ───────────────────────── #[rui::route(ssr|csr|static, "...")] ─────────────────────────
// 统一的页面路由声明宏:ssr / csr / static 与 `#[page]` 完全等价(`#[route(ssr,..)]` 即 `#[page(ssr,..)]` 别名)。
// 本框架是 **GraphQL-native**:JSON API 就是 GraphQL(/graphql + query!/mutation!/subscription!),
// 不做 REST —— 故 `#[route]` 没有 `api` 种类(写了会给出友好报错,引导用 GraphQL)。
struct RouteAttr {
    strat: TS2,
    pattern: String,
    name: Option<String>,
}
impl Parse for RouteAttr {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        // 第一个 token = 渲染策略。static 是关键字,单独 peek。
        let strat = if input.peek(Token![static]) {
            input.parse::<Token![static]>()?;
            quote! { rui::Strategy::Static }
        } else {
            let id: Ident = input.parse()?;
            match id.to_string().as_str() {
                "ssr" => quote! { rui::Strategy::Ssr },
                "csr" => quote! { rui::Strategy::Csr },
                "api" => {
                    return Err(syn::Error::new(
                        id.span(),
                        "#[rui::route] 不提供 REST `api`:本框架是 GraphQL-native,JSON API 用 GraphQL(/graphql + query!/mutation!)。#[route] 只支持 ssr / csr / static",
                    ))
                }
                other => {
                    return Err(syn::Error::new(
                        id.span(),
                        format!("#[rui::route] 第一个参数须是 ssr / csr / static,收到 `{other}`"),
                    ))
                }
            }
        };
        if input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
        }
        // 路由模式串 + 可选 name=(复用 page 语法)。
        let mut pattern = String::new();
        let mut name = None;
        while !input.is_empty() {
            if input.peek(LitStr) {
                pattern = input.parse::<LitStr>()?.value();
            } else {
                let id: Ident = input.parse()?;
                if id == "name" {
                    input.parse::<Token![=]>()?;
                    name = Some(input.parse::<LitStr>()?.value());
                } else {
                    return Err(syn::Error::new(
                        id.span(),
                        format!("#[rui::route] 只支持路由模式串 + 可选 name=\"..\",收到 `{id}`"),
                    ));
                }
            }
            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
        }
        Ok(RouteAttr { strat, pattern, name })
    }
}

#[proc_macro_attribute]
pub fn route(attr: TokenStream, item: TokenStream) -> TokenStream {
    let RouteAttr { strat, pattern, name } = syn::parse_macro_input!(attr as RouteAttr);
    let f = syn::parse_macro_input!(item as syn::ItemFn);
    emit_page(strat, pattern, name, f)
}

// ───────────────────────── #[rui::job] ─────────────────────────
// AsyncJob 声明:把 `fn name(ctx: &JobCtx, payload: P) -> JobResult { BODY }` 改写成
//   #[cfg(not wasm)] struct name;  +  impl rui::Job for name { type Payload = P; const NAME = ".."; fn run(..) { BODY } }
// marker 类型沿用用户函数名(`enqueue::<name>(P{..})` 与 `platform!{ jobs { ..::name } }` 直接引用)。
#[proc_macro_attribute]
pub fn job(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let f = syn::parse_macro_input!(item as syn::ItemFn);
    emit_job(f)
}
fn emit_job(f: syn::ItemFn) -> TokenStream {
    let vis = &f.vis;
    let name = &f.sig.ident;
    let name_str = name.to_string();
    let inputs = &f.sig.inputs;
    let output = &f.sig.output;
    let body = &f.block;
    if f.sig.asyncness.is_some() {
        return syn::Error::new_spanned(
            &f.sig,
            "#[rui::job] 第一版为同步(引擎同步);async 支持随 async 引擎到来",
        )
        .to_compile_error()
        .into();
    }
    if !f.sig.generics.params.is_empty() || f.sig.generics.where_clause.is_some() {
        return syn::Error::new_spanned(&f.sig.generics, "#[rui::job] 不支持泛型 / where 约束")
            .to_compile_error()
            .into();
    }
    if matches!(output, syn::ReturnType::Default) {
        return syn::Error::new_spanned(&f.sig, "#[rui::job] 必须返回 rui::JobResult(`-> JobResult`)")
            .to_compile_error()
            .into();
    }
    if inputs.iter().any(|a| matches!(a, syn::FnArg::Receiver(_))) {
        return syn::Error::new_spanned(inputs, "#[rui::job] 不支持 self 接收者:job handler 是自由函数")
            .to_compile_error()
            .into();
    }
    // 恰好两个参数:`ctx: &rui::JobCtx, payload: <类型>`。payload 类型 → Job::Payload。
    let typed: Vec<&syn::PatType> =
        inputs.iter().filter_map(|a| if let syn::FnArg::Typed(pt) = a { Some(pt) } else { None }).collect();
    if typed.len() != 2 {
        return syn::Error::new_spanned(
            inputs,
            "#[rui::job] handler 需恰好两个参数:`ctx: &rui::JobCtx, payload: <类型>`",
        )
        .to_compile_error()
        .into();
    }
    let payload_ty = &typed[1].ty;
    quote! {
        #[allow(non_camel_case_types)]
        #[cfg(not(target_arch = "wasm32"))]
        #vis struct #name;
        #[cfg(not(target_arch = "wasm32"))]
        impl rui::Job for #name {
            type Payload = #payload_ty;
            const NAME: &'static str = #name_str;
            // 用户原签名(ctx + payload)+ 原体作 run;payload 参数类型 == Self::Payload,合法。
            fn run(#inputs) #output #body
        }
    }
    .into()
}

// ───────────────────────── #[rui::cron(every = "..")] ─────────────────────────
// CronJob 声明:把 `fn name(ctx: &JobCtx, tick: CronTick) -> JobResult { BODY }` 改写成
//   marker `struct name;` + `impl rui::Job`(payload=CronTick,故 worker 能跑)+ `impl rui::CronJob`(带触发间隔)。
// 第一版 = 固定间隔(every = "30s"/"5m"/"1h"/"1d");完整 cron 表达式("0 0 2 * * *")是后续。
struct CronAttr {
    secs: u64,
}
impl Parse for CronAttr {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let kw: Ident = input
            .parse()
            .map_err(|_| syn::Error::new(Span::call_site(), "#[rui::cron] 需要 `every = \"<时长>\"`(如 every = \"30s\")"))?;
        if kw != "every" {
            return Err(syn::Error::new(
                kw.span(),
                "#[rui::cron] 第一版只支持 `every = \"<时长>\"`(如 every = \"30s\" / \"5m\" / \"1h\" / \"1d\");完整 cron 表达式是后续",
            ));
        }
        input.parse::<Token![=]>()?;
        let lit: LitStr = input.parse()?;
        let secs = parse_duration(&lit.value()).ok_or_else(|| {
            syn::Error::new(lit.span(), "时长格式:正整数 + s/m/h/d(如 \"30s\" / \"5m\" / \"1h\" / \"1d\")")
        })?;
        Ok(CronAttr { secs })
    }
}
// "30s"→30 / "5m"→300 / "2h"→7200 / "1d"→86400。无后缀或非正整数 → None。
fn parse_duration(s: &str) -> Option<u64> {
    let s = s.trim();
    let (num, mult) = if let Some(n) = s.strip_suffix('s') {
        (n, 1u64)
    } else if let Some(n) = s.strip_suffix('m') {
        (n, 60)
    } else if let Some(n) = s.strip_suffix('h') {
        (n, 3600)
    } else if let Some(n) = s.strip_suffix('d') {
        (n, 86400)
    } else {
        return None;
    };
    let n: u64 = num.trim().parse().ok()?;
    if n == 0 {
        return None;
    }
    Some(n * mult)
}

#[proc_macro_attribute]
pub fn cron(attr: TokenStream, item: TokenStream) -> TokenStream {
    let CronAttr { secs } = syn::parse_macro_input!(attr as CronAttr);
    let f = syn::parse_macro_input!(item as syn::ItemFn);
    emit_cron(secs, f)
}

fn emit_cron(secs: u64, f: syn::ItemFn) -> TokenStream {
    let vis = &f.vis;
    let name = &f.sig.ident;
    let name_str = name.to_string();
    let inputs = &f.sig.inputs;
    let output = &f.sig.output;
    let body = &f.block;
    if f.sig.asyncness.is_some() {
        return syn::Error::new_spanned(&f.sig, "#[rui::cron] 第一版为同步(引擎同步);async 支持随 async 引擎到来")
            .to_compile_error()
            .into();
    }
    if !f.sig.generics.params.is_empty() || f.sig.generics.where_clause.is_some() {
        return syn::Error::new_spanned(&f.sig.generics, "#[rui::cron] 不支持泛型 / where 约束")
            .to_compile_error()
            .into();
    }
    if matches!(output, syn::ReturnType::Default) {
        return syn::Error::new_spanned(&f.sig, "#[rui::cron] 必须返回 rui::JobResult(`-> JobResult`)")
            .to_compile_error()
            .into();
    }
    if f.sig.inputs.iter().any(|a| matches!(a, syn::FnArg::Receiver(_))) {
        return syn::Error::new_spanned(inputs, "#[rui::cron] 不支持 self 接收者:cron handler 是自由函数")
            .to_compile_error()
            .into();
    }
    let typed = inputs.iter().filter(|a| matches!(a, syn::FnArg::Typed(_))).count();
    if typed != 2 {
        return syn::Error::new_spanned(
            inputs,
            "#[rui::cron] handler 需恰好两个参数:`ctx: &rui::JobCtx, tick: rui::CronTick`",
        )
        .to_compile_error()
        .into();
    }
    quote! {
        #[allow(non_camel_case_types)]
        #[cfg(not(target_arch = "wasm32"))]
        #vis struct #name;
        // cron job 也是 Job(payload 固定 CronTick)→ worker 的 run_job 能按 NAME 解码 + 执行。
        #[cfg(not(target_arch = "wasm32"))]
        impl rui::Job for #name {
            type Payload = rui::CronTick;
            const NAME: &'static str = #name_str;
            fn run(#inputs) #output #body
        }
        #[cfg(not(target_arch = "wasm32"))]
        impl rui::CronJob for #name {
            const INTERVAL_SECS: u64 = #secs;
        }
    }
    .into()
}

// ───────────────────────── router!(路由表 = 候选页 / 路由组列表)─────────────────────────
// 用法:rui::router! {
//   [layout = path::shell,]                         // 全局外壳(包裹每个命中项)
//   pages::index, pages::detail, ...,               // 顶层页(各自 #[rui::page("...")] 声明模式)
//   group("/dash", layout = path::dash_shell) {     // 路由组:组内页共享 dash_shell 内层布局
//     pages::overview,                              // 模式 "/"  → "/dash"
//     pages::settings,                              // 模式 "/settings" → "/dash/settings"
//   },
//   fallback = not_found,
// }
// 生成 `pub fn route(path) -> Page`:按声明顺序匹配,命中第一个则取其 Page;都不中走 fallback。
// 组 = 一个 Page(key 固定为 "group:<prefix>"):组内不同页导航时 key 相同 → 不重建,
//   组布局(dash_shell)持续存在,只有内层 outlet(reactive_block 按 path 选叶子)换内容(不闪、保状态)。
// 注意:① 匹配按声明顺序(首中即用),具体字面量路由列在 `:param` 通配前。
//   ② 组内页可自带 `:param`:outlet 用 with_param_offset(组前缀段数) 包叶子 render,param 读到绝对段(已修)。
enum RouterItem {
    Page(syn::Path),
    Group { prefix: String, layout: syn::Path, pages: Vec<syn::Path> },
}
struct Router {
    layout: Option<syn::Path>,
    fallback: Option<syn::Path>,
    items: Vec<RouterItem>,
}
impl Parse for Router {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut layout = None;
        let mut fallback = None;
        let mut items = Vec::new();
        while !input.is_empty() {
            // group("/prefix", layout = path) { 页, 页, ... }
            if input.peek(Ident) && input.peek2(syn::token::Paren) && {
                let fork = input.fork();
                fork.parse::<Ident>().map(|i| i == "group").unwrap_or(false)
            } {
                input.parse::<Ident>()?; // group
                let head;
                syn::parenthesized!(head in input);
                let prefix: LitStr = head.parse()?;
                head.parse::<Token![,]>()?;
                let lkw: Ident = head.parse()?;
                if lkw != "layout" {
                    return Err(syn::Error::new(lkw.span(), "group(...) 里只支持 layout = <fn>"));
                }
                head.parse::<Token![=]>()?;
                let glayout: syn::Path = head.parse()?;
                let body;
                syn::braced!(body in input);
                let mut pages = Vec::new();
                while !body.is_empty() {
                    pages.push(body.parse::<syn::Path>()?);
                    if body.peek(Token![,]) {
                        body.parse::<Token![,]>()?;
                    }
                }
                if pages.is_empty() {
                    return Err(syn::Error::new(prefix.span(), "group(...) 至少需要一个成员页;空组匹配不到任何路由"));
                }
                items.push(RouterItem::Group { prefix: prefix.value(), layout: glayout, pages });
            } else if input.peek(Ident) && input.peek2(Token![=]) {
                // layout = ... / fallback = ...
                let kw: Ident = input.parse()?;
                input.parse::<Token![=]>()?;
                let val: syn::Path = input.parse()?;
                match kw.to_string().as_str() {
                    "layout" => layout = Some(val),
                    "fallback" => fallback = Some(val),
                    other => return Err(syn::Error::new(kw.span(), format!("router! 只支持 layout= / fallback= / group(...){{}},收到 `{other}`"))),
                }
            } else {
                // 调用形顶层项(如 make_page() / notgroup("/x"))不是合法页条目 → 明确诊断,
                // 而非 syn::Path 解析到 `(` 时的「unexpected token」。页条目须为路径;路由组用 group(...){}。
                if input.peek(Ident) && input.peek2(syn::token::Paren) {
                    let id: Ident = input.fork().parse()?;
                    return Err(syn::Error::new(
                        id.span(),
                        format!("router! 的页条目必须是页路径(如 `pages::index`);路由组用 `group(\"/前缀\", layout = <fn>) {{ ... }}`,收到 `{id}(...)`"),
                    ));
                }
                items.push(RouterItem::Page(input.parse::<syn::Path>()?));
            }
            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
        }
        Ok(Router { layout, fallback, items })
    }
}

// 共享路由代码生成:把路由项(页面 + 组 + API)生成 `pub fn route(path)->Page` + `pub fn api(method,path)->Option<ApiRoute>`。
// `router!` 与 `platform!` 都用它(`platform!` 再额外生成 `app() -> AppRuntime`)。
fn emit_route_table(layout: &Option<syn::Path>, fallback_tok: &TS2, items: &[RouterItem]) -> TS2 {
    // 每个 item → (匹配条件, 命中后的 Page 表达式)。
    let mut entries: Vec<(TS2, TS2)> = Vec::new();
    for item in items {
        match item {
            RouterItem::Page(p) => {
                entries.push((
                    quote! { rui::matches(#p::__RUI_PATTERN, __path) },
                    quote! { #p::view() },
                ));
            }
            RouterItem::Group { prefix, layout: glayout, pages } => {
                // 成员前缀模式匹配(运行时拼前缀):matches("/dash" + 成员模式, path)。
                let cond_for = |target: &TS2, m: &syn::Path| {
                    quote! { rui::matches(&::std::format!("{}{}", #prefix, #m::__RUI_PATTERN), #target) }
                };
                let path_tok = quote! { __path };
                let group_cond = {
                    let conds: Vec<TS2> = pages.iter().map(|m| cond_for(&path_tok, m)).collect();
                    quote! { #(#conds)||* }
                };
                // 内层 outlet:reactive_block 按当前 path 选叶子(组布局只建一次,叶子随 path 换)。
                // 组前缀段数 = 叶子 param 的绝对偏移:with_param_offset 让组内页的 `:param` 读到绝对段
                //(否则 #[page] 烘焙的是页内相对索引,会少算前缀段数 → 读错段)。
                let prefix_segs = prefix.split('/').filter(|s| !s.is_empty()).count();
                let lp_tok = quote! { (&__lp) };
                let mut leaf = quote! { rui::View(rui::dom::text("")) };
                for m in pages.iter().rev() {
                    let lc = cond_for(&lp_tok, m);
                    leaf = quote! { if #lc { rui::runtime::with_param_offset(#prefix_segs, || (#m::view().render)()) } else { #leaf } };
                }
                // 组策略 = 当前 path 命中的叶子的 strategy(直接加载 /dash/x 时按该叶子 ssr/csr/static 渲染)。
                // 组内导航始终是客户端(同 key 不回服务端),故跨策略叶子的切换天然客户端渲染,正确。
                let mut strat = quote! { rui::Strategy::Ssr };
                for m in pages.iter().rev() {
                    let sc = cond_for(&path_tok, m);
                    strat = quote! { if #sc { #m::__RUI_STRATEGY } else { #strat } };
                }
                let gkey = format!("group:{}", prefix);
                entries.push((
                    group_cond,
                    quote! {{
                        let __gp = __path.to_string();
                        rui::Page::new(#gkey, #strat, move || {
                            #glayout(&__gp, rui::View(rui::view::reactive_block(move || -> rui::View {
                                let __lp = rui::path().get();
                                #leaf
                            })))
                        })
                    }},
                ));
            }
        }
    }
    // 从后往前折叠成 if/else if … else { fallback }。
    let mut chain = quote! { rui::Page::new("not_found", rui::Strategy::Ssr, #fallback_tok) };
    for (cond, page) in entries.iter().rev() {
        chain = quote! { if #cond { #page } else { #chain } };
    }
    // 全局 layout 包裹(可选):保留命中项的 key/strategy,渲染交给 layout 包外壳。
    let wrap = match layout {
        Some(layout) => quote! {
            let (__key, __strategy, __render) = (__inner.key.clone(), __inner.strategy, __inner.render);
            let __p = __path.to_string();
            rui::Page {
                key: __key,
                strategy: __strategy,
                render: ::std::boxed::Box::new(move || #layout(&__p, __render())),
            }
        },
        None => quote! { __inner },
    };
    quote! {
        pub fn route(__path: &str) -> rui::Page {
            let __inner: rui::Page = #chain;
            #wrap
        }
    }
}

#[proc_macro]
pub fn router(input: TokenStream) -> TokenStream {
    let r = syn::parse_macro_input!(input as Router);
    let fallback = match &r.fallback {
        Some(f) => quote! { #f },
        None => {
            return syn::Error::new(Span::call_site(), "router! 需要 `fallback = <返回 View 的 fn>`")
                .to_compile_error()
                .into()
        }
    };
    emit_route_table(&r.layout, &fallback, &r.items).into()
}

// ───────────────────────── platform!(统一装配:路由 + 数据 + 订阅 + 任务 → AppRuntime)─────────────────────────
// 顶层关注点并列:
//   routes { layout=, pages..., group(){}, fallback= }   Web 路由表(页面多时收进 routes 段,顶层不嘈杂)
//   resolve = <fn>                                        GraphQL resolver(缺省 = rui::empty_resolver)
//   subscribe { snapshot = <fn>, feed = <fn> }            SSE 订阅(可选)
//   jobs { path::a, path::b }                             后台 AsyncJob(可选)
// 路由项(layout/pages/group/fallback)也可直接平铺在顶层(向后兼容:小应用不必包 routes{})。
// 生成 `route()`(同构)+ `pub fn app() -> rui::AppRuntime`(+ jobs 的 run_job;一处声明、bin 一行 `serve(app())`)。
// 取代「`router!` 生成 route + bin/ssr.rs 手搓 `App { .. }`」的分散装配。JSON API 是 GraphQL(/graphql),不在此声明。
// 解析单个路由项(layout= / fallback= / group(){} / 页路径):platform! 顶层平铺与 `routes { }` 段内共用。
fn parse_routing_item(
    input: ParseStream,
    layout: &mut Option<syn::Path>,
    fallback: &mut Option<syn::Path>,
    items: &mut Vec<RouterItem>,
) -> syn::Result<()> {
    let is_group = input.peek(Ident)
        && input.peek2(syn::token::Paren)
        && input.fork().parse::<Ident>().map(|i| i == "group").unwrap_or(false);
    if is_group {
        input.parse::<Ident>()?; // group
        let head;
        syn::parenthesized!(head in input);
        let prefix: LitStr = head.parse()?;
        head.parse::<Token![,]>()?;
        let lkw: Ident = head.parse()?;
        if lkw != "layout" {
            return Err(syn::Error::new(lkw.span(), "group(...) 里只支持 layout = <fn>"));
        }
        head.parse::<Token![=]>()?;
        let glayout: syn::Path = head.parse()?;
        let gbody;
        syn::braced!(gbody in input);
        let mut pages = Vec::new();
        while !gbody.is_empty() {
            pages.push(gbody.parse::<syn::Path>()?);
            if gbody.peek(Token![,]) {
                gbody.parse::<Token![,]>()?;
            }
        }
        if pages.is_empty() {
            return Err(syn::Error::new(prefix.span(), "group(...) 至少需要一个成员页"));
        }
        items.push(RouterItem::Group { prefix: prefix.value(), layout: glayout, pages });
    } else if input.peek(Ident) && input.peek2(Token![=]) {
        let kw: Ident = input.parse()?;
        input.parse::<Token![=]>()?;
        let val: syn::Path = input.parse()?;
        match kw.to_string().as_str() {
            "layout" => *layout = Some(val),
            "fallback" => *fallback = Some(val),
            other => {
                return Err(syn::Error::new(
                    kw.span(),
                    format!("路由项只支持 layout= / fallback= / group(){{}} / 页路径,收到 `{other}`(resolve= / subscribe{{}} / jobs{{}} 放在 platform! 顶层,不在 routes 内)"),
                ))
            }
        }
    } else {
        if input.peek(Ident) && input.peek2(syn::token::Paren) {
            let id: Ident = input.fork().parse()?;
            return Err(syn::Error::new(
                id.span(),
                format!("页条目必须是页路径(如 `pages::index`);路由组用 `group(\"/前缀\", layout = <fn>) {{ ... }}`,收到 `{id}(...)`"),
            ));
        }
        items.push(RouterItem::Page(input.parse::<syn::Path>()?));
    }
    Ok(())
}

struct Platform {
    layout: Option<syn::Path>,
    fallback: Option<syn::Path>,
    items: Vec<RouterItem>,
    resolve: Option<syn::Path>,
    subscribe: Option<(syn::Path, syn::Path)>, // (snapshot, feed)
    jobs: Vec<syn::Path>,                      // jobs { path::a, path::b } → 后台 AsyncJob
    crons: Vec<syn::Path>,                     // crons { path::a } → 定时任务(#[rui::cron])
    database: Option<String>,                  // database = postgres → 部署模型里的 DB 资源
}
impl Parse for Platform {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut layout = None;
        let mut fallback = None;
        let mut items = Vec::new();
        let mut resolve = None;
        let mut subscribe = None;
        let mut jobs: Vec<syn::Path> = Vec::new();
        let mut crons: Vec<syn::Path> = Vec::new();
        let mut database: Option<String> = None;
        while !input.is_empty() {
            let is_kw = |name: &str| {
                input.peek(Ident) && input.fork().parse::<Ident>().map(|i| i == name).unwrap_or(false)
            };
            if is_kw("subscribe") && input.peek2(syn::token::Brace) {
                input.parse::<Ident>()?; // subscribe
                let sbody;
                syn::braced!(sbody in input);
                let mut snap: Option<syn::Path> = None;
                let mut feed: Option<syn::Path> = None;
                while !sbody.is_empty() {
                    let k: Ident = sbody.parse()?;
                    sbody.parse::<Token![=]>()?;
                    let v: syn::Path = sbody.parse()?;
                    match k.to_string().as_str() {
                        "snapshot" => snap = Some(v),
                        "feed" => feed = Some(v),
                        o => {
                            return Err(syn::Error::new(
                                k.span(),
                                format!("subscribe {{ }} 只支持 snapshot = .. / feed = ..,收到 `{o}`"),
                            ))
                        }
                    }
                    if sbody.peek(Token![,]) {
                        sbody.parse::<Token![,]>()?;
                    }
                }
                let snap = snap
                    .ok_or_else(|| syn::Error::new(Span::call_site(), "subscribe { } 缺 snapshot = .."))?;
                let feed =
                    feed.ok_or_else(|| syn::Error::new(Span::call_site(), "subscribe { } 缺 feed = .."))?;
                subscribe = Some((snap, feed));
            } else if is_kw("jobs") && input.peek2(syn::token::Brace) {
                input.parse::<Ident>()?; // jobs
                let jbody;
                syn::braced!(jbody in input);
                while !jbody.is_empty() {
                    jobs.push(jbody.parse::<syn::Path>()?);
                    if jbody.peek(Token![,]) {
                        jbody.parse::<Token![,]>()?;
                    }
                }
            } else if is_kw("crons") && input.peek2(syn::token::Brace) {
                input.parse::<Ident>()?; // crons
                let cbody;
                syn::braced!(cbody in input);
                while !cbody.is_empty() {
                    crons.push(cbody.parse::<syn::Path>()?);
                    if cbody.peek(Token![,]) {
                        cbody.parse::<Token![,]>()?;
                    }
                }
            } else if is_kw("routes") && input.peek2(syn::token::Brace) {
                // routes { layout=, pages..., group(){}, fallback= }:把路由表整体收进来,顶层更清爽。
                input.parse::<Ident>()?; // routes
                let rbody;
                syn::braced!(rbody in input);
                while !rbody.is_empty() {
                    parse_routing_item(&rbody, &mut layout, &mut fallback, &mut items)?;
                    if rbody.peek(Token![,]) {
                        rbody.parse::<Token![,]>()?;
                    }
                }
            } else if is_kw("resolve") && input.peek2(Token![=]) {
                input.parse::<Ident>()?; // resolve
                input.parse::<Token![=]>()?;
                resolve = Some(input.parse::<syn::Path>()?);
            } else if is_kw("database") && input.peek2(Token![=]) {
                input.parse::<Ident>()?; // database
                input.parse::<Token![=]>()?;
                let kind: Ident = input.parse()?; // database = postgres
                let k = kind.to_string();
                if k != "postgres" {
                    return Err(syn::Error::new(kind.span(), format!("platform! database 第一版只支持 `postgres`,收到 `{k}`")));
                }
                database = Some(k);
            } else {
                // 顶层平铺的路由项(layout= / fallback= / group(){} / 页路径):向后兼容(小应用不必包 routes{})。
                parse_routing_item(input, &mut layout, &mut fallback, &mut items)?;
            }
            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
        }
        Ok(Platform { layout, fallback, items, resolve, subscribe, jobs, crons, database })
    }
}

#[proc_macro]
pub fn platform(input: TokenStream) -> TokenStream {
    let p = syn::parse_macro_input!(input as Platform);
    let fallback = match &p.fallback {
        Some(f) => quote! { #f },
        None => {
            return syn::Error::new(Span::call_site(), "platform! 需要 `fallback = <返回 View 的 fn>`")
                .to_compile_error()
                .into()
        }
    };
    let route_table = emit_route_table(&p.layout, &fallback, &p.items);
    // resolve 缺省 = empty_resolver(无数据层的最小骨架);subscribe 可选。
    let resolve_tok = match &p.resolve {
        Some(r) => quote! { #r },
        None => quote! { rui::empty_resolver },
    };
    let sse_tok = match &p.subscribe {
        Some((snap, feed)) => {
            quote! { ::std::option::Option::Some(rui::Sse { snapshot: #snap, subscribe: #feed }) }
        }
        None => quote! { ::std::option::Option::None },
    };
    // jobs + crons 都是 Job(cron 的 payload 固定 CronTick)→ 合并进 run_job 分发器(按 NAME 解码 payload → 调 run)。
    let dispatch_paths: Vec<&syn::Path> = p.jobs.iter().chain(p.crons.iter()).collect();
    let run_job_fn = if dispatch_paths.is_empty() {
        quote! {}
    } else {
        let dp = &dispatch_paths;
        quote! {
            #[cfg(not(target_arch = "wasm32"))]
            pub fn run_job(__name: &str, __ctx: &rui::JobCtx, __json: &str) -> ::std::option::Option<rui::JobResult> {
                #(
                    if __name == <#dp as rui::Job>::NAME {
                        let __p = <<#dp as rui::Job>::Payload as rui::gql::FromValue>::from_value(&rui::gql::parse(__json));
                        return ::std::option::Option::Some(<#dp as rui::Job>::run(__ctx, __p));
                    }
                )*
                ::std::option::Option::None
            }
        }
    };
    let register_jobs = if dispatch_paths.is_empty() {
        quote! {}
    } else {
        quote! { rui::set_job_dispatch(run_job); }
    };
    // crons:登记 (NAME, 间隔秒) 给 scheduler(serve 启动 scheduler 线程按间隔 enqueue → worker 执行)。
    let cron_paths = &p.crons;
    let register_crons = if p.crons.is_empty() {
        quote! {}
    } else {
        quote! {
            rui::set_crons(::std::vec![ #( (<#cron_paths as rui::Job>::NAME, <#cron_paths as rui::CronJob>::INTERVAL_SECS) ),* ]);
        }
    };
    // describe():部署模型(Pillar 3)。运行时读各原语的编译期常量(路由模式/策略、job NAME、cron 间隔、database)
    // 拼出 AppModel,供 `rui plan`(= `cargo run -- plan`)打印部署 DAG + provision plan。
    let mut route_nodes: Vec<TS2> = Vec::new();
    for item in &p.items {
        match item {
            RouterItem::Page(path) => route_nodes.push(quote! {
                rui::deploy::RouteNode { pattern: #path::__RUI_PATTERN.to_string(), strategy: #path::__RUI_STRATEGY }
            }),
            RouterItem::Group { prefix, pages, .. } => {
                for m in pages {
                    route_nodes.push(quote! {
                        rui::deploy::RouteNode {
                            pattern: ::std::format!("{}{}", #prefix, #m::__RUI_PATTERN),
                            strategy: #m::__RUI_STRATEGY,
                        }
                    });
                }
            }
        }
    }
    let graphql_lit = p.resolve.is_some();
    let sse_lit = p.subscribe.is_some();
    let db_expr = match &p.database {
        Some(k) => quote! { ::std::option::Option::Some(#k.to_string()) },
        None => quote! { ::std::option::Option::None },
    };
    let job_names: Vec<TS2> = p.jobs.iter().map(|j| quote! { <#j as rui::Job>::NAME.to_string() }).collect();
    let cron_nodes: Vec<TS2> = p
        .crons
        .iter()
        .map(|c| quote! { rui::deploy::CronNode { name: <#c as rui::Job>::NAME.to_string(), interval_secs: <#c as rui::CronJob>::INTERVAL_SECS } })
        .collect();
    let describe_fn = quote! {
        #[cfg(not(target_arch = "wasm32"))]
        pub fn describe() -> rui::AppModel {
            rui::AppModel {
                routes: ::std::vec![ #(#route_nodes),* ],
                graphql: #graphql_lit,
                sse: #sse_lit,
                database: #db_expr,
                jobs: ::std::vec![ #(#job_names),* ],
                crons: ::std::vec![ #(#cron_nodes),* ],
            }
        }
    };
    quote! {
        #route_table
        #run_job_fn
        #describe_fn
        // 统一装配:一处声明路由 + 数据 + 订阅 + 后台任务 + 定时任务 → AppRuntime(取代 bin 里手搓 `App { .. }`)。仅服务端。
        #[cfg(not(target_arch = "wasm32"))]
        pub fn app() -> rui::AppRuntime {
            #register_jobs
            #register_crons
            rui::AppRuntime { route, resolve: #resolve_tok, sse: #sse_tok }
        }
    }
    .into()
}

// ═══════════════════════════ GraphQL data 层 ═══════════════════════════

// ───────────────────────── 后端 schema:gql_schema! ─────────────────────────
// 把根注册成「对象类型」:为每个根字段生成 `impl Field<gqlf::字段> for QueryRoot { type Ty = 返回类型; }`,
// 于是 query! 的第一层投影与嵌套层走完全相同的 Field 机制(根也只是个对象)。
struct SField {
    name: Ident,
    is_list: bool,
    ty: Ident,
}
struct Section {
    root: Ident, // QueryRoot / MutationRoot / SubscriptionRoot
    fields: Vec<SField>,
}
struct SchemaDef {
    sections: Vec<Section>,
}
impl Parse for SchemaDef {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut sections = Vec::new();
        while !input.is_empty() {
            let kind: Ident = input.parse()?; // Query | Mutation | Subscription
            let root = Ident::new(&format!("{}Root", kind), kind.span());
            let content;
            syn::braced!(content in input);
            let mut fields = Vec::new();
            while !content.is_empty() {
                let name: Ident = content.parse()?;
                if content.peek(syn::token::Paren) {
                    let g;
                    syn::parenthesized!(g in content);
                    let _: TS2 = g.parse()?; // 参数签名忽略(执行器在运行时解析实参)
                }
                content.parse::<Token![:]>()?;
                let (is_list, ty) = if content.peek(syn::token::Bracket) {
                    let b;
                    syn::bracketed!(b in content);
                    (true, b.parse()?)
                } else {
                    (false, content.parse()?)
                };
                fields.push(SField { name, is_list, ty });
                if content.peek(Token![,]) {
                    content.parse::<Token![,]>()?;
                }
            }
            sections.push(Section { root, fields });
        }
        Ok(SchemaDef { sections })
    }
}

#[proc_macro]
pub fn gql_schema(input: TokenStream) -> TokenStream {
    let s = syn::parse_macro_input!(input as SchemaDef);
    let mut out = TS2::new();
    for sec in &s.sections {
        let root = &sec.root;
        let impls = sec.fields.iter().map(|f| {
            let m = &f.name;
            let ty = &f.ty;
            let tyq = if f.is_list {
                quote! { Vec<crate::__rui_registry::model::#ty> }
            } else {
                quote! { crate::__rui_registry::model::#ty }
            };
            quote! { impl rui::gql::Field<crate::__rui_registry::fields::#m> for #root { type Ty = #tyq; } }
        });
        out.extend(quote! {
            #[allow(non_camel_case_types, dead_code)]
            pub struct #root;
            #(#impls)*
        });
    }
    out.into()
}

// ───────────────────── selection 解析(query/mutation/subscription 共用)─────────────────────
enum ArgVal {
    Lit(String),    // 编译期渲染好的字面量片段(字符串带引号转义 / 数字 / 布尔)
    Var(syn::Expr), // 运行时用 ToGqlArg 按类型格式化的 Rust 表达式
}
struct Sel {
    alias: Option<Ident>,        // `别名: 真名` 里的别名(结果 struct 字段名 + 响应 key)
    name: Ident,                 // 真字段名(用于 Field<gqlf::name> 投影)
    args: Vec<(String, ArgVal)>, // 字段参数(支持嵌套,connection 必需)
    children: Vec<Sel>,
    spread: Option<Ident>,       // Some(片段名) = `...FragName` 片段 spread
}
// 解析 `(name: val, ...)`:LitStr/数字/布尔 → 编译期字面量;其余 → 运行时变量表达式。
fn parse_args(input: ParseStream) -> syn::Result<Vec<(String, ArgVal)>> {
    let content;
    syn::parenthesized!(content in input);
    let mut out = Vec::new();
    while !content.is_empty() {
        let n: Ident = content.parse()?;
        content.parse::<Token![:]>()?;
        let v = if content.peek(LitStr) {
            ArgVal::Lit(format!("\"{}\"", gql_esc(&content.parse::<LitStr>()?.value())))
        } else if content.peek(syn::LitInt) {
            ArgVal::Lit(content.parse::<syn::LitInt>()?.base10_digits().to_string())
        } else if content.peek(syn::LitFloat) {
            ArgVal::Lit(content.parse::<syn::LitFloat>()?.base10_digits().to_string())
        } else if content.peek(syn::LitBool) {
            ArgVal::Lit(content.parse::<syn::LitBool>()?.value.to_string())
        } else {
            ArgVal::Var(content.parse()?)
        };
        out.push((n.to_string(), v));
        if content.peek(Token![,]) {
            content.parse::<Token![,]>()?;
        }
    }
    Ok(out)
}
fn parse_sel(input: ParseStream) -> syn::Result<Sel> {
    // 片段 spread:`...FragName`
    if input.peek(Token![...]) {
        input.parse::<Token![...]>()?;
        let frag: Ident = input.parse()?;
        return Ok(Sel {
            alias: None,
            name: frag.clone(),
            args: Vec::new(),
            children: Vec::new(),
            spread: Some(frag),
        });
    }
    let first: Ident = input.parse()?;
    // `别名: 真名` —— 冒号后跟 ident 才是别名
    let (alias, name) = if input.peek(Token![:]) {
        input.parse::<Token![:]>()?;
        (Some(first), input.parse::<Ident>()?)
    } else {
        (None, first)
    };
    let args = if input.peek(syn::token::Paren) {
        parse_args(input)?
    } else {
        Vec::new()
    };
    let children = if input.peek(syn::token::Brace) {
        parse_selection_set(input)?
    } else {
        Vec::new()
    };
    Ok(Sel { alias, name, args, children, spread: None })
}
fn parse_selection_set(input: ParseStream) -> syn::Result<Vec<Sel>> {
    let c;
    syn::braced!(c in input);
    let mut v = Vec::new();
    while !c.is_empty() {
        v.push(parse_sel(&c)?);
    }
    Ok(v)
}
// 转义 GraphQL 字符串字面量里的 " 和 \(编译期,用于字面量参数)。
fn gql_esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

// 字段头:`别名: 真名` 或 `真名`。
fn sel_head(s: &Sel) -> String {
    match &s.alias {
        Some(a) => format!("{}: {}", a, s.name),
        None => s.name.to_string(),
    }
}
// 仅 Lit 参数渲染成编译期片段(Var 编译期无值;mutation selection 用不到嵌套变量参数 → 跳过)。
fn sel_args_lit(args: &[(String, ArgVal)]) -> String {
    let parts: Vec<String> = args
        .iter()
        .filter_map(|(n, v)| match v {
            ArgVal::Lit(s) => Some(format!("{}: {}", n, s)),
            ArgVal::Var(_) => None,
        })
        .collect();
    if parts.is_empty() {
        String::new()
    } else {
        format!("({})", parts.join(", "))
    }
}
// 编译期 selection 字符串(mutation! 用;支持别名 + 字面量参数;片段 spread 不在此支持,跳过)。
fn sel_to_string(sels: &[Sel]) -> String {
    let body = sels
        .iter()
        .filter(|s| s.spread.is_none())
        .map(|s| {
            let head = format!("{}{}", sel_head(s), sel_args_lit(&s.args));
            if s.children.is_empty() {
                head
            } else {
                format!("{} {{ {} }}", head, sel_to_string(&s.children))
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    // 每个选择集带 __typename(同 emit_selection;mutation! / fragment! 的串也要让 store 能规范化)。
    format!("__typename {body}")
}
// 运行时把一组参数 push 进 __q(query!/subscription! 用,支持变量)。
fn emit_args(args: &[(String, ArgVal)]) -> TS2 {
    if args.is_empty() {
        return quote! {};
    }
    let mut stmts = vec![quote! { __q.push_str("("); }];
    for (i, (n, v)) in args.iter().enumerate() {
        if i > 0 {
            stmts.push(quote! { __q.push_str(", "); });
        }
        let pre = format!("{}: ", n);
        stmts.push(quote! { __q.push_str(#pre); });
        match v {
            ArgVal::Lit(s) => stmts.push(quote! { __q.push_str(#s); }),
            ArgVal::Var(e) => {
                stmts.push(quote! { __q.push_str(&rui::gql::ToGqlArg::to_gql_arg(&(#e))); })
            }
        }
    }
    stmts.push(quote! { __q.push_str(")"); });
    quote! { #(#stmts)* }
}
// 运行时递归把 selection push 进 __q(支持别名 + 嵌套参数 + 变量)。
fn emit_selection(sels: &[Sel]) -> TS2 {
    // 每个对象选择集带 __typename:async-graphql 只返回被选字段,store 规范化要靠它认 entity 类型
    //(旧 exec 引擎是注入,无害)。只进 query 串,不进 typed 结果结构(gen_sel_struct 不含它)。
    let mut stmts = vec![quote! { __q.push_str("__typename "); }];
    for s in sels {
        // 片段 spread:把片段的 selection 字符串内联进查询(运行时取 Fragment::SELECTION)。
        if let Some(frag) = &s.spread {
            stmts.push(quote! { __q.push_str(<#frag as rui::gql::Fragment>::SELECTION); });
            stmts.push(quote! { __q.push_str(" "); });
            continue;
        }
        let head = sel_head(s);
        stmts.push(quote! { __q.push_str(#head); });
        stmts.push(emit_args(&s.args));
        if s.children.is_empty() {
            stmts.push(quote! { __q.push_str(" "); });
        } else {
            stmts.push(quote! { __q.push_str(" { "); });
            stmts.push(emit_selection(&s.children));
            stmts.push(quote! { __q.push_str("} "); });
        }
    }
    quote! { #(#stmts)* }
}

// 为一层 selection 合成 exact-fit struct(递归):标量字段走 Scalar,对象字段萃取元素类型
// 递归生成子 struct 并用 Reshape 包回容器形状。所有 struct 定义收集到 `structs`,放块开头。
fn gen_sel_struct(
    elem_ty: TS2,
    sels: &[Sel],
    counter: &mut usize,
    structs: &mut Vec<TS2>,
    prefix: &str,
) -> Ident {
    let my = *counter;
    *counter += 1;
    let sname = Ident::new(&format!("{}{}", prefix, my), Span::call_site());
    let mut defs = Vec::new();
    let mut parses = Vec::new();
    for s in sels {
        // 片段 spread:字段 = 片段名 snake;类型 = 片段命名 struct;读同一对象(片段字段内联在父对象)。
        if let Some(frag) = &s.spread {
            // 字段名 = 片段原名(PascalCase,几乎不会与 snake_case 真字段撞);读整个对象(片段字段内联在父对象)。
            defs.push(quote! { pub #frag: #frag, });
            parses.push(quote! { #frag: <#frag as rui::gql::FromValue>::from_value(v), });
            continue;
        }
        let real = &s.name; // 真名:做 Field<gqlf::real> 投影
        let field = s.alias.as_ref().unwrap_or(&s.name); // struct 字段名 = 别名或真名
        let key = field.to_string(); // 响应里的 key(服务端按别名返回)
        if s.children.is_empty() {
            defs.push(quote! {
                pub #field: <<#elem_ty as rui::gql::Field<crate::__rui_registry::fields::#real>>::Ty as rui::gql::Scalar>::Out,
            });
        } else {
            let orig = quote! { <#elem_ty as rui::gql::Field<crate::__rui_registry::fields::#real>>::Ty };
            let child_elem = quote! { <#orig as rui::gql::GqlElem>::Elem };
            let child = gen_sel_struct(child_elem, &s.children, counter, structs, prefix);
            defs.push(quote! { pub #field: <#orig as rui::gql::Reshape<#child>>::Out, });
        }
        parses.push(quote! { #field: rui::gql::FromValue::from_value(v.field(#key)), });
    }
    structs.push(quote! {
        #[derive(Clone, PartialEq)]
        #[allow(non_snake_case)]
        pub struct #sname { #(#defs)* }
        impl rui::gql::FromValue for #sname {
            fn from_value(v: &rui::gql::Value) -> Self {
                #sname { #(#parses)* }
            }
        }
    });
    sname
}

// mutation! 的编译期字段校验(递归全层):每个标量字段必须存在(Field 投影可解析),
// 每个对象字段必须是对象(GqlElem 可解析)且其子字段同样递归校验。与 query! 同等严格。
fn mutation_checks(elem: TS2, sels: &[Sel], out: &mut Vec<TS2>) {
    for s in sels {
        if s.spread.is_some() {
            continue; // mutation! 不支持片段 spread
        }
        let f = &s.name;
        if s.children.is_empty() {
            out.push(quote! {
                let _ = ::core::marker::PhantomData::<<#elem as rui::gql::Field<crate::__rui_registry::fields::#f>>::Ty>;
            });
        } else {
            let orig = quote! { <#elem as rui::gql::Field<crate::__rui_registry::fields::#f>>::Ty };
            let child = quote! { <#orig as rui::gql::GqlElem>::Elem };
            out.push(quote! { let _ = ::core::marker::PhantomData::<#child>; });
            mutation_checks(child, &s.children, out);
        }
    }
}

#[derive(PartialEq, Clone, Copy)]
enum Fetch {
    Query,    // 取一次
    Sub,      // 开 SSE 持续收
    Resource, // 反应式:参数里的 signal 变就重取(+ loading 状态)
}

// query! / subscription! / resource! 共用:生成 exact-fit struct + Signal<Vec<Row>> + transport。
fn expand_fetch(root: &Ident, args: &[(String, ArgVal)], sel: &[Sel], kind: Fetch) -> TS2 {
    let is_sub = matches!(kind, Fetch::Sub);
    let root_root = Ident::new(
        if is_sub { "SubscriptionRoot" } else { "QueryRoot" },
        Span::call_site(),
    );
    let mut counter = 0usize;
    let mut structs = Vec::new();
    let elem0 = quote! {
        <<crate::__rui_registry::schema::#root_root as rui::gql::Field<crate::__rui_registry::fields::#root>>::Ty as rui::gql::GqlElem>::Elem
    };
    let row = gen_sel_struct(elem0, sel, &mut counter, &mut structs, "__Row");
    let roots = root.to_string();
    let prefix = if is_sub { "subscription { " } else { "{ " };
    // 运行时构造查询串:根字段 + 根参数(支持变量)+ selection(支持别名/嵌套参数/变量)。
    let root_args = emit_args(args);
    let sel_emit = emit_selection(sel);
    let qexpr: TS2 = quote! {{
        let mut __q = ::std::string::String::from(#prefix);
        __q.push_str(#roots);
        #root_args
        __q.push_str(" { ");
        #sel_emit
        __q.push_str("} }");
        __q
    }};
    // 视图:memo 从 store 按命中的 keys 重建 + 订阅相关 entity。
    let rows_memo: TS2 = quote! {
        rui::reactive::memo(move || {
            __keys
                .get()
                .iter()
                .filter_map(|k| rui::gql::store::read_entity(k))
                .map(|__e| <#row as rui::gql::FromValue>::from_value(&__e))
                .collect::<Vec<#row>>()
        })
    };

    if matches!(kind, Fetch::Resource) {
        // 反应式查询:把 fetch 包进 effect。#qexpr 评估参数里的 Var(读 signal)→ 订阅;
        // 任一参数 signal 变 → effect 重跑 → 重建查询串 + 重新请求。返回 (rows, loading, error)。
        quote! {{
            #(#structs)*
            let __keys: rui::reactive::Signal<Vec<String>> = rui::reactive::Signal::new(Vec::new());
            let __loading: rui::reactive::Signal<bool> = rui::reactive::Signal::new(false);
            let __error: rui::reactive::Signal<Option<String>> = rui::reactive::Signal::new(None);
            let __k = __keys.clone();
            let __l = __loading.clone();
            let __e = __error.clone();
            let __h = rui::dom::on_fetch_handler(move |__t: &str| {
                let __v = rui::gql::parse(__t);
                // 失败:errors[](或 HTTP / 网络错误注入的 errors)→ 设 error 态、不 merge 垃圾(保留上次结果)。
                // 先设 error 再清 loading:view 的 error 分支优先,避免中间态短暂渲染陈旧 rows。
                if let Some(__msg) = rui::gql::errors_message(&__v) {
                    __e.set(Some(__msg));
                    __l.set(false);
                    return;
                }
                __e.set(None); // 成功:清错误
                let __payload = match __v.get("data").and_then(|d| d.get(#roots)) {
                    Some(p) => p.clone(),
                    None => __v.clone(),
                };
                let __mk = rui::gql::store::merge_all(&__payload);
                __k.set(__mk.clone());
                rui::gql::store::bump_all(&__mk);
                __l.set(false);
            });
            rui::reactive::on_cleanup(move || rui::dom::drop_fetch_handler(__h)); // 叶子/页销毁时回收
            {
                let __l = __loading.clone();
                rui::reactive::effect(move || {
                    __l.set(true);
                    rui::dom::gql(#qexpr, __h); // 重跑即重取(__h 复用)
                });
            }
            let __rows = #rows_memo;
            (__rows, __loading, __error)
        }}
    } else {
        let transport = if is_sub {
            quote! { rui::dom::subscribe }
        } else {
            quote! { rui::dom::gql }
        };
        quote! {{
            #(#structs)*
            // 响应 normalize 进 store,记录本查询命中的 entity keys;视图是 memo,从 store 重建并订阅。
            let __keys: rui::reactive::Signal<Vec<String>> = rui::reactive::Signal::new(Vec::new());
            let __k = __keys.clone();
            let __h = rui::dom::on_fetch_handler(move |__t: &str| {
                let __v = rui::gql::parse(__t);
                // 失败:跳过 merge(不把 errors 对象当数据写进 store 污染缓存),保留上次结果。
                if rui::gql::errors_message(&__v).is_some() {
                    return;
                }
                let __payload = match __v.get("data").and_then(|d| d.get(#roots)) {
                    Some(p) => p.clone(),
                    None => __v.clone(),
                };
                let __mk = rui::gql::store::merge_all(&__payload);
                __k.set(__mk.clone());
                rui::gql::store::bump_all(&__mk);
            });
            rui::reactive::on_cleanup(move || rui::dom::drop_fetch_handler(__h)); // 叶子/页销毁时回收(防泄漏 + 幽灵写)
            #transport(#qexpr, __h);
            #rows_memo
        }}
    }
}

// ───────────────────────── query! / subscription! ─────────────────────────
struct CQuery {
    root: Ident,
    args: Vec<(String, ArgVal)>,
    sel: Vec<Sel>,
}
impl Parse for CQuery {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let root: Ident = input.parse()?;
        let args = if input.peek(syn::token::Paren) {
            parse_args(input)?
        } else {
            Vec::new()
        };
        let sel = parse_selection_set(input)?;
        Ok(CQuery { root, args, sel })
    }
}

#[proc_macro]
pub fn query(input: TokenStream) -> TokenStream {
    let q = syn::parse_macro_input!(input as CQuery);
    expand_fetch(&q.root, &q.args, &q.sel, Fetch::Query).into()
}

#[proc_macro]
pub fn subscription(input: TokenStream) -> TokenStream {
    let q = syn::parse_macro_input!(input as CQuery);
    expand_fetch(&q.root, &q.args, &q.sel, Fetch::Sub).into()
}

/// 反应式查询:参数里读到的 signal 变化时自动重取(搜索 / 路由参数 / 过滤等)。
/// 返回 `(rows: Signal<Vec<Row>>, loading: Signal<bool>, error: Signal<Option<String>>)`。
/// 失败(GraphQL errors[] 或网络错误)→ error 置 Some(消息) 且保留上次 rows;成功 → error 清空。
#[proc_macro]
pub fn resource(input: TokenStream) -> TokenStream {
    let q = syn::parse_macro_input!(input as CQuery);
    expand_fetch(&q.root, &q.args, &q.sel, Fetch::Resource).into()
}

// ───────────────────────── mutation! ─────────────────────────
struct CMutation {
    target: Ident,
    root: Ident,
    args: Vec<(String, ArgVal)>, // 与 query! 一致:字面量 Lit / 运行时变量 Var
    sel: Vec<Sel>,
    optimistic: Option<syn::Expr>, // 可选乐观更新:预测实体(IntoValue)
    on_error: Option<syn::Expr>,   // 可选失败回调:Fn(String)(GraphQL errors / 网络错误)
}
impl Parse for CMutation {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let target: Ident = input.parse()?;
        input.parse::<Token![,]>()?;
        let root: Ident = input.parse()?;
        // 参数复用 query! 那套:字面量 → Lit(编译期),运行时表达式 → Var(经 ToGqlArg 转义)。
        let args = if input.peek(syn::token::Paren) {
            parse_args(input)?
        } else {
            Vec::new()
        };
        let sel = parse_selection_set(input)?;
        // 可选尾选项(顺序无关):`, optimistic: <expr>`(IntoValue) / `, on_error: <Fn(String)>`。
        let mut optimistic = None;
        let mut on_error = None;
        while input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
            let kw: Ident = input.parse()?;
            input.parse::<Token![:]>()?;
            match kw.to_string().as_str() {
                "optimistic" => optimistic = Some(input.parse::<syn::Expr>()?),
                "on_error" => on_error = Some(input.parse::<syn::Expr>()?),
                _ => {
                    return Err(syn::Error::new(kw.span(), "mutation! 末尾只支持 optimistic: / on_error:"))
                }
            }
        }
        Ok(CMutation { target, root, args, sel, optimistic, on_error })
    }
}

#[proc_macro]
pub fn mutation(input: TokenStream) -> TokenStream {
    let m = syn::parse_macro_input!(input as CMutation);
    let _ = &m.target; // target 现仅为语法保留(规范化缓存自动更新所有引用同一 entity 的视图),不再被闭包捕获
    let root = &m.root;
    let roots = root.to_string();
    let selstr = sel_to_string(&m.sel);
    let root_args = emit_args(&m.args); // 运行时拼参数(支持变量,经 ToGqlArg 转义)
    // 编译期字段校验:每个所选标量字段必须存在于 mutation 根返回的元素类型上。
    let elem = quote! {
        <<crate::__rui_registry::schema::MutationRoot as rui::gql::Field<crate::__rui_registry::fields::#root>>::Ty as rui::gql::GqlElem>::Elem
    };
    let mut checks = Vec::new();
    mutation_checks(elem, &m.sel, &mut checks);

    // 乐观更新:发请求前先把预测实体 merge 进 store(立即更新视图)+ 快照;
    // 响应回来先 restore 撤销乐观,再写真值(真值为空 = 失败 → 停留在回滚态)。
    let opt_setup = match &m.optimistic {
        Some(expr) => quote! {
            let __opt = rui::gql::IntoValue::into_value(&(#expr));
            let __snap = rui::gql::store::snapshot(&rui::gql::store::keys_of(&__opt));
            rui::gql::store::normalize_list(&__opt);
        },
        None => quote! {},
    };
    let opt_rollback = match &m.optimistic {
        Some(_) => quote! { rui::gql::store::restore(&__snap); },
        None => quote! {},
    };
    // 失败回调:用 Rc 在 mutation 构造时一次性持有(#expr 只求值一次)。每次调用克隆 Rc 进 handler,
    // 故外层闭包永远是 Fn —— 无论用户传闭包字面量还是已捕获的变量(原来 `let x=#expr` 会 move 出捕获变量,破坏 Fn)。
    let on_error_rc = match &m.on_error {
        Some(expr) => quote! {
            let __on_err: ::std::rc::Rc<dyn ::core::ops::Fn(::std::string::String)> = ::std::rc::Rc::new(#expr);
        },
        None => quote! {},
    };
    let on_error_clone = match &m.on_error {
        Some(_) => quote! { let __on_err = ::std::rc::Rc::clone(&__on_err); },
        None => quote! {},
    };
    let on_error_body = match &m.on_error {
        Some(_) => quote! { (&*__on_err)(__msg); },
        None => quote! { let _ = __msg; },
    };

    quote! {{
        const _: fn() = || { #(#checks)* };
        #on_error_rc
        move || {
            #opt_setup
            #on_error_clone
            let __h = rui::dom::on_fetch_handler(move |__r: &str| {
                let __v = rui::gql::parse(__r);
                #opt_rollback
                // 失败(errors[] / 网络错误)→ 回滚乐观后调 on_error,不把垃圾写进 store。
                if let Some(__msg) = rui::gql::errors_message(&__v) {
                    #on_error_body
                    return;
                }
                let __payload = match __v.get("data").and_then(|d| d.get(#roots)) {
                    Some(p) => p.clone(),
                    None => __v.clone(),
                };
                rui::gql::store::normalize_list(&__payload);
            });
            // 运行时拼 mutation 串:根字段 + 参数(字面量 + 变量)+ selection。
            let mut __q = ::std::string::String::from("mutation { ");
            __q.push_str(#roots);
            #root_args
            __q.push_str(" { ");
            __q.push_str(#selstr);
            __q.push_str(" } }");
            rui::dom::gql(__q, __h);
        }
    }}
    .into()
}

// ───────────────────────── fragment!(可复用片段 + Relay data masking)─────────────────────────
// 用法:fragment!(Name on Type { 字段 });  生成命名 exact-fit 数据结构 Name + Fragment::SELECTION。
// query! 里 `...Name` 把片段字段内联进查询;父结果持有 Name 子 struct(组件只能读片段声明的字段 = masking)。
struct CFragment {
    name: Ident,
    ty: Ident,
    sel: Vec<Sel>,
}
impl Parse for CFragment {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let name: Ident = input.parse()?;
        let on: Ident = input.parse()?;
        if on != "on" {
            return Err(syn::Error::new(on.span(), "fragment! 语法:Name on Type { 字段 }"));
        }
        let ty: Ident = input.parse()?;
        let sel = parse_selection_set(input)?;
        Ok(CFragment { name, ty, sel })
    }
}

#[proc_macro]
pub fn fragment(input: TokenStream) -> TokenStream {
    let f = syn::parse_macro_input!(input as CFragment);
    let name = &f.name;
    let ty = &f.ty;
    let prefix = format!("__{}_", name);
    let mut counter = 0usize;
    let mut structs = Vec::new();
    // 片段在 Type(经 app! registry 的 model,默认 crate::data::model)上做 exact-fit 校验 —— 字段不存在 / 类型错 → cargo build 报错。
    let elem = quote! { crate::__rui_registry::model::#ty };
    let row = gen_sel_struct(elem, &f.sel, &mut counter, &mut structs, &prefix);
    let selection = sel_to_string(&f.sel);
    quote! {
        #(#structs)*
        pub type #name = #row;
        impl rui::gql::Fragment for #name {
            const SELECTION: &'static str = #selection;
        }
    }
    .into()
}

// ───────────────────────── paginated!(Relay connection 游标分页)─────────────────────────
// 用法:let (rows, load_next, has_next, loading) = paginated!(字段(first: 每页数) { node 选择 });(与 query! 一致的 field(args){sel})
// 约定 connection 结构:字段 -> Connection{ edges:[Edge{ node, cursor }], page_info{ has_next_page, end_cursor } }。
// store 背书:edges 的 node 抽成独立 entity(留 ref),load_next 把新页追加进 connection record;
// node 被 mutation 改写 → 分页视图自动重算(完整 Relay 一致性)。
struct CPaginated {
    root: Ident,
    first: String, // 编译期就定好的每页数量字面量
    node_sel: Vec<Sel>,
}
impl Parse for CPaginated {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let root: Ident = input.parse()?;
        // 与 query! 一致:field(first: N) { node 选择 }。first 必须是整数字面量,
        // 否则清晰报错(不静默吞掉变量 / 接受 "5"/3.0 这类非法字面量)。
        let first = if input.peek(syn::token::Paren) {
            let content;
            syn::parenthesized!(content in input);
            let n: Ident = content.parse()?;
            if n != "first" {
                return Err(syn::Error::new(n.span(), "paginated! 参数只支持 first: <整数字面量>"));
            }
            content.parse::<Token![:]>()?;
            let lit: syn::LitInt = content.parse().map_err(|e| {
                syn::Error::new(e.span(), "paginated! 的 first 必须是整数字面量(不支持变量/字符串/浮点)")
            })?;
            lit.base10_digits().to_string()
        } else {
            "10".to_string()
        };
        let node_sel = parse_selection_set(input)?;
        Ok(CPaginated { root, first, node_sel })
    }
}

#[proc_macro]
pub fn paginated(input: TokenStream) -> TokenStream {
    let p = syn::parse_macro_input!(input as CPaginated);
    let root = &p.root;
    let roots = root.to_string();
    let first_str = p.first;
    let conn_key = format!("@conn:{}", roots);

    // 类型导航:QueryRoot.root → Connection;Connection.edges → Edge;Edge.node → node 元素类型。
    let conn_elem = quote! {
        <<crate::__rui_registry::schema::QueryRoot as rui::gql::Field<crate::__rui_registry::fields::#root>>::Ty as rui::gql::GqlElem>::Elem
    };
    let edge_elem = quote! {
        <<#conn_elem as rui::gql::Field<crate::__rui_registry::fields::edges>>::Ty as rui::gql::GqlElem>::Elem
    };
    let node_elem = quote! {
        <<#edge_elem as rui::gql::Field<crate::__rui_registry::fields::node>>::Ty as rui::gql::GqlElem>::Elem
    };

    let mut counter = 0usize;
    let mut structs = Vec::new();
    let node_row = gen_sel_struct(node_elem, &p.node_sel, &mut counter, &mut structs, "__Row");
    let edge_row = Ident::new(&format!("__Edge{}", counter), Span::call_site());
    let node_sel_emit = emit_selection(&p.node_sel);

    quote! {{
        #(#structs)*
        // edge 行:node 走 exact-fit 子 struct,cursor 编译期校验是 Edge 的标量字段。
        #[derive(Clone, PartialEq)]
        struct #edge_row {
            node: #node_row,
            cursor: <<#edge_elem as rui::gql::Field<crate::__rui_registry::fields::cursor>>::Ty as rui::gql::Scalar>::Out,
        }
        impl rui::gql::FromValue for #edge_row {
            fn from_value(v: &rui::gql::Value) -> Self {
                #edge_row {
                    node: <#node_row as rui::gql::FromValue>::from_value(v.field("node")),
                    cursor: rui::gql::FromValue::from_value(v.field("cursor")),
                }
            }
        }

        let __cursor: rui::reactive::Signal<String> = rui::reactive::Signal::new(String::new());
        let __has_next: rui::reactive::Signal<bool> = rui::reactive::Signal::new(false);
        let __loading: rui::reactive::Signal<bool> = rui::reactive::Signal::new(false);

        // 取一页:after 为游标,append=false 替换(首屏/refetch)、true 追加(load_next)。
        let __fetch = {
            let __cursor = __cursor.clone();
            let __has_next = __has_next.clone();
            let __loading = __loading.clone();
            move |__after: String, __append: bool| {
                __loading.set(true);
                let mut __q = ::std::string::String::from("{ ");
                __q.push_str(#roots);
                __q.push_str("(first: ");
                __q.push_str(#first_str);
                __q.push_str(", after: ");
                __q.push_str(&rui::gql::ToGqlArg::to_gql_arg(&__after));
                __q.push_str(") { edges { node { ");
                #node_sel_emit
                __q.push_str("} cursor } page_info { has_next_page end_cursor } } }");

                let __cursor = __cursor.clone();
                let __has_next = __has_next.clone();
                let __loading = __loading.clone();
                let __h = rui::dom::on_fetch_handler(move |__t: &str| {
                    let __v = rui::gql::parse(__t);
                    // root 返回 [Connection],取第一个
                    let __conn = match __v.get("data").and_then(|d| d.get(#roots)) {
                        Some(p) => p.as_list().get(0).cloned().unwrap_or(rui::gql::Value::Null),
                        None => rui::gql::Value::Null,
                    };
                    rui::gql::store::merge_connection(#conn_key, &__conn, __append);
                    let __pi = __conn.field("page_info");
                    __cursor.set(__pi.field("end_cursor").as_str().to_string());
                    __has_next.set(__pi.field("has_next_page").as_bool());
                    __loading.set(false);
                });
                rui::dom::gql(__q, __h);
            }
        };

        __fetch(::std::string::String::new(), false); // 首屏取第一页

        let __load_next = {
            let __cursor = __cursor.clone();
            let __has_next = __has_next.clone();
            let __loading = __loading.clone();
            let __fetch = __fetch.clone();
            move || {
                // 节流:加载中不重复取(防快速双击用同一游标取两次 → 重复 edge)。
                if __has_next.get() && !__loading.get() {
                    __fetch(__cursor.get(), true);
                }
            }
        };

        // 视图:从 store 的 connection record 读累积 edges(订阅 conn + 各 node 版本)。
        let __edges = rui::reactive::memo(move || {
            rui::gql::store::read_connection(#conn_key)
                .field("edges")
                .as_list()
                .iter()
                .map(|__e| <#edge_row as rui::gql::FromValue>::from_value(__e))
                .collect::<Vec<#edge_row>>()
        });

        (__edges, __load_next, __has_next, __loading)
    }}
    .into()
}

// ───────────────────────── gql_fields!:集中声明字段 marker ─────────────────────────
struct FieldList(Vec<Ident>);
impl Parse for FieldList {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let names = Punctuated::<Ident, Token![,]>::parse_terminated(input)?;
        Ok(FieldList(names.into_iter().collect()))
    }
}

// Relay connection 通用字段 marker:`#[derive(Ent)]` 自动生成的 <E>Edge/<E>Connection/<E>PageInfo
// 都依赖这 6 个,且它们跨所有实体共享(每实体各 emit 一份会重复定义)→ 由 gql_fields! 统一注入一次。
// 对用户显式列表去重:旧代码若仍列出它们也不会重复定义;用户自有同名 domain 字段同样复用这一份 marker。
const RELAY_FIELD_MARKERS: [&str; 6] =
    ["edges", "node", "cursor", "page_info", "has_next_page", "end_cursor"];

#[proc_macro]
pub fn gql_fields(input: TokenStream) -> TokenStream {
    let FieldList(names) = syn::parse_macro_input!(input as FieldList);
    // 用户字段:剔除与 Relay 保留 marker 同名的(避免与下方自动注入重复定义)。
    let user = names.into_iter().filter(|n| !RELAY_FIELD_MARKERS.contains(&n.to_string().as_str()));
    let relay = RELAY_FIELD_MARKERS.iter().map(|m| Ident::new(m, Span::call_site()));
    let decls = user.map(|n| quote! { pub struct #n; }).chain(relay.map(|n| quote! { pub struct #n; }));
    quote! {
        #[allow(non_camel_case_types, dead_code)]
        pub mod gqlf { #(#decls)* }
    }
    .into()
}

// ───────────────────────── #[derive(GqlObject)] / #[derive(Ent)] ─────────────────────────
// 共享:解析具名 struct 的字段 + #[gql(id)] 主键标记(GqlObject 与 Ent 复用同一份字段枚举逻辑)。
struct GqlStruct<'a> {
    name: &'a Ident,
    name_str: String,
    idents: Vec<&'a Ident>,
    names: Vec<String>,
    types: Vec<&'a Type>,
    id_field: Option<Ident>,
}
fn parse_gql_struct(input: &DeriveInput) -> syn::Result<GqlStruct<'_>> {
    let name = &input.ident;
    let fields = match &input.data {
        Data::Struct(s) => match &s.fields {
            Fields::Named(n) => &n.named,
            _ => return Err(syn::Error::new_spanned(name, "仅支持具名字段 struct")),
        },
        _ => return Err(syn::Error::new_spanned(name, "仅支持 struct")),
    };
    let mut id_field: Option<Ident> = None;
    for f in fields {
        for attr in &f.attrs {
            if attr.path().is_ident("gql") {
                let _ = attr.parse_nested_meta(|meta| {
                    if meta.path.is_ident("id") {
                        id_field = f.ident.clone();
                    }
                    Ok(())
                });
            }
        }
    }
    let idents: Vec<&Ident> = fields.iter().map(|f| f.ident.as_ref().unwrap()).collect();
    let names: Vec<String> = idents.iter().map(|i| i.to_string()).collect();
    let types: Vec<&Type> = fields.iter().map(|f| &f.ty).collect();
    Ok(GqlStruct { name, name_str: name.to_string(), idents, names, types, id_field })
}

// 同构类型层 + 编解码(GqlObject/GqlElem/Reshape/Field/IntoValue/FromValue);两端可见,wasm 也展开。
fn emit_gql_object(s: &GqlStruct) -> TS2 {
    let (name, name_str) = (s.name, &s.name_str);
    // id 可选:无 #[gql(id)] 的是 value object(Connection/Edge/PageInfo),gql_id=Null → 不规范化为独立 entity。
    let id_tokens = match &s.id_field {
        Some(i) => quote! { rui::gql::IntoValue::into_value(&self.#i) },
        None => quote! { rui::gql::Value::Null },
    };
    let field_impls = s.idents.iter().zip(s.types.iter()).map(|(id, ty)| {
        quote! { impl rui::gql::Field<crate::__rui_registry::fields::#id> for #name { type Ty = #ty; } }
    });
    let field_arms = s.idents.iter().zip(s.names.iter()).map(|(id, nm)| {
        quote! { #nm => Some(rui::gql::IntoValue::into_value(&self.#id)), }
    });
    let into_pairs = s.idents.iter().zip(s.names.iter()).map(|(id, nm)| {
        quote! { (#nm.to_string(), rui::gql::IntoValue::into_value(&self.#id)), }
    });
    let from_assigns = s.idents.iter().zip(s.names.iter()).map(|(id, nm)| {
        quote! { #id: rui::gql::FromValue::from_value(v.field(#nm)), }
    });
    quote! {
        impl rui::gql::GqlObject for #name {
            const TYPENAME: &'static str = #name_str;
            fn gql_id(&self) -> rui::gql::Value {
                #id_tokens
            }
            fn gql_field(&self, name: &str) -> Option<rui::gql::Value> {
                match name {
                    #(#field_arms)*
                    _ => None,
                }
            }
        }
        impl rui::gql::GqlElem for #name { type Elem = #name; }
        impl<S: rui::gql::FromValue> rui::gql::Reshape<S> for #name { type Out = S; }
        #(#field_impls)*
        impl rui::gql::IntoValue for #name {
            fn into_value(&self) -> rui::gql::Value {
                let mut __o = ::std::vec![
                    ("__typename".to_string(), rui::gql::Value::Str(#name_str.to_string())),
                    ("__id".to_string(), rui::gql::GqlObject::gql_id(self)),
                ];
                __o.extend(::std::vec![ #(#into_pairs)* ]);
                rui::gql::Value::Object(__o)
            }
        }
        impl rui::gql::FromValue for #name {
            fn from_value(v: &rui::gql::Value) -> Self {
                #name { #(#from_assigns)* }
            }
        }
    }
}

// 合成一个 value object(无 entity id)的**完整**定义:`pub struct` 本身 + emit_gql_object 的全套 impl
//(GqlObject/GqlElem/Reshape/Field/IntoValue/FromValue)。用于 #[derive(Ent)] 自动生成 Edge/Connection/PageInfo —
// 这些类型用户不写,故定义与 impl 都由宏产出(对比 #[derive(GqlObject)] 只产 impl、struct 是用户手写的)。
fn emit_relay_value_object(name: &Ident, field_idents: &[Ident], field_types: &[Type]) -> TS2 {
    let name_str = name.to_string();
    let idents: Vec<&Ident> = field_idents.iter().collect();
    let names: Vec<String> = field_idents.iter().map(|i| i.to_string()).collect();
    let types: Vec<&Type> = field_types.iter().collect();
    let gs = GqlStruct { name, name_str, idents, names, types, id_field: None };
    let impls = emit_gql_object(&gs);
    quote! {
        #[derive(Clone)]
        #[allow(dead_code)]
        pub struct #name { #(pub #field_idents: #field_types),* }
        #impls
    }
}

#[proc_macro_derive(GqlObject, attributes(gql))]
pub fn derive_gql_object(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as DeriveInput);
    match parse_gql_struct(&input) {
        Ok(s) => emit_gql_object(&s).into(),
        Err(e) => e.to_compile_error().into(),
    }
}

// #[derive(Ent)] = 单一真相源:同一个 struct 同时是 GraphQL 对象类型(emit_gql_object,同构)
// + 表映射(SqlEntity,native)。取代 GqlObject + async-graphql SimpleObject + sqlx FromRow 三重 derive。
//   #[derive(rui::Ent)] #[ent(table = "todos")] struct Todo { #[gql(id)] id: String, text: String, done: bool }
// 默认 GraphQL 字段名 = SQL 列名;主键取 #[gql(id)] 字段。selection→SQL 列投影由 gql::orm 在运行期完成。
// 此外**自动生成** Relay 分页三件套 <E>Edge/<E>PageInfo/<E>Connection + native 切片器 <E>Connection::page —— 见下方。
// 约束:实体须 `#[derive(Clone)]`(生成的 <E>Edge 持有并 clone 实体作 node);主键类型须实现 Display(游标 = 主键.to_string())。
#[proc_macro_derive(Ent, attributes(gql, ent))]
pub fn derive_ent(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as DeriveInput);
    let s = match parse_gql_struct(&input) {
        Ok(s) => s,
        Err(e) => return e.to_compile_error().into(),
    };
    // 表名:#[ent(table = "...")](struct 级,必填)。
    let mut table: Option<String> = None;
    for attr in &input.attrs {
        if attr.path().is_ident("ent") {
            let _ = attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("table") {
                    let v: LitStr = meta.value()?.parse()?;
                    table = Some(v.value());
                }
                Ok(())
            });
        }
    }
    let name = s.name;
    let table = match table {
        Some(t) => t,
        None => {
            return syn::Error::new_spanned(name, "#[derive(Ent)] 需要 #[ent(table = \"表名\")]")
                .to_compile_error()
                .into()
        }
    };
    // 主键:取 #[gql(id)] 字段(同 GqlObject 的 entity id);ORM 列投影恒并入它(store 规范化 __id 需要)。
    let pk = match &s.id_field {
        Some(i) => i.to_string(),
        None => {
            return syn::Error::new_spanned(name, "#[derive(Ent)] 需要一个 #[gql(id)] 字段作主键")
                .to_compile_error()
                .into()
        }
    };
    let cols = &s.names; // 全部字段名 = 列名(默认同名)
    let pk_ident = s.id_field.clone().expect("上文已校验存在 #[gql(id)] 字段"); // 主键字段(分页游标取它)
    let pk_ty: &Type = {
        let p = s.idents.iter().position(|i| **i == pk_ident).expect("主键字段必在字段列表中");
        s.types[p] // 主键类型:分页游标 = 主键.to_string(),编译期断言它实现 Display
    };
    let gqlobj = emit_gql_object(&s);

    // Relay connection 三件套(自动生成,用户不再手写):cursor = 主键值的字符串形式。
    //   <E>Edge { node: <E>, cursor: String }
    //   <E>PageInfo { has_next_page: bool, end_cursor: String }   ← 每实体一份(共享 PageInfo 无法 impl Field<app::fields::..>)
    //   <E>Connection { edges: Vec<<E>Edge>, page_info: <E>PageInfo }
    // 三者依赖的 6 个通用 marker(edges/node/cursor/page_info/has_next_page/end_cursor)由 gql_fields! 自动注入。
    let edge = Ident::new(&format!("{name}Edge"), name.span());
    let page_info = Ident::new(&format!("{name}PageInfo"), name.span());
    let conn = Ident::new(&format!("{name}Connection"), name.span());
    let sp = name.span();
    let id_ = |s: &str| Ident::new(s, sp);
    let edge_obj = emit_relay_value_object(
        &edge,
        &[id_("node"), id_("cursor")],
        &[parse_quote!(#name), parse_quote!(String)],
    );
    let page_info_obj = emit_relay_value_object(
        &page_info,
        &[id_("has_next_page"), id_("end_cursor")],
        &[parse_quote!(bool), parse_quote!(String)],
    );
    let conn_obj = emit_relay_value_object(
        &conn,
        &[id_("edges"), id_("page_info")],
        &[parse_quote!(::std::vec::Vec<#edge>), parse_quote!(#page_info)],
    );

    quote! {
        #gqlobj
        #edge_obj
        #page_info_obj
        #conn_obj
        // 编译期友好诊断:把「实体须 Clone / 主键须 Display」的约束指回实体定义(#name 带用户 span),
        // 而非让错误落在生成的 #[derive(Clone)] 或 page() 里的 .to_string()(指向宏展开代码,难懂)。
        #[cfg(not(target_arch = "wasm32"))]
        const _: fn() = || {
            fn __rui_ent_requires_clone<T: ::core::clone::Clone>() {}
            fn __rui_pk_requires_display<T: ::core::fmt::Display>() {}
            __rui_ent_requires_clone::<#name>();
            __rui_pk_requires_display::<#pk_ty>();
        };
        // 表映射:仅服务端(SqlEntity 在 gql::orm,native-only)。wasm 端只有上面同构的 GraphQL 类型层。
        #[cfg(not(target_arch = "wasm32"))]
        impl rui::gql::orm::SqlEntity for #name {
            const TABLE: &'static str = #table;
            const PK: &'static str = #pk;
            const COLUMNS: &'static [&'static str] = &[ #(#cols),* ];
        }
        // Relay 游标分页(仅服务端):resolver 一行 `#conn::page(first, &after)` 即得一页 connection。
        // 内存切片(Relay keyset 下推是后续阶段):取全列、按主键稳定排序,游标 = 主键值;after 为空取首页。
        #[cfg(not(target_arch = "wasm32"))]
        impl #conn {
            #[allow(dead_code)]
            pub fn page(first: i64, after: &str) -> ::std::vec::Vec<#conn> {
                let __all = rui::gql::orm::fetch_full::<#name>(rui::gql::orm::Q::new().order(#pk));
                let __start = if after.is_empty() {
                    0
                } else {
                    __all.iter()
                        .position(|__t| __t.#pk_ident.to_string() == after)
                        .map(|__i| __i + 1)
                        .unwrap_or(__all.len())
                };
                // first<=0 → __first=0 → 空页 + has_next_page=false(自洽,不返回「空页却称还有下一页」);
                // saturating_add 防 first 极大时 usize 溢出 panic(debug)。
                let __first = first.max(0) as usize;
                let __end = __start.saturating_add(__first).min(__all.len());
                let __slice = &__all[__start..__end];
                let __edges: ::std::vec::Vec<#edge> = __slice
                    .iter()
                    .map(|__t| #edge { node: ::core::clone::Clone::clone(__t), cursor: __t.#pk_ident.to_string() })
                    .collect();
                let __end_cursor = __slice.last().map(|__t| __t.#pk_ident.to_string()).unwrap_or_default();
                ::std::vec![#conn {
                    edges: __edges,
                    page_info: #page_info { has_next_page: __first > 0 && __end < __all.len(), end_cursor: __end_cursor },
                }]
            }
        }
    }
    .into()
}

// ───────────────────────── #[gql_root(query|mutation|subscription)] ─────────────────────────
// 「写方法即 schema」:标注一个根 impl,从方法签名自动生成
//   · 类型层 schema(两端可见):`pub struct QueryRoot;` + 每个方法的 `Field<gqlf::名> = 返回类型`
//     —— query!/mutation!/subscription! 据此做 exact-fit 编译期校验。
//   · resolver(仅服务端):原 impl 方法体 + 按字段名 dispatch 的 `resolve(field, args)`
//     —— 参数从 args 按类型(FromArg)提取,返回值 into_value。
// 于是后端不再手写 gql_schema! 声明,也不再手写 resolve_root —— 方法签名是唯一真相源。
//   #[gql_root(query)] impl Query { fn stocks(&self) -> Vec<Stock> { .. } fn stock(&self, id: String) -> Vec<Stock> { .. } }
#[proc_macro_attribute]
pub fn gql_root(attr: TokenStream, item: TokenStream) -> TokenStream {
    let kind = syn::parse_macro_input!(attr as Ident);
    let root_name = match kind.to_string().as_str() {
        "query" => "QueryRoot",
        "mutation" => "MutationRoot",
        "subscription" => "SubscriptionRoot",
        _ => {
            return syn::Error::new_spanned(&kind, "gql_root 需要 query / mutation / subscription")
                .to_compile_error()
                .into()
        }
    };
    let root = Ident::new(root_name, Span::call_site());
    let imp = syn::parse_macro_input!(item as syn::ItemImpl);
    let self_ty = &imp.self_ty; // 根 struct(应为 unit struct,如 `struct Query;`)

    let mut field_impls = Vec::new();
    let mut arms = Vec::new();
    for it in &imp.items {
        if let syn::ImplItem::Fn(m) = it {
            let mname = &m.sig.ident;
            let mname_s = mname.to_string();
            // 返回类型 → 该根字段的 Field::Ty(原样;query! 的 GqlElem 再处理 Vec<T>→T)。
            let ret = match &m.sig.output {
                syn::ReturnType::Type(_, t) => quote! { #t },
                syn::ReturnType::Default => quote! { () },
            };
            field_impls.push(quote! {
                impl rui::gql::Field<crate::__rui_registry::fields::#mname> for #root { type Ty = #ret; }
            });
            // 参数(跳过 &self)→ 从 args 按类型提取。
            let mut extracts = Vec::new();
            let mut call_args = Vec::new();
            for arg in &m.sig.inputs {
                if let syn::FnArg::Typed(pt) = arg {
                    if let syn::Pat::Ident(pi) = &*pt.pat {
                        let pname = &pi.ident;
                        let pname_s = pname.to_string();
                        let pty = &pt.ty;
                        extracts.push(quote! {
                            let #pname = <#pty as rui::gql::exec::FromArg>::from_arg(args, #pname_s);
                        });
                        call_args.push(quote! { #pname });
                    }
                }
            }
            arms.push(quote! {
                #mname_s => {
                    #(#extracts)*
                    let __r = #self_ty;
                    rui::gql::IntoValue::into_value(&__r.#mname(#(#call_args),*))
                }
            });
        }
    }

    quote! {
        // 类型层 schema:根类型 + 字段返回类型投影(前后端可见,供 query! 编译期校验)。
        #[allow(non_camel_case_types, dead_code)]
        pub struct #root;
        #(#field_impls)*

        // resolver:原 impl + 按字段名 dispatch(仅服务端,方法体读 store)。
        #[cfg(not(target_arch = "wasm32"))]
        #imp
        #[cfg(not(target_arch = "wasm32"))]
        impl #root {
            pub fn resolve(field: &str, args: &rui::gql::exec::Args) -> rui::gql::Value {
                match field {
                    #(#arms)*
                    _ => rui::gql::Value::Null,
                }
            }
        }
    }
    .into()
}

//! rui 的 view! 宏:JSX 式标记在编译期展开成引擎调用。纯 Rust,无 .html/.types/compile.mjs。
//!
//! 语法:
//!   <div class="x" style={expr} on:click={move || ...}> 子节点 </div>   元素 / 静态属性 / 表达式属性 / 事件
//!   <StatCard label="x" value={v} />                                    组件(首字母大写 → 调 crate::view::components::snake)
//!   <For list=rows item=r> <tr>...{ &r.symbol }...</tr> </For>          响应式列表(list 为 signal,变则重建)
//!   "文本" / { expr }(静态) / { move || expr }(响应式文本)
//!
//! 另含 GraphQL data 层宏:gql_schema! / gql_fields! / #[derive(GqlObject)] / query! / mutation! / subscription!
use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TS2};
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{Data, DeriveInput, Expr, Fields, Ident, LitStr, Stmt, Token, Type};

enum Node {
    El { tag: String, attrs: Vec<Attr>, children: Vec<Node> },
    For { list: Ident, item: Ident, key: Option<syn::Block>, children: Vec<Node> },
    // 条件渲染:when 为返回 bool 的闭包;true→children,false→fallback(可选,返回节点的闭包)。
    Show { when: syn::Block, fallback: Option<syn::Block>, children: Vec<Node> },
    // 多分支:命中第一个 when 为真的 <Match>,都不命中则什么也不渲染。
    Switch { arms: Vec<(syn::Block, Vec<Node>)> },
    Text(LitStr),
    Block(syn::Block),
}
enum Attr {
    Static { name: String, value: LitStr },
    Dyn { name: String, block: syn::Block },     // name={expr}
    Event { event: String, handler: syn::Block }, // on:<事件>={...}
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
        let name: Ident = input.parse()?;
        if input.peek(Token![:]) {
            // 前缀语法:on:<事件>={...} 事件处理;bind:<属性>={signal} 双向绑定。
            input.parse::<Token![:]>()?;
            let sub: Ident = input.parse()?;
            input.parse::<Token![=]>()?;
            let block: syn::Block = input.parse()?;
            match name.to_string().as_str() {
                "on" => attrs.push(Attr::Event { event: sub.to_string(), handler: block }),
                "bind" => attrs.push(Attr::Bind { prop: sub.to_string(), expr: block }),
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
        Node::El { tag, attrs, children } => {
            let is_component = tag.chars().next().map(|c| c.is_uppercase()).unwrap_or(false);
            if is_component {
                // 组件:具名 props(按名匹配、类型由编译器校验)+ children 槽。
                // <Card title=.. sub={x}>子节点</Card> → card(CardProps { title:.., sub:x, children:View(..) })
                let f = Ident::new(&to_snake(tag), Span::call_site());
                let props = Ident::new(&format!("{}Props", tag), Span::call_site());
                let mut fields: Vec<TS2> = Vec::new();
                for a in attrs {
                    match a {
                        Attr::Static { name, value } => {
                            let id = Ident::new(name, Span::call_site());
                            fields.push(quote! { #id: #value.to_string() });
                        }
                        Attr::Dyn { name, block } => {
                            let id = Ident::new(name, Span::call_site());
                            let v = unwrap_block(block);
                            fields.push(quote! { #id: #v });
                        }
                        Attr::Event { .. } | Attr::Bind { .. } | Attr::Ref { .. } => {
                            return quote! { compile_error!("组件属性只支持 名=\"值\" 或 名={表达式};事件 / 绑定 / ref 请在组件内部处理") };
                        }
                    }
                }
                if !children.is_empty() {
                    let b = emit_branch(children); // 子节点 → 单个 View,作为 children 槽传入
                    fields.push(quote! { children: rui::View(#b) });
                }
                return quote! {
                    crate::view::components::#f(crate::view::components::#props { #(#fields),* }).node()
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
                Attr::Event { event, handler } => {
                    // on:<事件>:把用户(零参)闭包包成忽略 payload 的 Fn(&str)。
                    let h = unwrap_block(handler);
                    quote! {{ let __h = #h; rui::dom::on(__n, #event, move |_: &str| __h()); }}
                }
                Attr::Ref { handle } => {
                    // 把刚创建 / 认领的元素 id 写进句柄;on_mount 里据此取真实节点。
                    let h = unwrap_block(handle);
                    quote! {{ let __rf = #h; __rf.set(__n); }}
                }
                Attr::Bind { prop, expr } => {
                    if prop != "value" {
                        quote! { compile_error!("目前只支持 bind:value(受控文本输入,signal 须为 Signal<String>)"); }
                    } else {
                        let s = unwrap_block(expr);
                        // 双向:signal→.value(effect 反应式回写)+ input 事件→signal。
                        quote! {{
                            { let __s = (#s).clone(); rui::reactive::effect(move || rui::dom::set_value(__n, &::std::format!("{}", __s.get()))); }
                            { let __s = (#s).clone(); rui::dom::on(__n, "input", move |__v: &str| __s.set(__v.to_string())); }
                        }}
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
    let props_name = Ident::new(&format!("{}Props", to_pascal(&name.to_string())), Span::call_site());
    let mut fields = Vec::new();
    let mut names = Vec::new();
    for arg in &f.sig.inputs {
        if let syn::FnArg::Typed(pt) = arg {
            if let syn::Pat::Ident(pi) = &*pt.pat {
                let id = &pi.ident;
                let ty = &pt.ty;
                fields.push(quote! { pub #id: #ty });
                names.push(id.clone());
            }
        }
    }
    quote! {
        #[allow(non_snake_case)]
        #vis struct #props_name { #(#fields),* }
        #vis fn #name(__props: #props_name) #output {
            let #props_name { #(#names),* } = __props;
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
}
impl Parse for PageAttr {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut strategy = quote! { rui::Strategy::Ssr };
        let mut pattern = String::new();
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
                    other => {
                        return Err(syn::Error::new(
                            id.span(),
                            format!("#[rui::page] 只支持 ssr / csr / static + 可选路由模式串,收到 `{other}`"),
                        ))
                    }
                }
            }
            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
        }
        Ok(PageAttr { strategy, pattern })
    }
}

#[proc_macro_attribute]
pub fn page(attr: TokenStream, item: TokenStream) -> TokenStream {
    let PageAttr { strategy: strat, pattern } = syn::parse_macro_input!(attr as PageAttr);
    let f = syn::parse_macro_input!(item as syn::ItemFn);
    let vis = &f.vis;
    let name = &f.sig.ident;
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
            // key = 页面模块路径:同一页(不同参数)key 相同,不同页 key 不同 → 导航时据此判断重建 or 仅换参数。
            rui::Page::new(module_path!(), #strat, move || {
                #(#bindings)* // 路由参数:模式里的 :name → 对应 signal(reactive,同页换参数即变)
                #body
            })
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
//   ② 组内页若自带 `:param`,索引是绝对路径的(前缀会偏移)→ 当前不支持组内页参数(用查询参数或顶层路由)。
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
    // 每个 item → (匹配条件, 命中后的 Page 表达式)。
    let mut entries: Vec<(TS2, TS2)> = Vec::new();
    for item in &r.items {
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
                let lp_tok = quote! { (&__lp) };
                let mut leaf = quote! { rui::View(rui::dom::text("")) };
                for m in pages.iter().rev() {
                    let lc = cond_for(&lp_tok, m);
                    leaf = quote! { if #lc { (#m::view().render)() } else { #leaf } };
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
    let mut chain = quote! { rui::Page::new("not_found", rui::Strategy::Ssr, #fallback) };
    for (cond, page) in entries.iter().rev() {
        chain = quote! { if #cond { #page } else { #chain } };
    }
    // 全局 layout 包裹(可选):保留命中项的 key/strategy,渲染交给 layout 包外壳。
    let wrap = match &r.layout {
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
                quote! { Vec<crate::data::model::#ty> }
            } else {
                quote! { crate::data::model::#ty }
            };
            quote! { impl rui::gql::Field<crate::gqlf::#m> for #root { type Ty = #tyq; } }
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
    sels.iter()
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
        .join(" ")
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
    let mut stmts = Vec::new();
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
                pub #field: <<#elem_ty as rui::gql::Field<crate::gqlf::#real>>::Ty as rui::gql::Scalar>::Out,
            });
        } else {
            let orig = quote! { <#elem_ty as rui::gql::Field<crate::gqlf::#real>>::Ty };
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
                let _ = ::core::marker::PhantomData::<<#elem as rui::gql::Field<crate::gqlf::#f>>::Ty>;
            });
        } else {
            let orig = quote! { <#elem as rui::gql::Field<crate::gqlf::#f>>::Ty };
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
        <<crate::api::schema::#root_root as rui::gql::Field<crate::gqlf::#root>>::Ty as rui::gql::GqlElem>::Elem
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
        <<crate::api::schema::MutationRoot as rui::gql::Field<crate::gqlf::#root>>::Ty as rui::gql::GqlElem>::Elem
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
    // 片段在 Type(约定在 crate::data::model)上做 exact-fit 校验 —— 字段不存在 / 类型错 → cargo build 报错。
    let elem = quote! { crate::data::model::#ty };
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
        <<crate::api::schema::QueryRoot as rui::gql::Field<crate::gqlf::#root>>::Ty as rui::gql::GqlElem>::Elem
    };
    let edge_elem = quote! {
        <<#conn_elem as rui::gql::Field<crate::gqlf::edges>>::Ty as rui::gql::GqlElem>::Elem
    };
    let node_elem = quote! {
        <<#edge_elem as rui::gql::Field<crate::gqlf::node>>::Ty as rui::gql::GqlElem>::Elem
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
            cursor: <<#edge_elem as rui::gql::Field<crate::gqlf::cursor>>::Ty as rui::gql::Scalar>::Out,
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

#[proc_macro]
pub fn gql_fields(input: TokenStream) -> TokenStream {
    let FieldList(names) = syn::parse_macro_input!(input as FieldList);
    let decls = names.iter().map(|n| quote! { pub struct #n; });
    quote! {
        #[allow(non_camel_case_types, dead_code)]
        pub mod gqlf { #(#decls)* }
    }
    .into()
}

// ───────────────────────── #[derive(GqlObject)] ─────────────────────────
#[proc_macro_derive(GqlObject, attributes(gql))]
pub fn derive_gql_object(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as DeriveInput);
    let name = &input.ident;
    let name_str = name.to_string();

    let fields = match &input.data {
        Data::Struct(s) => match &s.fields {
            Fields::Named(n) => &n.named,
            _ => {
                return syn::Error::new_spanned(name, "GqlObject 仅支持具名字段 struct")
                    .to_compile_error()
                    .into()
            }
        },
        _ => {
            return syn::Error::new_spanned(name, "GqlObject 仅支持 struct")
                .to_compile_error()
                .into()
        }
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
    // id 可选:无 #[gql(id)] 的是 value object(如 Connection/Edge/PageInfo),
    // gql_id 返回 Null → entity_key 为 None → 不被规范化为独立 entity(随父内联)。
    let id_tokens = match &id_field {
        Some(i) => quote! { rui::gql::IntoValue::into_value(&self.#i) },
        None => quote! { rui::gql::Value::Null },
    };

    let idents: Vec<&Ident> = fields.iter().map(|f| f.ident.as_ref().unwrap()).collect();
    let names: Vec<String> = idents.iter().map(|i| i.to_string()).collect();
    let types: Vec<&Type> = fields.iter().map(|f| &f.ty).collect();

    let field_impls = idents.iter().zip(types.iter()).map(|(id, ty)| {
        quote! { impl rui::gql::Field<crate::gqlf::#id> for #name { type Ty = #ty; } }
    });
    let field_arms = idents.iter().zip(names.iter()).map(|(id, nm)| {
        quote! { #nm => Some(rui::gql::IntoValue::into_value(&self.#id)), }
    });
    let into_pairs = idents.iter().zip(names.iter()).map(|(id, nm)| {
        quote! { (#nm.to_string(), rui::gql::IntoValue::into_value(&self.#id)), }
    });
    let from_assigns = idents.iter().zip(names.iter()).map(|(id, nm)| {
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
                impl rui::gql::Field<crate::gqlf::#mname> for #root { type Ty = #ret; }
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

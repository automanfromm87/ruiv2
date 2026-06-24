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
    For { list: Ident, item: Ident, children: Vec<Node> },
    Text(LitStr),
    Block(syn::Block),
}
enum Attr {
    Static { name: String, value: LitStr },
    Dyn { name: String, block: syn::Block }, // name={expr}
    Event { handler: syn::Block },           // on:click={...}
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
        let name: Ident = input.parse()?;
        if input.peek(Token![:]) {
            input.parse::<Token![:]>()?;
            let _ev: Ident = input.parse()?; // 只支持 click
            input.parse::<Token![=]>()?;
            attrs.push(Attr::Event { handler: input.parse()? });
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
    loop {
        if input.peek(Token![>]) {
            input.parse::<Token![>]>()?;
            break;
        }
        let name: Ident = input.parse()?;
        input.parse::<Token![=]>()?;
        let val: Ident = input.parse()?;
        if name == "list" {
            list = Some(val);
        } else if name == "item" {
            item = Some(val);
        }
    }
    let children = parse_children(input)?;
    Ok(Node::For {
        list: list.expect("<For> 需要 list="),
        item: item.expect("<For> 需要 item="),
        children,
    })
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
        if let Node::For { list, item, children } = c {
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
        } else {
            let cg = gen_node(c);
            quote! { let __c = #cg; rui::dom::append(__n, __c); }
        }
    }).collect()
}

fn gen_node(n: &Node) -> TS2 {
    match n {
        Node::Text(s) => quote! { rui::dom::text(#s) },
        Node::Block(b) => {
            if is_reactive(b) {
                quote! {{
                    let __t = rui::dom::text("");
                    let __f = #b;
                    rui::reactive::effect(move || rui::dom::set_text(__t, &format!("{}", __f())));
                    __t
                }}
            } else {
                quote! { rui::dom::text(&format!("{}", #b)) }
            }
        }
        Node::For { .. } => quote! { compile_error!("<For> 只能作为元素的子节点") },
        Node::El { tag, attrs, children } => {
            let is_component = tag.chars().next().map(|c| c.is_uppercase()).unwrap_or(false);
            if is_component {
                let f = Ident::new(&to_snake(tag), Span::call_site());
                let args: Vec<TS2> = attrs.iter().map(|a| match a {
                    Attr::Static { value, .. } => quote! { #value.to_string() },
                    Attr::Dyn { block, .. } => unwrap_block(block),
                    Attr::Event { handler } => unwrap_block(handler),
                }).collect();
                return quote! { crate::view::components::#f(#(#args),*) };
            }
            let astmts: Vec<TS2> = attrs.iter().map(|a| match a {
                Attr::Static { name, value } => quote! { rui::dom::attr(__n, #name, #value); },
                Attr::Dyn { name, block } => { let v = unwrap_block(block); quote! { rui::dom::attr(__n, #name, &(#v)); } }
                Attr::Event { handler } => { let h = unwrap_block(handler); quote! { rui::dom::on_click(__n, #h); } }
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
    gen_node(&v.0).into()
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
        #[derive(Clone)]
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

// query! / subscription! 共用:生成 exact-fit struct + Signal<Vec<Row>> + transport。
fn expand_fetch(root: &Ident, args: &[(String, ArgVal)], sel: &[Sel], is_sub: bool) -> TS2 {
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
            let __payload = match __v.get("data").and_then(|d| d.get(#roots)) {
                Some(p) => p.clone(),
                None => __v.clone(),
            };
            // 先把整批 entity 合并进 store(一致快照),再发布 key 列表(本视图重建),
            // 最后 bump 版本(通知其它引用同一 entity 的视图)。
            let __mk = rui::gql::store::merge_all(&__payload);
            __k.set(__mk.clone());
            rui::gql::store::bump_all(&__mk);
        });
        #transport(#qexpr, __h);
        rui::reactive::memo(move || {
            __keys
                .get()
                .iter()
                .filter_map(|k| rui::gql::store::read_entity(k))
                .map(|__e| <#row as rui::gql::FromValue>::from_value(&__e))
                .collect::<Vec<#row>>()
        })
    }}
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
    expand_fetch(&q.root, &q.args, &q.sel, false).into()
}

#[proc_macro]
pub fn subscription(input: TokenStream) -> TokenStream {
    let q = syn::parse_macro_input!(input as CQuery);
    expand_fetch(&q.root, &q.args, &q.sel, true).into()
}

// ───────────────────────── mutation! ─────────────────────────
struct CMutation {
    target: Ident,
    root: Ident,
    args: Vec<String>,
    sel: Vec<Sel>,
    optimistic: Option<syn::Expr>, // 可选乐观更新:预测实体(IntoValue)
}
impl Parse for CMutation {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let target: Ident = input.parse()?;
        input.parse::<Token![,]>()?;
        let root: Ident = input.parse()?;
        let mut args = Vec::new();
        let content;
        syn::parenthesized!(content in input);
        while !content.is_empty() {
            let n: Ident = content.parse()?;
            content.parse::<Token![:]>()?;
            let lit: syn::Lit = content.parse()?;
            let rendered = match &lit {
                syn::Lit::Str(s) => format!("{}: \"{}\"", n, gql_esc(&s.value())),
                syn::Lit::Int(i) => format!("{}: {}", n, i.base10_digits()),
                syn::Lit::Float(fl) => format!("{}: {}", n, fl.base10_digits()),
                syn::Lit::Bool(b) => format!("{}: {}", n, b.value),
                _ => format!("{}: null", n),
            };
            args.push(rendered);
            if content.peek(Token![,]) {
                content.parse::<Token![,]>()?;
            }
        }
        let sel = parse_selection_set(input)?;
        // 可选 `, optimistic: <expr>`(expr 是任意 IntoValue,如 Vec<Stock>)
        let optimistic = if input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
            let kw: Ident = input.parse()?;
            if kw != "optimistic" {
                return Err(syn::Error::new(kw.span(), "mutation! 末尾只支持 `optimistic: <expr>`"));
            }
            input.parse::<Token![:]>()?;
            Some(input.parse::<syn::Expr>()?)
        } else {
            None
        };
        Ok(CMutation { target, root, args, sel, optimistic })
    }
}

#[proc_macro]
pub fn mutation(input: TokenStream) -> TokenStream {
    let m = syn::parse_macro_input!(input as CMutation);
    let target = &m.target;
    let root = &m.root;
    let roots = root.to_string();
    let selstr = sel_to_string(&m.sel);
    let args_s = m.args.join(", ");
    let mut_str = format!("mutation {{ {}({}) {{ {} }} }}", roots, args_s, selstr);
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

    quote! {{
        const _: fn() = || { #(#checks)* };
        let _ = &#target; // target 仅保留语法兼容;规范化缓存自动更新所有引用同一 entity 的视图
        move || {
            #opt_setup
            let __h = rui::dom::on_fetch_handler(move |__r: &str| {
                let __v = rui::gql::parse(__r);
                let __payload = match __v.get("data").and_then(|d| d.get(#roots)) {
                    Some(p) => p.clone(),
                    None => __v.clone(),
                };
                #opt_rollback
                rui::gql::store::normalize_list(&__payload);
            });
            rui::dom::gql(#mut_str, __h);
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
        #[derive(Clone)]
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

//! 后端 API —— 「写方法即 schema」。方法签名就是 GraphQL schema;方法体是 resolver(仅服务端)。
//! resolver 现在很薄:读走 `rui::gql::orm::fetch`(selection 自动下推成 SQL 列投影),写走 `orm::write`。
//! 具体后端(PG / 内存)由 ssr.rs 经 `set_db_executor` 注入 —— resolver 不再 `match pg{Some/None}`,也无双 schema。
#![allow(dead_code)] // 根 struct 方法体仅服务端编译;wasm 端只用类型层 schema

use crate::data::model::{Todo, TodoConnection};
#[cfg(not(target_arch = "wasm32"))]
use rui::gql::orm::{self, Filter, Q, SetVal, Write};
use rui::gql_root;

pub struct Query;
pub struct Mutation;
pub struct Subscription;

#[gql_root(query)]
impl Query {
    fn todos(&self) -> Vec<Todo> {
        orm::fetch::<Todo>(Q::new().order("id"))
    }
    // Relay 游标分页:归档页用。切片器随 #[derive(Ent)] 自动生成,resolver 一行调用。
    fn todo_page(&self, first: i64, after: String) -> Vec<TodoConnection> {
        TodoConnection::page(first, &after)
    }
    // 服务端按文本过滤(resource! 搜索演示)。
    fn search(&self, q: String) -> Vec<Todo> {
        let q = q.trim();
        if q.is_empty() {
            return Vec::new();
        }
        orm::fetch::<Todo>(Q::new().contains("text", q).order("id"))
    }
    // 单条详情(路由参数页 /todo/:id 用):按 id 查,Vec 0/1 条。
    fn detail(&self, id: String) -> Vec<Todo> {
        orm::fetch::<Todo>(Q::new().eq("id", id))
    }
}

#[gql_root(mutation)]
impl Mutation {
    fn add_todo(&self, text: String) -> Vec<Todo> {
        let text = text.trim();
        if !text.is_empty() {
            // 下一个 id = 当前最大数字 id + 1(后端无关:先读 id 再算);text 是用户输入 → 参数化绑定(防注入)。
            let next = orm::fetch_all::<Todo>()
                .iter()
                .filter_map(|t| t.id.parse::<i64>().ok())
                .max()
                .unwrap_or(0)
                + 1;
            orm::write(Write::Insert {
                table: "todos",
                columns: vec!["id".into(), "text".into(), "done".into()],
                values: vec![next.to_string().into(), text.into(), false.into()],
            });
            crate::api::todos::broadcast();
            // 后台任务:入队一个 AsyncJob(立即返回,worker 异步执行;不阻塞 /graphql 响应)。
            rui::enqueue::<crate::api::jobs::notify_added>(next.to_string());
        }
        orm::fetch::<Todo>(Q::new().order("id"))
    }
    fn toggle_todo(&self, id: String) -> Vec<Todo> {
        orm::write(Write::Update {
            table: "todos",
            set: vec![("done".into(), SetVal::Toggle)],
            filter: Some(Filter::Eq("id".into(), id.into())),
        });
        crate::api::todos::broadcast();
        orm::fetch::<Todo>(Q::new().order("id"))
    }
    fn remove_todo(&self, id: String) -> Vec<Todo> {
        orm::write(Write::Delete {
            table: "todos",
            filter: Some(Filter::Eq("id".into(), id.into())),
        });
        crate::api::todos::broadcast();
        orm::fetch::<Todo>(Q::new().order("id"))
    }
    fn clear_done(&self) -> Vec<Todo> {
        orm::write(Write::Delete {
            table: "todos",
            filter: Some(Filter::Eq("done".into(), true.into())),
        });
        crate::api::todos::broadcast();
        orm::fetch::<Todo>(Q::new().order("id"))
    }
    fn complete_all(&self) -> Vec<Todo> {
        orm::write(Write::Update {
            table: "todos",
            set: vec![("done".into(), SetVal::Lit(true.into()))],
            filter: None,
        });
        crate::api::todos::broadcast();
        orm::fetch::<Todo>(Q::new().order("id"))
    }
}

#[gql_root(subscription)]
impl Subscription {
    fn todo_updates(&self) -> Vec<Todo> {
        orm::fetch::<Todo>(Q::new().order("id"))
    }
}

/// 聚合 resolver:把三个根的 dispatch 合成一个,供 rui::serve / serve_axum 注入(/graphql + 同构 SSR 共用)。
/// 走 rui 自带同步 exec 引擎(execute 会按当前根字段的 selection 设好 current_selection → ORM 据此投影列)。
#[cfg(not(target_arch = "wasm32"))]
pub fn resolve(
    kind: rui::gql::parser::OpKind,
    field: &str,
    args: &rui::gql::exec::Args,
) -> rui::gql::Value {
    use rui::gql::parser::OpKind;
    match kind {
        OpKind::Query => QueryRoot::resolve(field, args),
        OpKind::Mutation => MutationRoot::resolve(field, args),
        OpKind::Subscription => SubscriptionRoot::resolve(field, args),
    }
}

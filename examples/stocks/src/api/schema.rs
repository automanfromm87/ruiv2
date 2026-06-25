//! 后端 API —— 「写方法即 schema」。方法签名就是 GraphQL schema;方法体是 resolver(仅服务端)。
#![allow(dead_code)] // 根 struct 方法体仅服务端编译;wasm 端只用类型层 schema

use crate::data::model::{Todo, TodoConnection};
use rui::gql_root;

pub struct Query;
pub struct Mutation;
pub struct Subscription;

#[gql_root(query)]
impl Query {
    fn todos(&self) -> Vec<Todo> {
        crate::api::todos::all()
    }
    // Relay 游标分页:归档页用。
    fn todo_page(&self, first: i64, after: String) -> Vec<TodoConnection> {
        crate::api::todos::page(first, &after)
    }
    // 服务端按文本过滤(resource! 搜索演示)。
    fn search(&self, q: String) -> Vec<Todo> {
        crate::api::todos::search(&q)
    }
    // 单条详情(路由参数页 /todo/:id 用):按 id 查,Vec 0/1 条。
    fn detail(&self, id: String) -> Vec<Todo> {
        crate::api::todos::detail(&id)
    }
}

#[gql_root(mutation)]
impl Mutation {
    fn add_todo(&self, text: String) -> Vec<Todo> {
        crate::api::todos::add(&text)
    }
    fn toggle_todo(&self, id: String) -> Vec<Todo> {
        crate::api::todos::toggle(&id)
    }
    fn remove_todo(&self, id: String) -> Vec<Todo> {
        crate::api::todos::remove(&id)
    }
    fn clear_done(&self) -> Vec<Todo> {
        crate::api::todos::clear_done()
    }
    fn complete_all(&self) -> Vec<Todo> {
        crate::api::todos::complete_all()
    }
}

#[gql_root(subscription)]
impl Subscription {
    fn todo_updates(&self) -> Vec<Todo> {
        crate::api::todos::all()
    }
}

/// 聚合 resolver:把三个根的 dispatch 合成一个,供 rui::serve 注入(/graphql + 同构 SSR 共用)。
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

// ── async-graphql 服务端引擎(axum host 用;B2 双 schema:与上面 #[gql_root] 并存)──
// 独立的根 struct(不复用 Query/Mutation:#[gql_root] 已在其上留了同名同步方法,会冲突)。
// async fn resolver + 复用同一批 crate::api::todos 数据函数;以后 PG 池经 Context.data 注入即可。
// 订阅走 App.sse 内存广播(不经 GraphQL 执行)→ EmptySubscription。
#[cfg(not(target_arch = "wasm32"))]
pub mod ag {
    use crate::data::model::{Todo, TodoConnection};
    use async_graphql::{EmptySubscription, Object, Schema};

    pub struct AgQuery;
    #[Object(rename_fields = "snake_case", rename_args = "snake_case")]
    impl AgQuery {
        async fn todos(&self) -> Vec<Todo> {
            crate::api::todos::all()
        }
        async fn todo_page(&self, first: i64, after: String) -> Vec<TodoConnection> {
            crate::api::todos::page(first, &after)
        }
        async fn search(&self, q: String) -> Vec<Todo> {
            crate::api::todos::search(&q)
        }
        async fn detail(&self, id: String) -> Vec<Todo> {
            crate::api::todos::detail(&id)
        }
        // 镜像 subscription 字段:订阅的 SSR 初值经 transport 改写成 query 后在此 resolve(当前值)。
        async fn todo_updates(&self) -> Vec<Todo> {
            crate::api::todos::all()
        }
    }

    pub struct AgMutation;
    #[Object(rename_fields = "snake_case", rename_args = "snake_case")]
    impl AgMutation {
        async fn add_todo(&self, text: String) -> Vec<Todo> {
            crate::api::todos::add(&text)
        }
        async fn toggle_todo(&self, id: String) -> Vec<Todo> {
            crate::api::todos::toggle(&id)
        }
        async fn remove_todo(&self, id: String) -> Vec<Todo> {
            crate::api::todos::remove(&id)
        }
        async fn clear_done(&self) -> Vec<Todo> {
            crate::api::todos::clear_done()
        }
        async fn complete_all(&self) -> Vec<Todo> {
            crate::api::todos::complete_all()
        }
    }

    pub type AppSchema = Schema<AgQuery, AgMutation, EmptySubscription>;
    pub fn build_schema() -> AppSchema {
        Schema::build(AgQuery, AgMutation, EmptySubscription).finish()
    }
}

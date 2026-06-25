//! PostgreSQL 数据后端(仅服务端)。DATABASE_URL 存在 → resolver 经 ctx 拿到 PgPool 走这里;否则回退内存 api::todos。
//! resolver 是 async-graphql 的 async fn → 直接 `.await` sqlx 查询(这正是切 async-graphql 的回报)。
//! entity id = todos.id(与 async-graphql SimpleObject / 客户端 store 的 id 对齐)。

use crate::data::model::{PageInfo, Todo, TodoConnection, TodoEdge};
use sqlx::PgPool;
use tokio::sync::OnceCell;

/// PG 后端句柄:持 URL + 懒连接池。连接池**首次 resolver 调用时**才建(那时已在 serve_axum 的 runtime 内,
/// 满足 sqlx「需要 Tokio context」)—— 不能在 main(无 runtime)里 connect_lazy,会 panic。
pub struct Pg {
    url: String,
    pool: OnceCell<PgPool>,
}
impl Pg {
    /// DATABASE_URL 存在 → Some(Pg)(此刻不连接,只存 URL);否则 None → resolver 回退内存。
    pub fn from_env() -> Option<Pg> {
        std::env::var("DATABASE_URL").ok().map(|url| {
            println!("rui · 数据后端:PostgreSQL");
            Pg { url, pool: OnceCell::new() }
        })
    }
    /// 取连接池(首次在当前 runtime 内 connect;失败返回 None → 调用方回退内存)。
    async fn pool(&self) -> Option<&PgPool> {
        self.pool
            .get_or_try_init(|| PgPool::connect(&self.url))
            .await
            .map_err(|e| eprintln!("rui · PG 连接失败:{e}"))
            .ok()
    }
}

// ── 读 ──
pub async fn all(pg: &Pg) -> Vec<Todo> {
    let Some(pool) = pg.pool().await else { return Vec::new() };
    sqlx::query_as::<_, Todo>("select id, text, done from todos order by id::int")
        .fetch_all(pool)
        .await
        .unwrap_or_default()
}
pub async fn detail(pg: &Pg, id: &str) -> Vec<Todo> {
    let Some(pool) = pg.pool().await else { return Vec::new() };
    sqlx::query_as::<_, Todo>("select id, text, done from todos where id = $1")
        .bind(id)
        .fetch_all(pool)
        .await
        .unwrap_or_default()
}
pub async fn search(pg: &Pg, q: &str) -> Vec<Todo> {
    let q = q.trim();
    if q.is_empty() {
        return Vec::new();
    }
    let Some(pool) = pg.pool().await else { return Vec::new() };
    sqlx::query_as::<_, Todo>("select id, text, done from todos where text ilike $1 order by id::int")
        .bind(format!("%{q}%"))
        .fetch_all(pool)
        .await
        .unwrap_or_default()
}
pub async fn page(pg: &Pg, first: i64, after: &str) -> Vec<TodoConnection> {
    // 简化:取全表后在内存切片成 connection(与 api::todos::page 同逻辑;真分页可改 SQL limit/keyset)。
    let rows = all(pg).await;
    let start = if after.is_empty() {
        0
    } else {
        rows.iter().position(|t| t.id == after).map(|i| i + 1).unwrap_or(rows.len())
    };
    let end = (start + first.max(0) as usize).min(rows.len());
    let slice = &rows[start..end];
    let edges = slice.iter().map(|t| TodoEdge { node: t.clone(), cursor: t.id.clone() }).collect();
    let end_cursor = slice.last().map(|t| t.id.clone()).unwrap_or_default();
    vec![TodoConnection { edges, page_info: PageInfo { has_next_page: end < rows.len(), end_cursor } }]
}

// ── 写(都返回写后的全表,与内存版一致)──
pub async fn add(pg: &Pg, text: &str) -> Vec<Todo> {
    let Some(pool) = pg.pool().await else { return Vec::new() };
    let text = text.trim();
    if !text.is_empty() {
        // id = 当前最大数字 id + 1(与内存版的自增语义近似)。
        let _ = sqlx::query("insert into todos (id, text, done) select (coalesce(max(id::int), 0) + 1)::text, $1, false from todos")
            .bind(text)
            .execute(pool)
            .await;
    }
    all(pg).await
}
pub async fn toggle(pg: &Pg, id: &str) -> Vec<Todo> {
    if let Some(pool) = pg.pool().await {
        let _ = sqlx::query("update todos set done = not done where id = $1").bind(id).execute(pool).await;
    }
    all(pg).await
}
pub async fn remove(pg: &Pg, id: &str) -> Vec<Todo> {
    if let Some(pool) = pg.pool().await {
        let _ = sqlx::query("delete from todos where id = $1").bind(id).execute(pool).await;
    }
    all(pg).await
}
pub async fn clear_done(pg: &Pg) -> Vec<Todo> {
    if let Some(pool) = pg.pool().await {
        let _ = sqlx::query("delete from todos where done").execute(pool).await;
    }
    all(pg).await
}
pub async fn complete_all(pg: &Pg) -> Vec<Todo> {
    if let Some(pool) = pg.pool().await {
        let _ = sqlx::query("update todos set done = true").execute(pool).await;
    }
    all(pg).await
}

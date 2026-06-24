//! 待办数据后端(仅服务端):内存 store + 增删改 + SSE 订阅广播 + 游标分页。
//! 被 crate::api::schema 的 #[gql_root] 方法体调用;每次写操作都 broadcast(),订阅者(/live、首页)实时刷新。
use crate::data::model::{PageInfo, Todo, TodoConnection, TodoEdge};
use rui::gql::value::{IntoValue, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Mutex, OnceLock};

fn seed() -> Vec<Todo> {
    [
        ("1", "学习 rui 框架", true),
        ("2", "用它写个 todolist", false),
        ("3", "配合 tailwind 调样式", false),
    ]
    .iter()
    .map(|(id, t, d)| Todo { id: id.to_string(), text: t.to_string(), done: *d })
    .collect()
}

fn store() -> &'static Mutex<Vec<Todo>> {
    static S: OnceLock<Mutex<Vec<Todo>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(seed()))
}
fn subs() -> &'static Mutex<Vec<Sender<String>>> {
    static S: OnceLock<Mutex<Vec<Sender<String>>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(Vec::new()))
}
fn next_id() -> String {
    static N: AtomicU64 = AtomicU64::new(100);
    N.fetch_add(1, Ordering::Relaxed).to_string()
}

// ── 数据访问(schema 的 resolver 方法调用)──

pub fn all() -> Vec<Todo> {
    store().lock().unwrap().clone()
}

pub fn add(text: &str) -> Vec<Todo> {
    let text = text.trim();
    if !text.is_empty() {
        store().lock().unwrap().push(Todo { id: next_id(), text: text.to_string(), done: false });
        broadcast();
    }
    all()
}

pub fn toggle(id: &str) -> Vec<Todo> {
    {
        let mut v = store().lock().unwrap();
        if let Some(t) = v.iter_mut().find(|t| t.id == id) {
            t.done = !t.done;
        }
    }
    broadcast();
    all()
}

pub fn remove(id: &str) -> Vec<Todo> {
    store().lock().unwrap().retain(|t| t.id != id);
    broadcast();
    all()
}

pub fn clear_done() -> Vec<Todo> {
    store().lock().unwrap().retain(|t| !t.done);
    broadcast();
    all()
}

pub fn complete_all() -> Vec<Todo> {
    {
        let mut v = store().lock().unwrap();
        for t in v.iter_mut() {
            t.done = true;
        }
    }
    broadcast();
    all()
}

/// Relay 游标分页(归档页):cursor = id;after 为上一页最后一个 id(空 = 第一页)。
pub fn page(first: i64, after: &str) -> Vec<TodoConnection> {
    let all = store().lock().unwrap().clone();
    let start = if after.is_empty() {
        0
    } else {
        all.iter().position(|t| t.id == after).map(|i| i + 1).unwrap_or(all.len())
    };
    let end = (start + first.max(0) as usize).min(all.len());
    let slice = &all[start..end];
    let edges: Vec<TodoEdge> =
        slice.iter().map(|t| TodoEdge { node: t.clone(), cursor: t.id.clone() }).collect();
    let end_cursor = slice.last().map(|t| t.id.clone()).unwrap_or_default();
    vec![TodoConnection { edges, page_info: PageInfo { has_next_page: end < all.len(), end_cursor } }]
}

/// 单条详情(/todo/:id):按 id 查,命中返回 1 条、否则空。
pub fn detail(id: &str) -> Vec<Todo> {
    store().lock().unwrap().iter().filter(|t| t.id == id).cloned().collect()
}

/// 服务端按文本过滤(resource! 搜索演示):空串返回空。
pub fn search(q: &str) -> Vec<Todo> {
    let q = q.trim();
    if q.is_empty() {
        return Vec::new();
    }
    store().lock().unwrap().iter().filter(|t| t.text.contains(q)).cloned().collect()
}

// ── 订阅 / SSE ──

/// SSE 推送 / 初值:标准 `{"data":{"todo_updates":[全字段]}}`。
pub fn snapshot_json() -> String {
    let todos = store().lock().unwrap().clone();
    Value::Object(vec![(
        "data".to_string(),
        Value::Object(vec![("todo_updates".to_string(), todos.into_value())]),
    )])
    .to_json()
}

pub fn add_subscriber() -> Receiver<String> {
    let (tx, rx) = channel();
    subs().lock().unwrap().push(tx);
    rx
}

fn broadcast() {
    let json = snapshot_json();
    subs().lock().unwrap().retain(|tx| tx.send(json.clone()).is_ok());
}

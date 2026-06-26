//! SSE 订阅广播(仅服务端)。数据存取已移到 ORM(rui::gql::orm + 注入的 DbExecutor);
//! 这里只管订阅者集合 + 快照广播:任一写操作后 schema 的 mutation 调 broadcast(),订阅者(首页 subscription!)实时刷新。
use crate::data::model::Todo;
use rui::gql::value::{IntoValue, Value};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Mutex, OnceLock};

fn subs() -> &'static Mutex<Vec<Sender<String>>> {
    static S: OnceLock<Mutex<Vec<Sender<String>>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(Vec::new()))
}

/// SSE 推送 / 初值:标准 `{"data":{"todo_updates":[全字段]}}`。读当前后端(PG / 内存)的全列快照。
pub fn snapshot_json() -> String {
    let todos = rui::gql::orm::fetch_all::<Todo>();
    Value::Object(vec![(
        "data".to_string(),
        Value::Object(vec![("todo_updates".to_string(), todos.into_value())]),
    )])
    .to_json()
}

pub fn add_subscriber() -> Receiver<String> {
    let (tx, rx) = channel();
    subs().lock().unwrap_or_else(|e| e.into_inner()).push(tx);
    rx
}

/// 任一写操作后广播当前快照给所有订阅者(首页 subscription! 实时反映增删改;PG / 内存后端都生效)。
pub fn broadcast() {
    let json = snapshot_json();
    subs().lock().unwrap_or_else(|e| e.into_inner()).retain(|tx| tx.send(json.clone()).is_ok());
}

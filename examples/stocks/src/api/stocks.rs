//! 股票数据后端(仅服务端):内存 store + 行情 ticker + SSE 订阅广播 + 数据访问。
//! 被 crate::api::schema 的 #[gql_root] 方法体调用;model 在 crate::data::model。
use crate::data::model::{PageInfo, Stock, StockConnection, StockEdge};
use rui::gql::value::{IntoValue, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Mutex, OnceLock};

fn seed() -> Vec<Stock> {
    [
        ("AAPL", "Apple Inc.", 192.30, 1.24),
        ("MSFT", "Microsoft Corp.", 421.50, 0.86),
        ("NVDA", "NVIDIA Corp.", 135.50, 3.71),
        ("GOOG", "Alphabet Inc.", 174.10, -0.42),
        ("AMZN", "Amazon.com Inc.", 186.40, 2.05),
        ("META", "Meta Platforms", 504.20, -1.18),
        ("TSLA", "Tesla Inc.", 248.90, -2.63),
    ]
    .iter()
    .map(|(s, n, p, c)| Stock { symbol: s.to_string(), name: n.to_string(), price: *p, change: *c })
    .collect()
}

fn store() -> &'static Mutex<Vec<Stock>> {
    static S: OnceLock<Mutex<Vec<Stock>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(seed()))
}
fn subs() -> &'static Mutex<Vec<Sender<String>>> {
    static S: OnceLock<Mutex<Vec<Sender<String>>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(Vec::new()))
}

// ── 数据访问 API(被 schema.rs 的 #[gql_root] resolver 方法调用)──

pub fn all_stocks() -> Vec<Stock> {
    store().lock().unwrap().clone()
}
pub fn stock(id: &str) -> Vec<Stock> {
    store().lock().unwrap().iter().filter(|s| s.symbol == id).cloned().collect()
}

/// Relay 游标分页:cursor = symbol;after 为上一页最后一个 symbol(空 = 第一页)。
pub fn page(first: i64, after: &str) -> Vec<StockConnection> {
    let all = store().lock().unwrap().clone();
    let start = if after.is_empty() {
        0
    } else {
        all.iter().position(|s| s.symbol == after).map(|i| i + 1).unwrap_or(all.len())
    };
    let end = (start + first.max(0) as usize).min(all.len());
    let slice = &all[start..end];
    let edges: Vec<StockEdge> =
        slice.iter().map(|s| StockEdge { node: s.clone(), cursor: s.symbol.clone() }).collect();
    let end_cursor = slice.last().map(|s| s.symbol.clone()).unwrap_or_default();
    vec![StockConnection {
        edges,
        page_info: PageInfo { has_next_page: end < all.len(), end_cursor },
    }]
}
pub fn set_price(symbol: &str, price: f64) -> Vec<Stock> {
    {
        let mut v = store().lock().unwrap();
        for s in v.iter_mut() {
            if s.symbol == symbol {
                s.price = price;
            }
        }
    }
    broadcast(); // mutation 也推给订阅者
    store().lock().unwrap().clone()
}

// ── 订阅 / SSE / ticker ──

/// 订阅推送 / SSE 初值:标准 `{"data":{"price_updates":[全字段]}}`(客户端 decode_rows 再投影)。
pub fn snapshot_json() -> String {
    let stocks = store().lock().unwrap().clone();
    Value::Object(vec![(
        "data".to_string(),
        Value::Object(vec![("price_updates".to_string(), stocks.into_value())]),
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

/// ticker:每次微调价格 + 广播给所有订阅者。
pub fn tick_and_broadcast() {
    static STEP: AtomicU64 = AtomicU64::new(0);
    let step = STEP.fetch_add(1, Ordering::Relaxed);
    {
        let mut v = store().lock().unwrap();
        for (i, s) in v.iter_mut().enumerate() {
            let d = ((step.wrapping_mul(7).wrapping_add(i as u64 * 13)) % 11) as i64 - 5; // -5..=5
            s.price = ((s.price + d as f64 / 10.0).max(1.0) * 100.0).round() / 100.0;
        }
    }
    broadcast();
}

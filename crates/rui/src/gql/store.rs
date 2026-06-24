//! 规范化缓存(Relay 式 normalized store)—— 前后端同构 query! 两端都会用到。
//!
//! entity key = "__typename:__id"(derive(GqlObject) 的 into_value 注入这两个 meta 字段,
//! 执行器投影时始终保留)。query/mutation/subscription 的响应 normalize 进来,bump 对应 entity
//! 的版本 signal;query 返回的视图是 memo,读 store 时订阅相关版本 —— 于是 mutation 写回同一
//! entity 后,所有引用它的视图自动重算更新(跨 query 数据一致,Relay 灵魂)。
//!
//! 写入顺序:先把整批 entity 全部合并进 store(merge_all,不 bump),再发布 key 列表 / bump 版本
//! —— 这样任何被唤醒的视图看到的都是「全部合并完」的一致快照,不会读到半合并的中间态。
//!
//! 本轮规范化到顶层 entity;嵌套对象(如 order.items)随顶层 entity 内联存储。

use crate::gql::value::Value;
use crate::reactive::{untrack, Signal};
use std::cell::RefCell;
use std::collections::HashMap;

thread_local! {
    static ENTITIES: RefCell<HashMap<String, Value>> = RefCell::new(HashMap::new());
    static VERSIONS: RefCell<HashMap<String, Signal<u64>>> = RefCell::new(HashMap::new());
}

fn version(key: &str) -> Signal<u64> {
    VERSIONS.with(|v| {
        v.borrow_mut()
            .entry(key.to_string())
            .or_insert_with(|| Signal::new(0))
            .clone()
    })
}

/// 把任意标量 Value 变成 entity key 的一段(支持 String / Int / Float / Bool 的 id)。
fn scalar_key(v: &Value) -> Option<String> {
    match v {
        Value::Str(s) if !s.is_empty() => Some(s.clone()),
        Value::Int(n) => Some(n.to_string()),
        Value::Float(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn entity_key(obj: &Value) -> Option<String> {
    let tn = obj.get("__typename")?.as_str();
    if tn.is_empty() {
        return None;
    }
    let id = scalar_key(obj.get("__id")?)?;
    Some(format!("{}:{}", tn, id))
}

/// 合并字段(后写覆盖)进现有 entity —— 不同 query 选不同字段时累积成完整记录。
fn merge_into(key: &str, obj: &Value) {
    ENTITIES.with(|e| {
        let mut e = e.borrow_mut();
        let cur = e
            .entry(key.to_string())
            .or_insert_with(|| Value::Object(Vec::new()));
        if let (Value::Object(dst), Value::Object(src)) = (cur, obj) {
            for (k, v) in src {
                if let Some(slot) = dst.iter_mut().find(|(dk, _)| dk == k) {
                    slot.1 = v.clone();
                } else {
                    dst.push((k.clone(), v.clone()));
                }
            }
        }
    });
}

fn bump(key: &str) {
    let ver = version(key);
    untrack(|| ver.set(ver.get() + 1)); // 触发订阅该 entity 的视图重算
}

/// 把一个值规范化进 store:带 __typename/__id 的(嵌套)对象抽成独立 entity(合并 + 记 touched),
/// 原位置换成 {"$ref": key};普通对象/列表递归处理。返回换好 ref 的值。
fn normalize_value(v: &Value, touched: &mut Vec<String>) -> Value {
    match v {
        Value::List(xs) => Value::List(xs.iter().map(|x| normalize_value(x, touched)).collect()),
        Value::Object(_) => {
            let inner = normalize_fields(v, touched);
            if let Some(key) = entity_key(v) {
                merge_into(&key, &inner);
                touched.push(key.clone());
                Value::Object(vec![("$ref".to_string(), Value::Str(key))])
            } else {
                inner // 普通对象(无 id):字段已递归规范化
            }
        }
        _ => v.clone(),
    }
}
fn normalize_fields(obj: &Value, touched: &mut Vec<String>) -> Value {
    match obj {
        Value::Object(fs) => Value::Object(
            fs.iter().map(|(k, val)| (k.clone(), normalize_value(val, touched))).collect(),
        ),
        _ => obj.clone(),
    }
}

/// 合并一组顶层对象进 store(嵌套 entity 抽独立 + 父留 ref),返回顶层 entity keys。
/// 嵌套 entity 在此立即 bump(通知其它引用它的视图);顶层 keys 由调用方 bump(保证一致快照)。
pub fn merge_all(v: &Value) -> Vec<String> {
    let mut top = Vec::new();
    let mut touched = Vec::new();
    for obj in v.as_list() {
        if let Some(key) = entity_key(obj) {
            let inner = normalize_fields(obj, &mut touched);
            merge_into(&key, &inner);
            top.push(key);
        }
    }
    bump_all(&touched); // 嵌套 entity 通知(顶层由调用方 bump)
    top
}

/// bump 一组 entity 的版本(通知订阅这些 entity 的视图重算)。
pub fn bump_all(keys: &[String]) {
    for k in keys {
        bump(k);
    }
}

/// 合并 + bump(便捷:mutation 用 —— 它没有自己的 key 列表,直接通知所有相关视图)。
pub fn normalize_list(v: &Value) -> Vec<String> {
    let keys = merge_all(v);
    bump_all(&keys);
    keys
}

/// 读取 entity 并 de-normalize(把 {"$ref":k} 递归 inline 回完整对象)。
/// 在 memo/effect 内调用 → 订阅本 entity 及所有被引用的嵌套 entity 版本
/// (于是嵌套 entity 被 mutation 改写后,引用它的视图也自动重算)。
pub fn read_entity(key: &str) -> Option<Value> {
    version(key).get(); // 订阅版本
    let raw = ENTITIES.with(|e| e.borrow().get(key).cloned())?;
    Some(denormalize(&raw))
}
fn denormalize(v: &Value) -> Value {
    match v {
        Value::Object(fs) => {
            if fs.len() == 1 && fs[0].0 == "$ref" {
                if let Value::Str(k) = &fs[0].1 {
                    return read_entity(k).unwrap_or(Value::Null); // 递归读(订阅嵌套版本)
                }
            }
            Value::Object(fs.iter().map(|(k, val)| (k.clone(), denormalize(val))).collect())
        }
        Value::List(xs) => Value::List(xs.iter().map(denormalize).collect()),
        _ => v.clone(),
    }
}

// ── connection record(分页:Relay 式 store 背书的游标连接)──
// conn record 存在 ENTITIES 里(key 由调用方给,如 "@conn:字段名"),形如
//   { edges: [ {node: {$ref}, cursor}, ... ], page_info: {has_next_page, end_cursor} }
// load_next 把新页 edges 追加进 record;node 抽成独立 entity(留 ref)→ node 被 mutation
// 改写后,分页视图(订阅了这些 node 版本)自动重算(完整 Relay 一致性)。

/// 合并一页 connection 进 store:edges 的 node 抽成独立 entity(留 ref);
/// append=true 追加新页、false 替换(首屏 / refetch);page_info 覆盖为最新。bump conn + 受影响 node。
pub fn merge_connection(conn_key: &str, conn: &Value, append: bool) {
    let mut touched = Vec::new();
    let new_edges: Vec<Value> = conn
        .field("edges")
        .as_list()
        .iter()
        .map(|e| normalize_fields(e, &mut touched)) // edge.node(有 id)→ ref
        .collect();
    let page_info = conn.field("page_info").clone();
    ENTITIES.with(|s| {
        let mut s = s.borrow_mut();
        let rec = s.entry(conn_key.to_string()).or_insert_with(|| {
            Value::Object(vec![
                ("edges".to_string(), Value::List(Vec::new())),
                ("page_info".to_string(), Value::Null),
            ])
        });
        if let Value::Object(fs) = rec {
            if let Some((_, Value::List(lst))) = fs.iter_mut().find(|(k, _)| k == "edges") {
                if !append {
                    lst.clear();
                }
                // 按 cursor 去重追加(幂等):重复 load 同一页 / 重发请求不会产生重复 edge。
                for e in new_edges {
                    let c = e.field("cursor").as_str().to_string();
                    let dup = !c.is_empty() && lst.iter().any(|x| x.field("cursor").as_str() == c);
                    if !dup {
                        lst.push(e);
                    }
                }
            }
            if let Some((_, pi)) = fs.iter_mut().find(|(k, _)| k == "page_info") {
                *pi = page_info;
            }
        }
    });
    touched.push(conn_key.to_string());
    bump_all(&touched);
}

/// 读取 connection record 并 de-normalize(node ref → inline,订阅 conn + 各 node 版本)。
pub fn read_connection(conn_key: &str) -> Value {
    version(conn_key).get(); // 订阅 conn 版本
    let raw = ENTITIES
        .with(|s| s.borrow().get(conn_key).cloned())
        .unwrap_or_else(|| Value::Object(Vec::new()));
    denormalize(&raw)
}

// ── 乐观更新(optimistic mutation 的快照 / 回滚)──

/// 递归收集一个值里所有 entity 的 key(top + 嵌套),用于乐观写入前快照。
pub fn keys_of(v: &Value) -> Vec<String> {
    let mut out = Vec::new();
    collect_keys(v, &mut out);
    out
}
fn collect_keys(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::List(xs) => xs.iter().for_each(|x| collect_keys(x, out)),
        Value::Object(fs) => {
            if let Some(k) = entity_key(v) {
                out.push(k);
            }
            fs.iter().for_each(|(_, val)| collect_keys(val, out));
        }
        _ => {}
    }
}

/// 记录这些 key 当前在 store 里的值(None = 当前不存在),供乐观更新失败 / 响应回来后回滚。
pub fn snapshot(keys: &[String]) -> Vec<(String, Option<Value>)> {
    ENTITIES.with(|e| {
        let e = e.borrow();
        keys.iter().map(|k| (k.clone(), e.get(k).cloned())).collect()
    })
}

/// 恢复快照(撤销乐观写入),并 bump 这些 key(视图回到写入前状态)。
pub fn restore(snap: &[(String, Option<Value>)]) {
    ENTITIES.with(|e| {
        let mut e = e.borrow_mut();
        for (k, v) in snap {
            match v {
                Some(val) => {
                    e.insert(k.clone(), val.clone());
                }
                None => {
                    e.remove(k);
                }
            }
        }
    });
    let keys: Vec<String> = snap.iter().map(|(k, _)| k.clone()).collect();
    bump_all(&keys);
}

/// 清空缓存(SSR 每次渲染前调用,保证请求间隔离,不依赖「每连接一线程」的偶然性)。
pub fn reset() {
    ENTITIES.with(|e| e.borrow_mut().clear());
    VERSIONS.with(|v| v.borrow_mut().clear());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gql::value::parse;

    #[test]
    fn nested_entity_normalized_and_denormalized() {
        reset();
        // 一个 Order 含两个 Item(都带 __typename/__id)
        let payload = parse(
            r#"[{"__typename":"Order","__id":"1","id":"1","items":[
                {"__typename":"Item","__id":"A","sku":"A","qty":2},
                {"__typename":"Item","__id":"B","sku":"B","qty":3}]}]"#,
        );
        let keys = merge_all(&payload);
        assert_eq!(keys, vec!["Order:1".to_string()]); // 顶层只有 Order
        // 嵌套 Item 被抽成独立 entity
        assert_eq!(read_entity("Item:A").unwrap().field("sku").as_str(), "A");
        // 父 entity 原始存储里 items 存的是 ref
        let raw = ENTITIES.with(|e| e.borrow().get("Order:1").cloned()).unwrap();
        assert_eq!(raw.field("items").as_list()[0].get("$ref").unwrap().as_str(), "Item:A");
        // read_entity de-normalize:items 应 inline 回完整 Item
        let order = read_entity("Order:1").unwrap();
        let items = order.field("items").as_list();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].field("sku").as_str(), "A");
        assert_eq!(items[1].field("qty").as_i64(), 3);
        reset();
    }

    #[test]
    fn connection_append_and_node_update() {
        reset();
        // 第一页(replace)
        merge_connection(
            "@c",
            &parse(
                r#"{"edges":[{"node":{"__typename":"Stock","__id":"A","symbol":"A","price":1.0},"cursor":"A"}],
                    "page_info":{"has_next_page":true,"end_cursor":"A"}}"#,
            ),
            false,
        );
        // 第二页(append 追加,非替换)
        merge_connection(
            "@c",
            &parse(
                r#"{"edges":[{"node":{"__typename":"Stock","__id":"B","symbol":"B","price":2.0},"cursor":"B"}],
                    "page_info":{"has_next_page":false,"end_cursor":"B"}}"#,
            ),
            true,
        );
        let conn = read_connection("@c");
        let edges = conn.field("edges").as_list();
        assert_eq!(edges.len(), 2); // 累积成 2 条
        assert_eq!(edges[0].field("node").field("symbol").as_str(), "A");
        assert_eq!(edges[1].field("node").field("symbol").as_str(), "B");
        // 分页列表里的 node 是独立 entity:单独 mutation 改写 Stock:A
        merge_all(&parse(r#"[{"__typename":"Stock","__id":"A","symbol":"A","price":999.0}]"#));
        let conn2 = read_connection("@c");
        // 分页视图自动看到新值(完整 Relay 一致性)
        assert_eq!(conn2.field("edges").as_list()[0].field("node").field("price").as_f64(), 999.0);
        // 去重:重复 load 第一页(append)不应产生重复 edge(幂等)
        merge_connection(
            "@c",
            &parse(
                r#"{"edges":[{"node":{"__typename":"Stock","__id":"A","symbol":"A","price":999.0},"cursor":"A"}],
                    "page_info":{"has_next_page":false,"end_cursor":"A"}}"#,
            ),
            true,
        );
        assert_eq!(read_connection("@c").field("edges").as_list().len(), 2); // 仍是 A,B(A 被去重)
        reset();
    }

    #[test]
    fn nested_entity_cross_query_update() {
        reset();
        merge_all(&parse(
            r#"[{"__typename":"Order","__id":"1","id":"1","items":[{"__typename":"Item","__id":"A","sku":"A","qty":2}]}]"#,
        ));
        // 另一处单独改写 Item:A(模拟别的 query/mutation)
        merge_all(&parse(r#"[{"__typename":"Item","__id":"A","sku":"A","qty":99}]"#));
        // 读 Order 视图能看到嵌套 Item 的新值(跨 query 一致)
        let order = read_entity("Order:1").unwrap();
        assert_eq!(order.field("items").as_list()[0].field("qty").as_i64(), 99);
        reset();
    }
}

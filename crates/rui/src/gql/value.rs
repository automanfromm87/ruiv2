//! 运行时值模型 + JSON 编解码 —— 前后端共用(取代旧 json.rs 的 Obj/Val)。
//! 递归下降 JSON parser:嵌套对象/数组、字符串转义(含 \uXXXX)、bool/null、int/float 区分。

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    List(Vec<Value>),
    Object(Vec<(String, Value)>),
}

impl Value {
    pub fn get(&self, key: &str) -> Option<&Value> {
        if let Value::Object(fs) = self {
            fs.iter().find(|(k, _)| k == key).map(|(_, v)| v)
        } else {
            None
        }
    }
    /// 取对象字段,缺失返回 &Null(给 FromValue 链式解析用,避免到处 unwrap)。
    pub fn field(&self, key: &str) -> &Value {
        const NULL: Value = Value::Null;
        self.get(key).unwrap_or(&NULL)
    }
    pub fn as_str(&self) -> &str {
        if let Value::Str(s) = self {
            s
        } else {
            ""
        }
    }
    pub fn as_f64(&self) -> f64 {
        match self {
            Value::Float(n) => *n,
            Value::Int(n) => *n as f64,
            _ => 0.0,
        }
    }
    pub fn as_i64(&self) -> i64 {
        match self {
            Value::Int(n) => *n,
            Value::Float(n) => *n as i64,
            _ => 0,
        }
    }
    pub fn as_bool(&self) -> bool {
        matches!(self, Value::Bool(true))
    }
    pub fn as_list(&self) -> &[Value] {
        if let Value::List(xs) = self {
            xs
        } else {
            &[]
        }
    }
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    pub fn to_json(&self) -> String {
        let mut s = String::new();
        self.write_json(&mut s);
        s
    }
    fn write_json(&self, out: &mut String) {
        match self {
            Value::Null => out.push_str("null"),
            Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Value::Int(n) => out.push_str(&n.to_string()),
            Value::Float(n) => out.push_str(&n.to_string()),
            Value::Str(s) => write_json_str(s, out),
            Value::List(xs) => {
                out.push('[');
                for (i, x) in xs.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    x.write_json(out);
                }
                out.push(']');
            }
            Value::Object(fs) => {
                out.push('{');
                for (i, (k, v)) in fs.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_json_str(k, out);
                    out.push(':');
                    v.write_json(out);
                }
                out.push('}');
            }
        }
    }
}

fn write_json_str(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

// ───────────────────────────── parser ─────────────────────────────

/// 从 GraphQL 响应里提取错误信息;无错误返回 None。数据层据此把失败传到 UI
/// (resource! 的 error 态、mutation! 的 on_error)。把"非法响应"也当失败,避免 HTTP 错误页 /
/// 解析垃圾被当成空成功(只有结构正确的 `{data, errors:[]}` / `{data, ...}` 才算成功):
///   · 非 JSON 对象(HTML 错误页 / 裸值 / 解析失败)→ 失败
///   · errors 是非空 list → 连接 message
///   · errors 缺失 → 有 data 才算成功,否则失败(缺 data/errors)
///   · errors 存在但不是 list(null/对象/字符串/数字)→ 失败(格式错误)
pub fn errors_message(v: &Value) -> Option<String> {
    if !matches!(v, Value::Object(_)) {
        return Some("非法响应(非 JSON 对象)".to_string());
    }
    match v.get("errors") {
        None => {
            if v.get("data").is_some() {
                None // 标准成功信封 {data, ...}
            } else {
                Some("非法响应(缺少 data/errors)".to_string())
            }
        }
        Some(Value::List(errs)) if errs.is_empty() => None, // 显式成功 errors:[]
        Some(Value::List(errs)) => {
            let msgs: Vec<String> = errs
                .iter()
                .map(|e| {
                    let m = e.field("message").as_str();
                    if m.is_empty() {
                        "GraphQL 错误".to_string()
                    } else {
                        m.to_string()
                    }
                })
                .collect();
            Some(msgs.join("; "))
        }
        Some(_) => Some("非法响应(errors 字段格式错误)".to_string()),
    }
}

pub fn parse(s: &str) -> Value {
    let mut p = P { b: s.as_bytes(), i: 0 };
    p.ws();
    p.value()
}

struct P<'a> {
    b: &'a [u8],
    i: usize,
}
impl<'a> P<'a> {
    fn ws(&mut self) {
        while self.i < self.b.len() && (self.b[self.i] as char).is_ascii_whitespace() {
            self.i += 1;
        }
    }
    fn ch(&self) -> u8 {
        if self.i < self.b.len() {
            self.b[self.i]
        } else {
            0
        }
    }
    fn value(&mut self) -> Value {
        self.ws();
        match self.ch() {
            b'{' => self.object(),
            b'[' => self.array(),
            b'"' => Value::Str(self.string()),
            b't' => {
                self.lit("true");
                Value::Bool(true)
            }
            b'f' => {
                self.lit("false");
                Value::Bool(false)
            }
            b'n' => {
                self.lit("null");
                Value::Null
            }
            _ => self.number(),
        }
    }
    fn lit(&mut self, w: &str) {
        if self.b[self.i..].starts_with(w.as_bytes()) {
            self.i += w.len();
        }
    }
    fn object(&mut self) -> Value {
        self.i += 1; // {
        let mut fields = Vec::new();
        loop {
            self.ws();
            if self.ch() == b'}' {
                self.i += 1;
                break;
            }
            if self.ch() != b'"' {
                break;
            }
            let key = self.string();
            self.ws();
            if self.ch() == b':' {
                self.i += 1;
            }
            let v = self.value();
            fields.push((key, v));
            self.ws();
            if self.ch() == b',' {
                self.i += 1;
            } else if self.ch() == b'}' {
                self.i += 1;
                break;
            } else {
                break;
            }
        }
        Value::Object(fields)
    }
    fn array(&mut self) -> Value {
        self.i += 1; // [
        let mut xs = Vec::new();
        loop {
            self.ws();
            if self.ch() == b']' {
                self.i += 1;
                break;
            }
            xs.push(self.value());
            self.ws();
            if self.ch() == b',' {
                self.i += 1;
            } else if self.ch() == b']' {
                self.i += 1;
                break;
            } else {
                break;
            }
        }
        Value::List(xs)
    }
    fn string(&mut self) -> String {
        self.i += 1; // 开引号
        let mut buf: Vec<u8> = Vec::new();
        while self.i < self.b.len() {
            let c = self.b[self.i];
            match c {
                b'"' => {
                    self.i += 1;
                    break;
                }
                b'\\' => {
                    self.i += 1;
                    let e = self.ch();
                    self.i += 1;
                    match e {
                        b'"' => buf.push(b'"'),
                        b'\\' => buf.push(b'\\'),
                        b'/' => buf.push(b'/'),
                        b'n' => buf.push(b'\n'),
                        b't' => buf.push(b'\t'),
                        b'r' => buf.push(b'\r'),
                        b'b' => buf.push(0x08),
                        b'f' => buf.push(0x0c),
                        b'u' => {
                            let end = (self.i + 4).min(self.b.len());
                            let hex = std::str::from_utf8(&self.b[self.i..end]).unwrap_or("");
                            self.i = end;
                            if let Ok(cp) = u32::from_str_radix(hex, 16) {
                                if let Some(ch) = char::from_u32(cp) {
                                    let mut tmp = [0u8; 4];
                                    buf.extend_from_slice(ch.encode_utf8(&mut tmp).as_bytes());
                                }
                            }
                        }
                        _ => {}
                    }
                }
                _ => {
                    buf.push(c);
                    self.i += 1;
                }
            }
        }
        String::from_utf8(buf).unwrap_or_default()
    }
    fn number(&mut self) -> Value {
        let start = self.i;
        let mut is_float = false;
        while self.i < self.b.len() {
            match self.b[self.i] {
                b'0'..=b'9' | b'-' | b'+' => self.i += 1,
                b'.' | b'e' | b'E' => {
                    is_float = true;
                    self.i += 1;
                }
                _ => break,
            }
        }
        let txt = std::str::from_utf8(&self.b[start..self.i]).unwrap_or("0");
        if is_float {
            Value::Float(txt.parse().unwrap_or(0.0))
        } else {
            match txt.parse::<i64>() {
                Ok(n) => Value::Int(n),
                Err(_) => Value::Float(txt.parse().unwrap_or(0.0)),
            }
        }
    }
}

// ───────────────────────── 编解码 trait ─────────────────────────
// query! 投影出的 exact-fit struct 用 FromValue 解析响应;resolver/序列化用 IntoValue。

pub trait FromValue {
    fn from_value(v: &Value) -> Self;
}
pub trait IntoValue {
    fn into_value(&self) -> Value;
}

impl FromValue for String {
    fn from_value(v: &Value) -> Self {
        v.as_str().to_string()
    }
}
impl FromValue for i64 {
    fn from_value(v: &Value) -> Self {
        v.as_i64()
    }
}
impl FromValue for f64 {
    fn from_value(v: &Value) -> Self {
        v.as_f64()
    }
}
impl FromValue for bool {
    fn from_value(v: &Value) -> Self {
        v.as_bool()
    }
}
impl<T: FromValue> FromValue for Vec<T> {
    fn from_value(v: &Value) -> Self {
        v.as_list().iter().map(T::from_value).collect()
    }
}
impl<T: FromValue> FromValue for Option<T> {
    fn from_value(v: &Value) -> Self {
        if v.is_null() {
            None
        } else {
            Some(T::from_value(v))
        }
    }
}

impl IntoValue for String {
    fn into_value(&self) -> Value {
        Value::Str(self.clone())
    }
}
impl IntoValue for str {
    fn into_value(&self) -> Value {
        Value::Str(self.to_string())
    }
}
impl IntoValue for i64 {
    fn into_value(&self) -> Value {
        Value::Int(*self)
    }
}
impl IntoValue for f64 {
    fn into_value(&self) -> Value {
        Value::Float(*self)
    }
}
impl IntoValue for bool {
    fn into_value(&self) -> Value {
        Value::Bool(*self)
    }
}
impl<T: IntoValue> IntoValue for Vec<T> {
    fn into_value(&self) -> Value {
        Value::List(self.iter().map(|x| x.into_value()).collect())
    }
}
impl<T: IntoValue> IntoValue for Option<T> {
    fn into_value(&self) -> Value {
        match self {
            Some(x) => x.into_value(),
            None => Value::Null,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_nested() {
        let src = r#"{"a":1,"b":[{"x":true,"y":"hi\n"},{"x":false,"y":null}],"c":1.5,"d":-42}"#;
        let v = parse(src);
        assert_eq!(v.field("a").as_i64(), 1);
        assert_eq!(v.field("c").as_f64(), 1.5);
        assert_eq!(v.field("d").as_i64(), -42);
        let b = v.field("b").as_list();
        assert_eq!(b.len(), 2);
        assert!(b[0].field("x").as_bool());
        assert_eq!(b[0].field("y").as_str(), "hi\n");
        assert!(b[1].field("y").is_null());
        assert_eq!(parse(&v.to_json()), v); // 往返
    }

    #[test]
    fn int_vs_float() {
        assert_eq!(parse("7"), Value::Int(7));
        assert_eq!(parse("7.0"), Value::Float(7.0));
        assert_eq!(parse("1e3"), Value::Float(1000.0));
    }

    #[test]
    fn escapes() {
        let v = parse(r#""a\"b\\cé""#);
        assert_eq!(v.as_str(), "a\"b\\cé");
        assert_eq!(parse(&v.to_json()).as_str(), "a\"b\\cé");
    }

    #[test]
    fn from_value_collections() {
        let xs: Vec<i64> = FromValue::from_value(&parse("[1,2,3]"));
        assert_eq!(xs, vec![1, 2, 3]);
    }

    #[test]
    fn errors_message_classification() {
        // 成功:有 data + 空 errors / 缺 errors
        assert_eq!(errors_message(&parse(r#"{"data":{"x":1},"errors":[]}"#)), None);
        assert_eq!(errors_message(&parse(r#"{"data":{"x":1}}"#)), None);
        // 失败:非空 errors → 连接 message
        assert_eq!(
            errors_message(&parse(r#"{"data":null,"errors":[{"message":"boom"}]}"#)),
            Some("boom".to_string())
        );
        assert_eq!(
            errors_message(&parse(r#"{"errors":[{"message":"a"},{"message":"b"}]}"#)),
            Some("a; b".to_string())
        );
        // 失败:非法响应(非对象 / 缺 data&errors / errors 非 list)
        assert!(errors_message(&parse("0")).is_some()); // 非 JSON → parse 兜底成数字 → 非对象
        assert!(errors_message(&parse("{}")).is_some()); // 缺 data/errors
        assert!(errors_message(&parse(r#"{"errors":"boom"}"#)).is_some()); // errors 非 list
        // error 无 message → 占位,不丢失"有错误"信号
        assert_eq!(errors_message(&parse(r#"{"errors":[{}]}"#)), Some("GraphQL 错误".to_string()));
    }
}

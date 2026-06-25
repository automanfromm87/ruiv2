//! 服务端 GraphQL 文档 parser(递归下降,纯 std)。
//! 支持:operation(query/mutation/subscription,可匿名)、嵌套 selection、字段参数、别名、一次多根。
//! 客户端 query!/mutation!/subscription! 生成的查询字符串由它解析,交给通用执行器(exec.rs)。
//! 变量定义 `query Foo($x: T)` 会被跳过(客户端把变量值内联进查询串,服务端只见字面量)。

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OpKind {
    Query,
    Mutation,
    Subscription,
}

#[derive(Debug, Clone)]
pub enum AVal {
    Str(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    Null,
}

#[derive(Debug, Clone)]
pub struct Field {
    pub alias: Option<String>,
    pub name: String,
    pub args: Vec<(String, AVal)>,
    pub selection: Vec<Field>,
}

#[derive(Debug, Clone)]
pub struct Operation {
    pub kind: OpKind,
    pub selection: Vec<Field>,
}

pub struct Document {
    pub ops: Vec<Operation>,
}

// selection 最大嵌套深度:超过即停止递归(改迭代跳过),防深嵌套 `{{{…` 把递归下降打爆栈 →
// Rust 栈溢出是硬 abort(guard-page),catch_unwind 抓不住 → 整进程死。这是未授权远程 DoS 的根因防护。
const MAX_DEPTH: usize = 64;

struct P<'a> {
    b: &'a [u8],
    i: usize,
    depth: usize,
}

impl<'a> P<'a> {
    fn ws(&mut self) {
        // GraphQL 里逗号是 insignificant,连同空白一起跳过
        while self.i < self.b.len() {
            let c = self.b[self.i];
            if c.is_ascii_whitespace() || c == b',' {
                self.i += 1;
            } else {
                break;
            }
        }
    }
    fn ch(&self) -> u8 {
        if self.i < self.b.len() {
            self.b[self.i]
        } else {
            0
        }
    }
    fn ident(&mut self) -> String {
        let start = self.i;
        while self.i < self.b.len() {
            let c = self.b[self.i];
            if c.is_ascii_alphanumeric() || c == b'_' {
                self.i += 1;
            } else {
                break;
            }
        }
        String::from_utf8_lossy(&self.b[start..self.i]).into_owned()
    }
    fn string(&mut self) -> String {
        self.i += 1; // 开引号
        let start = self.i;
        while self.i < self.b.len() && self.b[self.i] != b'"' {
            self.i += 1;
        }
        let s = String::from_utf8_lossy(&self.b[start..self.i]).into_owned();
        self.i += 1; // 闭引号
        s
    }
    fn value(&mut self) -> AVal {
        self.ws();
        match self.ch() {
            b'"' => AVal::Str(self.string()),
            b't' | b'f' => {
                let id = self.ident();
                AVal::Bool(id == "true")
            }
            b'n' => {
                let _ = self.ident();
                AVal::Null
            }
            _ => {
                let start = self.i;
                let mut is_float = false;
                while self.i < self.b.len() {
                    let c = self.b[self.i];
                    if c.is_ascii_digit() || c == b'-' || c == b'+' {
                        self.i += 1;
                    } else if c == b'.' || c == b'e' || c == b'E' {
                        is_float = true;
                        self.i += 1;
                    } else {
                        break;
                    }
                }
                if self.i == start {
                    // 无法识别的 token(如 [ { @ $)——跳过一字节以保证前进,当作 Null。
                    self.i += 1;
                    return AVal::Null;
                }
                let txt = String::from_utf8_lossy(&self.b[start..self.i]);
                if is_float {
                    AVal::Float(txt.parse().unwrap_or(0.0))
                } else {
                    AVal::Int(txt.parse().unwrap_or(0))
                }
            }
        }
    }
    fn args(&mut self) -> Vec<(String, AVal)> {
        let mut out = Vec::new();
        self.i += 1; // (
        loop {
            self.ws();
            if self.ch() == b')' || self.ch() == 0 {
                self.i += 1;
                break;
            }
            let before = self.i;
            let name = self.ident();
            self.ws();
            if self.ch() == b':' {
                self.i += 1;
            }
            let val = self.value();
            if !name.is_empty() {
                out.push((name, val));
            }
            self.ws();
            if self.i == before {
                self.i += 1; // 进度保护:本轮没消费任何东西 → 跳过一字节,避免死循环
            }
        }
        out
    }
    fn selection_set(&mut self) -> Vec<Field> {
        let mut out = Vec::new();
        self.i += 1; // {
        loop {
            self.ws();
            if self.ch() == b'}' || self.ch() == 0 {
                self.i += 1;
                break;
            }
            let before = self.i;
            let f = self.field();
            if self.i == before {
                self.i += 1; // 进度保护:非法 token(如 @ # !)→ 跳过一字节,避免死循环
                continue;
            }
            if !f.name.is_empty() {
                out.push(f);
            }
        }
        out
    }
    fn field(&mut self) -> Field {
        self.ws();
        let mut name = self.ident();
        let mut alias = None;
        self.ws();
        if self.ch() == b':' {
            // 前面其实是别名:`alias: realname`
            self.i += 1;
            self.ws();
            alias = Some(name);
            name = self.ident();
            self.ws();
        }
        let args = if self.ch() == b'(' { self.args() } else { Vec::new() };
        self.ws();
        let selection = if self.ch() == b'{' {
            // 深度守卫:超 MAX_DEPTH 不再递归,改迭代跳过这层平衡花括号(防深嵌套打爆栈 → 进程 abort)。
            if self.depth >= MAX_DEPTH {
                self.skip_braces();
                Vec::new()
            } else {
                self.depth += 1;
                let s = self.selection_set();
                self.depth -= 1;
                s
            }
        } else {
            Vec::new()
        };
        Field { alias, name, args, selection }
    }
    fn skip_braces(&mut self) {
        // 迭代跳过一个平衡的 `{ ... }`(超最大嵌套深度时替代递归,杜绝栈溢出)。
        let mut d = 0;
        loop {
            let c = self.ch();
            if c == 0 {
                break;
            }
            if c == b'{' {
                d += 1;
            } else if c == b'}' {
                d -= 1;
                self.i += 1;
                if d == 0 {
                    break;
                }
                continue;
            }
            self.i += 1;
        }
    }
    fn skip_var_defs(&mut self) {
        // 跳过 operation 的变量定义 `(...)`(平衡括号)
        let mut depth = 0;
        loop {
            let c = self.ch();
            if c == 0 {
                break;
            }
            if c == b'(' {
                depth += 1;
            } else if c == b')' {
                depth -= 1;
                self.i += 1;
                if depth == 0 {
                    break;
                }
                continue;
            }
            self.i += 1;
        }
    }
    fn operation(&mut self) -> Option<Operation> {
        self.ws();
        if self.ch() == 0 {
            return None;
        }
        let mut kind = OpKind::Query;
        if self.ch() != b'{' {
            let kw = self.ident();
            kind = match kw.as_str() {
                "mutation" => OpKind::Mutation,
                "subscription" => OpKind::Subscription,
                _ => OpKind::Query,
            };
            self.ws();
            // 可选 operation 名
            if self.ch() != b'{' && self.ch() != b'(' {
                let _ = self.ident();
                self.ws();
            }
            // 可选变量定义
            if self.ch() == b'(' {
                self.skip_var_defs();
                self.ws();
            }
        }
        if self.ch() != b'{' {
            return None;
        }
        let selection = self.selection_set();
        Some(Operation { kind, selection })
    }
}

pub fn parse(s: &str) -> Document {
    let mut p = P { b: s.as_bytes(), i: 0, depth: 0 };
    let mut ops = Vec::new();
    loop {
        p.ws();
        if p.ch() == 0 {
            break;
        }
        match p.operation() {
            Some(op) => ops.push(op),
            None => break,
        }
    }
    Document { ops }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_with_args_and_alias() {
        let doc = parse(r#"{ s: stock(id: "AAPL") { symbol price } orders { id items { sku qty } } }"#);
        assert_eq!(doc.ops.len(), 1);
        let sel = &doc.ops[0].selection;
        assert_eq!(sel.len(), 2);
        assert_eq!(sel[0].alias.as_deref(), Some("s"));
        assert_eq!(sel[0].name, "stock");
        assert_eq!(sel[0].args.len(), 1);
        assert_eq!(sel[0].args[0].0, "id");
        assert_eq!(sel[1].name, "orders");
        assert_eq!(sel[1].selection[1].name, "items");
        assert_eq!(sel[1].selection[1].selection.len(), 2);
    }

    #[test]
    fn mutation_kind() {
        let doc = parse(r#"mutation { set_price(symbol: "AAPL", price: 200.0) { symbol price } }"#);
        assert_eq!(doc.ops[0].kind, OpKind::Mutation);
        assert_eq!(doc.ops[0].selection[0].name, "set_price");
        assert_eq!(doc.ops[0].selection[0].args.len(), 2);
    }

    // 进度保护:畸形输入必须终止(否则服务端线程被钉死 = 远程 DoS)。
    #[test]
    fn malformed_input_terminates() {
        let _ = parse("{ @ }");
        let _ = parse("{ stock(id: $v) { symbol } }");
        let _ = parse("{ a(x: [) }");
        let _ = parse("{ # garbage ! }");
        let _ = parse("{{{{");
        // 仍能正常解析合法查询
        let doc = parse("{ stocks { symbol } }");
        assert_eq!(doc.ops[0].selection[0].name, "stocks");
    }

    // 深嵌套不爆栈(MAX_DEPTH 守卫):1M 层 `{` 必须正常返回,而非递归下降栈溢出 → 进程 abort(远程 DoS)。
    #[test]
    fn deep_nesting_does_not_overflow_stack() {
        let payload = "{".repeat(1_000_000);
        let _ = parse(&payload); // 不爆栈即通过(超 MAX_DEPTH 后迭代跳过,不再递归)
        // 守卫不影响合法的中等嵌套(< MAX_DEPTH):a{b{c{d}}} 仍正确解析
        let doc = parse("{ a { b { c { d } } } }");
        assert_eq!(doc.ops[0].selection[0].selection[0].selection[0].selection[0].name, "d");
    }
}

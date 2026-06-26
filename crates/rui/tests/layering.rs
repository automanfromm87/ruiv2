//! 分层 gate:强制依赖方向 L1 kernel <- L2 view/runtime <- L3 data <- L4 host。
//! 任一禁止的上行 / 跨层边出现即测试红 —— 比物理嵌套 / 拆 crate 更轻,但同样守住架构方向(重构不回退)。
//! 只查"非注释代码行"(注释里出现 crate::server 等是文档说明,不算真实依赖)。
//!
//! 分层(modules-with-discipline,物理文件暂不嵌套 / 不拆 crate):
//!   L1 kernel  reactive.rs · props.rs            —— 纯响应式内核,零 crate:: 依赖
//!   L2 ui      view.rs · dom.rs · runtime.rs     —— 视图树 / 元素模型 / 路由;只向下依赖 L1
//!   L3 data    gql/*                              —— GraphQL 值/类型/缓存/执行;只向下依赖 L1
//!   L4 host    server.rs · server_axum.rs        —— HTTP/SSR/浏览器宿主;依赖以下各层 + 注入 transport

fn code_lines(src: &str) -> impl Iterator<Item = &str> {
    src.lines().filter(|l| {
        let t = l.trim_start();
        !t.starts_with("//") && !t.starts_with('*') && !t.starts_with("/*")
    })
}

fn assert_no(src: &str, file: &str, needles: &[&str]) {
    for l in code_lines(src) {
        for n in needles {
            assert!(!l.contains(n), "分层违规:{file} 不得依赖 `{n}`,但出现于代码行:\n  {}", l.trim());
        }
    }
}

#[test]
fn kernel_is_dependency_free() {
    // L1 kernel 必须零 crate:: 依赖 → 可独立抽出 / 嵌入 / 单测(将来可作为 rui-kernel 单独发布)。
    for (f, src) in [
        ("reactive.rs", include_str!("../src/reactive.rs")),
        ("props.rs", include_str!("../src/props.rs")),
    ] {
        assert_no(src, f, &["crate::"]);
    }
}

#[test]
fn no_upward_or_crosslayer_edges() {
    // L2(dom/view/runtime)不得 NAME host(server);破 dom→server 循环后此条恒绿。
    assert_no(include_str!("../src/dom.rs"), "dom.rs", &["crate::server"]);
    assert_no(include_str!("../src/view.rs"), "view.rs", &["crate::server"]);
    assert_no(include_str!("../src/runtime.rs"), "runtime.rs", &["crate::server"]);
    // L3(gql)不得 NAME host(server)/ 上层(runtime/view)—— 数据层不知道页面 / 宿主。
    for (f, src) in [
        ("gql/mod.rs", include_str!("../src/gql/mod.rs")),
        ("gql/store.rs", include_str!("../src/gql/store.rs")),
        ("gql/exec.rs", include_str!("../src/gql/exec.rs")),
        ("gql/value.rs", include_str!("../src/gql/value.rs")),
        ("gql/parser.rs", include_str!("../src/gql/parser.rs")),
        ("gql/orm.rs", include_str!("../src/gql/orm.rs")),
    ] {
        assert_no(src, f, &["crate::server", "crate::runtime", "crate::view"]);
    }
}

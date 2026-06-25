//! 宏「拒绝契约」回归:每个 fail/*.rs 必须编译失败,且 stderr 匹配同名 .stderr 黄金文件。
//! 守住宏对非法用法的**明确报错**(而非静默丢弃 / 产出难懂的下游错误)。Phase C/D 改宏 emission 时,
//! 任一契约漂移会让这里变红。重新生成黄金文件:`TRYBUILD=overwrite cargo test -p rui-macros-tests`。
#[test]
fn macro_rejection_contracts() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/fail/*.rs");
}

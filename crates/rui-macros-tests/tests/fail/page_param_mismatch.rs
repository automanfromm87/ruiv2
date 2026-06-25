// 契约:#[rui::page] 的签名参数必须对应路由模式里的 `:seg`(此处 `wrong` 不在 `/todo/:id` 中)→ 明确报错,
// 让拼写错指向真正的笔误,而不是静默丢弃参数。
#[rui::page("/todo/:id")]
fn detail(wrong: rui::reactive::Signal<String>) -> rui::View {
    rui::view! { <div>"x"</div> }
}

fn main() {}

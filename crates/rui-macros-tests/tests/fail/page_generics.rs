// 契约:#[rui::page] 拒绝泛型 / where —— 页面被改写为 fn() -> rui::Page,泛型无处安放,须明确报错。
#[rui::page]
fn home<T>(x: T) -> rui::View {
    rui::view! { <div>"x"</div> }
}

fn main() {}

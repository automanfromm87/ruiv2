// 契约:#[rui::component] 拒绝泛型 / where —— props 走具名结构体 + builder,泛型无处安放,须明确报错。
#[rui::component]
fn thing<T>(label: T) -> rui::View {
    rui::view! { <div>"x"</div> }
}

fn main() {}

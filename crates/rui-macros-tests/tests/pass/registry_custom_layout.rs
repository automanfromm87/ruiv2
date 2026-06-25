// 证明 rui::app! 的目录解耦:用**非默认**路径(任意目录,含跨 crate 风格的多段路径)映射四个键,
// 生成的 crate::__rui_registry::{components,model,schema,fields} 应全部可达 → 宏不再绑死固定目录。
mod my_components {}
mod my_model {}
mod my_roots {}
mod my_markers {}

rui::app! {
    components = crate::my_components,
    model = crate::my_model,
    schema = crate::my_roots,
    fields = crate::my_markers,
}

fn main() {
    // registry 四键均解析到自定义模块(空模块即可证明 re-export 间接层编译通过 = 解耦成立)。
    #[allow(unused_imports)]
    use crate::__rui_registry::{components, fields, model, schema};
}

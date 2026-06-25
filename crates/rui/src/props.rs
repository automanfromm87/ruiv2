//! `#[rui::component]` 的 typed-builder 支持类型。
//!
//! 组件 props 用 builder 构造,以支持「可选 prop + 默认值」同时保住「必填 prop 编译期强制」:
//! builder 的每个字段是一个类型参数,初始 `Missing`,setter 调用后变 `Set<T>`;`build()` 只在所有
//! **必填**字段都是 `Set<T>` 时才存在(必填漏设 → `build()` 不可用 = 编译错);**可选**字段在 `build()`
//! 里 `or_default(默认值)`(未设取默认、已设取值)。这些类型由宏生成的代码引用,应用一般不直接用。

/// 字段未设状态。
pub struct Missing;
/// 字段已设状态(持有值)。
pub struct Set<T>(pub T);

/// `build()` 取值:`Set` 取已设值,`Missing` 调默认闭包取默认值。仅可选字段用(必填字段在 `build()` 的
/// Self 类型里被固定为 `Set<T>`、直接 `.0` 取值)。默认是**惰性**的:闭包只在字段省略(Missing)时才求值,
/// 故 prop 已提供时默认表达式不会执行(无副作用/无开销)。
pub trait OrDefault<T> {
    fn or_default(self, default: impl FnOnce() -> T) -> T;
}
impl<T> OrDefault<T> for Missing {
    fn or_default(self, default: impl FnOnce() -> T) -> T {
        default()
    }
}
impl<T> OrDefault<T> for Set<T> {
    fn or_default(self, _default: impl FnOnce() -> T) -> T {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn or_default_picks_set_else_default() {
        assert_eq!(Missing.or_default(|| 5), 5); // 未设 → 默认
        assert_eq!(Set(9).or_default(|| 5), 9); // 已设 → 已设值
        assert_eq!(Set(String::from("x")).or_default(|| String::from("d")), "x");
        assert_eq!(Missing.or_default(|| String::from("d")), "d");
    }

    #[test]
    fn default_is_lazy() {
        // 已设时默认闭包不执行(惰性)
        let ran = Cell::new(false);
        let v = Set(7).or_default(|| {
            ran.set(true);
            0
        });
        assert_eq!(v, 7);
        assert!(!ran.get(), "prop 已提供 → 默认表达式不应执行");
    }
}

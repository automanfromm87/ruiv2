//! 同构运行时:页面渲染(scope + mount + 切页 dispose)+ wasm 入口辅助。
//! `client!` 宏在应用 crate 里展开成 wasm 导出(alloc/render_route/dispatch/on_fetch),内部转调这里。

use crate::dom;
use crate::reactive::{scope, Scope};
use std::cell::RefCell;

thread_local! {
    // 当前页的响应式作用域;切页时先 dispose(销毁上一页 query memo,防泄漏 + 幽灵重算)。
    static PAGE_SCOPE: RefCell<Option<Scope>> = const { RefCell::new(None) };
}

/// 渲染某路径:dispose 上一页 → 在新 scope 内调用户的 route 生成根节点 → mount。
/// 客户端挂到 #app;服务端设为文档根(take_html 序列化用)。
pub fn render_path(route: fn(&str) -> u32, path: &str) {
    PAGE_SCOPE.with(|s| {
        if let Some(sc) = s.borrow_mut().take() {
            sc.dispose();
        }
    });
    let (node, sc) = scope(|| route(path));
    dom::mount(node);
    PAGE_SCOPE.with(|s| *s.borrow_mut() = Some(sc));
}

// ── wasm 入口辅助(由 client! 宏包成 #[no_mangle] 导出)──

/// JS 在 wasm 内存里分配缓冲区,用来把路径 / JSON 字符串传进来。
pub fn alloc(len: usize) -> *mut u8 {
    let mut v = vec![0u8; len];
    let p = v.as_mut_ptr();
    core::mem::forget(v);
    p
}

/// JS 写好路径后调用:渲染对应页。
///
/// # Safety
/// `ptr`/`len` 必须来自上面的 `alloc`(由 JS 侧保证)。
pub unsafe fn render_route(ptr: *mut u8, len: usize, route: fn(&str) -> u32) {
    let path = String::from_utf8_lossy(&Vec::from_raw_parts(ptr, len, len)).into_owned();
    render_path(route, &path);
}

/// 事件触发时由 JS 调用。
pub fn dispatch(id: u32) {
    dom::run_handler(id);
}

/// fetch 完成时由 JS 调用。
///
/// # Safety
/// `ptr`/`len` 必须来自 `alloc`。
pub unsafe fn on_fetch(id: u32, ptr: *mut u8, len: usize) {
    let text = String::from_utf8_lossy(&Vec::from_raw_parts(ptr, len, len)).into_owned();
    dom::run_fetch(id, &text);
}

/// 在应用 crate 里生成 wasm 客户端入口(导出 alloc/render_route/dispatch/on_fetch)。
/// 用法(应用 lib.rs):`rui::client!(crate::route);`
#[macro_export]
macro_rules! client {
    ($route:path) => {
        // 这些 #[no_mangle] extern "C" 导出只对 wasm 目标有意义。
        // 必须 cfg 门控到 wasm32,否则 native 构建也会发出 `alloc`/`dispatch` 等
        // 通用全局符号,在 cdylib / 与 libc 链接时有冲突风险。
        #[cfg(target_arch = "wasm32")]
        #[no_mangle]
        pub extern "C" fn alloc(len: usize) -> *mut u8 {
            $crate::runtime::alloc(len)
        }
        #[cfg(target_arch = "wasm32")]
        #[no_mangle]
        pub extern "C" fn render_route(ptr: *mut u8, len: usize) {
            unsafe { $crate::runtime::render_route(ptr, len, $route) }
        }
        #[cfg(target_arch = "wasm32")]
        #[no_mangle]
        pub extern "C" fn dispatch(id: u32) {
            $crate::runtime::dispatch(id)
        }
        #[cfg(target_arch = "wasm32")]
        #[no_mangle]
        pub extern "C" fn on_fetch(id: u32, ptr: *mut u8, len: usize) {
            unsafe { $crate::runtime::on_fetch(id, ptr, len) }
        }
    };
}

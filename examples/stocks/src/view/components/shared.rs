//! 组件与页面共享的小类型:过滤器枚举、Relay 片段(data masking)。

// 过滤器(页面与 Toolbar 共享)。
#[derive(Clone, Copy, PartialEq)]
pub enum Filter {
    All,
    Active,
    Done,
}
impl Filter {
    pub fn keep(self, done: bool) -> bool {
        match self {
            Filter::All => true,
            Filter::Active => !done,
            Filter::Done => done,
        }
    }
}

// Relay 式片段 + data masking:TodoItem 声明自己要的数据(id/text/done)。
rui::fragment!(TodoView on Todo { id text done });

// Context 演示用:一个"当前用户名"上下文(newtype 包 Signal,按类型注入)。
// 页面 provide_context(Greeting(..)),深层组件 use_context::<Greeting>() 取到同一个 signal —— 免 prop-drill。
#[derive(Clone)]
pub struct Greeting(pub rui::reactive::Signal<String>);

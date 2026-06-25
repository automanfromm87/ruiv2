use rui::reactive::Signal;
use rui::{component, view};
use std::cell::Cell;
use std::rc::Rc;

// 运行计时:on_mount 启 setInterval 每秒 +1;on_cleanup 离开页面时 clearInterval(否则定时器泄漏)。
#[component]
pub fn uptime() -> rui::View {
    let secs = Signal::new(0i64);
    let timer: Rc<Cell<u32>> = Rc::new(Cell::new(0)); // 持有 timer id 供 cleanup
    rui::on_mount({
        let (secs, timer) = (secs.clone(), timer.clone());
        move || {
            let id = rui::dom::set_interval(1000, move || secs.set(secs.get() + 1));
            timer.set(id);
        }
    });
    rui::on_cleanup({
        let timer = timer.clone();
        move || rui::dom::clear_interval(timer.get())
    });
    view! {
        <span class="rounded-md bg-slate-800/70 px-2 py-1 text-xs tabular-nums text-slate-400">
            { move || format!("⏱ {}s", secs.get()) }
        </span>
    }
}

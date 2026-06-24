//! stocks SSR 服务器:spawn 行情 ticker,然后用框架的 rui::serve 起服务。
//! 应用侧只需把 route / resolve / SSE hook 装进 rui::App。

use std::thread;
use std::time::Duration;

fn main() {
    // 行情 ticker:每 1.5s 微调价格并广播给订阅者(应用数据,非框架职责)。
    thread::spawn(|| loop {
        thread::sleep(Duration::from_millis(1500));
        stocks::api::stocks::tick_and_broadcast();
    });

    rui::serve(rui::App {
        route: stocks::route,
        resolve: stocks::api::schema::resolve,
        sse: Some(rui::Sse {
            snapshot: stocks::api::stocks::snapshot_json,
            subscribe: stocks::api::stocks::add_subscriber,
        }),
    });
}

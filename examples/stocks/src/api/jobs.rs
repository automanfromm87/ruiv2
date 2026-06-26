//! 示例 AsyncJob —— 演示 v2 后台任务(`#[rui::job]`)。native-only。
//! 由 GraphQL mutation(`add_todo`)入队(`rui::enqueue::<notify_added>(id)`);后台 worker 线程异步执行,
//! 不阻塞 /graphql 响应。第一版内存队列 + 进程内 worker(见 rui::jobs 的已知限制)。

use rui::{CronTick, JobCtx, JobResult};

/// 新待办入库后触发:假装做「发通知 / 写搜索索引 / 打点统计」之类的后台活。
/// `#[rui::job]` 把它改写成 marker 类型 `notify_added` + `impl rui::Job`(payload = String = 新待办 id)。
#[rui::job]
pub fn notify_added(ctx: &JobCtx, id: String) -> JobResult {
    println!("📣 [job:{}] 新待办 #{id} 已入库 —(假装)发送通知 / 更新搜索索引 / 打点统计", ctx.job());
    Ok(())
}

/// 定时任务演示:每 5 秒触发一次(scheduler 线程按间隔 enqueue → worker 异步执行;复用 AsyncJob 那套)。
/// `#[rui::cron]` 把它改写成 payload = CronTick 的 Job + `impl rui::CronJob`(带触发间隔)。
#[rui::cron(every = "5s")]
pub fn heartbeat(ctx: &JobCtx, tick: CronTick) -> JobResult {
    println!("⏰ [cron:{}] 心跳 #{} —(假装)清理过期会话 / 刷新缓存 / 对账", ctx.job(), tick.count);
    Ok(())
}

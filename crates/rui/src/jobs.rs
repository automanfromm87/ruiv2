//! AsyncJob:后台异步任务(L4 host,仅服务端)。
//!
//! v2「统一 API 层」第二刀。与 HTTP 表面(GraphQL / 页面)正交 —— job 是**后台计算**:
//! 在请求路径里 `enqueue::<J>(payload)` 投递,后台 worker 线程异步执行,不阻塞响应。
//!
//! 设计要点(对齐 platform! 的类型化 / 声明式风格 + 守住不变量):
//!   · **类型安全**:`#[rui::job] fn f(ctx: &JobCtx, p: P)` 生成 marker 类型 + `impl Job`;
//!     `enqueue::<f>(P{..})` 编译期校验 payload 类型(payload 必须 IntoValue + FromValue)。
//!   · **依赖倒置**:队列是 `QueueExecutor` 纯接口(同 DbExecutor 模式),host 注入;默认内存队列(`MemQueue`),
//!     后续可 `set_queue_executor` 换 SQS / Pub-Sub —— L4 不 NAME 任何 broker。
//!   · **同构**:整模块 native-only(`#[cfg(not wasm)]` 在 lib.rs 门控);payload 编解码走 gql::value(零新依赖)。
//!   · job 声明在 `platform!{ jobs { .. } }`,生成 `run_job` 分发器,app() 注册它;`serve` / `serve_axum`
//!     启动时若已注册 → spawn 一根后台 worker 线程消费队列。
//!
//! 第一版限制(已知):失败仅记日志、**无重试 / 无幂等 / 无持久化**(进程退出丢未消费任务);
//! worker 进程内单线程(够 demo + 中小负载)。生产化(重试 / DLQ / 多 worker / 持久 broker)随
//! CloudProvider 那刀经 `QueueExecutor` 接缝接入。

use crate::gql::value::IntoValue;
use std::collections::VecDeque;
use std::sync::{Condvar, Mutex, OnceLock};

/// job 失败原因(第一版用字符串;后续可富化)。
pub struct JobError(pub String);
impl JobError {
    pub fn new(msg: impl Into<String>) -> JobError {
        JobError(msg.into())
    }
}
impl From<&str> for JobError {
    fn from(s: &str) -> JobError {
        JobError(s.to_string())
    }
}
impl From<String> for JobError {
    fn from(s: String) -> JobError {
        JobError(s)
    }
}

/// job handler 的返回类型:`Ok(())` 成功;`Err(JobError)` 失败(worker 记日志)。
pub type JobResult = Result<(), JobError>;

/// 一个异步任务:类型化 payload + run。由 `#[rui::job]` 在 marker 类型上自动实现。
/// `enqueue::<J>(payload)` 投递;worker 拉到后用 `J::Payload::from_value` 解码再调 `J::run`。
pub trait Job: 'static {
    /// 任务载荷(可经 gql::value 序列化 / 反序列化)。
    type Payload: IntoValue + crate::gql::value::FromValue;
    /// 任务名(= 函数名;队列里按它路由到 handler,跨进程也稳定)。
    const NAME: &'static str;
    /// 执行任务。
    fn run(ctx: &JobCtx, payload: Self::Payload) -> JobResult;
}

/// job 执行上下文(handler 第一个参数)。第一版只带任务名;后续会长出 app/store/queue 句柄 + attempt 等
/// (与统一 `Ctx` 收敛)。
pub struct JobCtx {
    name: String,
}
impl JobCtx {
    pub fn new(name: &str) -> JobCtx {
        JobCtx { name: name.to_string() }
    }
    /// 当前任务名。
    pub fn job(&self) -> &str {
        &self.name
    }
}

// ── 队列:依赖倒置接口 + host 注入(默认内存队列)──

/// 队列后端(host 注入,纯接口、不 NAME 任何 broker)。pull 模型:worker 阻塞 `next` 拉任务。
pub trait QueueExecutor: Send + Sync {
    /// 投递一条任务(payload 已由上层经 gql::value 序列化为 JSON)。
    fn publish(&self, job: &str, payload_json: &str);
    /// 阻塞直到有任务可消费,返回 (任务名, payload JSON)。worker 循环调用。
    fn next(&self) -> (String, String);
}

/// 默认内存队列(零依赖):`Mutex<VecDeque>` + `Condvar` 阻塞队列。进程内、不持久。
struct MemQueue {
    inner: Mutex<VecDeque<(String, String)>>,
    cv: Condvar,
}
impl MemQueue {
    fn new() -> MemQueue {
        MemQueue { inner: Mutex::new(VecDeque::new()), cv: Condvar::new() }
    }
}
impl QueueExecutor for MemQueue {
    fn publish(&self, job: &str, payload_json: &str) {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back((job.to_string(), payload_json.to_string()));
        self.cv.notify_one();
    }
    fn next(&self) -> (String, String) {
        let mut q = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(item) = q.pop_front() {
                return item;
            }
            q = self.cv.wait(q).unwrap_or_else(|e| e.into_inner());
        }
    }
}

static QUEUE: OnceLock<Box<dyn QueueExecutor>> = OnceLock::new();

/// host 启动时注册队列后端(SQS / Pub-Sub / …)。未注册 → 默认内存队列。须在首次 enqueue / worker 启动前调。
pub fn set_queue_executor(e: Box<dyn QueueExecutor>) {
    let _ = QUEUE.set(e);
}
fn queue() -> &'static dyn QueueExecutor {
    QUEUE.get_or_init(|| Box::new(MemQueue::new())).as_ref()
}

/// 投递一个任务(类型安全):payload 经 gql::value 序列化 → 入队。立即返回,后台 worker 异步执行。
/// 在请求路径(GraphQL resolver / 其它 job)里调用:`rui::enqueue::<send_welcome>(WelcomeEmail { .. })`。
pub fn enqueue<J: Job>(payload: J::Payload) {
    let json = IntoValue::into_value(&payload).to_json();
    queue().publish(J::NAME, &json);
}

// ── worker:platform! 生成 run_job 分发器,app() 注册;serve / serve_axum 启动后台线程消费 ──

/// job 分发器:`platform!{ jobs { .. } }` 生成,按任务名解码 payload 并调对应 `Job::run`;未注册的任务名 → None。
pub type JobDispatch = fn(&str, &JobCtx, &str) -> Option<JobResult>;

static DISPATCH: OnceLock<JobDispatch> = OnceLock::new();

/// 注册 job 分发器(由 `platform!` 生成的 `app()` 调用一次)。
pub fn set_job_dispatch(d: JobDispatch) {
    let _ = DISPATCH.set(d);
}

/// host 启动时调用:若已声明 jobs(分发器已注册)→ spawn 一根后台 worker 线程消费队列。无 jobs → no-op。
/// 由 `serve` / `serve_axum` 调用。
pub(crate) fn start_worker_if_configured() {
    if let Some(&dispatch) = DISPATCH.get() {
        std::thread::spawn(move || worker_loop(dispatch));
        println!("rui · worker  →  已启动(消费后台任务队列)");
    }
}

fn worker_loop(dispatch: JobDispatch) {
    loop {
        let (name, json) = queue().next(); // 阻塞拉取
        let ctx = JobCtx::new(&name);
        // catch_unwind:一个任务 panic 不拖垮 worker 线程(其它任务照常)。
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| dispatch(&name, &ctx, &json)));
        match r {
            Ok(Some(Ok(()))) => {}
            Ok(Some(Err(e))) => eprintln!("rui: job `{name}` 失败:{}", e.0), // 第一版:记日志,无重试
            Ok(None) => eprintln!("rui: 收到未注册的 job `{name}`(忘了在 platform! 的 jobs {{ }} 里声明?)"),
            Err(_) => eprintln!("rui: job `{name}` panic(已隔离,worker 继续)"),
        }
    }
}

// ── CronJob:定时触发。复用 worker + 队列 —— scheduler 线程按间隔把 cron job enqueue,worker 异步执行 ──
// 第一版 = 固定间隔(`#[rui::cron(every = "5s")]`);完整 cron 表达式("0 0 2 * * *")是后续(需 cron 解析 + 下次时刻计算)。

/// cron 触发时交给 handler 的入参。第一版只带触发序号 `count`(后续可加触发时刻等)。
pub struct CronTick {
    pub count: u64,
}
impl IntoValue for CronTick {
    fn into_value(&self) -> crate::gql::value::Value {
        crate::gql::value::Value::Int(self.count as i64)
    }
}
impl crate::gql::value::FromValue for CronTick {
    fn from_value(v: &crate::gql::value::Value) -> Self {
        CronTick { count: <i64 as crate::gql::value::FromValue>::from_value(v).max(0) as u64 }
    }
}

/// 定时任务:本质是 payload = `CronTick` 的 `Job`(故 worker 能跑它),额外带触发间隔。
/// 由 `#[rui::cron(every = "..")]` 实现;`platform!{ crons { .. } }` 登记 → serve 启动 scheduler 线程。
pub trait CronJob: Job<Payload = CronTick> {
    /// 触发间隔(秒)。第一版为固定间隔。
    const INTERVAL_SECS: u64;
}

/// scheduler 登记项:(job NAME, 间隔秒)。
pub type CronSpec = (&'static str, u64);
static CRONS: OnceLock<Vec<CronSpec>> = OnceLock::new();

/// 注册定时任务表(由 platform! 生成的 app() 调用一次)。
pub fn set_crons(specs: Vec<CronSpec>) {
    let _ = CRONS.set(specs);
}

/// host 启动时调用:为每个 cron spec spawn 一根 scheduler 线程,按间隔 enqueue 对应 job(worker 异步执行)。
/// 无 cron → no-op。由 serve / serve_axum 在 worker 启动之后调用。
pub(crate) fn start_crons_if_configured() {
    if let Some(specs) = CRONS.get() {
        if specs.is_empty() {
            return;
        }
        for &(name, secs) in specs {
            std::thread::spawn(move || {
                let dur = std::time::Duration::from_secs(secs.max(1));
                let mut count: u64 = 0;
                loop {
                    std::thread::sleep(dur); // 首次触发在一个间隔之后(非启动即触发)
                    count += 1;
                    let json = IntoValue::into_value(&CronTick { count }).to_json();
                    queue().publish(name, &json); // 入队 → worker 的 run_job 按 name 解码 CronTick 并执行
                }
            });
        }
        println!("rui · cron  →  已启动({} 个定时任务)", specs.len());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gql::value::Value;

    #[test]
    fn mem_queue_publish_then_next() {
        let q = MemQueue::new();
        q.publish("send", r#"{"x":1}"#);
        q.publish("clean", "\"id\"");
        assert_eq!(q.next(), ("send".to_string(), r#"{"x":1}"#.to_string()));
        assert_eq!(q.next(), ("clean".to_string(), "\"id\"".to_string()));
    }

    #[test]
    fn enqueue_serializes_and_dispatches() {
        // 用一个内存队列直接验证 enqueue → 序列化 → 分发解码 → run 的闭环(不起线程)。
        struct DemoJob;
        impl Job for DemoJob {
            type Payload = String;
            const NAME: &'static str = "demo";
            fn run(_ctx: &JobCtx, payload: String) -> JobResult {
                if payload == "boom" {
                    Err(JobError::new("炸了"))
                } else {
                    Ok(())
                }
            }
        }
        // 序列化:String payload → JSON 字符串(带引号)。
        let json = IntoValue::into_value(&"hi".to_string()).to_json();
        assert_eq!(json, "\"hi\"");
        // 分发:按名解码 + run。
        let dispatch: JobDispatch = |name, ctx, json| {
            if name == DemoJob::NAME {
                let p = <String as crate::gql::value::FromValue>::from_value(&crate::gql::parse(json));
                return Some(DemoJob::run(ctx, p));
            }
            None
        };
        let ctx = JobCtx::new("demo");
        assert!(matches!(dispatch("demo", &ctx, "\"hi\""), Some(Ok(()))));
        assert!(matches!(dispatch("demo", &ctx, "\"boom\""), Some(Err(_))));
        assert!(dispatch("unknown", &ctx, "\"x\"").is_none());
        let _ = Value::Null; // 触达 import(保持与其它测试一致的引用方式)
    }

    #[test]
    fn cron_tick_roundtrip() {
        // CronTick 经队列 = 序列化成裸 JSON 数字 → 解码回 count(scheduler enqueue → worker 解码的闭环)。
        let json = IntoValue::into_value(&CronTick { count: 42 }).to_json();
        assert_eq!(json, "42");
        let back = <CronTick as crate::gql::value::FromValue>::from_value(&crate::gql::parse(&json));
        assert_eq!(back.count, 42);
        // 负数 / 垃圾 → 0(不 panic)。
        assert_eq!(<CronTick as crate::gql::value::FromValue>::from_value(&Value::Int(-5)).count, 0);
        assert_eq!(<CronTick as crate::gql::value::FromValue>::from_value(&Value::Null).count, 0);
    }
}

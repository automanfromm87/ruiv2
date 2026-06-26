//! 部署模型(rui::deploy,仅服务端):从 `platform!{}` 声明推导出的 Application Model → **部署 DAG**。
//!
//! v2 Pillar 3 第一刀「compile → DAG → plan」:`platform!` 生成 `describe() -> AppModel`(运行时读各原语的
//! 编译期常量:路由 `__RUI_PATTERN`/`__RUI_STRATEGY`、`<Job>::NAME`、`<Cron>::INTERVAL_SECS`、`database` 声明)。
//! `AppModel::graph()` 推导出一张**有向无环图**(节点 = 可部署资源,边 = 依赖),`rui plan`(= `cargo run -- plan`)
//! 据此打印:分层 / 拓扑 provision 顺序 / 环检测 / Graphviz DOT / CloudProvider 映射。
//!
//! DAG 的意义:**拓扑排序 = provision 顺序**(依赖在前:先建 DB / 队列,再起依赖它们的计算);**环 = 配置错误**。
//! 本刀只到「生成 plan」;真正 provision(调 AWS/GCP SDK)是后续(optional crate,不破零依赖默认)。

use crate::view::Strategy;
use std::collections::HashMap;

/// 一条路由(页面)。strategy = `#[rui::page]` 的渲染策略(static 可走 CDN;ssr/csr 要 compute)。
pub struct RouteNode {
    pub pattern: String,
    pub strategy: Strategy,
}

/// 一个定时任务。
pub struct CronNode {
    pub name: String,
    pub interval_secs: u64,
}

/// 从 `platform!{}` 声明推导出的应用模型(DAG 的**输入**)。`describe()`(platform! 生成)产出它。
pub struct AppModel {
    pub routes: Vec<RouteNode>,
    pub graphql: bool,            // 声明了 resolve
    pub sse: bool,                // 声明了 subscribe
    pub database: Option<String>, // 声明了 database = <kind>(如 "postgres")
    pub jobs: Vec<String>,        // job NAME
    pub crons: Vec<CronNode>,     // (name, interval)
}

// ── 部署 DAG ──

/// DAG 节点种类(决定 CloudProvider 映射)。
#[derive(Clone, Copy, PartialEq)]
pub enum NodeKind {
    Compute,  // 计算单元(web / worker)
    Database, // 关系型存储
    Queue,    // 消息队列
    Schedule, // 定时调度器
    Static,   // 静态资源(CDN / 对象存储)
}

/// DAG 节点 = 一个可部署资源 / 计算单元。`id` 唯一、稳定。
pub struct Node {
    pub id: &'static str,
    pub label: String,
    pub kind: NodeKind,
}

/// 有向边:`from` **依赖** `to`(部署时 `to` 必须先 provision)。`label` = 关系(读写 / 消费 / 触发…)。
pub struct Edge {
    pub from: &'static str,
    pub to: &'static str,
    pub label: &'static str,
}

/// 部署 DAG:节点 + 有向依赖边。支持拓扑排序(provision 顺序)+ 环检测 + 分层 + DOT。
pub struct DeployGraph {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}

impl DeployGraph {
    /// `id` 直接依赖的节点(出边目标)。
    fn deps_of(&self, id: &str) -> Vec<&'static str> {
        self.edges.iter().filter(|e| e.from == id).map(|e| e.to).collect()
    }

    /// 拓扑排序 = **provision 顺序**(被依赖者在前)。检测到环 → `Err(环上节点序列)`。
    /// DFS 后序:递归进每个节点的依赖,依赖先入列 → 自身后入列。gray/black 着色检测环。
    pub fn topo(&self) -> Result<Vec<&Node>, String> {
        let mut state: HashMap<&'static str, u8> = HashMap::new(); // 0/缺=未访问,1=在栈(gray),2=完成(black)
        let mut order: Vec<&Node> = Vec::new();
        for n in &self.nodes {
            self.dfs(n.id, &mut state, &mut order, &mut Vec::new())?;
        }
        Ok(order)
    }

    fn dfs<'a>(
        &'a self,
        id: &'static str,
        state: &mut HashMap<&'static str, u8>,
        order: &mut Vec<&'a Node>,
        stack: &mut Vec<&'static str>,
    ) -> Result<(), String> {
        match state.get(id) {
            Some(2) => return Ok(()),
            Some(1) => {
                stack.push(id);
                return Err(stack.join(" → "));
            }
            _ => {}
        }
        state.insert(id, 1);
        stack.push(id);
        for d in self.deps_of(id) {
            self.dfs(d, state, order, stack)?;
        }
        stack.pop();
        state.insert(id, 2);
        if let Some(n) = self.nodes.iter().find(|n| n.id == id) {
            order.push(n);
        }
        Ok(())
    }

    /// 节点层级(到叶子的最长依赖深度):叶子=0,否则 1+max(依赖层级)。用于分层展示(同层可并行 provision)。
    pub fn levels(&self) -> HashMap<&'static str, usize> {
        let mut memo: HashMap<&'static str, usize> = HashMap::new();
        // 按拓扑顺序(依赖在前)填,依赖层级已知;若有环则 topo 已报错、这里不会被调用到坏路径。
        if let Ok(order) = self.topo() {
            for n in order {
                let lvl = self.deps_of(n.id).iter().map(|d| memo.get(*d).copied().unwrap_or(0) + 1).max().unwrap_or(0);
                memo.insert(n.id, lvl);
            }
        }
        memo
    }

    /// Graphviz DOT(可直接 `dot -Tpng` 渲染成图)。
    pub fn to_dot(&self) -> String {
        let mut s = String::from("digraph deploy {\n  rankdir=LR;\n  node [shape=box];\n");
        for n in &self.nodes {
            s.push_str(&format!("  \"{}\";\n", n.id));
        }
        for e in &self.edges {
            s.push_str(&format!("  \"{}\" -> \"{}\" [label=\"{}\"];\n", e.from, e.to, e.label));
        }
        s.push_str("}\n");
        s
    }
}

fn strat_str(s: Strategy) -> &'static str {
    match s {
        Strategy::Ssr => "ssr",
        Strategy::Csr => "csr",
        Strategy::Static => "static",
    }
}

fn fmt_dur(secs: u64) -> String {
    if secs % 86400 == 0 {
        format!("{}d", secs / 86400)
    } else if secs % 3600 == 0 {
        format!("{}h", secs / 3600)
    } else if secs % 60 == 0 {
        format!("{}m", secs / 60)
    } else {
        format!("{}s", secs)
    }
}

// 节点 → CloudProvider(AWS 示例)资源。
fn aws_of(id: &str) -> &'static str {
    match id {
        "web" => "ECS Fargate + ALB",
        "worker" => "ECS Fargate(SQS consumer)",
        "database" => "RDS Postgres",
        "queue" => "SNS + SQS",
        "schedule" => "EventBridge Scheduler",
        "static" => "S3 + CloudFront",
        _ => "—",
    }
}

impl AppModel {
    fn needs_queue(&self) -> bool {
        !self.jobs.is_empty() || !self.crons.is_empty()
    }

    /// 从声明推导出部署 DAG:节点 = 可部署资源,边 = 依赖(from 依赖 to → to 先 provision)。
    pub fn graph(&self) -> DeployGraph {
        let has_db = self.database.is_some();
        let has_queue = self.needs_queue();
        let has_cron = !self.crons.is_empty();
        let has_jobs = !self.jobs.is_empty();

        let mut nodes: Vec<Node> = Vec::new();
        let mut edges: Vec<Edge> = Vec::new();

        // 叶子资源(无依赖)先声明,便于阅读。
        if let Some(db) = &self.database {
            nodes.push(Node { id: "database", label: db.clone(), kind: NodeKind::Database });
        }
        if has_queue {
            nodes.push(Node { id: "queue", label: "jobs 队列".into(), kind: NodeKind::Queue });
        }
        nodes.push(Node { id: "static", label: "app.wasm / styles.css / static 页".into(), kind: NodeKind::Static });

        // 计算单元(依赖资源)。
        let mut web_label = String::from("SSR/CSR 页面");
        if self.graphql {
            web_label.push_str(" + GraphQL");
        }
        if self.sse {
            web_label.push_str(" + SSE");
        }
        nodes.push(Node { id: "web", label: web_label, kind: NodeKind::Compute });
        if has_db {
            edges.push(Edge { from: "web", to: "database", label: "读写" });
        }
        if has_jobs {
            edges.push(Edge { from: "web", to: "queue", label: "enqueue" });
        }

        if has_queue {
            nodes.push(Node { id: "worker", label: "消费队列(jobs/crons 执行体)".into(), kind: NodeKind::Compute });
            edges.push(Edge { from: "worker", to: "queue", label: "消费" });
            if has_db {
                edges.push(Edge { from: "worker", to: "database", label: "读写" });
            }
        }
        if has_cron {
            nodes.push(Node { id: "schedule", label: format!("{} 个定时任务", self.crons.len()), kind: NodeKind::Schedule });
            edges.push(Edge { from: "schedule", to: "queue", label: "定时触发" });
        }

        DeployGraph { nodes, edges }
    }

    /// 打印部署 plan:Application Model(输入)+ 部署 DAG(分层 / 拓扑 provision 顺序 / 环检测)+ DOT + CloudProvider 映射。
    pub fn print_plan(&self) {
        let (mut ssr, mut csr, mut stat) = (0, 0, 0);
        for r in &self.routes {
            match r.strategy {
                Strategy::Ssr => ssr += 1,
                Strategy::Csr => csr += 1,
                Strategy::Static => stat += 1,
            }
        }

        println!("═══════════════════ rui deploy plan ═══════════════════\n");

        // ── Application Model(DAG 的输入:platform! 声明)──
        println!("Application Model(platform! 声明):");
        println!("  Routes: {} 条(ssr {} / csr {} / static {})", self.routes.len(), ssr, csr, stat);
        for r in &self.routes {
            println!("    {:<24} {}", r.pattern, strat_str(r.strategy));
        }
        println!("  GraphQL : {}", if self.graphql { "/graphql" } else { "(无)" });
        println!("  SSE     : {}", if self.sse { "/graphql/subscribe" } else { "(无)" });
        println!("  Database: {}", self.database.as_deref().unwrap_or("(无)"));
        println!("  Jobs    : {}", if self.jobs.is_empty() { "(无)".into() } else { self.jobs.join(", ") });
        if self.crons.is_empty() {
            println!("  Crons   : (无)");
        } else {
            let cs: Vec<String> =
                self.crons.iter().map(|c| format!("{}(every {})", c.name, fmt_dur(c.interval_secs))).collect();
            println!("  Crons   : {}", cs.join(", "));
        }

        // ── 部署 DAG ──
        let g = self.graph();
        println!("\n部署 DAG({} 节点 / {} 边):", g.nodes.len(), g.edges.len());
        for n in &g.nodes {
            println!("    [{}] {}", n.id, n.label);
        }
        println!("  依赖边(from 依赖 to → to 先 provision):");
        for e in &g.edges {
            println!("    {:<9} → {:<9} [{}]", e.from, e.to, e.label);
        }

        match g.topo() {
            Ok(order) => {
                // 分层视图(同层无相互依赖,可并行 provision)。
                let levels = g.levels();
                let maxlvl = levels.values().copied().max().unwrap_or(0);
                println!("  分层(同层可并行):");
                for lvl in 0..=maxlvl {
                    let here: Vec<&str> =
                        g.nodes.iter().filter(|n| levels.get(n.id).copied().unwrap_or(0) == lvl).map(|n| n.id).collect();
                    if !here.is_empty() {
                        let tag = if lvl == 0 { "(叶子,先 provision)" } else { "" };
                        println!("    L{} {} {}", lvl, tag, here.join(", "));
                    }
                }
                // 拓扑 provision 顺序。
                let seq: Vec<&str> = order.iter().map(|n| n.id).collect();
                println!("  ✓ 无环 · provision 顺序(拓扑):{}", seq.join(" → "));
            }
            Err(cycle) => {
                println!("  ✗ 检测到依赖环:{}(无法生成 provision 顺序,请打破环)", cycle);
            }
        }

        // ── Graphviz DOT(可渲染成真图)──
        println!("\nGraphviz DOT(`… | dot -Tpng -o deploy.png` 渲染):");
        for line in g.to_dot().lines() {
            println!("  {}", line);
        }

        // ── CloudProvider 映射(示例 = AWS;真正 provision 是后续)──
        println!("\nCloudProvider 映射(示例 = AWS;本刀只生成 plan,provision 是后续):");
        for n in &g.nodes {
            println!("  {:<9} → {}", n.id, aws_of(n.id));
        }

        println!("\n═══════════════════════════════════════════════════════");
    }
}

/// bin 入口辅助:`cargo run --bin ssr -- plan`(即 `rui plan`)时打印部署 plan 并退出;否则返回继续启动服务。
/// 用法(bin/ssr.rs):`rui::maybe_plan(crate::describe);` 放在 serve 之前(且在连接 DB 之前)。
pub fn maybe_plan(model_fn: impl FnOnce() -> AppModel) {
    if std::env::args().skip(1).any(|a| a == "plan") {
        model_fn().print_plan();
        std::process::exit(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(id: &'static str, kind: NodeKind) -> Node {
        Node { id, label: id.into(), kind }
    }

    #[test]
    fn topo_orders_deps_first() {
        // web 依赖 database + queue;worker 依赖 queue。provision 顺序里 database/queue 必在 web/worker 之前。
        let g = DeployGraph {
            nodes: vec![n("web", NodeKind::Compute), n("worker", NodeKind::Compute), n("database", NodeKind::Database), n("queue", NodeKind::Queue)],
            edges: vec![
                Edge { from: "web", to: "database", label: "" },
                Edge { from: "web", to: "queue", label: "" },
                Edge { from: "worker", to: "queue", label: "" },
            ],
        };
        let order = g.topo().expect("无环");
        let pos = |id: &str| order.iter().position(|x| x.id == id).unwrap();
        assert!(pos("database") < pos("web"));
        assert!(pos("queue") < pos("web"));
        assert!(pos("queue") < pos("worker"));
        // 分层:资源 L0,计算 L1。
        let lv = g.levels();
        assert_eq!(lv["database"], 0);
        assert_eq!(lv["queue"], 0);
        assert_eq!(lv["web"], 1);
    }

    #[test]
    fn topo_detects_cycle() {
        // a → b → a:有环,topo 报错。
        let g = DeployGraph {
            nodes: vec![n("a", NodeKind::Compute), n("b", NodeKind::Compute)],
            edges: vec![Edge { from: "a", to: "b", label: "" }, Edge { from: "b", to: "a", label: "" }],
        };
        assert!(g.topo().is_err());
    }

    #[test]
    fn model_graph_derivation() {
        // 有 db + jobs + crons → 节点含 database/queue/worker/schedule/web/static;边含 web→queue、schedule→queue。
        let m = AppModel {
            routes: vec![RouteNode { pattern: "/".into(), strategy: Strategy::Ssr }],
            graphql: true,
            sse: false,
            database: Some("postgres".into()),
            jobs: vec!["j".into()],
            crons: vec![CronNode { name: "c".into(), interval_secs: 5 }],
        };
        let g = m.graph();
        let ids: Vec<&str> = g.nodes.iter().map(|n| n.id).collect();
        for want in ["database", "queue", "worker", "schedule", "web", "static"] {
            assert!(ids.contains(&want), "缺节点 {want}");
        }
        assert!(g.edges.iter().any(|e| e.from == "schedule" && e.to == "queue"));
        assert!(g.edges.iter().any(|e| e.from == "web" && e.to == "queue"));
        assert!(g.topo().is_ok());
    }
}

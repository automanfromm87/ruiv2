//! rui CLI —— 纯 std,零依赖。
//!   rui init <name>   从内置模板脚手架一个新项目
//!   rui dev           构建 wasm + tailwind(可选)+ 起 SSR 开发服务器
//!   rui build         生产构建(release wasm + minify tailwind)
//!
//! 构建编排复刻原 build.mjs:cargo → wasm → 拷到 web/app.wasm → tailwind → cargo run ssr。
//! 不依赖 bun/node;tailwind 为可选(检测到输入文件且 tailwindcss 可用才跑)。

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("init") => init(args.get(1).map(String::as_str)),
        Some("dev") => build_and_run(false, true),
        Some("build") => build_and_run(true, false),
        _ => {
            eprintln!(
                "rui —— 全栈 Rust 框架 CLI\n\n  rui init <name>   脚手架一个新项目\n  rui dev           构建 + 起 SSR 开发服务器(http://127.0.0.1:8084)\n  rui build         生产构建"
            );
            std::process::exit(1);
        }
    }
}

fn die(msg: impl AsRef<str>) -> ! {
    eprintln!("rui: {}", msg.as_ref());
    std::process::exit(1);
}

fn run_cargo(args: &[&str]) {
    let status = Command::new("cargo")
        .args(args)
        .status()
        .unwrap_or_else(|e| die(format!("无法运行 cargo:{e}")));
    if !status.success() {
        die(format!("cargo {} 失败", args.join(" ")));
    }
}

// ───────────────────────── dev / build ─────────────────────────

fn build_and_run(release: bool, run: bool) {
    if !Path::new("Cargo.toml").exists() {
        die("当前目录没有 Cargo.toml —— 请在 rui 项目根目录运行(或先 rui init)");
    }
    let profile = if release { "release" } else { "debug" };

    // ① cargo → wasm(cdylib)
    let mut a = vec!["build", "--target", "wasm32-unknown-unknown", "--lib"];
    if release {
        a.push("--release");
    }
    println!("· 构建 wasm …");
    run_cargo(&a);

    // ② 拷贝 wasm 产物 → web/app.wasm
    let pkg = package_name();
    let target = find_target_dir();
    let wasm = target
        .join("wasm32-unknown-unknown")
        .join(profile)
        .join(format!("{pkg}.wasm"));
    fs::create_dir_all("web").ok();
    fs::copy(&wasm, "web/app.wasm")
        .unwrap_or_else(|e| die(format!("拷贝 wasm 失败:{} ({e})", wasm.display())));
    println!("· → web/app.wasm");

    // ③ tailwind(可选)
    tailwind(release);

    // ④ 起 SSR 服务器
    if run {
        println!("· 启动 SSR 服务器 …");
        let mut a = vec!["run", "--bin", "ssr"];
        if release {
            a.push("--release");
        }
        run_cargo(&a); // 阻塞
    }
}

/// 从 ./Cargo.toml 读 [package] name(产物名把 - 换成 _)。
fn package_name() -> String {
    let txt = fs::read_to_string("Cargo.toml").unwrap_or_else(|e| die(format!("读 Cargo.toml:{e}")));
    let mut in_package = false;
    for line in txt.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            // 容忍 [ package ] 括号内空白
            in_package = l.trim_start_matches('[').trim_end_matches(']').trim() == "package";
            continue;
        }
        if in_package {
            if let Some((k, v)) = l.split_once('=') {
                if k.trim() == "name" {
                    // 精确匹配 name 键;容忍单/双引号
                    let name = v.trim().trim_matches(|c| c == '"' || c == '\'');
                    return name.replace('-', "_");
                }
            }
        }
    }
    die("Cargo.toml 里找不到 [package] name");
}

/// 向上找含 `target/wasm32-unknown-unknown` 的目录(workspace 的 target 在根)。
fn find_target_dir() -> PathBuf {
    // 优先 CARGO_TARGET_DIR(CI / 共享 target 常用)。
    if let Ok(t) = env::var("CARGO_TARGET_DIR") {
        if !t.is_empty() {
            return PathBuf::from(t);
        }
    }
    let mut d = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    loop {
        let t = d.join("target");
        if t.join("wasm32-unknown-unknown").is_dir() {
            return t;
        }
        if !d.pop() {
            return PathBuf::from("target");
        }
    }
}

fn tailwind(release: bool) {
    let input = ["tailwind.css", "tw-input.css", "styles.in.css"]
        .into_iter()
        .find(|f| Path::new(f).exists());
    let Some(input) = input else {
        // 没有 tailwind 输入 → 写一个空 styles.css 避免 404
        fs::create_dir_all("web").ok();
        let _ = fs::write("web/styles.css", "");
        return;
    };
    let mut a = vec!["-i", input, "-o", "web/styles.css"];
    if release {
        a.push("--minify");
    }
    match Command::new("tailwindcss").args(&a).status() {
        Ok(s) if s.success() => println!("· → web/styles.css"),
        _ => {
            eprintln!("· (跳过 tailwind:未安装 tailwindcss;写入空 styles.css)");
            let _ = fs::write("web/styles.css", "");
        }
    }
}

// ───────────────────────── init ─────────────────────────

fn init(name: Option<&str>) {
    let name = name.unwrap_or_else(|| die("用法:rui init <name>"));
    // 校验合法 crate 名:以字母或 _ 开头、只含字母/数字/-/_、且替换 - 为 _ 后不是单个 _。
    // 首字符不能是 - 或数字(Cargo 包名要求 XID-start);也不能退化成 Rust 通配符 `_`
    // (否则 ssr.rs 里 `<crate>::route` 变成非法的 `_::route`)。
    let crate_name = name.replace('-', "_");
    let first_ok = matches!(name.chars().next(), Some(c) if c.is_ascii_alphabetic() || c == '_');
    let charset_ok = name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if name.is_empty() || !first_ok || !charset_ok || crate_name == "_" {
        die("项目名:以字母或 _ 开头,只含字母 / 数字 / - / _,且不能是单个 _");
    }
    let root = PathBuf::from(name);
    if root.exists() {
        die(format!("目录 {name} 已存在"));
    }
    // 框架 crate 路径(由 CLI 自身位置推断:<ws>/target/<profile>/rui → <ws>/crates)。
    let crates = framework_crates();
    let rui = crates.join("rui");

    let files: &[(&str, String)] = &[
        (
            "Cargo.toml",
            TPL_CARGO
                .replace("{NAME}", name)
                .replace("{RUI}", &rui.display().to_string()),
        ),
        ("tailwind.css", TPL_TAILWIND.to_string()),
        ("src/lib.rs", TPL_LIB.to_string()),
        ("src/bin/ssr.rs", TPL_SSR.replace("{NAME}", &name.replace('-', "_"))),
        // 目录即规范:生成 data/api/view 空骨架(mod.rs 仅注释引导,按需往里填)。
        ("src/data/mod.rs", TPL_DATA_MOD.to_string()),
        ("src/api/mod.rs", TPL_API_MOD.to_string()),
        ("src/view/mod.rs", TPL_VIEW_MOD.to_string()),
    ];
    for (rel, content) in files {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).ok();
        fs::write(&p, content).unwrap_or_else(|e| die(format!("写 {}: {e}", p.display())));
    }
    println!("✓ 已创建 rui 项目 {name}/\n\n  cd {name}\n  rui dev      # → http://127.0.0.1:8084");
}

/// current_exe = <ws>/target/<profile>/rui → 返回 <ws>/crates
fn framework_crates() -> PathBuf {
    env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf)) // <ws>/target/<profile>
        .and_then(|p| p.parent().map(Path::to_path_buf)) // <ws>/target
        .and_then(|p| p.parent().map(Path::to_path_buf)) // <ws>
        .map(|ws| ws.join("crates"))
        .unwrap_or_else(|| PathBuf::from("crates"))
}

// ───────────────────────── 内置模板 ─────────────────────────
//
// 模板是 ../templates/ 下的真实文件(占位符 {NAME}/{RUI} 在 init 里替换),
// 用 include_str! 编进 CLI。这样模板可读、能被编辑器/rust-analyzer 检查,而不是裸塞在源码字符串里。
//
// 注意:模板生成 data/api/view 空目录骨架(mod.rs 只注释引导,不含任何业务实体/页面);
// 数据层由用户按自己业务添加(见各 mod.rs 与 lib.rs 顶部注释)。

const TPL_CARGO: &str = include_str!("../templates/Cargo.toml.tpl");
const TPL_TAILWIND: &str = include_str!("../templates/tailwind.css.tpl");
const TPL_LIB: &str = include_str!("../templates/lib.rs.tpl");
const TPL_SSR: &str = include_str!("../templates/ssr.rs.tpl");
const TPL_DATA_MOD: &str = include_str!("../templates/data_mod.rs.tpl");
const TPL_API_MOD: &str = include_str!("../templates/api_mod.rs.tpl");
const TPL_VIEW_MOD: &str = include_str!("../templates/view_mod.rs.tpl");

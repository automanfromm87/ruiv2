[package]
name = "{NAME}"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib", "rlib"]

[dependencies]
rui = { path = "{RUI}" }

[[bin]]
name = "ssr"
path = "src/bin/ssr.rs"

[profile.release]
opt-level = "s"
lto = true
panic = "abort"

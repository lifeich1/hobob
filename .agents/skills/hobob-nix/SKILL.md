---
name: hobob-nix
description: hobob 的 nix devShell 使用经验与构建环境约束。当需要跑 cargo 构建/测试但找不到工具链、nix develop 行为异常（shellHook 劫持、print-dev-env 副作用）、遇到依赖版本/toolchain 兼容问题、修改 flake.nix、或评估 nix build/vendor 子模块链路时调用。
---

# hobob-nix — devShell 与构建环境经验

## 环境总览

- 本机**直接 shell 没有 cargo**：一切 cargo/cross 命令须在 `nix develop` 的 devShell 内执行。
- `flake.nix` devShell 提供：`cargo`/`rustc`/`rustLibSrc`（来自 `pkgs.rustPlatform`）、`clippy`、`rustfmt`、`pkg-config`、`cargo-watch`、`cargo-edit`，外加 `openssl`/`sqlite`（sqlite 是 v1 遗留，勿依赖）；环境变量 `RUST_SRC_PATH` 供编辑器使用。
- 常用命令（优化后的 flake 可直接跑，无需绕行）：
  ```bash
  cd <repo>
  nix develop -c cargo test -p hobob
  nix develop -c cargo check -p hobob
  nix develop -c cargo run -p hobob -- --port 3731
  ```

## shellHook 经验（坑史与现状）

- **历史坑（2026-09 之前）**：旧 shellHook 会 `exec nvim -S Session.vim`（仓库里有 `Session.vim` 时劫持一切）并 `exec $SHELL`（吞掉 `nix develop -c <cmd>`）；`nix print-dev-env` 的输出末尾还有 `eval "${shellHook:-}"`，直接 source 也会触发 hook。
- **当时的绕行法（现在不再需要，但方法仍有效）**：`nix print-dev-env <repo> > env`，删除末尾 `eval "${shellHook:-}"` 一行后 `source env` 再跑 cargo。
- **现状**：shellHook 已去 exec——自动开 `Session.vim` 仅当 stdin 是 tty（`[ -t 0 ]`）且未设 `HOBOB_NO_SESSION=1`，且用普通调用（非 exec）执行 nvim，退出后回到 shell。交互自动开 nvim 前若系统有 zsh（`command -v zsh` 探测，无则跳过），会把 `SHELL` 指向系统 zsh，使 nvim 内 `:terminal`/`:sh` 默认 shell 为 zsh。因此：
  - `nix develop -c <cmd>` 在仓库根目录**直接可用**；
  - 自动化/agent 环境（非 tty）不会触发 nvim；
  - 交互用户不想自动开 session 时 `HOBOB_NO_SESSION=1 nix develop`。
- **约定**：永远不要在 shellHook 里 `exec`（会吞掉 `-c` 命令）；需要交互便利用「tty 守卫 + 普通调用」。

## 工具链与依赖版本约束（重要）

- devShell 工具链 **rustc/cargo 1.82.0**（`nixpkgs` 是系统 channel 的 path 输入，升级需要 github 可达）。
- 由此锁定：
  - `redb = "2.4"`：redb ≥2.5 需要 rustc 1.85（edition2024）。
  - `bincode = "1.3"`：bincode 2.x 全部需要 rustc 1.85；bincode 1.3 **根函数** `bincode::serialize/deserialize` 为 fixint+小端（`DefaultOptions` 反而是 varint，禁止用）。
  - `clap` 必须开 `env` feature（`--state`/`HOBOB_STATE`）。
  - `serde_json::Value` 不能进 bincode（反序列化 `deserialize_any` 不支持），store 类型里未类型化 JSON 一律存字符串。
- cargo 走 tuna 镜像（`~/.cargo/config.toml`）；github.com 直连在本机环境超时，`nix flake update` 依赖的 github 输入会失败。
- **升级工具链的条件与风险**：需要 github 可达并 `nix flake update`（或改 nixpkgs 输入）；升级 rustc ≥1.85 后可把 bincode 升回 2.x——**但 bincode 1.3 与 2.x 的字节格式不兼容，升级前必须先为 `state.redb` 存量数据安排迁移或接受重来**（M0 无存量数据，成本低）。

## vendor 子模块与 nix build 的已知限制

- `hobob/Cargo.toml` 的 `bilibili-api-rs` 是 path 依赖，指向 `vendor/bilibili-api-rs`（git 子模块，锁 `7df423a`）。新 clone 后必须 `git submodule update --init`；升级 SOP 见 `vendor/UPGRADE.md`。
- **`nix build` 缺 vendor 源码（实测结论）**：nix 的 flake git tree 会**丢弃 gitlink**，`src = ./.` 的 store 拷贝里没有 `vendor/bilibili-api-rs`。已实测不可行的方案：
  - `builtins.fetchGit { url = ./.; submodules = true; }` → 纯求值模式下 `./.` 是 store 拷贝（无 .git），报 "doesn't fetch unlocked input"。
  - `path:vendor/bilibili-api-rs` 相对路径输入 → 同样解析到 flake 的 store 拷贝，不存在该目录。
- **可行出路（二选一，需要时再做）**：① 有网时把上游仓库作为 flake git 输入（`git+https://github.com/lifeich1/bilibili-api-rs?ref=7df423a`）再 copy 进 src；② 放弃子模块，vendor 改真文件快照。目前构建请一律走 devShell 的 cargo；`cross build` 不受影响（容器挂载宿主 checkout，子模块可见）。

## 常见故障速查

| 现象 | 原因/处理 |
| --- | --- |
| `nix develop -c <cmd>` 没执行、蹦出 nvim | 旧 shellHook；现在已修复，若回归请检查 `exec` |
| `cargo` 报 edition2024 未稳定 | 依赖要求 rustc 1.85；降级该依赖（见上方版本约束） |
| `error: failed to download ...` | cargo 镜像/tuna 可达性；github 直连不可用 |
| `DatabaseAlreadyOpen`（redb） | 同一文件有未 drop 的 `Database` 实例，测试里用作用域收口 |
| `prepare_log` 写 `~/log4rs.yml` 失败 | HOME 不可写（沙箱）；验证时用 `HOME=/tmp/xxx` 重跑 |

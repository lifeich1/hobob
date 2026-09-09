# AGENTS.md — hobob 项目导航

B 站 UP 主关注管理 web app（Rust，*WIP*）。被 bibi&lili 踢出的 hobo 自用工具。

## Project

- Rust workspace（`Cargo.toml` members 仅 `hobob`），edition 2021。
- 入口：`hobob/src/main.rs` → `hobob/src/lib.rs`（`prepare_log` + `main_loop`：hub 主循环 + spawn `www` 服务与 `systems::fetch_loop`）。
- 前端：warp + tera 模板 + jQuery（`hobob/templates/`、`hobob/static/`），SSE 事件推送（`/ev/engine`）。

## Commands

- 首次 clone 后：`git submodule update --init`（vendor/bilibili-api-rs 子模块）
- 运行：`cargo run -p hobob -- --port 3731`（默认端口 3731；`--state <PATH>`/`HOBOB_STATE` 指定 v2 状态文件）
- 测试：`cargo test -p hobob`（www.rs 路由 e2e、db 逻辑/持久化 roundtrip、systems fetch 循环与 lua 动态 system、ecs 内核、store.rs redb 存储测试、logkv 日志镜像、libcall lua 桥）
- 交叉编译（部署 ARM 设备）：`cross build --bin hobob -r --target aarch64-unknown-linux-gnu`（容器配置见 `etc/`，需 podman/docker + cross；先确保子模块已 init）
- 部署：scp 产物到 `opi:/lintd/`，用 `hobob_dbgconn restart` 重启
- ⚠️ 以上 cargo/cross 命令须在 `nix develop` 的 devShell 内执行：本机直接 shell 无 `cargo`，`flake.nix` 自带 cargo/rustc/clippy/rustfmt；`nix develop -c` 可直接用（shellHook 已去 exec，自动开 `Session.vim` 仅限交互 tty，可 `HOBOB_NO_SESSION=1` 关闭）。工具链/依赖版本约束与坑详见 skill `hobob-nix`

## Architecture

- `hobob/src/db/`（`db/mod.rs`）：数据中枢 `WeiYuanHui`/`WeiYuan` + `Snapshot`（ECS `world` + `res` 内存索引；mpsc/watch/broadcast 通道 + `#SYSEV#` 动态 system 事件通道）。**实际使用的数据层（精炼导读见 `hobob/src/db/README.md`）。**
- `hobob/src/ecs.rs`：轻量 ECS 内核（`World`/`Storage`/`ptr_eq` 结构共享判定，im::HashMap 底层），无 tokio/store 依赖。
- `hobob/src/store.rs`：v2 持久化（redb + bincode，`state.redb`）：9 张表（meta/systems/ec:* + `kv:log`）+ typed CRUD + `VersionedRecord` 版本信封/迁移钩子 + `VolatileBuffer` 批量 flush + 布局升级链；**M1 起已接管数据路径**（`WeiYuanHui::open` 全量加载、`persist_diff` 直写/stage、close 强刷；稳态零读）。
- `hobob/src/logkv.rs`：M2 起**日志双写**（文件 + `kv:log` KV 镜像，布局升级 2→3）：自定义 log4rs appender（`hobob_kv` kind，target 黑名单挡 `hobob::store`/`hobob::logkv`）+ 全局 sync_channel + hub drain（`run()` 每轮/`close()` last-drain）+ seq 续号 + 条数裁剪（上限 CLI `--log-kv-max`/`HOBOB_LOG_KV_MAX`，默认 20_000）。详见 `.plans/m2-log-kv-appender.md`。
- `vendor/bilibili-api-rs`：git 子模块（上游 `git@github.com:lifeich1/bilibili-api-rs.git`，锁 `7df423a`）；升级 SOP 见 `vendor/UPGRADE.md`。
- `hobob/src/www.rs`：warp 路由（`/op/*` 操作、`/card/*` 渲染、`/ev/engine` SSE）、tera 渲染、boon schema 校验。
- `hobob/src/systems.rs`：基础 system（原 `engine.rs` 迁入）：`fetch_loop` 抓取循环 + 动态 system 框架（`TriggerEvent`/`DynSystemRegistry`：native→lua **两段式分发**、`lib.` 前缀 oneshot 库函数、condition 预编译求值、`set_hook` 指令上限超时；内置 `builtin.tick`）；事件经 db `#SYSEV#` 通道上报，hub 分发。
- `hobob/src/libcall.rs`：lua↔Rust 桥（M3 T2/T3）：mlua 沙箱 + `ctx.admin`（直接改 `&mut Snapshot`，含 `register_system`/`unregister_system`/`reload_system`/`reload_all`）/`ctx.bapi`（`spawn_blocking` 同步桥 + 本地 5s 兜底超时）。lua 回调**同步跑在 hub 线程**（不开 mlua `send` feature，超时靠指令上限；D6 ② 的 `spawn_blocking` 隔离已按决议弃用，见 `.plans/m3-mlua-dynsys.md`）。
- `hobob/src/data_schema.rs`：JSON schema 校验，schema 从远程 `https://lintd.xyz/hobob/*.json` 加载（离线 panic）。
- `hobob_dbgconn/`：tarpc RPC 工具，远程 alive/restart hobob（端口 21321）。
- `monkey/`：油猴脚本（已过时，调用的 `/get/user`、`/op/setliveurl` 路由当前不存在）。

## Conventions

- 错误处理 `anyhow::Result`；日志 `log` + `log4rs`（配置 `~/log4rs.yml`，模板 `hobob/assets/log4rs.yml`）。
- CLI 参数用 `clap`（derive）。
- 状态修改必须走 `WeiYuan::apply/update` 通道，勿直接改 bench。
- debug 构建模板从磁盘 `templates/` 加载（工作目录须为 `hobob/`），release 编译期内嵌。

## Context/Token 规范（文档分层导航）

读（先索引、后深挖，按需加载）：
- 4 层导航：本文件（常驻索引）→ 根 `README.md`（目录导航表 + 跨目录关键事实 + 构建运行）→ 各目录 `README.md`（模块导航：符号表/谁在用/坑/测试）→ `.agents/skills/hobob-*.md`（lib.rs 启动、nix 工具链专项，`run_skill` 按需加载，勿预读）。
- 源码最后读、只读片段：按 README 符号表行号（如 `Snapshot` ~196）用 read_file offset/limit 定位；禁止通读大文件（`db/mod.rs` ~2.5k 行、`store.rs` ~1.9k 行、`www.rs` ~660 行——先读对应 README 小节）。
- `target/` 勿读；标「已弃用/遗留」的（`xtask/`、`monkey/`、`assets/db_init.sql`）勿据此推断现状；坑与测试入口 README 已集中列，勿重读测试源码。

写（事实按层归属，不跨层复制）：
- 跨目录 → 根 README「关键事实」；单模块 → 目录 README；深度背景 → skill；高频常驻 → 本文件。
- 目录 README 固定骨架：一句话职责 → 表格（路径/符号/路由，带行号）→ 坑 → 测试入口；用表格不用散文。
- 新符号/新坑/新路由同步写入 README 表格（行号漂移时更新）；过期条目标「已弃用/遗留」。
- 本文件只写「做什么 + 去哪看」，不搬 README 正文。

## Notes

（待补充）

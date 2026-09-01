# AGENTS.md — hobob 项目导航

B 站 UP 主关注管理 web app（Rust，*WIP*）。被 bibi&lili 踢出的 hobo 自用工具。

## Project

- Rust workspace（`Cargo.toml` members 仅 `hobob`），edition 2021。
- 入口：`hobob/src/main.rs` → `hobob/src/lib.rs`（`prepare_log` + `main_loop`：spawn `www` 服务与 `engine` 循环）。
- 前端：warp + tera 模板 + jQuery（`hobob/templates/`、`hobob/static/`），SSE 事件推送（`/ev/engine`）。

## Commands

- 运行：`cargo run -p hobob -- --port 3731`（默认端口 3731）
- 测试：`cargo test -p hobob`（www.rs 路由端到端测试、db.rs 逻辑测试、chunk 解析器测试）
- 交叉编译（部署 ARM 设备）：`cross build --bin hobob -r --target aarch64-unknown-linux-gnu`（容器配置见 `etc/`，需 podman/docker + cross）
- 部署：scp 产物到 `opi:/lintd/`，用 `hobob_dbgconn restart` 重启
- ⚠️ 本机当前无 `cargo`，且外部 path 依赖 `../../bilibili-api-rs` 缺失——无法在本机构建/测试

## Architecture

- `hobob/src/db.rs`：数据中枢 `WeiYuanHui`/`WeiYuan`/`FullBench`（im 不可变结构 + mpsc/watch/broadcast 通道 + 落盘 `~/bench.json`）。**实际使用的数据层。**
- `hobob/src/www.rs`：warp 路由（`/op/*` 操作、`/card/*` 渲染、`/ev/engine` SSE）、tera 渲染、boon schema 校验。
- `hobob/src/engine.rs`：后台循环，消费 `commands`（目前仅 `fetch`），用 `bilibili-api-rs` 抓取，bucket 速率控制。
- `hobob/src/data_schema.rs`：JSON schema 校验，schema 从远程 `https://lintd.xyz/hobob/*.json` 加载（离线 panic）。
- `hobob/src/chunk.rs` + `chunkir.lalrpop`：Chunk AST + 解析器（测试用）。
- `hobob/src/vm.rs`、`bench.rs`：未完成实验（`todo!()`），勿依赖。
- `hobob_dbgconn/`：tarpc RPC 工具，远程 alive/restart hobob（端口 21321）。
- `monkey/`：油猴脚本（已过时，调用的 `/get/user`、`/op/setliveurl` 路由当前不存在）。

## Conventions

- 错误处理 `anyhow::Result`；日志 `log` + `log4rs`（配置 `~/log4rs.yml`，模板 `hobob/assets/log4rs.yml`）。
- CLI 参数用 `clap`（derive）。
- 状态修改必须走 `WeiYuan::apply/update` 通道，勿直接改 bench。
- debug 构建模板从磁盘 `templates/` 加载（工作目录须为 `hobob/`），release 编译期内嵌。

## Docs（按需深挖，勿重读源码）

- 各业务目录均有 `README.md`（导航/模块详述/坑）；深度专项见 `.agents/skills/hobob-lib/SKILL.md`（`run_skill` 调用）。
- 跨目录事实、部署链路见根 `README.md`。

## Notes

（待补充）

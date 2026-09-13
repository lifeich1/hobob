---
name: hobob-lib
description: 理解 hobob/src/lib.rs（crate 入口）的结构、启动流程与宏定义。当需要修改 hobob 的启动逻辑、日志初始化、CLI 参数、vpath!/schema_uri! 宏，或梳理 main_loop 中 www/systems 任务的启动与优雅关闭时序时调用。
---

# hobob/src/lib.rs

## 概述
`hobob/src/lib.rs`（172 行）是 crate 入口：日志初始化、模块声明、CLI 解析与 `main_loop` 启动编排。

## 主要功能
1. **日志初始化**：`prepare_log()` 创建 `~/` 目录，`~/log4rs.yml` 不存在则从 `assets/log4rs.yml` 复制；注册自定义 appender kind `hobob_kv`（`logkv::KvAppenderDeserializer`）后 `log4rs::init_file`；用户配置缺 `hobob_kv` 时 `log::warn`（KV 镜像静默降级，文件日志不受影响）。
2. **模块声明**：`data_schema`、`db`、`ecs`、`libcall`、`logkv`、`store`、`systems`、`www`（`chunk`/`chunkir`/`vm`/`bench` 已于 M3 T1 删除，`engine.rs` 于 M1 删除）。
3. **命令行参数**：`Flags`（clap derive）——`--port`（默认 3731）、`--state`/`HOBOB_STATE`（状态文件路径）、`--log-kv-max`/`HOBOB_LOG_KV_MAX`（KV 日志留存上限，默认 20_000）。
4. **主循环与任务启动**：`Store::open_or_create` + `WeiYuanHui::open`（全量加载，失败即退出）→ `attach_logkv()` → hub 循环 `run()` 与 tick/drain 编排；Ctrl+C 调 `center.close()`（停机强刷 store）并最多等 30s 优雅退出。

## 核心结构体
### Flags
- `port`: HTTP 服务器端口号，默认 3731
- `state` / `log_kv_max`: 状态文件路径与 KV 日志留存上限

### 主要函数
1. `prepare_log()`：初始化日志系统（创建目录 + 复制模板 + 注册 `hobob_kv`）。
2. `main_loop()`：`Store::open_or_create` + `WeiYuanHui::open`（全量加载 state.redb）→ `attach_logkv()` → 分派 `www::build_app` 与 `systems::fetch_loop` → hub 主循环 → Ctrl+C 优雅关闭。

### 宏
- `vpath!`：运行时文件路径（`~/`、`~/log4rs.yml`），基于 `std::env::home_dir()`
- `schema_uri!`：远程 JSON schema URL（`https://lintd.xyz/hobob/{id}.json`）

## 第三方依赖
- `anyhow`：错误处理
- `clap`：命令行参数解析
- `log` + `log4rs`：日志系统
- `tokio`：异步运行时
- `warp`：Web 服务器框架
- `mlua`：lua 沙箱（libcall，M3）
- `bilibili-api-rs`（vendor 子模块）：B 站 API

## 入口链
`src/main.rs`（bin）→ `prepare_log()` → `main_loop()`；`main_loop` 内部 spawn `www` 服务与 `systems::fetch_loop`，二者共享 `WeiYuanHui` 的 chair 句柄。

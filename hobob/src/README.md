# hobob/src — 模块详述

本目录是 hobob 的全部 Rust 源码。以下按文件说明职责与关键实现，agent 处理具体问题时先看对应小节，避免通读全文。

## `lib.rs` — crate 入口（140 行）

- `prepare_log()`：创建 `~/`（`home_dir`）下日志目录，若 `~/log4rs.yml` 不存在则从 `assets/log4rs.yml` 复制并初始化 log4rs。
- `main_loop()`：解析 `Flags`（`--port`，默认 3731）→ `WeiYuanHui::load(~/"bench.json")` → 并行 spawn 两个任务：
  - `www::build_app` 的 warp 服务（Ctrl+C / `chair.close` 触发 graceful shutdown）
  - `engine::main_loop`
  - 等待 `ctrl_c`，调用 `center.close()` 并最多等 30s 优雅退出。
- 宏：`vpath!`（运行时文件路径：`~/`、`~/log4rs.yml`、`~/bench.json`）、`schema_uri!`（`https://lintd.xyz/hobob/{id}.json` 远程 schema URL）。
- 模块声明：`bench`、`data_schema`、`db`、`engine`、`vm`、`www`、`chunk` + lalrpop 生成的 `chunkir`。
- 二进制入口是 `src/main.rs`（仅调用 `prepare_log` + `main_loop`）。

## `db/` — 数据中枢（`db/mod.rs` 1794 行，核心）

数据中枢 `WeiYuanHui`/`WeiYuan` + `Snapshot`（ECS `world` 组件 + `res` 内存索引），mpsc 提交（`ptr_eq` 冲突校验）/ watch 发布 / broadcast 事件。**db 已目录化，精炼导读见 `db/README.md`（架构要点、符号表、谁在用、坑、测试）；处理数据层问题先读它定位到符号，按需深挖，勿通读源码。**

## `www.rs` — HTTP 层（661 行）

- `TERA`（lazy_static）：debug 从 `templates/**/*.html` 磁盘加载；release 用 `include_str!` 内嵌 4 个模板。
- 渲染管线：`render(page, Result<Value>)` → tera 渲染，失败统一进 `failure.html`。
- 校验：所有出站数据过 `ChairData::checker(schema_uri!(...))`（boon，见 `data_schema.rs`）。
- 路由见 `hobob/README.md` 路由表；实现要点：`simpleapi()`（POST JSON 解析，16KB 上限，非法即 `UnparsableQuery` reject）、`create_op/do_api`（把 db 层方法包装成 API）、`route_card`（三组卡片渲染）。
- SSE：`/ev/engine` 用 `BroadcastStream` 转发 events，`Lagged` 时发 comment 提示。
- tests：warp::test 全路由端到端测试（`test_op_*`、`test_card_*`、`test_sse`）。

## `engine.rs` — 后台引擎（362 行）

- `main_loop`：`while let Ok(bench) = runner.recv()` → 有命令则 `take_cmds`（带长度校验的原子取走）逐个 `exec_cmd`；无命令则 `exec_timers`（取 `up_index.ctime` 最旧 uid 补 `fetch`，`bucket_hang`）。之后按 `bucket_duration_to_next` 设 deadline 睡眠等待 `runner.changed()`。
- `exec_cmd`：目前仅实现 `fetch`（`livelist` 是 `todo!()`）。
- `do_fetch`：`bilibili-api-rs` 的 `user(uid).info()` + `latest_videos()`；失败时 `bucket_double_gap`；成功时 `pick_basic/pick_live/pick_video` 提取精简字段写入 `up_info[uid].pick`，`raw` 存完整响应，`bucket_good`。
- 注意：`Client::new()` 直接使用 bilibili-api-rs 默认凭据，无持久化登录态。

## `data_schema.rs` — JSON schema（356 行）

- `ChairData`（单例 `CHAIR_DATA_SCHEMA`）：用 `boon::Compiler` 编译远程 schema（`schema_uri!` → `https://lintd.xyz/hobob/...`，Draft 2020-12），`checker(uri)` 返回校验闭包。
- schema 集合：`utils/ts`、`log`、`runtime/bucket`、`runtime/db` 等（`new()` 中逐个注册）。
- **坑**：构建时即请求远程 URL，网络不可达直接 panic；离线开发需改 `schema_uri!` 或本地起服务。

## `chunk.rs` + `chunkir.lalrpop` — Chunk 语言（64 行 + 语法文件）

- `Chunk`/`Expr`/`CondExpr`/`Reg`：一种小 AST（`FetchUp`、`FetchRandLive`、`Set/Get`、`Print`、`Extract`、`If` 等），serde 序列化。
- `chunkir.lalrpop`：lalrpop 语法文件，`build.rs` 编译生成 `chunkir` 解析器。
- tests：`test_data/chunk_00*.in.txt` 解析结果与 `*.expect.json` 比对（`chunk_003/004` 暂无 expect，未启用）。
- 当前用途：仅 `show_expect_value` bin 与测试使用；疑似未来"脚本化刷新规则"的设计。

## `vm.rs` + `bench.rs` — 未完成实验（勿依赖）

- `vm.rs`：`Machine`（bench + watch + trunk 通道 + 定时器），`infer/step` 均为 `todo!()`。
- `bench.rs`：另一套 `Bench` 实现——文件系统式 `DNode`（`Plain`/`Dir`/`Index`），`pull_log` 等为 `todo!()`。
- 两者未被 `db` 模块使用，历史遗留设计。

## `bin/show_expect_value.rs`

打印若干 `Chunk` 示例的 pretty JSON（`=== sample N ===`），供人工对照解析器输出。

## 测试数据 `test_data/`

`chunk_00*.in.txt`（chunk 语言源码）+ `chunk_00*.expect.json`（期望 AST）。改解析器时同步更新 expect。

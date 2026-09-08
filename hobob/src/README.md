# hobob/src — 模块详述

本目录是 hobob 的全部 Rust 源码。以下按文件说明职责与关键实现，agent 处理具体问题时先看对应小节，避免通读全文。

## `lib.rs` — crate 入口（162 行）

- `prepare_log()`：创建 `~/`（`home_dir`）下日志目录，若 `~/log4rs.yml` 不存在则从 `assets/log4rs.yml` 复制并初始化 log4rs。
- `main_loop()`：解析 `Flags`（`--port` 默认 3731、`--state`/`HOBOB_STATE` 指定状态文件路径）→ `Store::open_or_create` + `WeiYuanHui::open`（全量加载，失败即退出）→ 分派两个 chair 任务并跑 hub 主循环：
  - `www::build_app` 的 warp 服务（Ctrl+C / closing 标志触发 graceful shutdown）
  - `systems::fetch_loop`（后台抓取循环）
  - hub 主循环（tokio::select ctrl_c / `center.run()`）消费 chair 提交 + 每轮 maybe_flush；Ctrl+C 调 `center.close()`（停机强刷 store）并最多等 30s 优雅退出。
- 宏：`vpath!`（运行时文件路径：`~/`、`~/log4rs.yml`）、`schema_uri!`（`https://lintd.xyz/hobob/{id}.json` 远程 schema URL）。
- 模块声明：`bench`、`data_schema`、`db`、`ecs`、`store`、`systems`、`vm`、`www`、`chunk` + lalrpop 生成的 `chunkir`（M1 不再有 `engine.rs`）。
- 二进制入口是 `src/main.rs`（仅调用 `prepare_log` + `main_loop`）。

## `db/` — 数据中枢（`db/mod.rs` 2532 行，核心）

数据中枢 `WeiYuanHui`/`WeiYuan` + `Snapshot`（ECS `world` 组件 + `res` 内存索引），mpsc 提交（`ptr_eq` 冲突校验）/ watch 发布 / broadcast 事件 / `#SYSEV#` 动态 system 事件通道；持久化桥接（`open` 全量加载、`persist_diff` 直写/stage、close 强刷）。**db 已目录化，精炼导读见 `db/README.md`（架构要点、符号表、谁在用、坑、测试）；处理数据层问题先读它定位到符号，按需深挖，勿通读源码。**

## `ecs.rs` — 轻量 ECS 内核（550 行）

`Entity`(u64)/`Component`/`Storage<T>`（im::HashMap，结构共享）/`World`：`spawn`/`spawn_at`/`insert`/`remove`/`get`/`get_mut`（CoW）/`iter`/`iter2` + `ptr_eq`（world 结构未变判定）。纯数据结构，无 tokio/store 依赖。

## `www.rs` — HTTP 层（660 行）

- `TERA`（lazy_static）：debug 从 `templates/**/*.html` 磁盘加载；release 用 `include_str!` 内嵌 4 个模板。
- 渲染管线：`render(page, Result<Value>)` → tera 渲染，失败统一进 `failure.html`。
- 校验：所有出站数据过 `ChairData::checker(schema_uri!(...))`（boon，见 `data_schema.rs`）。
- 路由见 `hobob/README.md` 路由表；实现要点：`simpleapi()`（POST JSON 解析，16KB 上限，非法即 `UnparsableQuery` reject）、`create_op/do_api`（把 db 层方法包装成 API）、`route_card`（三组卡片渲染）。
- SSE：`/ev/engine` 用 `BroadcastStream` 转发 events，`Lagged` 时发 comment 提示。
- tests：warp::test 全路由端到端测试（`test_op_*`、`test_card_*`、`test_sse`）。

## `systems.rs` — 基础 system（491 行，原 `engine.rs`）

- `fetch_loop`：`while let Ok(bench) = runner.recv()` → 有命令则 `take_cmds`（带长度校验的原子取走）逐个 `exec_cmd`；无命令则发 `Tick` 事件（hub 分发内置 `builtin.tick` 补抓，见下）。之后按 `bucket_duration_to_next` 设 deadline 睡眠等待 `runner.changed()`。
- `exec_cmd`：目前仅实现 `fetch`（`livelist` 是 `todo!()`）。
- `do_fetch`：`bilibili-api-rs` 的 `user(uid).info()` + `latest_videos()`；失败时 `bucket_double_gap` + 上报 `FetchFailed` 事件；成功时 `apply_fetch` 组件写回 + `bucket_good` + 上报 `FetchDone` 事件（组件字段 → `db` 组件，`raw` 内存 only）。
- 动态 system 框架：`TriggerEvent`（Tick/FetchDone/FetchFailed/UpStateChanged）、`DynSystemRegistry`（hub 持有，按名序分发、handler 报错记日志继续）、内置 `builtin.tick`（原 `exec_timers`：commands 空时取 `up_index.ctime` 最旧 uid 补 `fetch` + `bucket_hang`）。store `systems` 表不加载（lua/condition 语法 M3 定）。
- 注意：`Client::new()` 直接使用 bilibili-api-rs 默认凭据，无持久化登录态。

## `store.rs` — redb 持久化（1880 行）

redb + bincode：8 张表（`meta`/`systems`/`ec:brick`/`ec:video_post`/`ec:live_post`/`ec:comment_post`/`ec:runtime`/`ec:group`）+ typed CRUD + `VersionedRecord` 版本信封/迁移钩子（brick V1→V2）+ `VolatileBuffer` 批量 flush（256 条/5s）+ 布局升级链（schema_version 1→2）+ 全量 `list_*` 加载 API + `StoreStats` commit 埋点。**M1 起接管数据路径**：启动全量加载（`WeiYuanHui::open`）、运行期 `persist_diff` 直写/stage、close 强刷；稳态零 redb 读。

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

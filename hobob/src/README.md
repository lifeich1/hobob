# hobob/src — 模块详述

本目录是 hobob 的全部 Rust 源码。以下按文件说明职责与关键实现，agent 处理具体问题时先看对应小节，避免通读全文。

## `lib.rs` — crate 入口（172 行）

- `prepare_log()`：创建 `~/`（`home_dir`）下日志目录，若 `~/log4rs.yml` 不存在则从 `assets/log4rs.yml` 复制并初始化 log4rs。
- `main_loop()`：解析 `Flags`（`--port` 默认 3731、`--state`/`HOBOB_STATE`、`--log-kv-max`、`--heartbeat-interval`/`HOBOB_HEARTBEAT_INTERVAL`（秒，0 = 禁用心跳，**默认 0**——watchdog T2/T3 落地前默认禁用，避免对空 bus 持续告警）、`--bus`/`HOBOB_BUS`（D-Bus，默认 system））→ `Store::open_or_create` + `WeiYuanHui::open`（全量加载，失败即退出）→ 分派两个 chair 任务并跑 hub 主循环：
  - `www::build_app` 的 warp 服务（Ctrl+C / closing 标志触发 graceful shutdown）
  - `systems::fetch_loop`（后台抓取循环）
  - hub 主循环（tokio::select：1s tick touch 活跃度 / ctrl_c+SIGTERM / `center.run()`）消费 chair 提交 + 每轮 maybe_flush；Ctrl+C/SIGTERM **先发 D-Bus `Bye`（心跳启用时）再调 `center.close()`**（停机强刷 store）并最多等 30s 优雅退出。
- 宏：`vpath!`（运行时文件路径：`~/`、`~/log4rs.yml`）、`schema_uri!`（`https://lintd.xyz/hobob/{id}.json` 远程 schema URL）。
- 模块声明：`data_schema`、`db`、`ecs`、`keepalive`、`libcall`、`logkv`、`store`、`systems`、`www`（`chunk`/`chunkir`/`vm`/`bench` 已在 M3 T1 删除，`engine.rs` 在 M1 删除）。
- 二进制入口是 `src/main.rs`（仅调用 `prepare_log` + `main_loop`）。

## `keepalive.rs` — 保活（M4/T1：活跃度计数器 + zbus 心跳 client，协议常量 + proxy + 单测）

- **坑：tick 是活跃度唯一证据**——hub `run()` 空闲时阻塞在 `updates.recv().await` 无轮次，活跃度只能靠 `main_loop` select! 的 1s tick `Liveness::touch`；心跳任务独立 spawn 只读不碰 hub。
- **坑：seq 是单调毫秒非墙钟**——`Liveness` 值 = 进程启动 `Instant` 起 elapsed；NTP 回拨会让墙钟 seq 变小，无 RTC 的 ARM 设备会误判停滞（D4）。
- **坑：SIGTERM 必须并入 shutdown**——`shutdown_signal()` 同时捕获 ctrl_c 与 SIGTERM；watchdog 拉起流程先发 SIGTERM，不捕获会绕过停机强刷（§4.3 双信号问题）。
- **坑：Bye 先于 `close()`**——强刷期间 tick 已停、seq 冻结，若等 `close()` 返回再发，`close()` 耗时接近 `stall_timeout`(6s) 时 watchdog 会对优雅停机中的 hobob 拉起（D7）。
- **坑：fetch_loop 独立探活（D13）**——fetch_loop 持独立 `Liveness` 每轮 `touch`，随心跳上报 fetch_seq；hub 与 fetch 两路 seq 各自独立判停滞，任一冻结即拉起。
- 符号：`Bus`（system/session，clap ValueEnum，默认 system，两侧必须一致）；`protocol`（`WATCHDOG_NAME`/`WATCHDOG_PATH`/`WATCHDOG_IFACE`、`DEFAULT_INTERVAL`=2s、`TICK`=1s、`RPC_TIMEOUT`=2s、`RECONNECT_MIN/MAX`=1s/30s——watchdog 侧（T2/T3）同名实现，以 M4 D3/D8 为准）；`Liveness`（`new`/`touch`/`get`，Arc&lt;AtomicU64&gt; 单调毫秒）；`enabled(interval)`（interval=0 禁用）；`run_heartbeat(liveness, fetch_liveness, bus, interval)`（连接/调用失败退避重连 1s→30s 不 fatal，backoff 仅 heartbeat 成功才重置，单轮调用包 RPC_TIMEOUT）；`send_bye(bus, reason)`（发不出仅 log，整包超时兜底）。
- proxy：`Watchdog1` trait（`heartbeat(hub_seq, fetch_seq)`（D13 双 seq）/`bye(reason)`），目标 `xyz.lintd.hobob.Watchdog` @ `/xyz/lintd/hobob/Watchdog`，interface `Watchdog1`。
- 测试：`cargo test -p hobob keepalive`（Liveness 单调/clone 共享/实例独立、bus 默认、interval=0 禁用）。

## `db/` — 数据中枢（`db/mod.rs` 3094 行，核心）

数据中枢 `WeiYuanHui`/`WeiYuan` + `Snapshot`（ECS `world` 组件 + `res` 内存索引），mpsc 提交（`ptr_eq` 冲突校验）/ watch 发布 / broadcast 事件 / `#SYSEV#` 动态 system 事件通道；持久化桥接（`open` 全量加载、`persist_diff` 直写/stage、close 强刷）。**db 已目录化，精炼导读见 `db/README.md`（架构要点、符号表、谁在用、坑、测试）；处理数据层问题先读它定位到符号，按需深挖，勿通读源码。**

## `ecs.rs` — 轻量 ECS 内核（550 行）

`Entity`(u64)/`Component`/`Storage<T>`（im::HashMap，结构共享）/`World`：`spawn`/`spawn_at`/`insert`/`remove`/`get`/`get_mut`（CoW）/`iter`/`iter2` + `ptr_eq`（world 结构未变判定）。纯数据结构，无 tokio/store 依赖。

## `www.rs` — HTTP 层（794 行）

- `TERA`（lazy_static）：debug 从 `templates/**/*.html` 磁盘加载；release 用 `include_str!` 内嵌 4 个模板。
- 渲染管线：`render(page, Result<Value>)` → tera 渲染，失败统一进 `failure.html`。
- 校验：所有出站数据过 `ChairData::checker(schema_uri!(...))`（boon，见 `data_schema.rs`）。
- 路由见 `hobob/README.md` 路由表；实现要点：`simpleapi()`（POST JSON 解析，16KB 上限，非法即 `UnparsableQuery` reject）、`create_op/do_api`（把 db 层方法包装成 API）、`route_card`（三组卡片渲染）。
- SSE：`/ev/engine` 用 `BroadcastStream` 转发 events，`Lagged` 时发 comment 提示。
- tests：warp::test 全路由端到端测试（`test_op_*`、`test_card_*`、`test_sse`）。

## `libcall.rs` — libcall 接口（mlua 桥，M3 T2/T3，994 行）

lua 与 Rust 能力的桥梁，两组注册进每次 call 新建的 `ctx` 表（不进 `_G`，计划 D14）。

- 沙箱：`new_sandbox()` = `StdLib::ALL_SAFE ^ (IO|OS|PACKAGE)`（DEBUG 本就不在 `ALL_SAFE`；`^` 等价差集的前提由 `test_sandbox_stdlib_mask_premise` 钉住，mlua 未实现 `Not`/`Sub`）+ 全局 `json.encode/decode`。**T5 补漏**：`loadfile`/`dofile` 属 base 库、`StdLib` 掩码去不掉且能读盘，故显式置 nil（`load` 保留）。
- admin（直接改 `&mut Snapshot`，失败返回 `false`/`nil` + `ctx.admin.last_error()`）：`follow`/`unfollow`/`toggle_group`/`new_group`/`refresh`/`get_state`/`set_silent`/`register_system`/`unregister_system`/`reload_system`/`reload_all`。负 uid/gid 由 `valid_arg` 在 libcall 层拦截（schema 侧 `minimum: 0` 兜底）；`register_system` 经 registry 句柄编译注册 + 落盘（`lib.` 前缀 = oneshot、condition 校验在 registry 侧）；`set_silent` 恒返回 false——`force_silence` 目前只上报 Silenced 事件，真实静音语义待 T6。
- bapi（`spawn_blocking` + 新 current-thread runtime 同步桥，`BAPI_TIMEOUT` 本地 5s 兜底，失败返回 `nil, err_msg`）：`info`/`latest_videos`/`recent_posts`/`card`/`live_info`/`xlive_recommend`。
- 执行模型（T3）：`build_ctx_table(lua, scope, snap, Option<&Store>, Option<DynSystemRegistry>)`，两个 `None` 时对应能力拒绝并写 `last_error`（不给假成功）；lua 回调**同步跑在 hub 线程**（不开 mlua `send`），超时靠 registry 侧 `set_hook` 指令上限（D6 ② 已按决议弃用，见 `.plans/m3-mlua-dynsys.md`）。
- tests：19 个（admin 操作/负参数拒绝/set_silent 未实现/json 编解码/沙箱拒绝与掩码前提/标准库可用/bapi 签名与同步桥三例/register_system 编译注册与 oneshot 校验）。

## `systems.rs` — 基础 system（1607 行，原 `engine.rs`）

- `fetch_loop`：`while let Ok(bench) = runner.recv()` → 有命令则 `take_cmds`（带长度校验的原子取走）逐个 `exec_cmd`；无命令则发 `Tick` 事件（hub 分发内置 `builtin.tick` 补抓，见下）。之后按 `bucket_duration_to_next` 设 deadline 睡眠等待 `runner.changed()`。
- `exec_cmd`：目前仅实现 `fetch`（`livelist` 是 `todo!()`）。
- `do_fetch`：`bilibili-api-rs` 的 `user(uid).info()` + `latest_videos()`；失败时 `bucket_double_gap` + 上报 `FetchFailed` 事件；成功时 `apply_fetch` 组件写回 + `bucket_good` + 上报 `FetchDone` 事件（组件字段 → `db` 组件，`raw` 内存 only）。
- 动态 system 框架（M3 T3）：`TriggerEvent`（Tick/FetchDone/FetchFailed/UpStateChanged，含 `uid`）、`DynSystemRegistry`（`Rc<RefCell<DynInner>>` 句柄，hub 持有；**两段式** native→lua 分发，各自名序、handler 报错记日志继续）。lua system 在 `WeiYuanHui::open` 从 store `systems` 表加载（D3）：`lib.` 前缀 = oneshot（加载期执行一次、返回 table 存 `_lib.<短名>`，D16），其余 = 事件响应型（`condition` 预编译为 `function(event) return (<expr>) end`，`""`/`"always"` 恒真，D15）。超时靠 `set_hook` 指令上限（`LUA_INSTRUCTION_LIMIT` 1M ≈ 50ms；condition 100K）；回调报错/中断后探针实例可用性（D13：可复用则仅记日志，不可用且有 store 则全量重建）。内置 `builtin.tick`（原 `exec_timers`：commands 空时取 `up_index.ctime` 最旧 uid 补 `fetch` + `bucket_hang`）。
- 注意：`Client::new()` 直接使用 bilibili-api-rs 默认凭据，无持久化登录态。
- 分发语义：dispatch 先收集 spec 再执行，故**本事件中途注册/注销/热加载的 system 从下一个事件起生效**（同一事件的 condition/回调看不到中途变更）。
- 修复新增符号（T3 复查）：`unregister(name)`（统一 event/oneshot 注销）、`saved_state`/`restore_state`（落盘失败回滚**旧版本**而非删除）、`hook_active` + `HookGuard`（嵌套 `guarded_call` 复用外层指令上限）、`readonly_event_copy`（每 spec 一份只读 event 副本）。

## `store.rs` — redb 持久化（2514 行）

redb + bincode：9 张表（`meta`/`systems`/`ec:brick`/`ec:video_post`/`ec:live_post`/`ec:comment_post`/`ec:runtime`/`ec:group` + `kv:log`）+ typed CRUD + `VersionedRecord` 版本信封/迁移钩子（brick V1→V2）+ `VolatileBuffer` 批量 flush（256 条/5s）+ 布局升级链（schema_version 1→2）+ 全量 `list_*` 加载 API + `StoreStats` commit 埋点。**M1 起接管数据路径**：启动全量加载（`WeiYuanHui::open`）、运行期 `persist_diff` 直写/stage、close 强刷；稳态零 redb 读。

## `data_schema.rs` — JSON schema（358 行）

- `ChairData`（单例 `CHAIR_DATA_SCHEMA`）：用 `boon::Compiler` 编译远程 schema（`schema_uri!` → `https://lintd.xyz/hobob/...`，Draft 2020-12），`checker(uri)` 返回校验闭包。
- schema 集合：`utils/ts`、`log`、`runtime/bucket`、`runtime/db` 等（`new()` 中逐个注册）。
- **坑**：构建时即请求远程 URL，网络不可达直接 panic；离线开发需改 `schema_uri!` 或本地起服务。

## 已删除（M3 T1）

`chunk.rs`、`chunkir.lalrpop`、`build.rs`（lalrpop）、`vm.rs`、`bench.rs`、`bin/show_expect_value.rs`、`test_data/`（chunk 用例）均已删除；`Cargo.toml` 中 `lalrpop`/`lalrpop-util` 移除，新增 `mlua`。

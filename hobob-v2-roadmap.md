# hobob 2.0 实现路线大纲（DRAFT）

> 来源：[DRAFT 001: hobob 2.0 设计](https://www.kdocs.cn/l/cp4PIcdqwvaC)

## 1. 现状基线（v1）

| 方面 | 现状 |
| --- | --- |
| 前端 | warp + tera 模板 + jQuery，SSE（`/ev/engine`）推送 |
| 数据层 | `db.rs`：`WeiYuanHui`/`WeiYuan`/`FullBench`，im 不可变结构 + mpsc/watch/broadcast 通道，落盘 `~/bench.json` |
| 引擎 | `engine.rs` 后台循环消费 commands（fetch），bilibili-api-rs 抓取，bucket 速率控制 |
| 校验 | `data_schema.rs`：boon schema 从 `https://lintd.xyz/hobob/` 远程加载（离线 panic） |
| 实验代码 | `chunk.rs`（Chunk AST + lalrpop 解析器）、`vm.rs`、`bench.rs`（`todo!()`） |
| 运维 | `hobob_dbgconn`（tarpc RPC 远程 alive/restart），交叉编译 aarch64 部署 `opi:/lintd/` |
| 已知痛点 | 单进程紧耦合；JSON 全量落盘；远程 schema 离线即挂；SSE 单向推送 |

## 2. 目标架构（v2）

**需求清单**：动态超控（mlua）、ECS 设计、保活（心跳 + watchdog）、日志系统、界面。

1. **ECS**：自研轻量 ECS（借鉴 bevy_ecs 风格）。初始化从 redb + bincode 读取，component 变化写回 redb；**同进程读取（www 渲染、libcall、lua）走内存 ECS world（权威数据源），redb 仅作持久化**；直读 redb 仅限预留的跨进程只读场景（v2 不做）；观察者转发 SSE（保留 `/ev/engine` 为外部事件流）。
2. **动态 system**：触发事件 = fetch 完成（成功/失败）、UP 主状态变更（关注/取关/静默/分组）、定时 tick（日志事件不入选）；执行注册的 lua 回调，先串行（带超时），后期按需并行；**定时 tick 接管 v1 `exec_timers` 的补抓调度，bucket 速率控制保留**。
3. **KV 数据库**（redb）两类表：动态 system 表（k: name → v: lua callback + condition）；EC 数据表（k: entity id → `brick`（基础资料 + 管理状态）/ `videoPost` `livePost` `commentPost` `runtime`）。写入策略：brick 直写，易变表批量 flush（M1 细化）。
4. **libcall** 两组：**admin**（增删 entity＝关注/取关、管理动态 system、管理 entity 分组）、**bapi**（info 拉取、video list、xlive 推荐；复用 bilibili-api-rs，缺失接口回补）。
5. **保活**：zbus（D-Bus）向自研/既有 watchdog 守护进程发心跳；**心跳由独立 tokio 任务发送，携带主循环活跃度——主循环停滞超阈值时主动报障，watchdog 感知并拉起**；**`hobob_dbgconn` 与主进程内嵌 tarpc server（端口 21321）退役，远程 alive/restart 由 watchdog 承接**；接口协议在 M4 定义。
6. **mlua**：嵌入 Lua，实现"动态超控"（脚本化逻辑）。
7. **日志系统**：log4rs appender 收集所有日志，附带写入 ECS-KV；**appender 与存储层自身日志只走文件，不入 KV（防递归）**。
8. **界面**：leptos + axum，WebSocket server push（leptos websocket example 模式）；**渲染模式 CSR（默认），不做 SSR+hydration，服务端只做 API + WS + 静态资源**；SSE 保留为外部事件流，界面走 WS。
9. **API**：v1 HTTP API（`/op/*`）全量重设计，`monkey/` 油猴脚本同步重写；**校验随新 API 重定：废弃远程 boon schema（`data_schema.rs`），消除 lintd.xyz 离线 panic**。
10. **遗留清理**：`chunk.rs`、`vm.rs`、`bench.rs` 全部移除，由动态 system（lua）取代。
11. **依赖**：`bilibili-api-rs` vendor 进本仓库（默认 git 子模块，M0 定）。

## 3. 分阶段路线

**依赖与工作量**：M0 → M1 → {M2、M3、M4 可并行} → M5 → M6。工作量标度：M0 S、M1 L、M2 S、M3 M、M4 S、M5 L、M6 M。

### M0 — 存储地基：redb + bincode（S）

- [ ] 依赖引入：`redb`、`bincode`（`zbus` 留到 M4、`axum`/`leptos` 留到 M5）。
- [ ] `bilibili-api-rs` vendor 进本仓库（默认 git 子模块），本地构建不再依赖外部 path。
- [ ] KV 表结构落地（按 §2.3）：动态 system 表（k: name → v: {lua callback, condition}）；EC 数据表（k: entity id → `brick` / `videoPost` `livePost` `commentPost` `runtime`）。
- [ ] bincode 编解码规范：显式版本号 + 迁移钩子（防 schema 漂移）。
- [ ] redb 文件位置：默认 `~/.hobob/state.redb`，路径可配置（细化点）。
- [ ] 验收：`cargo test -p hobob` 全绿；空库初始化 + 样例数据读写跑通。
- [ ] 本阶段不做：API/界面改动、lua、保活；bench.json 迁移工具（暂缓，不讨论）。

### M1 — ECS 核心（自研轻量，bevy_ecs 风格）（L）

- [ ] Entity：up、group、videoPost/livePost/commentPost（动态数据）、log、runtime 配置。
- [ ] Component：`brick`（基础资料 + 管理状态：关注/分组/静默）、易变组件（视频/直播/评论动态、bucket runtime）。
- [ ] System 划分：基础 system（fetch engine、持久化、渲染取数）＋ 动态 system 框架（触发事件注册表：fetch 完成 / UP 主状态变更 / 定时 tick；**tick 接管 `exec_timers` 补抓调度，bucket 速率保留**）。
- [ ] 启动初始化从 redb+bincode 全量读取；写入策略按 §2.3：brick 直写、易变表批量 flush（阈值/间隔/停机强刷）。
- [ ] 读取路径：www 渲染、libcall、lua 走内存 ECS world（权威）；观察者将变更转发 SSE（`/ev/engine` 保留）；redb 仅作持久化。
- [ ] 通道收敛：`WeiYuanHui` 的 mpsc/watch/broadcast 映射为 ECS 系统间消息总线；状态修改仍走统一提交通道（`WeiYuan::apply/update` 语义）。
- [ ] 验收：v1 路由端到端测试全部迁移并绿（v1 API 保留至 M5 重设计）；redb 最小性能冒烟（写路径/写放大/fsync 频率）。
- [ ] 本阶段不做：API 重设计、界面框架迁移、lua 集成。

### M2 — 日志系统入 KV：appender 基础（S，随 M1 完成即可提前落地）

- [ ] 自定义 log4rs appender：结构化日志写入 redb KV（时间/级别/模块/消息/上下文）。
- [ ] **appender 与 redb 存储层自身日志只走文件，不入 KV（防递归死循环）**。
- [ ] 保留文本日志输出（更新 `~/log4rs.yml` 模板），KV 日志供查询与界面展示。
- [ ] 日志留存策略（容量上限、环形淘汰/按天分桶）。日志事件**不**作为动态 system 触发源。
- [ ] 验收：日志双写（文件 + KV）跑通，KV 查询可读；存储层日志无递归。
- [ ] 本阶段不做：日志查看 UI（留 M5）。

### M3 — libcall + mlua 动态超控（M）

- [ ] libcall 接口设计（两组）：
  - **admin**：增删 entity（关注/取关）、管理动态 system（增删改注册）、管理 entity 分组；
  - **bapi**：info 拉取、video list、xlive 推荐——封装 vendor 后的 `bilibili-api-rs`；**查证 xlive 推荐接口是否已存在，缺失则回补到该仓库**。
- [ ] 动态 system 落地：事件（fetch 完成 / UP 主状态变更 / 定时 tick）→ 执行注册的 lua 回调（condition 过滤）；**串行执行 + 超时兜底**，并行化留到后期按需（spawn_blocking 池 + 并发写冲突处理）。
- [ ] mlua 集成：Lua 沙箱（仅经 libcall 访问能力，无文件/网络直通）、脚本超时与错误隔离、热加载；脚本与 condition 持久化在 redb 动态 system 表。
- [ ] 移除 `chunk.rs`、`vm.rs`、`bench.rs` 及 lalrpop 解析器 build.rs 相关产物与测试数据。
- [ ] 验收：一个示例 lua 回调（如"抓取完成后按 condition 修改分组"）端到端跑通。
- [ ] 本阶段不做：脚本编辑 UI（M5）、并行执行。

### M4 — 保活（zbus 心跳）（S）

- [ ] 引入 `zbus`：**心跳由独立 tokio 任务发送（与 ECS 主循环解耦）；心跳携带主循环活跃度（循环序号/最后 tick 时间），停滞超阈值时主动报障，watchdog 感知并拉起**；定义 D-Bus service 名/接口与心跳间隔、超时协议（细化点）。
- [ ] **`hobob_dbgconn` 退役**：移除主进程内嵌 tarpc server（端口 21321）与 `hobob_dbgconn/` crate；远程 alive/restart 由 watchdog 承接（人工运维走 SSH/systemd）。
- [ ] 心跳停止/异常退出路径：优雅停机 vs watchdog 强杀；redb 崩溃恢复 + 易变表未 flush 数据兜底；误杀恢复演练。
- [ ] 界面层 WS 断线重连与保活协同。
- [ ] 验收：opi 设备上 kill 主进程，watchdog 按预期拉起；人为停滞主循环，watchdog 能感知并拉起。
- [ ] 本阶段不做：多进程拆分。

### M5 — 界面迁移 + API 重设计：leptos + axum + WS（L）

- [ ] `warp → axum`：路由骨架平移（`/`、`/static/*`）。
- [ ] `tera+jQuery → leptos`：卡片列表/分组筛选/标签页组件化；**渲染模式 CSR（默认），不做 SSR+hydration，服务端仅 API + WS + 静态资源**；wasm 前端构建（trunk/wasm-bindgen）纳入构建脚本与 CI。
- [ ] server push 走 WS（leptos websocket example 模式）；**SSE `/ev/engine` 保留为外部事件流（v1 兼容/调试），界面走 WS**。
- [ ] **v1 HTTP API（`/op/*`）全量重设计**：新路由/新 JSON 协议（与 admin libcall 对齐）；`monkey/` 油猴脚本**同步重写**。
- [ ] **校验方案随新 API 重定**：废弃远程 boon schema（`data_schema.rs` 移除/重写），内嵌或类型化校验，消除 lintd.xyz 离线 panic。
- [ ] 新 API 鉴权评估（内网自用，简单 token 可选）。
- [ ] 前端日志查看页（基于 M2 的 KV 日志）。
- [ ] 端到端测试重写：axum 测试替 warp::test，覆盖新 API 与 WS push。
- [ ] 验收：v2 界面 + 新 API + monkey 脚本全链路可用。
- [ ] 本阶段不做：SSR+hydration、远程 boon schema。

### M6 — 收敛、部署与打磨（M）

- [ ] vendor 依赖升级策略：bilibili-api-rs 与上游同步流程（文档化）。
- [ ] 交叉编译（cross, aarch64）＋ zbus/D-Bus 在目标设备的可用性验证（需 dbus daemon + watchdog 守护进程就位）。
- [ ] `opi` 部署与 watchdog 联调（hobob_dbgconn 已退役）。
- [ ] redb 在树莓派/香橙派上的读写性能冒烟（易变表批量 flush 与 brick 直写的实际写放大）。
- [ ] 备份与恢复：redb 拷贝式备份策略 + 恢复演练（含 watchdog 强杀后状态校验）。
- [ ] 构建/测试固化：`cargo test`、cross 交叉编译、前端 wasm 构建的脚本或轻量 CI。
- [ ] 文档更新：README、AGENTS.md、模块 README 同步 v2 架构。
- [ ] 本阶段不做：新功能开发。

## 4. 已定决策

| # | 问题 | 结论 | 对应 |
| --- | --- | --- | --- |
| 1 | ECS 内核选型 | 自研轻量 ECS（借鉴 bevy_ecs 风格），不用现成 crate | §2.1 |
| 2 | 动态 system 触发事件 | fetch 完成、UP 主状态变更、定时 tick；日志事件不入选 | §2.2 |
| 3 | lua 回调执行模型 | 先串行落地（带超时），后期按需并行 | §2.2 |
| 4 | SSE 与 WS 分工 | 并存过渡：SSE 保留为外部事件流，界面走 WS | §2.1/§2.8 |
| 5 | brick 与易变表边界 | brick = 基础资料 + 管理状态；易变 = 动态 + runtime | §2.3 |
| 6 | component 写回策略 | 混合：brick 直写，易变表批量 flush | §2.3 |
| 7 | watchdog 形态 | 自研/既有 watchdog 守护进程 + zbus D-Bus 心跳 | §2.5 |
| 8 | bapi 封装方式 | 复用 bilibili-api-rs；xlive 推荐等缺失接口回补该仓库 | §2.4 |
| 9 | v1 HTTP API 兼容性 | 全量重设计，monkey 油猴脚本同步重写 | §2.9 |
| 10 | chunk/vm/bench 去留 | 全部移除，由动态 system（lua）取代 | §2.10 |
| 11 | bilibili-api-rs 依赖管理 | vendor 进本仓库（默认 git 子模块） | §2.11 |
| 12 | hobob_dbgconn 去留 | 退役：移除内嵌 tarpc server 与 `hobob_dbgconn/` crate，alive/restart 由 watchdog 承接 | §2.5 |
| 13 | 读路径 | 同进程走内存 ECS world（权威），redb 仅持久化；跨进程只读仅预留（v2 不做） | §2.1 |
| 14 | 日志防递归 | appender 与存储层自身日志只走文件，不入 KV | §2.7 |
| 15 | 渲染模式 | CSR（默认），不做 SSR+hydration | §2.8 |
| 16 | schema 时机 | 随 M5 API 重设计一并废弃远程 boon schema | §2.9 |
| 17 | 心跳与故障感知 | 独立 tokio 任务发心跳 + 携带主循环活跃度，停滞超阈值主动报障 | §2.5 |
| 18 | bench.json 迁移 | 非生产数据，迁移工具暂缓（当前不讨论） | — |

**剩余待细化点**（进入对应阶段时细化）：

- M0：vendor 具体方式（子模块 vs 拷贝）与升级流程；redb 文件位置与配置项。
- M1：易变表批量 flush 窗口参数（阈值/间隔/停机强刷）；ECS 系统间消息总线的具体通道形态；最小性能冒烟指标。
- M2：日志留存策略参数（容量上限/分桶粒度）。
- M3：动态 system 的 lua 回调 API 形状与 condition 语法；bilibili-api-rs 中 xlive 推荐接口的查证结论。
- M4：watchdog 的 D-Bus service 名/接口与心跳超时协议数值；主循环活跃度指标与故障阈值；dbgconn 退役过渡安排。
- M5：新 `/op/*` API 的路由与 JSON 协议设计（与 admin libcall 对齐）；CSR 前端构建链（trunk）集成细节；鉴权评估结论。

## 5. 主要风险

- redb 单文件 ACID 语义 + 易变表高频写入的写放大，目标设备 IO 有限（M1 最小冒烟 + M6 设备冒烟；批量 flush 缓解）。
- bincode 版本漂移导致已持久化数据不可读（显式版本 + 迁移钩子，M0 就要定）。
- Lua 动态 system：脚本错误/死循环拖垮主进程（串行 + 超时兜底，仍需隔离执行与事件丢弃策略）；沙箱逃逸风险。
- 心跳双刃：主循环活跃度阈值调不好会误报（watchdog 误杀健康进程）或漏报（真故障不拉起）——M4 需演练调参。
- zbus 依赖 D-Bus：目标设备需 dbus daemon 与 watchdog 守护进程就位，交叉编译与部署链路多一个环节。
- 前端整体重写（leptos CSR + WS）+ API 全量重设计 + monkey 重写工作量大；leptos wasm 构建链（trunk + wasm32 目标）纳入交叉编译/CI 的复杂度。
- 与引擎/存储改造并行时易长期无可用版本——按 M0→M6 顺序每阶段保持可运行。
- vendor 后 bilibili-api-rs 与上游两处维护，升级需手动同步（需文档化流程）。
- 日志入 KV 的递归风险（已定：appender/存储层日志仅走文件，实现时须强制过滤来源）。

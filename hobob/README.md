# hobob — 主应用

B 站 UP 主关注管理 web app（workspace 唯一成员 crate）。单进程内四部分协作：

- **`www`**（`src/www.rs`）：warp HTTP 服务，渲染页面（tera 模板）+ 操作 API + SSE 事件推送
- **`systems`**（`src/systems.rs`，原 `engine.rs`）：后台抓取循环 `fetch_loop`，消费 `commands` 队列，用 `bilibili-api-rs` 抓取 UP 主数据；动态 system 框架（`TriggerEvent`/`DynSystemRegistry`，native→lua 两段式分发 + `lib.` oneshot + 内置 `builtin.tick`）
- **`db`**（`src/db/`，核心）：数据中枢 `WeiYuanHui`/`WeiYuan`，持权威 `Snapshot`（ECS `world` + 内存索引 `res`），全部修改经 chair 通道提交（`ptr_eq` 冲突校验）、watch/broadcast 发布、`#SYSEV#` 事件通道分发动态 system（详见 `src/db/README.md`）
- **`store`**（`src/store.rs`）：redb + bincode 持久化（详见下）

## 目录结构

| 路径 | 职责 |
| --- | --- |
| `src/lib.rs` | crate 入口：日志初始化 `prepare_log`（`~/log4rs.yml` 陈旧检测 warn）、启动主循环 `main_loop`（hub 循环 + spawn www/fetch_loop）、`vpath!`/`schema_uri!` 宏、CLI `Flags`（`--port` 默认 3731、`--state`/`HOBOB_STATE` 指定状态文件、`--log-kv-max`/`HOBOB_LOG_KV_MAX` 默认 20_000）、`Store::open_or_create` 失败即退出 |
| `src/db/`（`mod.rs`） | 数据层（详见 `src/db/README.md`）：`WeiYuanHui`/`WeiYuan` + `Snapshot`（ECS `world`：Brick/LivePost/VideoPost/GroupInfo 等组件 + entity 1 runtime；`res` 内存索引/队列），通道提交 + 快照发布 + 持久化桥接（`open` 全量加载、`persist_diff` 直写/stage） |
| `src/ecs.rs` | 轻量 ECS 内核（`Entity`/`Component`/`Storage`/`World`：spawn/insert/remove/get/iter + `ptr_eq` 结构共享判定；无外部依赖） |
| `src/store.rs` | redb + bincode 持久化：9 张表（meta/systems/ec:* + `kv:log`）、typed CRUD、`VersionedRecord` 版本信封 + 迁移钩子（Brick V1→V2）、`VolatileBuffer` 批量 flush、布局升级链（M2 升 3）；**M1 起接管数据路径**（brick/group 直写、video/live/comment/runtime 批量 flush、close 强刷，稳态零读） |
| `src/logkv.rs` | M2 日志 KV 镜像：自定义 log4rs appender（`hobob_kv` kind，target 黑名单挡 `hobob::store`/`hobob::logkv`）+ 全局 sync_channel + hub drain 编排（`run()` 每轮/`close()` last-drain）+ seq 续号 + 条数裁剪（`--log-kv-max` 默认 20_000） |
| `src/systems.rs` | 基础 system：`fetch_loop` 抓取循环、动态 system 框架（`TriggerEvent`/`DynSystemRegistry`：native→lua 两段式分发、`lib.` oneshot 库函数、condition 预编译、`set_hook` 指令上限超时；`builtin.tick` 补抓）；原 `engine.rs` 迁入（已删） |
| `src/libcall.rs` | M3 libcall（lua↔Rust 桥）：mlua 沙箱（禁 IO/OS/PACKAGE）+ 全局 `json` + `ctx.admin`（改 `&mut Snapshot`，含 `register/unregister/reload_system/reload_all`）/`ctx.bapi`（同步桥） |
| `src/www.rs` | warp 路由、tera 渲染、SSE、boon schema 校验 |
| `src/data_schema.rs` | JSON schema 编译（boon），schema 从 `https://lintd.xyz/hobob/` 远程加载 |
| `src/bin/perf_smoke.rs` | M1 性能冒烟 bin（200 up × 10 轮 fetch 突发，release 运行，见 `.plans/m1-ecs-core.md` §7） |
| `src/bin/show_expect_value.rs` | 辅助 bin：打印 Chunk AST 示例 JSON |
| `templates/` | tera HTML 模板；debug 从磁盘加载，release 编译期 `include_str!` 内嵌（`src/www.rs` `TERA`） |
| `static/` | 前端静态资源（`index.js` 用 jQuery 加载卡片/筛选/标签页，`favicon.ico`） |
| `assets/` | `log4rs.yml`（日志配置模板，含 `hobob_kv` appender；首启复制到 `~/log4rs.yml`，M2 旧模板缺 `hobob_kv` 时 prepare_log 会 `log::warn` 提示，自动降级不阻止启动）、`db_init.sql`（**遗留**，当前不用 SQLite） |

## 数据流

```
浏览器 ──GET/POST──> www (warp 路由)
                      │ 经 WeiYuan（chair 句柄）提交
                      ▼
              WeiYuanHui 数据中枢 (Snapshot: ECS world + res)
                      │ watch/broadcast 分发   │ mpsc 提交      │ 持久化（store 直写/stage）
                      ▼                        ▼               ▼
              页面渲染 / SSE 事件          systems::fetch_loop   state.redb
                                          │ 消费 commands（如 fetch）
                                          ▼
                               bilibili-api-rs 抓取
                                          │ apply_fetch 组件写回 + #SYSEV# 事件上报
                                          ▼
                                 Snapshot → hub 校验发布（T4 起落盘：brick 直写、易变批量 flush）
```

- 所有状态修改走 `WeiYuan::apply/update`（mpsc 通道，带冲突检测）；读取走 `recv`（watch 快照，COW）。
- 空闲补抓：`systems::fetch_loop` 在 commands 空时发 `Tick` 事件 → hub 分发内置 `builtin.tick`（原 `exec_timers`）按 `ctime` 索引挑最旧未刷新的 UP 主补抓，受 bucket 速率（`runtime.bucket.gap`）控制；`/op/silence` 把 gap 翻倍实现静默。
- 事件（events）经 broadcast 通道推给 `/ev/engine` SSE；`#SYSEV#` 载荷（动态 system 触发）由 hub 提取分发，不进 SSE。

## HTTP 路由表（`src/www.rs` `build_app`）

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| GET | `/` | 首页（index.html，含 runtime.index 状态） |
| POST | `/op/follow` | 关注/取关 UP 主，body `{"uid": int, "enable": bool}` |
| POST | `/op/refresh` | 刷新单个 UP 主，body `{"uid": int}` |
| POST | `/op/silence` | 静默（bucket gap 翻倍） |
| POST | `/op/toggle/group` | 把 UP 主移入/移出分组，body `{"uid", "gid"}` |
| POST | `/op/touch/group` | 新建分组，body `{"gid", "pin", "name"}` |
| GET | `/card/one/{uid}` | 单个 UP 主卡片 HTML |
| GET | `/card/ulist/{gid}/{order}/{start}/{len}` | 分组内用户卡片列表 HTML（分页） |
| GET | `/card/filter/options` | 分组筛选下拉选项 HTML |
| GET | `/ev/engine` | SSE 事件流（keep-alive + lag 提示） |
| GET | `/static/*`、`/favicon.ico` | 静态资源 |

body 为 JSON（上限 16KB），返回 `"success"` 或 `{"err": ...}`；所有 POST 经 `simpleapi` 解析。页面渲染结果再经 `ChairData::checker` 做 JSON schema 校验。

## 数据模型

数据在 `db` 模块中由 `Snapshot` 承载：ECS `world`（up/group 组件 + runtime 实体）+ `res`（`up_index`/`up_by_fid`/`uid_index`/`gid_index`/`events`/`logs`/`commands` 等内存索引/队列），hub/chair 通道维护。字段与业务方法细节见 `src/db/README.md`。

## 测试

```bash
cargo test -p hobob
```

- `www.rs` tests：warp::test 对每个路由做端到端断言（follow 后 bench 状态、SSE 推送等）
- `src/db/`（`mod.rs`）tests：通道/持久化 roundtrip/排序/动态 system 事件分发逻辑、lua system 加载与热加载/分组持久化/Silenced 事件
- `src/systems.rs` tests：fetch 循环（取命令/关闭/补抓 deadline）、`pick_*` 纯函数、lua 动态 system（加载/oneshot/condition/指令上限斩杀/热加载/native 先于 lua）
- `src/ecs.rs` tests：ECS 内核（spawn/insert/CoW/iter/ptr_eq）
- `store.rs` tests：空库初始化/样例 roundtrip/错文件守卫/codec/迁移链（含 brick V1→V2）/版本过高拒绝/entity id/直写/批量 flush 四路径/group CRUD/布局 1→2 升级/list_* 全量（T1–T16）/kv:log 追加/裁剪/查询/升级到 3（T17–T22）
- `logkv.rs` tests：自定义 appender 结构/黑名单/满丢弃/模板检查/业务级别映射/drain 批写裁剪（T2–T3，含 `logkv_global` 集成测试）
- `tests/logkv_e2e.rs`：M2 T4 端到端——真实 log4rs logger + hub 业务镜像 → kv:log 查询（N4 防递归/N5 双写同源/N7 业务镜像，独立进程）

## 已知坑

- **远程 schema**：`data_schema.rs` 启动即从 `https://lintd.xyz/hobob/*.json` 拉取 schema（`schema_uri!` 宏），离线环境 `ChairData` 构建会 panic。
- **模板加载差异**：debug 从 `templates/` 磁盘目录读（工作目录必须是 crate 根），release 内嵌编译期模板。
- **vendor 子模块**：`bilibili-api-rs` 是 path 依赖，位于 `vendor/bilibili-api-rs`（git 子模块，锁 commit）；新 clone 后需 `git submodule update --init`，升级 SOP 见 `vendor/UPGRADE.md`。
- **state.redb**：启动 `Store::open_or_create` + `WeiYuanHui::open` 全量加载，**失败即退出**（取代 M0 探针的仅日志）。`serde_json::Value` 不能参与 bincode 反序列化，store 类型中「未类型化 JSON」字段一律存 JSON 字符串（M1 校准语义）。
- **log4rs.yml 陈旧**：M2 起 `assets/log4rs.yml` 模板新增 `hobob_kv` appender（KV 日志镜像）；部署机已有 `~/log4rs.yml` 不会自动更新，`prepare_log` 检测缺 `hobob_kv` 时 `log::warn` 提示（KV 静默降级，文件日志不受影响）。手动用模板更新 `~/log4rs.yml` 后重启即可启用 KV 日志。

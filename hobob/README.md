# hobob — 主应用

B 站 UP 主关注管理 web app（workspace 唯一成员 crate）。单进程内三部分协作：

- **`www`**（`src/www.rs`）：warp HTTP 服务，渲染页面（tera 模板）+ 操作 API + SSE 事件推送
- **`engine`**（`src/engine.rs`）：后台抓取循环，消费 `commands` 队列，用 `bilibili-api-rs` 抓取 UP 主数据
- **`db`**（`src/db.rs`）：数据中枢 `WeiYuanHui`，持有全量状态 `FullBench`（不可变数据结构 `im`），所有修改经通道提交，定期持久化到 `~/bench.json`

## 目录结构

| 路径 | 职责 |
| --- | --- |
| `src/lib.rs` | crate 入口：日志初始化 `prepare_log`、启动主循环 `main_loop`、`vpath!`/`schema_uri!` 宏、CLI `Flags`（`--port` 默认 3731、`--state`/`HOBOB_STATE` 指定 v2 状态文件）、store 启动探针 |
| `src/db.rs` | 数据层（详见 `src/README.md`）：`WeiYuanHui`/`WeiYuan`/`FullBench`，通道 + 持久化（**v1 实际使用的数据层**） |
| `src/store.rs` | v2 持久化地基（redb + bincode）：7 张表、typed CRUD、版本信封 + 迁移钩子、`VolatileBuffer` 批量 flush；M0 仅测试 + 启动探针使用，**未接管 v1 数据路径** |
| `src/www.rs` | warp 路由、tera 渲染、SSE、boon schema 校验 |
| `src/engine.rs` | 后台引擎循环、`fetch` 命令执行、bucket 速率控制 |
| `src/data_schema.rs` | JSON schema 编译（boon），schema 从 `https://lintd.xyz/hobob/` 远程加载 |
| `src/chunk.rs` + `src/chunkir.lalrpop` | Chunk AST 类型 + lalrpop 解析器（build.rs 生成，见 `src/README.md`） |
| `src/vm.rs`、`src/bench.rs` | 未完成的设计实验（含 `todo!()`，勿依赖） |
| `src/bin/show_expect_value.rs` | 辅助 bin：打印 Chunk AST 示例 JSON |
| `src/test_data/` | chunk 解析器测试用例（`in.txt` + `expect.json` 配对） |
| `templates/` | tera HTML 模板；debug 从磁盘加载，release 编译期 `include_str!` 内嵌（`src/www.rs` `TERA`） |
| `static/` | 前端静态资源（`index.js` 用 jQuery 加载卡片/筛选/标签页，`favicon.ico`） |
| `assets/` | `log4rs.yml`（日志配置模板，首启复制到 `~/log4rs.yml`）、`db_init.sql`（**遗留**，当前不用 SQLite） |
| `build.rs` | lalrpop 解析器生成 |

## 数据流

```
浏览器 ──GET/POST──> www (warp 路由)
                      │ 经 WeiYuan（chair 句柄）提交
                      ▼
              WeiYuanHui 数据中枢 (FullBench, im 不可变结构)
                      │ watch/broadcast 分发         │ mpsc 提交
                      ▼                             ▼
              页面渲染 / SSE 事件               engine 主循环
                                              │ 消费 commands（如 fetch）
                                              ▼
                                   bilibili-api-rs 抓取
                                              │ modify_up_info 写回
                                              ▼
                                     FullBench → 定时 dump 到 ~/bench.json
```

- 所有状态修改走 `WeiYuan::apply/update`（mpsc 通道，带冲突检测）；读取走 `recv`（watch 快照，COW）。
- `engine` 空闲时由 `exec_timers` 按 `ctime` 索引挑最旧未刷新的 UP 主自动补抓，受 bucket 速率（`runtime.bucket.gap`）控制；`/op/silence` 把 gap 翻倍实现静默。
- 事件（events）经 broadcast 通道推给 `/ev/engine` SSE。

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

## 数据模型（`FullBench`）

`up_info`（uid → raw/pick）、`up_index`（排序索引，`__by_weight__` 权重列表）、`up_by_fid`（分组内 uid 有序列表）、`up_join_group`、`events`、`group_info`（gid → name/removable）、`logs`、`runtime`（bucket/log_filter/event_filter/db/index 配置）、`commands`（引擎消费队列）。细节见 `src/db.rs`。

## 测试

```bash
cargo test -p hobob
```

- `www.rs` tests：warp::test 对每个路由做端到端断言（follow 后 bench 状态、SSE 推送等）
- `db.rs` tests：通道/持久化/排序逻辑
- `chunk.rs` tests：解析器对 `test_data/chunk_*.in.txt` 的 AST 与 `*.expect.json` 比对
- `store.rs` tests：空库初始化/样例数据 roundtrip/错文件守卫/codec/迁移链/版本过高拒绝/entity id/直写/批量 flush 四路径/systems 一致性/配置解析（T1–T12）

## 已知坑

- **远程 schema**：`data_schema.rs` 启动即从 `https://lintd.xyz/hobob/*.json` 拉取 schema（`schema_uri!` 宏），离线环境 `ChairData` 构建会 panic。
- **模板加载差异**：debug 从 `templates/` 磁盘目录读（工作目录必须是 crate 根），release 内嵌编译期模板。
- **vendor 子模块**：`bilibili-api-rs` 是 path 依赖，位于 `vendor/bilibili-api-rs`（git 子模块，锁 commit）；新 clone 后需 `git submodule update --init`，升级 SOP 见 `vendor/UPGRADE.md`。
- **state.redb（v2）**：启动时 store 探针会在 `--state`/`HOBOB_STATE`/`$HOME/.hobob/state.redb` 建空库；M0 探针失败仅记日志不阻断。`serde_json::Value` 不能参与 bincode 反序列化，v2 类型中「未类型化 JSON」字段一律存 JSON 字符串（M1 校准语义）。
- `vm.rs`、`bench.rs` 是未完成的替代设计（`todo!()`），`db.rs` 才是实际使用的数据层。

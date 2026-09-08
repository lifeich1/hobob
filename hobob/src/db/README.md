# hobob/src/db — 数据中枢（`mod.rs` 1794 行，核心）

v1 **实际使用的数据层**（v2 `../store.rs` 尚未接管）。本页是模块导航：先定位到符号/小节，再决定是否深挖源码，避免通读全文件。

## 架构（M1 · 方案 `.plans/m1-ecs-core.md`）

模块 doc comment（`mod.rs` 开头约 60 行）是最权威概述，要点：

- v1 `FullBench`（9 个 im 字段）已演进为 `Snapshot { world, res }`（ECS 见 `../ecs.rs`）：
  - **world**（组件）：up 实体 = `Brick`/`LivePost`/`VideoPost`/`CommentPost`/`RawInfo`；group 实体 = `GroupInfo`/`Members`；entity 1 = runtime（`RuntimeCfg`）。实体号：1 = runtime，2/3 = 内置组「全部/特殊关注」，≥4 普通实体（与 store `INITIAL_ENTITY_ID` 衔接）。
  - **res**（`Resources`，内存索引/队列，随快照传播、不落 redb）：`up_index`（排序索引）、`up_by_fid`/`uid_index`/`gid_index`、`events`/`logs`/`commands`、`closing`。
- **hub/chair**：`WeiYuanHui` 持权威 `Snapshot` + 三通道（`updates` mpsc 提交 / `publish` watch 发布 / `ev` broadcast 事件）；`WeiYuan` 是可 Clone 的 chair 句柄。提交 `(base, next)` 由 hub 用 `ptr_eq` 校验 base 仍是权威值，不匹配即 abort——**所有修改必须走 chair `update`/`apply`**。

## 关键符号

| 符号 | 作用 |
| --- | --- |
| `Snapshot`（~172）`Resources`（~149） | 权威/视图数据单元 + 内存索引/队列 |
| `Brick`/`LivePost`/`VideoPost`/`CommentPost`（~52–112） | up 实体组件（字段语义见方案 §4.3；`pick_*` 反投影回 v1 JSON 形状） |
| `WeiYuanHui`（~1018） | hub：`load`/`new_chair`/`listen_events`/`close`（`From<Snapshot>` 构造） |
| `WeiYuan`（~1187） | chair：`recv` 读快照、`update`/`apply` 提交闭包修改、`log`/`count` 记日志计数、`readonly` |
| `Snapshot` 方法（impl 自 ~203） | 业务：`follow`/`refresh`/`force_silence`/`toggle_group`/`touch_group`/`users_pick`/`filter_options`/`apply_fetch`；bucket 节奏：`bucket_duration_to_next`/`good`/`hang`/`double_gap`；查询：`runtime_get`/`runtime_set_field`/`pick_of` |
| 自由函数（~941–995） | `now_timestamp`、`pick_basic`/`pick_live`/`pick_video`（engine 写回时精简字段） |

## 谁在用

- `../lib.rs`：`Store::open_or_create` + `WeiYuanHui::open` → `new_chair` 分发 www/fetch_loop，hub 主循环消费提交，Ctrl+C 后 `close`/`closed` 优雅退出。
- `../www.rs`：经 chair 提交操作 / 读快照渲染 / 订阅事件推 SSE。
- `../systems.rs`：基础 system（原 `engine.rs` 迁入）——`fetch_loop`（chair `recv` 取 `commands` → 抓取 → `apply_fetch` 写回，bucket 控制节奏）+ 动态 system 框架（`TriggerEvent`/`DynSystemRegistry`/`builtin.tick`）。
- `../store.rs`（v2 持久化）：T4 已桥接——`WeiYuanHui::open(&store)` 启动全量加载（`load_snapshot`），
  运行期 patch 经 `persist_diff` 落盘（brick/group 直写 + 易变 stage），`close()` 停机强刷；稳态零 redb 读。

## 坑

- **持久化入口是 `WeiYuanHui::open(&store)`**（T4 起）：从 state.redb 全量加载重建 world/索引（`load_snapshot`），失败返回 Err（lib.rs 失败即退出）；v1 `load(bench.json)` 已删除（D12），旧 bench.json 不迁移。
- 通道容量 mpsc/broadcast 均为 64；`events` 由 hub drain 后广播，订阅落后会 `Lagged`（www SSE 有提示）。
- `VCounter`（统计/落盘节流）留在 hub 私有、不进快照：push_miss/broadcast_void 等不随 `publish` 发布。
- 索引/排序维护封装在 `Snapshot` 方法内，勿绕过方法直接动 world/res。
- 运行状态与 store 的 entity id 语义衔接（`ENTITY_RUNTIME` 等常量），改布局先看 `../store.rs`。

## 测试

`mod tests`（`mod.rs` ~1311–1794，约 480 行）：runtime dump 节流、通道提交/冲突 abort、排序与业务逻辑。入口：`cargo test -p hobob`。

# HOBOB

一个被 bibi&lili 踢出的 hobo 自用的 B 站 UP 主关注管理 web app（*WIP*）。

跟踪关注的 bilibili UP 主：后台引擎按速率限制抓取 UP 主的直播/视频/个人信息，前端以卡片列表展示，支持分组筛选、关注/取关、静默等操作。

## 目录导航（给 agent 的索引）

| 目录 | 职责 | 详细文档 |
| --- | --- | --- |
| `hobob/` | 主应用（workspace 唯一成员 crate），warp web + 后台抓取引擎 | [hobob/README.md](hobob/README.md) |
| `hobob_dbgconn/` | 独立 crate：tarpc RPC 工具，远程检查/重启 hobob 进程 | [hobob_dbgconn/README.md](hobob_dbgconn/README.md) |
| `monkey/` | Tampermonkey 油猴脚本，把 bilibili 页面操作接到 hobob API | [monkey/README.md](monkey/README.md) |
| `xtask/` | `cargo xtask` 构建/部署脚本（**已弃用**，git 历史有记录） | [xtask/README.md](xtask/README.md) |
| `etc/` | cross 交叉编译容器配置（aarch64 / armv7） | [etc/README.md](etc/README.md) |
| `.agents/skills/` | 项目级 agent 技能（按需加载的代码理解文档） | [.agents/skills/README.md](.agents/skills/README.md) |
| `target/` | 构建产物，git 忽略，勿读 | — |

## 关键事实（跨目录）

- **外部 path 依赖**：`hobob/Cargo.toml` 依赖 `bilibili-api-rs = { path = "../../bilibili-api-rs/bilibili-api-rs" }`，构建需要该仓库存在于上层目录 `../bilibili-api-rs`。
- **数据文件**：运行时状态持久化为 `~/bench.json`（`hobob/src/db.rs`），日志配置模板复制到 `~/log4rs.yml`（`hobob/assets/log4rs.yml`）。
- **端口**：hobob web 默认 `3731`；hobob_dbgconn RPC 默认 `21321`。
- **目标设备**：部署目标为树莓派/香橙派（脚本里称 `my-pi`、`opi`），交叉编译产物 scp 到 `opi:/lintd/`。
- **远程 schema**：`hobob/src/data_schema.rs` 从 `https://lintd.xyz/hobob/*.json` 加载 JSON schema 校验数据，离线/无外网环境会失败。

## 构建与运行

```bash
# 运行（默认端口 3731，可从 ~/bench.json 恢复状态）
cargo run -p hobob

# 测试
cargo test -p hobob

# 交叉编译（cross，容器配置见 etc/；本机需要 podman/docker + cross）
cross build --bin hobob -r --target aarch64-unknown-linux-gnu
```

部署到设备：交叉编译产物 scp 到 `opi:/lintd/`，然后用 `hobob_dbgconn` 远程重启（见其 README）。

## 架构速览

- 单进程三部分：`www`（warp HTTP + SSE + tera 模板）、`engine`（后台循环，消费 commands 抓取 bilibili 数据）、`db`（`WeiYuanHui` 数据中枢：im 不可变数据结构 + mpsc/watch/broadcast 通道 + 磁盘持久化）。
- 前端页面（`/`、`/card/*`）与操作 API（`/op/*`）数据均经 JSON schema 校验（boon），schema 从远程加载。
- 详细数据流、路由表、模块地图见 [hobob/README.md](hobob/README.md) 与 [hobob/src/README.md](hobob/src/README.md)。

## License

[WTFPL](http://www.wtfpl.net/)

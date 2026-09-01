# hobob_dbgconn — hobob 进程远程管理工具

独立 crate（不在 workspace 内，需 `cargo build -p` 于其目录或单独编译）。用 tarpc（TCP + JSON）提供 RPC：远程检查 hobob 是否存活、重启 hobob。典型场景：hobob 部署在树莓派/香橙派上，从外部机器/脚本控制它的生命周期。

## 用法

不带子命令 = 启动 server（监听 `0.0.0.0:21321`，默认端口 `--port 21321`）；带子命令 = 作为 client 连接 `--server-ip`（默认 `127.0.0.1`）。

```bash
# 在目标设备上启动 server
hobob_dbgconn --port 21321

# 检查 hobob 是否存活（返回 pid，-1 表示未运行）
hobob_dbgconn --server-ip <设备IP> alive

# 重启 hobob（-f 强制 SIGKILL，否则 SIGINT；-b 指定二进制路径）
hobob_dbgconn --server-ip <设备IP> restart -f -b /lintd/hobob -- --port 3731
```

## RPC 协议（`src/lib.rs` `Dbgconn` trait）

| 方法 | 参数 | 返回 |
| --- | --- | --- |
| `alive` | — | hobob 的 pid；未运行返回 `-1` |
| `restart` | `force_kill: bool`、`binary: String`、`args: Vec<String>` | 重启后的 pid；失败 `-1`；**binary 不含 "hobob" 时拒绝并返回 `-2`**（安全校验） |

## 实现要点

- server（`src/server.rs`）：`pidof hobob` 找进程 → 先 kill（SIGINT 优雅 / SIGKILL 强制，循环等到退出）→ `cmd(binary, args).start()` 拉起新进程 → 返回 `alive()` 结果。`max_channels_per_key(1, ip)` 限制每 IP 一个连接。
- client（`src/client.rs`）：tarpc client，`alive` 输出 `dead hobob.` / `alive hobob, pid = N`，`restart` 输出新 pid。
- 单进程二选一：`Flags.command` 为 `None` 即 server（`src/main.rs` 判断）。
- 依赖 `duct`（进程调用）、`tarpc`、`tokio`、`clap`、`env_logger`。

## 坑

- server 与 hobob 须在同一设备上（用 `pidof`/`kill` 操作本机进程）。
- `restart` 的 `args` 是透传给 hobob 的 CLI 参数（如 `--port`）。

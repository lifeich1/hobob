# hobob_dbgconn/src — 源码

| 文件 | 职责 |
| --- | --- |
| `lib.rs`（54 行） | crate 入口：`Flags`（clap：`--port` 默认 21321、`--server-ip` 默认 127.0.0.1、子命令 `alive`/`restart`）；`#[tarpc::service] Dbgconn` trait 定义 RPC 契约；`is_server()` 判断模式 |
| `server.rs`（103 行） | server 端：`RpcServer` 实现 `Dbgconn`；`get_hobob_pid()`（`pidof`）、`kill_hobob(force)`（SIGINT/SIGKILL）；tcp listen + Json 格式 + 每 IP 1 通道、最多 10 并发 |
| `client.rs`（41 行） | client 端：tcp connect + `DbgconnClient`，按子命令调 RPC 并打印结果 |
| `main.rs`（14 行） | 二进制入口：解析 Flags，无子命令跑 `server_main`，否则 `client_main` |

要点：`restart` 的 binary 安全校验（必须含 "hobob"）在 server 端 `restart()` 内；`max_frame_length(usize::MAX)` 两侧都设了。修改 RPC 契约需同时改 `lib.rs` 的 trait 与 `server.rs` 的 impl。

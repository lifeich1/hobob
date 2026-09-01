# etc — cross 交叉编译配置

为 [`cross`](https://github.com/cross-rs/cross) 交叉编译提供自定义容器配置（根 `Cross.toml` 引用）。目标设备是 ARM 单板机（树莓派/香橙派，代码与脚本中称 `my-pi`/`opi`）。

## 配置

| 目标 triple | 目录 | 说明 |
| --- | --- | --- |
| `aarch64-unknown-linux-gnu`（默认） | `etc/aarch64-unknown-linux-gnu/` | 基于 `ghcr.io/cross-rs/aarch64-unknown-linux-gnu:main`，安装 `libssl-dev:arm64` |
| `armv7-unknown-linux-gnueabihf` | `etc/armv7-unknown-linux-gnueabihf/` | 基于 `ghcr.io/cross-rs/armv7-unknown-linux-gnueabihf:0.2.5`，安装 `libssl-dev:armhf` |

每个目录：
- `Dockerfile`：FROM cross 基础镜像 → 替换 `apt.list` 为国内镜像源（清华 tuna，focal）→ 添加目标架构（`dpkg --add-architecture`）→ 装 openssl 交叉库（hobob 依赖 `openssl-sys`）
- `apt.list`：amd64（主机侧）+ arm64/armhf（目标侧）的 sources.list

## 使用

```bash
# 本机需 podman 或 docker（README 提示 Linux 上 cross 配 podman 更稳）
cross build --bin hobob -r --target aarch64-unknown-linux-gnu
# 产物：target/aarch64-unknown-linux-gnu/release/hobob
```

部署链路见根 README（scp 到 `opi:/lintd/` + `hobob_dbgconn restart`）；`xtask` 曾封装该流程，已弃用。

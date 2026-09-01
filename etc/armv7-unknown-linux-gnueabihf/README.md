# etc/armv7-unknown-linux-gnueabihf — 交叉编译容器配置（armv7）

- `Dockerfile`：FROM `ghcr.io/cross-rs/armv7-unknown-linux-gnueabihf:0.2.5`；COPY 本目录 `apt.list` 为容器 sources.list；`dpkg --add-architecture armhf` 后安装 `libssl-dev:armhf`。
- `apt.list`：清华 tuna 镜像，focal；amd64 走 `ubuntu/`，arm64 走 `ubuntu-ports/`（与 aarch64 目录相同，目标架构不同）。

被根 `Cross.toml` 的 `[target.armv7-unknown-linux-gnueabihf] dockerfile` 引用。当前默认目标是 aarch64，armv7 需显式 `--target` 使用。

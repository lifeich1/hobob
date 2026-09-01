# etc/aarch64-unknown-linux-gnu — 交叉编译容器配置（aarch64）

- `Dockerfile`：FROM `ghcr.io/cross-rs/aarch64-unknown-linux-gnu:main`；COPY 本目录 `apt.list` 为容器 sources.list；`dpkg --add-architecture arm64` 后安装 `libssl-dev:arm64`。
- `apt.list`：清华 tuna 镜像，focal；amd64 走 `ubuntu/`，arm64 走 `ubuntu-ports/`。

被根 `Cross.toml` 的 `[target.aarch64-unknown-linux-gnu] dockerfile` 引用（也是 `cross` 的默认目标 `default-target`）。构建命令见 `etc/README.md`。

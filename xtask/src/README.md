# xtask/src — 源码

| 文件 | 职责 |
| --- | --- |
| `main.rs`（22 行） | `Maker` 实现 `lintd_taskops::Addon::dist()`：`cross -v build --bin hobob -r` + `scp` 到 `opi:/lintd/`；`main()` 调 `Maker::make()` 注册 |

依赖 `duct`（进程调用）与 `lintd-taskops`（xtask 框架）。已弃用（见 `xtask/README.md`）。

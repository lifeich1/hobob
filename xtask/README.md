# xtask — cargo xtask 构建/部署脚本

`cargo xtask` 模式的独立 crate：`cargo xtask dist` 执行 `Maker`（`xtask/src/main.rs`，基于 `lintd-taskops` 的 `Recipe`/`Addon`）。

## 动作

`dist`：用 cross 交叉编译 hobob 的 release 二进制，再 scp 到部署设备：

```bash
cargo xtask dist
# 等价于：
cross -v build --bin hobob -r            # 输出 target/<triple>/release/hobob
scp ./target/aarch64-unknown-linux-gnu/release/hobob opi:/lintd/
```

## ⚠️ 状态：已弃用

git 历史提交 `:fire: xtask too big to use` 已宣告弃用；`Cargo.toml` 未加入 workspace（根 `Cargo.toml` members 只有 `hobob`），日常构建请直接用 `cross build`（见根 README），部署用 scp + `hobob_dbgconn restart`。保留此目录仅作历史参考。

# vendor/bilibili-api-rs 升级 SOP

## 基本信息

- 上游：`git@github.com:lifeich1/bilibili-api-rs.git`（https 备用：`https://github.com/lifeich1/bilibili-api-rs.git`）
- 当前锁定 commit：`7df423ac6e8ecba05adf7b030b4c27c748f8b1a2`（2024 年 8 月，`v0.3.10` 之后的 lock update；描述 `v0.3.1-60-g7df423a`）
- 引入方式：git 子模块（`vendor/bilibili-api-rs`），`hobob/Cargo.toml` 以 path 依赖引用 `../vendor/bilibili-api-rs/bilibili-api-rs`，并保留 `version = "0.3"` 作为 API 兼容性约束。
- 子模块初始创建时的来源：本机原 clone `/home/fool/hub/bilibili-api-rs`（当时 github.com 直连不可达，采用本地 clone 离线完成 `submodule add`；`.gitmodules` 已记录规范上游 URL，见 M0 实施记录）。

## 升级流程

1. 同步上游：
   ```bash
   git -C vendor/bilibili-api-rs fetch origin
   git -C vendor/bilibili-api-rs diff 7df423a..origin/master --stat   # 先看差异
   ```
2. 锁定新 commit（或 tag）：
   ```bash
   git -C vendor/bilibili-api-rs checkout <NEW_COMMIT_OR_TAG>
   ```
3. 在 `nix develop` 内更新 lockfile 并跑全量测试：
   ```bash
   cd <repo> && nix develop -c bash
   cargo update -p bilibili-api-rs
   cargo test -p hobob
   ```
   （注意：本仓库 devShell 的 shellHook 在存在 `Session.vim` 时启动 nvim；可用 `cd /tmp && nix develop <repo> -c <cmd>` 绕开。）
4. 提交子模块指针 + `Cargo.lock`：
   ```bash
   git add vendor/bilibili-api-rs Cargo.lock hobob/Cargo.toml   # 如 path/version 约束有变
   git commit -m '⬆️ vendor - bilibili-api-rs → <NEW_COMMIT>'
   ```
5. 验证：`git submodule status` 输出应为干净的 ` <commit> vendor/bilibili-api-rs`（无 `+`/`-` 前缀）。

## 回退方式

- 升级后异常：`git -C vendor/bilibili-api-rs checkout 7df423ac6e8ecba05adf7b030b4c27c748f8b1a2`，重新 `cargo update -p bilibili-api-rs` + 全量测试，提交子模块指针即可。

## 交叉编译 / nix 提示

- `cross build` 前必须 `git submodule update --init`（子模块未 init 会缺源码）。
- `flake.nix` 的 `src = ./.` 只含 gitlink 不含子模块内容：nix build 链路需改用 `builtins.fetchGit { url = ./.; submodules = true; }` 或显式 input（M0 仅验证 devShell 路径）。

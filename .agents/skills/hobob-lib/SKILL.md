---
name: hobob-lib
description: 理解 hobob/src/lib.rs（crate 入口）的结构、启动流程与宏定义。当需要修改 hobob 的启动逻辑、日志初始化、CLI 参数、vpath!/schema_uri! 宏，或梳理 main_loop 中 www/engine 任务的启动与优雅关闭时序时调用。
---

# hobob/src/lib.rs

## 概述
`hobob/src/lib.rs` 是项目的主库文件，包含核心功能的定义和配置。

## 主要功能
1. **日志初始化**：
   - 使用 `log4rs` 初始化日志配置
   - 创建日志目录和配置文件
   - 提供日志相关的宏定义（vpath! 和 schema_uri!）

2. **模块导入和配置**：
   - 导入多个子模块（bench, data_schema, db, engine, vm, www, chunk + lalrpop 生成的 chunkir）
   - 使用 Lalrpop 定义解析器（chunkir）

3. **命令行参数解析**：
   - 定义 `Flags` 结构体用于处理命令行参数
   - 包含 HTTP 服务器端口配置（默认 3731）

4. **主循环和任务启动**：
   - 启动 Web 服务器（www::build_app，warp）和引擎任务（engine::main_loop）
   - 处理程序退出信号（Ctrl+C），触发 WeiYuanHui::close() 优雅关闭（最多等 30s）

## 核心结构体
### Flags
- `port`: HTTP 服务器端口号，默认为 3731

### 主要函数
1. `prepare_log()`：
   - 初始化日志系统
   - 创建日志目录，若 `~/log4rs.yml` 不存在则从 `assets/log4rs.yml` 复制模板并初始化

2. `main_loop()`：
   - 从 `~/bench.json` 加载状态（`WeiYuanHui::load`）
   - 启动 Web 服务器和引擎任务
   - 处理 Ctrl+C 与优雅关闭

### 宏
- `vpath!`：运行时文件路径（`~/`、`~/log4rs.yml`、`~/bench.json`），基于 `home_dir()`
- `schema_uri!`：远程 JSON schema URL（`https://lintd.xyz/hobob/{id}.json`）

## 第三方依赖
- `anyhow`：错误处理
- `clap`：命令行参数解析
- `lalrpop-util`：解析器生成（chunkir）
- `log4rs`：日志系统
- `tokio`：异步运行时
- `warp`：Web 服务器框架

## 入口链
`src/main.rs`（bin）→ `prepare_log()` → `main_loop()`；main_loop 内部 spawn `www` 服务与 `engine` 主循环，二者共享 `WeiYuanHui` 的 chair 句柄。

# hobob/src/lib.rs

## 概述
`hobob/src/lib.rs` 是项目的主库文件，包含核心功能的定义和配置。

## 主要功能
1. **日志初始化**：
   - 使用 `log4rs` 初始化日志配置
   - 创建日志目录和配置文件
   - 提供日志相关的宏定义（vpath! 和 schema_uri!）

2. **模块导入和配置**：
   - 导入多个子模块（bench, data_schema, db, engine, vm, www）
   - 使用 Lalrpop 定义解析器（chunkir）

3. **命令行参数解析**：
   - 定义 `Flags` 结构体用于处理命令行参数
   - 包含 HTTP 服务器端口配置

4. **主循环和任务启动**：
   - 启动 Web 服务器和引擎任务
   - 处理程序退出信号（Ctrl+C）
   - 提供优雅的关闭逻辑

## 核心结构体
### Flags
- `port`: HTTP 服务器端口号，默认为 3731

### 主要函数
1. `prepare_log()`：
   - 初始化日志系统
   - 创建日志目录和配置文件

2. `main_loop()`：
   - 启动主程序逻辑
   - 包含 Web 服务器和引擎的任务启动
   - 处理程序关闭逻辑

## 第三方依赖
- `anyhow`：用于错误处理
- `clap`：用于命令行参数解析
- `lalrpop-util`：用于解析器生成
- `log4rs`：用于日志系统
- `tokio`：用于异步运行时
- `warp`：用于 Web 服务器框架

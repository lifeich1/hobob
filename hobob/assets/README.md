# hobob/assets — 运行时资源

| 文件 | 用途 |
| --- | --- |
| `log4rs.yml` | log4rs 日志配置模板。首次启动由 `lib.rs::prepare_log` 复制到 `~/log4rs.yml`，此后改运行时配置应改 `~/log4rs.yml`（本目录文件只是模板）。 |
| `db_init.sql` | SQLite 建表脚本（userinfo/usersync/videoinfo/…）。**遗留文件**：当前数据层（`db` 模块）以 `~/bench.json` JSON 持久化，不用 SQLite；仅保留作历史参考，勿据此推断现状。 |

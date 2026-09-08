//! M2 T4：KV 日志端到端（独立 binary，与 tests/logkv_global.rs 不共享进程、互不污染全局 TX/RX 静态）。
//!
//! 覆盖 N4/N5/N7 的完整链路：真实 log4rs logger（KV appender 挂 `hobob` logger，additive:false）
//! → 进程日志（黑/白名单）+ hub 业务镜像（chair.log → `Snapshot::log` → `mirror_op`）
//! → hub drain（open 时 take 全局 RX）→ kv:log 落盘 → reopen 查询断言。
//!
//! 注意：log4rs 全局 logger 与 logkv 全局 TX/RX 静态在进程内均只能初始化一次，
//! 故所有断言合并到单个 `#[tokio::test]`，不可拆成多个并行测试。

use hobob::db::WeiYuanHui;
use hobob::logkv::{KvAppender, KvAppenderConfig, KvAppenderDeserializer};
use hobob::store::{LogQuery, Store, StoreConfig};
use log4rs::config::Deserialize as Log4rsDeserialize;
use log4rs::config::{Appender, Config, Deserializers, Logger, Root};
use log::LevelFilter;
use tempfile::tempdir;

#[tokio::test]
async fn end_to_end_log4rs_and_hub_mirror_into_kv() {
    // 1) KV appender 走生产反序列化路径 → 设全局 TX/RX 静态
    let app = KvAppenderDeserializer
        .deserialize(
            KvAppenderConfig { capacity: Some(64) },
            &Deserializers::new(),
        )
        .expect("kv appender deserialize ok");
    assert!(KvAppender::sender().is_some(), "全局 sender 已安装");

    // 2) 真实 log4rs logger：KV appender 只挂 `hobob` logger（additive:false），root 不动
    let config = Config::builder()
        .appender(Appender::builder().build("kv", app))
        .logger(
            Logger::builder()
                .appender("kv")
                .additive(false)
                .build("hobob", LevelFilter::Info),
        )
        .build(Root::builder().build(LevelFilter::Off))
        .expect("log4rs config build");
    log4rs::init_config(config).expect("log4rs init ok");

    // 3) 临时 store + hub：生产路径 open 后显式 attach_logkv（take 全局 RX 作 log_rx）
    let dir = tempdir().unwrap();
    let cfg = StoreConfig::new(dir.path().join("state.redb"));
    let store = Store::open_or_create(&cfg).unwrap();
    let mut center = WeiYuanHui::open(store).expect("hub open ok");
    center.attach_logkv().expect("attach logkv ok");

    // 4) 进程日志：白名单 `hobob::db` 应入 KV；黑名单 `hobob::store`/`hobob::logkv` 不应入
    log::info!(target: "hobob::db", "e2e info message");
    log::warn!(target: "hobob::store", "store noise must be filtered");
    log::error!(target: "hobob::logkv", "logkv noise must be filtered");

    // 5) 业务镜像：chair.log → update → `Snapshot::log` → mirror_op（target="op"）。
    //    v1 业务级别：0 最重（ERROR），3 → DEBUG 域。
    let mut chair = center.new_chair();
    chair.log(3, "biz debug mirror");
    chair.log(0, "biz fatal mirror");
    assert!(center.run().await, "hub run 处理 update + drain");
    center.close(); // last-drain + store 强刷

    // 6) reopen 查询断言
    let store2 = Store::open_or_create(&cfg).unwrap();

    // N5 双写同源：hobob::db 记录存在，msg/level/ts/loc 与进程日志一致
    let db_logs = store2
        .query_logs(&LogQuery {
            target_prefix: Some("hobob::db"),
            ..LogQuery::default()
        })
        .unwrap();
    let e2e = db_logs
        .iter()
        .find(|(_, r)| r.msg == "e2e info message")
        .expect("hobob::db 记录已落盘");
    assert_eq!(e2e.1.level, 3, "log::info → level=3");
    assert_eq!(e2e.1.ctx, "", "ctx 本期恒空");
    assert!(e2e.1.loc.is_some(), "loc 带 file:line");
    assert!(e2e.1.ts_ms > 0, "ts_ms 为 Unix 毫秒");

    // N4 防递归：黑名单 target 零记录（store/logkv 自身日志只走文件）
    assert!(
        store2
            .query_logs(&LogQuery {
                target_prefix: Some("hobob::store"),
                ..LogQuery::default()
            })
            .unwrap()
            .is_empty(),
        "store 模块日志不入 KV"
    );
    assert!(
        store2
            .query_logs(&LogQuery {
                target_prefix: Some("hobob::logkv"),
                ..LogQuery::default()
            })
            .unwrap()
            .is_empty(),
        "logkv 模块日志不入 KV"
    );

    // N7 业务镜像：target="op"，v1 级别映射正确，最新在前
    let op_logs = store2
        .query_logs(&LogQuery {
            target_prefix: Some("op"),
            ..LogQuery::default()
        })
        .unwrap();
    assert_eq!(op_logs.len(), 2, "两条业务日志均镜像入 KV");
    assert_eq!(op_logs[0].1.msg, "biz fatal mirror", "最新在前");
    assert_eq!(op_logs[0].1.level, 1, "v1 level 0 → ERROR(1)");
    assert_eq!(op_logs[1].1.msg, "biz debug mirror");
    assert_eq!(op_logs[1].1.level, 4, "v1 level 3 → DEBUG(4)");
    assert_eq!(op_logs[1].1.loc, None, "业务镜像 loc 恒空");
}

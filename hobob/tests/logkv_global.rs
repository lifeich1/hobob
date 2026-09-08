//! M2 T3：logkv 全局通道集成测试（独立进程，不与 lib 测试并行共享 TX/RX 静态）。
//!
//! 依赖全局 TX/RX 静态的断言放这里：lib 测试（db 层 test_weiyuan_log 等会触发
//! `Snapshot::log` → `mirror_op` 推入全局 TX）若与它们共享同一进程会造成条目污染。
//! 集成测试文件编译为独立 binary，无交叉污染。
//!
//! 注意：全局 TX 是 `OnceLock`（只能 set 一次）、RX 只能被 take 一次，因此所有
//! 需要全局通道的断言**必须合并在单个测试函数**内按序执行，不可拆成多个并行测试。

use hobob::logkv::{self, KvAppender, KvAppenderConfig, KvAppenderDeserializer, LogEntry};
use log4rs::config::Deserialize as Log4rsDeserialize;
use log4rs::config::Deserializers;
use std::time::Duration;

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 走生产路径（`Deserializers` 注册 + 反序列化构造）验证全局 TX/RX 接线，
/// 并验证业务镜像 `mirror_op` 产生 `target="op"` 且级别映射正确。
#[test]
fn global_channel_wiring_and_mirror_op() {
    let d = KvAppenderDeserializer;
    let app = d
        .deserialize(
            KvAppenderConfig { capacity: Some(8) },
            &Deserializers::new(),
        )
        .expect("deserialize ok");
    let rx = logkv::take_receiver().expect("global receiver installed");

    // --- appender 走 log4rs Record 路径 ---
    let rec = log::Record::builder()
        .args(format_args!("via deserializer"))
        .level(log::Level::Error)
        .target("hobob::www")
        .file(Some("hobob/src/logkv.rs"))
        .line(Some(42))
        .build();
    app.append(&rec).expect("append ok");
    let entry = rx
        .recv_timeout(Duration::from_secs(1))
        .expect("entry received");
    assert_eq!(entry.msg, "via deserializer");
    assert_eq!(entry.level, 1);

    // --- 业务镜像 try_push 走同一全局 sender ---
    assert!(KvAppender::sender().is_some());
    assert!(KvAppender::try_push(LogEntry {
        ts_ms: now_ms(),
        level: 3,
        target: "op".to_owned(),
        msg: "mirror push".to_owned(),
        loc: None,
        ctx: String::new(),
    }));
    let mirrored = rx
        .recv_timeout(Duration::from_secs(1))
        .expect("mirror entry");
    assert_eq!(mirrored.target, "op");
    assert_eq!(mirrored.msg, "mirror push");

    // --- mirror_op（T3 业务镜像挂点）: v1 级别 0（最重）→ level=1 ---
    assert!(hobob::logkv::mirror_op(0, "fatal error"));
    let entry = rx
        .recv_timeout(Duration::from_secs(1))
        .expect("entry received");
    assert_eq!(entry.target, "op");
    assert_eq!(entry.level, 1);
    assert_eq!(entry.msg, "fatal error");
    assert_eq!(entry.loc, None);
    assert_eq!(entry.ctx, "");

    // --- mirror_op: v1 级别 4（最轻）→ level=5 ---
    assert!(hobob::logkv::mirror_op(4, "trace noise"));
    let entry = rx
        .recv_timeout(Duration::from_secs(1))
        .expect("entry received");
    assert_eq!(entry.target, "op");
    assert_eq!(entry.level, 5);
    assert_eq!(entry.msg, "trace noise");
}

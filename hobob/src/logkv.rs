//! M2：日志 KV 镜像通道（D-A `logkv.rs`，见 `.plans/m2-log-kv-appender.md`）。
//!
//! 职责：log4rs 自定义 appender（`kind: hobob_kv`）与业务日志镜像（`target="op"`，
//! T3 接 db hub `log()`）共用一条 `sync_channel`；hub 主循环 drain 把通道条目
//! 批量写入 `kv:log` 表（T3 接线）。
//!
//! 防递归（D4，本模块硬性约束）：
//! - 本模块与 appender 内**零 `log` crate 调用**，错误只计数（`store::LOG_DROPPED`）+
//!   `eprintln!` 直写 stderr；
//! - `hobob::store`/`hobob::logkv` 前缀 target 黑名单不入 KV（只走文件 appender）。
//!
//! 满/断连不阻塞调用方：`try_send` 失败只计数丢弃（文件日志始终实时，KV 仅是镜像）。

use crate::store::{LogRecordV1, Store, LOG_DROPPED};
use anyhow::Result;
use log::Record;
use log4rs::append::Append;
use log4rs::config::{Deserialize as Log4rsDeserialize, Deserializers};
use serde::Deserialize;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::{Mutex, OnceLock};

/// 通道容量默认值（与 `assets/log4rs.yml` 的 `hobob_kv.capacity` 保持一致）。
pub const DEFAULT_CAPACITY: usize = 8192;

/// KV 日志留存上限默认值（D5：CLI `--log-kv-max` / env `HOBOB_LOG_KV_MAX` 缺省值）。
pub const DEFAULT_KV_LOG_MAX: u64 = 20_000;

/// 防递归黑名单前缀（D4）：这些 target 的日志只走文件、不入 KV。
const BLACKLIST_PREFIXES: &[&str] = &["hobob::store", "hobob::logkv"];

/// 入 KV 通道条目（hub drain 时转 `LogRecordV1` 落盘）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEntry {
    /// Unix 毫秒。
    pub ts_ms: i64,
    /// 1=ERROR 2=WARN 3=INFO 4=DEBUG 5=TRACE（与 `LogRecordV1.level` 同构）。
    pub level: u8,
    /// 模块路径（log4rs `record.target()`）；业务镜像 = `"op"`。
    pub target: String,
    pub msg: String,
    /// `"file:line"`（可选）。
    pub loc: Option<String>,
    /// 业务上下文 JSON 文本（本期恒空串；M3/M5 填充点）。
    pub ctx: String,
}

impl From<LogEntry> for LogRecordV1 {
    fn from(e: LogEntry) -> Self {
        Self {
            ts_ms: e.ts_ms,
            level: e.level,
            target: e.target,
            msg: e.msg,
            loc: e.loc,
            ctx: e.ctx,
        }
    }
}

/// 全局 sender：appender 与业务镜像共用（`SyncSender: Send + Sync`）。
static TX: OnceLock<SyncSender<LogEntry>> = OnceLock::new();
/// 全局 receiver：hub 在 store 就绪后 `take_receiver()` 取走一次（`Receiver: !Sync`）。
static RX: Mutex<Option<Receiver<LogEntry>>> = Mutex::new(None);

/// log4rs 级别 → u8：1=ERROR 2=WARN 3=INFO 4=DEBUG 5=TRACE。
pub fn level_to_u8(level: log::Level) -> u8 {
    match level {
        log::Level::Error => 1,
        log::Level::Warn => 2,
        log::Level::Info => 3,
        log::Level::Debug => 4,
        log::Level::Trace => 5,
    }
}

/// 黑名单判定：精确模块边界（`hobob::store` 与 `hobob::storefront` 不同）。
fn is_blacklisted(target: &str) -> bool {
    BLACKLIST_PREFIXES.iter().any(|p| {
        target == *p
            || target
                .strip_prefix(p)
                .is_some_and(|rest| rest.starts_with("::"))
    })
}

/// Unix 毫秒（不引入 chrono，store 同样无此依赖）。
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// KV appender（log4rs 自定义 kind `hobob_kv`）。
///
/// `append()` 内**零 `log` crate 调用**（防递归 D4）；`try_send` 失败只计数 +
/// `eprintln!`，返回 Ok 不抛错（log4rs 内部错误路径也不会触发日志）。
#[derive(Debug)]
pub struct KvAppender {
    tx: SyncSender<LogEntry>,
}

impl KvAppender {
    /// 测试直构：新建独立通道，不触碰全局静态（各测试互不污染）。
    #[cfg(test)]
    pub fn new_for_test(capacity: usize) -> (Self, Receiver<LogEntry>) {
        let (tx, rx) = sync_channel(capacity);
        (Self { tx }, rx)
    }

    /// 全局 sender（业务镜像 db hub `log()` 用；未初始化时 None）。
    pub fn sender() -> Option<SyncSender<LogEntry>> {
        TX.get().cloned()
    }

    /// hub 在 store 就绪后调用一次（T3）：take 全局 receiver。
    pub fn take_receiver() -> Option<Receiver<LogEntry>> {
        RX.lock().unwrap().take()
    }

    /// 业务镜像/测试入队：满或断连返回 false 并计数，不阻塞。
    pub fn try_push(entry: LogEntry) -> bool {
        match TX.get() {
            Some(tx) => match tx.try_send(entry) {
                Ok(()) => true,
                Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                    LOG_DROPPED.fetch_add(1, Ordering::Relaxed);
                    false
                }
            },
            None => false,
        }
    }
}

/// free function 别名：`KvAppender::try_push`（D10，供 `mirror_op` 及外部调用）。
pub fn try_push(entry: LogEntry) -> bool {
    KvAppender::try_push(entry)
}

/// free function 别名：`KvAppender::take_receiver`（D10，供 db hub `attach_logkv` 调用）。
pub fn take_receiver() -> Option<Receiver<LogEntry>> {
    KvAppender::take_receiver()
}

/// 业务级别（v1：0 最重）→ KV u8（1=ERROR … 5=TRACE，D8）：`i32 + 1` 钳制到 [1,5]。
pub fn business_level_to_u8(level: i32) -> u8 {
    level.saturating_add(1).clamp(1, 5) as u8
}

/// 业务日志镜像（T3 挂 db hub `Snapshot::log`，D2）：`target = "op"`、`ctx` 恒空。
/// 仅镜像通过 v1 过滤（maxlv/bufl/fitl）后实际入 `res.logs` 的条目；满/断连返回
/// false 并计数，不阻塞调用方（`res.logs` 权威在内存，镜像尽力而为）。
pub fn mirror_op(level: i32, msg: &str) -> bool {
    try_push(LogEntry {
        ts_ms: now_ms(),
        level: business_level_to_u8(level),
        target: "op".to_owned(),
        msg: msg.to_owned(),
        loc: None,
        ctx: String::new(),
    })
}

/// hub 每轮 drain（T3，编排收本模块 D10）：非阻塞收尽 `rx` → 攒批（seq 从 `*seq`
/// 连续分配，调用方持有并初始化）→ `append_logs` 单事务写 → 超限 `trim_logs`。
/// 返回写入条数。
///
/// 失败语义（D4/D6）：`append_logs` 写失败计 `LOG_DROPPED` 丢弃该批、`eprintln!`
/// 诊断，不重试不 panic 不记日志（下轮 drain 自然续跑）；`trim_logs` 失败仅诊断
/// （无数据丢失，下轮自动重试）。`rx`/`seq` 走参数注入：hub 持状态、测试可隔离，
/// 避免触碰全局 `TX`/`RX` 静态造成跨测试竞态。
pub fn drain_logs(
    rx: &mut Receiver<LogEntry>,
    store: &mut Store,
    seq: &mut u64,
    kv_max: u64,
) -> usize {
    let mut batch: Vec<(u64, LogRecordV1)> = Vec::new();
    while let Ok(entry) = rx.try_recv() {
        let s = *seq;
        *seq = s.wrapping_add(1);
        batch.push((s, entry.into()));
    }
    let n = batch.len();
    if n == 0 {
        return 0;
    }
    if let Err(e) = store.append_logs(&batch) {
        LOG_DROPPED.fetch_add(n as u64, Ordering::Relaxed);
        eprintln!("[logkv] drain append_logs failed, dropped {n}: {e:#}");
        return 0;
    }
    if kv_max > 0 {
        if let Err(e) = store.trim_logs(kv_max) {
            eprintln!("[logkv] drain trim_logs failed: {e:#}");
        }
    }
    n
}

impl Append for KvAppender {
    fn append(&self, record: &Record) -> Result<()> {
        let target = record.target();
        if is_blacklisted(target) {
            return Ok(());
        }
        let entry = LogEntry {
            ts_ms: now_ms(),
            level: level_to_u8(record.level()),
            target: target.to_owned(),
            msg: format!("{}", record.args()),
            loc: record
                .file()
                .map(|f| format!("{f}:{}", record.line().unwrap_or(0))),
            ctx: String::new(),
        };
        if let Err(e) = self.tx.try_send(entry) {
            LOG_DROPPED.fetch_add(1, Ordering::Relaxed);
            eprintln!("[logkv] kv appender try_send failed: {e}");
        }
        Ok(())
    }

    fn flush(&self) {}
}

/// appender 配置（yml `kind: hobob_kv` 的 config 块）。
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct KvAppenderConfig {
    /// 通道容量；缺省 `DEFAULT_CAPACITY`。
    pub capacity: Option<usize>,
}

/// log4rs 自定义 kind 反序列化器（`Deserializers::insert("hobob_kv", ...)` 注册）。
#[derive(Debug, Default)]
pub struct KvAppenderDeserializer;

impl Log4rsDeserialize for KvAppenderDeserializer {
    type Trait = dyn Append;
    type Config = KvAppenderConfig;

    fn deserialize(
        &self,
        config: Self::Config,
        _deserializers: &Deserializers,
    ) -> Result<Box<dyn Append>> {
        let capacity = config.capacity.unwrap_or(DEFAULT_CAPACITY);
        let tx = match TX.get() {
            // 已有全局通道（refresh_rate 重载等二次构造）：复用，避免 drain 侧断连
            Some(tx) => tx.clone(),
            None => {
                let (tx, rx) = sync_channel(capacity);
                TX.set(tx.clone()).ok();
                let mut g = RX.lock().unwrap();
                if g.is_none() {
                    *g = Some(rx);
                }
                tx
            }
        };
        Ok(Box::new(KvAppender { tx }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use log::Record;
    use std::time::Duration;

    /// Record<'static> 辅助构造（msg 必须为字面量，使 format_args 产生 'static lifetime）。
    macro_rules! record {
        ($level:expr, $target:expr, $msg:expr) => {
            Record::builder()
                .args(format_args!($msg))
                .level($level)
                .target($target)
                .file(Some("hobob/src/logkv.rs"))
                .line(Some(42))
                .build()
        };
    }

    #[test]
    fn level_mapping_matches_d4() {
        assert_eq!(level_to_u8(log::Level::Error), 1);
        assert_eq!(level_to_u8(log::Level::Warn), 2);
        assert_eq!(level_to_u8(log::Level::Info), 3);
        assert_eq!(level_to_u8(log::Level::Debug), 4);
        assert_eq!(level_to_u8(log::Level::Trace), 5);
    }

    #[test]
    fn blacklist_precise_module_boundary() {
        assert!(is_blacklisted("hobob::store"));
        assert!(is_blacklisted("hobob::store::meta"));
        assert!(is_blacklisted("hobob::logkv"));
        assert!(!is_blacklisted("hobob::db"));
        assert!(!is_blacklisted("hobob::storefront"));
        assert!(!is_blacklisted("op"));
    }

    #[test]
    fn appender_forwards_struct_entry() {
        let (app, rx) = KvAppender::new_for_test(16);
        let rec = record!(log::Level::Info, "hobob::db", "hello kv");
        app.append(&rec).expect("append ok");
        let entry = rx
            .recv_timeout(Duration::from_secs(1))
            .expect("entry received");
        assert_eq!(entry.level, 3);
        assert_eq!(entry.target, "hobob::db");
        assert_eq!(entry.msg, "hello kv");
        assert_eq!(entry.loc.as_deref(), Some("hobob/src/logkv.rs:42"));
        assert!(entry.ts_ms > 0);
        assert_eq!(entry.ctx, "");
    }

    #[test]
    fn blacklisted_target_skipped() {
        let (app, rx) = KvAppender::new_for_test(16);
        let rec = record!(log::Level::Warn, "hobob::store", "should not enter kv");
        app.append(&rec).expect("append ok");
        assert!(
            rx.try_recv().is_err(),
            "blacklisted target must not enter channel"
        );
    }

    #[test]
    fn full_channel_drops_and_counts() {
        let before = LOG_DROPPED.load(Ordering::Relaxed);
        let (app, rx) = KvAppender::new_for_test(1);
        app.append(&record!(log::Level::Info, "hobob::db", "first"))
            .expect("first append ok");
        app.append(&record!(log::Level::Info, "hobob::db", "second"))
            .expect("full append must not error");
        assert_eq!(LOG_DROPPED.load(Ordering::Relaxed), before + 1);
        let got = rx
            .recv_timeout(Duration::from_secs(1))
            .expect("first entry intact");
        assert_eq!(got.msg, "first");
    }

    #[test]
    fn template_has_hobob_kv_appender() {
        let tpl = include_str!("../assets/log4rs.yml");
        assert!(
            tpl.contains("kind: hobob_kv"),
            "template must define hobob_kv appender"
        );
        // hobob logger 挂载（root 不动）
        let hobob_logger = tpl
            .split("loggers:")
            .nth(1)
            .expect("loggers section")
            .split("additive: false")
            .next()
            .unwrap_or("");
        assert!(
            hobob_logger.contains("hobob_kv"),
            "hobob logger must attach hobob_kv"
        );
    }

    // ---- M2 T3：业务级别映射 + mirror_op + drain 编排 ----

    #[test]
    fn business_level_mapping_d8() {
        // v1 语义：0 最重 → ERROR(1), 4 最轻 → TRACE(5)
        assert_eq!(business_level_to_u8(0), 1, "level 0 → ERROR");
        assert_eq!(business_level_to_u8(1), 2, "level 1 → WARN");
        assert_eq!(business_level_to_u8(2), 3, "level 2 → INFO");
        assert_eq!(business_level_to_u8(3), 4, "level 3 → DEBUG");
        assert_eq!(business_level_to_u8(4), 5, "level 4 → TRACE");
        // 越界钳制
        assert_eq!(business_level_to_u8(-1), 1, "level -1 clamped to 1");
        assert_eq!(business_level_to_u8(5), 5, "level 5 clamped to 5");
        assert_eq!(business_level_to_u8(99), 5, "level 99 clamped to 5");
    }

    #[test]
    fn drain_logs_writes_batch_and_trims() {
        // 独立通道 + tempdir store，不碰全局 TX/RX
        let (tx, mut rx) = sync_channel::<LogEntry>(16);
        let dir = tempfile::tempdir().unwrap();
        let cfg = crate::store::StoreConfig::new(dir.path().join("test.redb"));
        let mut store = crate::store::Store::open_or_create(&cfg).unwrap();

        let mut seq = 1u64;
        // 推 3 条，不裁剪
        for i in 0..3 {
            tx.send(LogEntry {
                ts_ms: 1000 + i,
                level: 3,
                target: "op".to_owned(),
                msg: format!("msg-{i}"),
                loc: None,
                ctx: String::new(),
            })
            .unwrap();
        }
        let written = drain_logs(&mut rx, &mut store, &mut seq, 0);
        assert_eq!(written, 3);

        let all = store
            .query_logs(&crate::store::LogQuery::default())
            .unwrap();
        assert_eq!(all.len(), 3);
        // seq 从 1 开始
        assert_eq!(all[0].0, 3, "latest seq=3");
        assert_eq!(all[2].0, 1, "oldest seq=1");
        assert_eq!(all[0].1.msg, "msg-2");
        assert_eq!(all[2].1.msg, "msg-0");

        // seq 续号（空洞？不，是连续递增，从 4 开始）
        assert_eq!(seq, 4, "seq continued to 4");
    }

    #[test]
    fn drain_logs_trims_by_kv_max() {
        let (tx, mut rx) = sync_channel::<LogEntry>(16);
        let dir = tempfile::tempdir().unwrap();
        let cfg = crate::store::StoreConfig::new(dir.path().join("test.redb"));
        let mut store = crate::store::Store::open_or_create(&cfg).unwrap();

        let mut seq = 1u64;
        // 推 5 条，kv_max=3 → 裁剪保留最新 3 条
        for i in 0..5 {
            tx.send(LogEntry {
                ts_ms: 1000 + i,
                level: 3,
                target: "op".to_owned(),
                msg: format!("msg-{i}"),
                loc: None,
                ctx: String::new(),
            })
            .unwrap();
        }
        let written = drain_logs(&mut rx, &mut store, &mut seq, 3);
        assert_eq!(written, 5);

        let all = store
            .query_logs(&crate::store::LogQuery::default())
            .unwrap();
        assert_eq!(all.len(), 3, "trimmed to 3");
        // 保留最新 3 条：seq 3,4,5
        assert_eq!(all[0].0, 5, "newest seq=5");
        assert_eq!(all[2].0, 3, "oldest after trim seq=3");
    }
}

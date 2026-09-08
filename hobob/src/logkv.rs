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

use crate::store::{LogRecordV1, LOG_DROPPED};
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
        target == *p || target.strip_prefix(p).is_some_and(|rest| rest.starts_with("::"))
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

    /// 全局静态互斥：触碰 `TX`/`RX` 的测试串行执行。
    static TEST_LOCK: Mutex<()> = Mutex::new(());

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
        let entry = rx.recv_timeout(Duration::from_secs(1)).expect("entry received");
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
        assert!(rx.try_recv().is_err(), "blacklisted target must not enter channel");
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
        let got = rx.recv_timeout(Duration::from_secs(1)).expect("first entry intact");
        assert_eq!(got.msg, "first");
    }

    #[test]
    fn deserializer_wires_global_channel() {
        // 走生产路径（Deserializers 注册 + 反序列化构造），验证全局 TX/RX 接线；
        // 与业务镜像 try_push 同通道。
        let _guard = TEST_LOCK.lock().unwrap();
        let d = KvAppenderDeserializer;
        let app = d
            .deserialize(
                KvAppenderConfig {
                    capacity: Some(8),
                },
                &Deserializers::new(),
            )
            .expect("deserialize ok");
        let rx = KvAppender::take_receiver().expect("global receiver installed");
        let rec = record!(log::Level::Error, "hobob::www", "via deserializer");
        app.append(&rec).expect("append ok");
        let entry = rx.recv_timeout(Duration::from_secs(1)).expect("entry received");
        assert_eq!(entry.msg, "via deserializer");
        assert_eq!(entry.level, 1);

        // 业务镜像 try_push 走同一全局 sender
        assert!(KvAppender::sender().is_some());
        assert!(KvAppender::try_push(LogEntry {
            ts_ms: now_ms(),
            level: 3,
            target: "op".to_owned(),
            msg: "mirror push".to_owned(),
            loc: None,
            ctx: String::new(),
        }));
        let mirrored = rx.recv_timeout(Duration::from_secs(1)).expect("mirror entry");
        assert_eq!(mirrored.target, "op");
        assert_eq!(mirrored.msg, "mirror push");
    }

    #[test]
    fn template_has_hobob_kv_appender() {
        let tpl = include_str!("../assets/log4rs.yml");
        assert!(tpl.contains("kind: hobob_kv"), "template must define hobob_kv appender");
        // hobob logger 挂载（root 不动）
        let hobob_logger = tpl
            .split("loggers:")
            .nth(1)
            .expect("loggers section")
            .split("additive: false")
            .next()
            .unwrap_or("");
        assert!(hobob_logger.contains("hobob_kv"), "hobob logger must attach hobob_kv");
    }
}

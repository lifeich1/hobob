//! M4/T1 保活模块：主循环活跃度计数器 + zbus 心跳 client。
//!
//! - [`Liveness`]：单调毫秒计数器（进程启动 `Instant` 起 elapsed），由 `main_loop`
//!   的 1s tick 分支 `touch`。**非墙钟**——NTP 回拨会让墙钟 seq 变小/持平，
//!   无 RTC 的 ARM 设备会误判停滞（`.plans/m4-keepalive-watchdog.md` D4）。
//! - [`run_heartbeat`]：独立 tokio 任务，按 interval 读计数器发 `Heartbeat(seq)`；
//!   连不上 watchdog 仅告警退避重连，不 fatal（D2）；单轮调用外包
//!   [`protocol::HEARTBEAT_TIMEOUT`]，卡死只影响单轮心跳。
//! - [`send_bye`]：优雅停机通知。`main_loop` 进入 shutdown 分支**立即**发，先于
//!   `close()`——强刷期间 tick 已停、seq 冻结，`close()` 耗时接近 `stall_timeout`
//!   时 watchdog 会对优雅停机中的 hobob 发起拉起（D7）。
//!
//! watchdog 侧（service/supervisor/状态机）见 `hobob_watchdog`（M4/T2/T3，未建）；
//! 协议常量在 [`protocol`]，两侧各持一份、以 D3 为准。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use zbus::connection::Builder;

/// D-Bus 目标 bus（D3）：默认 system；调试用 session，两侧必须一致。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum Bus {
    /// system bus（部署默认；需 `etc/dbus-1/xyz.lintd.hobob.conf` policy）
    #[default]
    System,
    /// session bus（开发调试）
    Session,
}

impl std::fmt::Display for Bus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::System => "system",
            Self::Session => "session",
        })
    }
}

/// D-Bus 协议常量与默认参数（D3/D8，watchdog 侧同名实现）。
pub mod protocol {
    use std::time::Duration;

    /// watchdog 持有的 well-known bus name。
    pub const WATCHDOG_NAME: &str = "xyz.lintd.hobob.Watchdog";
    /// watchdog 对象路径。
    pub const WATCHDOG_PATH: &str = "/xyz/lintd/hobob/Watchdog";
    /// watchdog 接口名。
    pub const WATCHDOG_IFACE: &str = "xyz.lintd.hobob.Watchdog1";

    /// 心跳周期默认值（D8；`0 = 禁用心跳`）。
    pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(2);
    /// 活跃度 tick 周期（`main_loop` select! 分支，D4）。
    pub const TICK: Duration = Duration::from_secs(1);
    /// 单轮 Heartbeat/Bye 调用超时（卡死不阻塞心跳循环/停机流程）。
    pub const RPC_TIMEOUT: Duration = Duration::from_secs(2);
    /// 心跳连接退避重连起点（D2：连接失败不 fatal）。
    pub const RECONNECT_MIN: Duration = Duration::from_secs(1);
    /// 心跳连接退避重连上限。
    pub const RECONNECT_MAX: Duration = Duration::from_secs(30);
}

fn start_instant() -> Instant {
    static START: OnceLock<Instant> = OnceLock::new();
    *START.get_or_init(Instant::now)
}

fn monotonic_ms() -> u64 {
    u64::try_from(start_instant().elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// 心跳是否启用（D8）：`interval == 0` 禁用（不 spawn 心跳任务、不发 Bye）。
#[must_use]
pub fn enabled(interval: Duration) -> bool {
    !interval.is_zero()
}

/// 主循环活跃度计数器（D4）：单调毫秒，仅由 `main_loop` tick `touch`。
///
/// 心跳任务只 `get`；hub 停滞时计数器冻结 → seq 不前进 → watchdog 判停滞（D5）。
#[derive(Clone)]
pub struct Liveness(Arc<AtomicU64>);

impl Default for Liveness {
    fn default() -> Self {
        Self::new()
    }
}

impl Liveness {
    #[must_use]
    pub fn new() -> Self {
        Self(Arc::new(AtomicU64::new(monotonic_ms())))
    }

    /// tick 分支调用：刷新为当前单调毫秒。
    pub fn touch(&self) {
        self.0.store(monotonic_ms(), Ordering::Relaxed);
    }

    /// 当前单调毫秒（心跳 seq）。
    #[must_use]
    pub fn get(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

/// zbus proxy：watchdog `Watchdog1` 接口的 client 端（D3）。
#[zbus::proxy(
    interface = "xyz.lintd.hobob.Watchdog1",
    default_service = "xyz.lintd.hobob.Watchdog",
    default_path = "/xyz/lintd/hobob/Watchdog"
)]
trait Watchdog1 {
    /// hobob 周期心跳；seq = 活跃度单调毫秒（D4）。
    fn heartbeat(&self, seq: u64) -> zbus::Result<()>;
    /// 优雅停机通知；watchdog 置 Bye 不再拉起（D7）。
    fn bye(&self, reason: &str) -> zbus::Result<()>;
}

/// 建立到 watchdog 的 zbus 连接（D3：bus 由 `--bus`/`HOBOB_BUS` 决定，
/// 开发调试时两侧必须指到同一 bus，否则 system bus 上无 policy 必连不上）。
async fn connect(bus: Bus) -> zbus::Result<Watchdog1Proxy<'static>> {
    let builder = match bus {
        Bus::System => Builder::system()?,
        Bus::Session => Builder::session()?,
    };
    let conn = builder.build().await?;
    Watchdog1Proxy::builder(&conn).build().await
}

/// 心跳发送循环（D2/D4/D8）：连接失败/调用失败均退避重连（1s→30s），不 fatal；
/// 连接成功后按 interval 发 `Heartbeat(seq)`，seq 取自 [`Liveness`]。
///
/// backoff 仅在 heartbeat 成功后重置——连接成功不算，避免 watchdog 已激活但
/// 调用持续失败时以固定 1Hz hammer D-Bus。
pub async fn run_heartbeat(liveness: Liveness, bus: Bus, interval: Duration) {
    let mut backoff = protocol::RECONNECT_MIN;
    loop {
        let proxy = match connect(bus).await {
            Ok(p) => p,
            Err(e) => {
                log::warn!("keepalive: connect watchdog failed ({e:#}); retry in {backoff:?}");
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(protocol::RECONNECT_MAX);
                continue;
            }
        };
        loop {
            tokio::time::sleep(interval).await;
            let seq = liveness.get();
            match tokio::time::timeout(protocol::RPC_TIMEOUT, proxy.heartbeat(seq)).await {
                Ok(Ok(())) => {
                    backoff = protocol::RECONNECT_MIN;
                    continue;
                }
                Ok(Err(e)) => log::warn!("keepalive: heartbeat failed ({e:#}); reconnect"),
                Err(_) => log::warn!("keepalive: heartbeat timeout; reconnect"),
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(protocol::RECONNECT_MAX);
            break;
        }
    }
}

/// 优雅停机通知（D7）：发不出仅 log，不影响退出；整包
/// [`protocol::RPC_TIMEOUT`] 兜底，防止 D-Bus 连接挂死拖住停机。
pub async fn send_bye(bus: Bus, reason: &str) {
    let send = async {
        match connect(bus).await {
            Ok(p) => {
                if let Err(e) = p.bye(reason).await {
                    log::warn!("keepalive: bye failed: {e:#}");
                }
            }
            Err(e) => log::warn!("keepalive: bye connect failed: {e:#}"),
        }
    };
    if tokio::time::timeout(protocol::RPC_TIMEOUT, send).await.is_err() {
        log::warn!("keepalive: bye timed out; continue shutdown anyway");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn liveness_touch_is_monotonic() {
        let l = Liveness::new();
        let before = l.get();
        std::thread::sleep(Duration::from_millis(5));
        l.touch();
        assert!(l.get() > before, "seq 必须单调前进（D4）");
    }

    #[test]
    fn liveness_clone_shares_counter() {
        let l = Liveness::new();
        let l2 = l.clone();
        l2.touch();
        assert_eq!(l.get(), l2.get(), "clone 共享同一计数器");
    }

    #[test]
    fn bus_default_is_system() {
        assert_eq!(Bus::default(), Bus::System, "D3：默认 system bus");
    }

    #[test]
    fn heartbeat_disabled_when_interval_zero() {
        assert!(!enabled(Duration::ZERO), "interval=0 必须禁用心跳（D8）");
        assert!(enabled(protocol::DEFAULT_INTERVAL));
    }
}

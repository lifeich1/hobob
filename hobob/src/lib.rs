use anyhow::{Context, Result};
use clap::Parser;
use log4rs::config::Deserializers;
use std::fs::File;
use std::io::BufWriter;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

macro_rules! vpath {
    () => {
        vpath(&VP::Rt)
    };
    (@log_cf) => {
        vpath(&VP::LogCf)
    };
}

enum VP {
    Rt,
    LogCf,
}

#[allow(deprecated)]
fn vpath(vp: &VP) -> String {
    let h = std::env::home_dir();
    let hp = h.expect("not found home dir");
    let hs = hp.to_str().expect("home dir path not utf8");
    format!(
        "{hs}{}",
        match vp {
            VP::Rt => "",
            VP::LogCf => "/log4rs.yml",
        }
    )
}
macro_rules! schema_uri {
    ($id:literal) => {
        concat!("https://lintd.xyz/hobob/", $id, ".json")
    };
    ($id:literal, $v:expr) => {
        &format!(concat!("https://lintd.xyz/hobob/", $id, "/{}.json"), $v)
    };
}

mod data_schema;
pub mod db;
pub mod ecs;
pub mod keepalive;
pub mod libcall;
pub mod logkv;
pub mod store;
pub mod systems;
pub mod www;

use db::WeiYuanHui;

/// # Errors
/// Throw log setup errors.
pub fn prepare_log() -> Result<()> {
    std::fs::create_dir_all(vpath!())
        .with_context(|| format!("faild create_dir_all {}", vpath!()))?;

    let lcf = vpath!(@log_cf);
    let log_cf = Path::new(&lcf);
    if !log_cf.exists() {
        let f = File::create(log_cf).with_context(|| format!("failed create file {log_cf:?}"))?;
        let mut w = BufWriter::new(f);
        let cf = include_str!("../assets/log4rs.yml");
        w.write_all(cf.as_bytes())
            .with_context(|| format!("failed write file {log_cf:?}"))?;
    }
    // M2（D11）：init 前读用户配置文本做陈旧检测（缺 hobob_kv 则 init 后 warn，不自动改写）
    let has_kv_appender = std::fs::read_to_string(log_cf)
        .map(|s| s.contains("hobob_kv"))
        .unwrap_or(false);
    // M2（D10）：注册自定义 appender kind `hobob_kv`（保留内置 kinds 的 Deserializers::new）
    let mut deserializers = Deserializers::new();
    deserializers.insert("hobob_kv", logkv::KvAppenderDeserializer);
    log4rs::init_file(log_cf, deserializers).context("failed init log config file")?;
    if !has_kv_appender {
        log::warn!(
            "{log_cf:?} 缺 hobob_kv appender，KV 日志镜像未启用；请按 hobob/assets/log4rs.yml 模板更新"
        );
    }

    log::info!(
        "{} version {}; logger prepared",
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_VERSION")
    );

    Ok(())
}

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Flags {
    /// Http server port
    #[arg(long, default_value_t = 3731)]
    port: u16,
    /// redb 状态文件路径；未指定时依次取 $HOBOB_STATE、$HOME/.hobob/state.redb
    #[arg(long, env = "HOBOB_STATE")]
    state: Option<PathBuf>,
    /// KV 日志留存上限（条数，超限裁剪最旧）；0 = 不裁剪
    #[arg(long, env = "HOBOB_LOG_KV_MAX", default_value_t = logkv::DEFAULT_KV_LOG_MAX)]
    log_kv_max: u64,
    /// 心跳周期（秒，M4/D8；发往 hobob_watchdog，见 keepalive.rs）；0 = 禁用心跳
    #[arg(long, env = "HOBOB_HEARTBEAT_INTERVAL", default_value_t = 2)]
    heartbeat_interval: u64,
    /// 心跳/停机通知走哪个 D-Bus（M4/D3；开发调试用 session，须与 watchdog 侧一致）
    #[arg(long, env = "HOBOB_BUS", value_enum, default_value_t = keepalive::Bus::default())]
    bus: keepalive::Bus,
}

/// # Errors
/// Throw runtime errors.
pub async fn main_loop() -> Result<()> {
    let flags = Flags::parse();
    #[allow(deprecated)]
    let home = std::env::home_dir();
    let state_cfg = store::StoreConfig::new(store::StoreConfig::resolve(flags.state, home));
    // T4（D12 / M1 探针升级）：store 打开失败即退出（取代 M0 non-fatal probe 日志）；
    // WeiYuanHui::open 全量加载失败同样即退出。
    let store = store::Store::open_or_create(&state_cfg)?;
    // M2（T3/D5）：KV 日志留存上限经 `--log-kv-max`/`HOBOB_LOG_KV_MAX` 注入 hub drain 点，
    // 随后显式接入日志镜像通道（store 已就绪；陈旧 ~/log4rs.yml 时静默降级）。
    let mut center = WeiYuanHui::open_with(store, flags.log_kv_max)?;
    center.attach_logkv()?;
    {
        let chair = center.new_chair();
        let app = www::build_app(&mut center);
        let port = flags.port;
        log::error!("listening on port {port}");
        tokio::spawn(async move {
            let (_, run) =
                warp::serve(app).bind_with_graceful_shutdown(([0, 0, 0, 0], port), async move {
                    chair.clone().until_closing().await;
                });
            log::info!("www app running");
            run.await;
            log::info!("www app stopped");
        });
    }

    {
        let chair = center.new_chair();
        tokio::spawn(async move {
            log::info!("engine starting");
            systems::fetch_loop(chair).await;
            log::info!("engine stopped");
        });
    }

    // hub 调度循环（M1 §4.5）：消费 chairs 提交（try_push 内含持久化 diff）→ 每轮 maybe_flush；
    // Ctrl+C/SIGTERM 在循环内 close()（closing 标志发布 → www/engine 优雅退出 → closed()）。
    // M4/T1（D4/D8）：select! 新增 1s tick 分支 touch 活跃度计数器——hub 空闲时阻塞在
    // updates.recv() 无轮次，tick 是「select! 循环存活」的直接证据；心跳任务独立 spawn，
    // 只读计数器不碰 hub（§4.3）。SIGTERM 并入 shutdown：watchdog 拉起流程先发 SIGTERM，
    // 不捕获会绕过停机强刷（§4.3 双信号问题）。
    let liveness = keepalive::Liveness::new();
    let heartbeat_interval = Duration::from_secs(flags.heartbeat_interval);
    if keepalive::enabled(heartbeat_interval) {
        tokio::spawn(keepalive::run_heartbeat(
            liveness.clone(),
            flags.bus,
            heartbeat_interval,
        ));
    }
    async fn shutdown_signal() {
        use tokio::signal::unix::SignalKind;
        let mut term =
            tokio::signal::unix::signal(SignalKind::terminate()).expect("install SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => log::error!("Caught ^C, quiting"),
            _ = term.recv() => log::error!("Caught SIGTERM, quiting"),
        }
    }
    let mut shutdown = Box::pin(shutdown_signal());
    let mut tick = tokio::time::interval(keepalive::protocol::TICK);
    loop {
        tokio::select! {
            _ = tick.tick() => liveness.touch(),
            _ = &mut shutdown => {
                // M4/D7：进 shutdown 分支立即发 Bye，先于 close()——强刷期间 tick 已停、
                // seq 冻结，close() 耗时接近 stall_timeout 时 watchdog 会对优雅停机中的
                // hobob 发起拉起；发不出仅 log 不影响退出。interval=0 时跳过（心跳禁用）。
                if keepalive::enabled(heartbeat_interval) {
                    keepalive::send_bye(flags.bus, "graceful shutdown").await;
                }
                center.close();
                break;
            }
            running = center.run() => {
                if !running {
                    log::error!("hub updates channel closed unexpectedly");
                    break;
                }
            }
        }
    }
    tokio::time::timeout(std::time::Duration::from_secs(30), center.closed())
        .await
        .map_err(|e| log::error!("force killing, graceful shutdown timeout: {}", e))
        .ok();

    Ok(())
}

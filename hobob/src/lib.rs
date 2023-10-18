use anyhow::Result;
use log::LevelFilter;
use log4rs::append::file::FileAppender;
use log4rs::config::{Appender, Config, Root};
use log4rs::encode::pattern::PatternEncoder;

macro_rules! vpath {
    () => {
        concat!(env!("HOME"), "/.cache/hobob")
    };
    (@log_cf) => {
        concat!(vpath!(), "/log4rs.yml")
    };
    (@bench) => {
        concat!(vpath!(), "/bench.json")
    };
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
pub mod engine;
pub mod www;

use db::WeiYuanHui;

/// # Errors
/// Throw log setup errors.
pub fn prepare_log() -> Result<()> {
    std::fs::create_dir_all(vpath!())?;

    // TODO use log_cf

    let logfile = FileAppender::builder()
        .encoder(Box::new(PatternEncoder::new(
            "{d(%Y-%m-%d %H:%M:%S)} # {M}/{l} - {P}:{I} # {m}{n}",
        )))
        .build(".cache/hobob_cache.log")?;

    let config = Config::builder()
        .appender(Appender::builder().build("logfile", Box::new(logfile)))
        .build(Root::builder().appender("logfile").build(LevelFilter::Info))?;

    log4rs::init_config(config)?;

    log::info!(
        "{} version {}; logger prepared",
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_VERSION")
    );

    Ok(())
}

/// # Errors
/// Throw runtime errors.
pub async fn main_loop() -> Result<()> {
    // FIXME: use load
    let mut center = WeiYuanHui::default();
    {
        let chair = center.new_chair();
        let app = www::build_app(&mut center);
        tokio::spawn(async move {
            let (_, run) =
                warp::serve(app).bind_with_graceful_shutdown(([0, 0, 0, 0], 3731), async move {
                    chair.clone().until_closing().await;
                });
            log::info!("www app running");
            run.await;
            log::info!("www app stopped");
        });
    }
    // TODO emit engine thread

    tokio::signal::ctrl_c().await?;
    log::error!("Caught ^C, quiting");
    center.close();
    tokio::time::timeout(std::time::Duration::from_secs(30), center.closed())
        .await
        .map_err(|e| log::error!("force killing, graceful shutdown timeout: {}", e))
        .ok();

    Ok(())
}

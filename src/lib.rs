use anyhow::Result;
use log::LevelFilter;
use log4rs::append::file::FileAppender;
use log4rs::config::{Appender, Config, Root};
use log4rs::encode::pattern::PatternEncoder;

macro_rules! var_path {
    () => {
        "/var/lifeich1/hobob"
    };
    (@log) => {
        concat!(var_path!(), "/log4rs.yml")
    };
}

pub mod db;
//pub mod engine;
//pub mod www;

pub fn prepare_log() -> Result<()> {
    std::fs::create_dir_all(var_path!())?;

    // TODO

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

pub async fn main_loop() -> Result<()> {
    let (_shutdown0, _rx) = tokio::sync::oneshot::channel::<i32>();

    /*
    tokio::spawn(async move {
        www::run(rx).await;
    });

    tokio::signal::ctrl_c().await?;
    log::info!("Caught ^C, quiting");
    engine::handle().send(engine::Command::Shutdown).await?;
    */

    Ok(())
}

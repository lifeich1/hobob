use error_chain::error_chain;
use log::LevelFilter;
use log4rs::append::file::FileAppender;
use log4rs::config::{Appender, Config, Root};
use log4rs::encode::pattern::PatternEncoder;

pub mod db;
pub mod engine;
pub mod www;

error_chain! {
    foreign_links {
        Db(rusqlite::Error);
        BiliApi(bilibili_api_rs::error::ApiError);
        InitLog(log::SetLoggerError);
        ConfigLog(log4rs::config::runtime::ConfigErrors);
        Io(std::io::Error);
        CommandMpsc(tokio::sync::mpsc::error::SendError<engine::Command>);
    }
}

pub fn prepare_log() -> Result<()> {
    let logfile = FileAppender::builder()
        .encoder(Box::new(PatternEncoder::new(
            "{d(%Y-%m-%d %H:%M:%S)} # {M}/{l} - {P}:{I} # {m}{n}",
        )))
        .build(".cache/hobob_cache.log")?;

    let config = Config::builder()
        .appender(Appender::builder().build("logfile", Box::new(logfile)))
        .build(
            Root::builder()
                .appender("logfile")
                .build(LevelFilter::Info),
        )?;

    log4rs::init_config(config)?;

    log::info!("logger prepared");

    Ok(())
}

pub async fn main_loop() -> Result<()> {
    let (_shutdown0, rx) = tokio::sync::oneshot::channel::<i32>();

    tokio::spawn(async move {
        www::run(rx).await;
    });

    let _ = tokio::signal::ctrl_c().await?;
    log::info!("Caught ^C, quiting");
    engine::handle().send(engine::Command::Shutdown).await?;

    Ok(())
}

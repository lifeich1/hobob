use anyhow::{Context, Result};
use clap::Parser;
use lalrpop_util::lalrpop_mod;
use log4rs::config::Deserializers;
use std::fs::File;
use std::io::BufWriter;
use std::io::Write;
use std::path::Path;

macro_rules! vpath {
    () => {
        vpath(&VP::Rt)
    };
    (@log_cf) => {
        vpath(&VP::LogCf)
    };
    (@bench) => {
        vpath(&VP::Bench)
    };
}

enum VP {
    Rt,
    LogCf,
    Bench,
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
            VP::Bench => "/bench.json",
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

pub mod bench;
mod data_schema;
pub mod db;
pub mod engine;
pub mod vm;
pub mod www;

use db::WeiYuanHui;

mod chunk;
lalrpop_mod!(
    #[allow(clippy::ptr_arg)]
    #[rustfmt::skip]
    chunkir
);

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
    log4rs::init_file(log_cf, Deserializers::default()).context("failed init log config file")?;

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
}

/// # Errors
/// Throw runtime errors.
pub async fn main_loop() -> Result<()> {
    let flags = Flags::parse();
    let mut center = WeiYuanHui::load(vpath!(@bench));
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
            engine::main_loop(chair).await;
            log::info!("engine stopped");
        });
    }

    tokio::signal::ctrl_c().await?;
    log::error!("Caught ^C, quiting");
    center.close();
    tokio::time::timeout(std::time::Duration::from_secs(30), center.closed())
        .await
        .map_err(|e| log::error!("force killing, graceful shutdown timeout: {}", e))
        .ok();

    Ok(())
}

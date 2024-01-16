use anyhow::{Context, Result};
use hobob::{main_loop, prepare_log};

async fn run() -> Result<()> {
    prepare_log().context("fail prepare log")?;
    main_loop().await?;
    Ok(())
}

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        log::error!("FATAL: {e:#?}");
        panic!("Error at startup: {e:?}");
    }

    log::info!("quit");
}

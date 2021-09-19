use hobob_cache::*;

#[tokio::main]
async fn main() {
    if let Err(e) = prepare_log() {
        panic!("Error at startup: {}", e);
    }

    if let Err(e) = main_loop().await {
        panic!("Error at main_loop: {}", e);
    }

    log::info!("waiting on graceful shutdown");
    engine::done_shutdown().await;
    db::blocking_shutdown();

    log::info!("quit");
}

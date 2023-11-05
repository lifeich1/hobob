use anyhow::Result;
use clap::Parser;
use hobob_dbgconn::{client_main, server_main, Flags};

#[tokio::main]
async fn main() -> Result<()> {
    let flags = Flags::parse();
    env_logger::init();
    if flags.is_server() {
        server_main(flags).await
    } else {
        client_main(flags).await
    }
}

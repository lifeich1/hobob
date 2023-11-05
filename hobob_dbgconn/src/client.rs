use crate::{Command, DbgconnClient, Flags};
use anyhow::Result;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use tarpc::{client, context, tokio_serde::formats::Json};

/// # Errors
pub async fn main(flags: Flags) -> Result<()> {
    let ip: Ipv4Addr = flags.server_ip.parse()?;
    let addr = SocketAddr::V4(SocketAddrV4::new(ip, flags.port));
    let mut transport = tarpc::serde_transport::tcp::connect(addr, Json::default);
    transport.config_mut().max_frame_length(usize::MAX);

    log::debug!("connecting");
    let client = DbgconnClient::new(client::Config::default(), transport.await?).spawn();
    log::debug!("get client");

    match flags.command {
        Some(Command::Alive) => {
            let pid = client.alive(context::current()).await?;
            if pid < 0 {
                println!("dead hobob.");
            } else {
                println!("alive hobob, pid = {pid}");
            }
        }
        Some(Command::Restart {
            force,
            binary,
            args,
        }) => {
            println!(
                "restart finished, pid = {}",
                client
                    .restart(context::current(), force, binary, args)
                    .await?
            );
        }
        None => anyhow::bail!("client without valid command !!"),
    }
    Ok(())
}

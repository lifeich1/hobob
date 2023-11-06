use crate::{Dbgconn, Flags};
use anyhow::{bail, Result};
use duct::cmd;
use futures::{future, prelude::*};
use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;
use tarpc::{
    context,
    server::{self, incoming::Incoming, Channel},
    tokio_serde::formats::Json,
};

#[derive(Clone)]
struct RpcServer();

fn get_hobob_pid() -> Result<i32> {
    let Ok(line) = cmd!("pidof", "hobob").read() else {
        // according to man PIDOF(8), exit status 1 means program not found.
        return Ok(-1);
    };
    let pids: Vec<String> = line.split(' ').map(String::from).collect();
    if pids.len() != 1 {
        bail!("expect one process, got {}", pids.len());
    }
    Ok(pids[0].parse()?)
}

fn kill_hobob(force: bool) -> Result<()> {
    let Ok(line) = cmd!("pidof", "hobob").read() else {
        // according to man PIDOF(8), exit status 1 means program not found.
        return Ok(());
    };
    let pids: Vec<&str> = line.split(' ').collect();
    if pids.is_empty() {
        Ok(())
    } else {
        let mut args = vec!["--signal", if force { "SIGKILL" } else { "SIGINT" }];
        args.extend(pids);
        cmd("kill", args).run()?;
        bail!("shutting down")
    }
}

#[tarpc::server]
impl Dbgconn for RpcServer {
    async fn alive(self, _: context::Context) -> i32 {
        get_hobob_pid().unwrap_or_else(|e| {
            log::error!("alive: {e:#}");
            -1
        })
    }
    async fn restart(
        self,
        ctx: context::Context,
        force_kill: bool,
        binary: String,
        args: Vec<String>,
    ) -> i32 {
        if !binary.contains("hobob") {
            // security check
            return -2;
        }
        while let Err(e) = kill_hobob(force_kill) {
            log::error!("kill: {e:#}");
            tokio::time::sleep(Duration::from_millis(800)).await;
        }
        let status = cmd(binary, args).start();
        if let Err(e) = status {
            log::error!("run: {e:#}");
            return -1;
        }
        self.alive(ctx).await
    }
}
/// # Errors
/// # Panics
/// Panic on listener limit
pub async fn main(flags: Flags) -> Result<()> {
    log::error!("initing ...");
    let addr = (IpAddr::V4(Ipv4Addr::UNSPECIFIED), flags.port);
    let mut listener = tarpc::serde_transport::tcp::listen(&addr, Json::default).await?;
    log::error!("listening ...");
    listener.config_mut().max_frame_length(usize::MAX);
    listener
        // Ignore accept errors.
        .filter_map(|r| future::ready(r.ok()))
        .map(server::BaseChannel::with_defaults)
        // Limit channels to 1 per IP.
        .max_channels_per_key(1, |t| t.transport().peer_addr().unwrap().ip())
        // serve is generated by the service attribute. It takes as input any type implementing
        // the generated World trait.
        .map(|channel| {
            log::info!("spawn server");
            let server = RpcServer();
            channel.execute(server.serve())
        })
        // Max 10 channels.
        .buffer_unordered(10)
        .for_each(|()| async {})
        .await;

    Ok(())
}

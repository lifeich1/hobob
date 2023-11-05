use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
pub struct Flags {
    /// Server listen port
    #[arg(short, long, default_value_t = 21321)]
    port: u16,
    /// Server ip, working on client
    #[arg(short, long, default_value = "127.0.0.1")]
    server_ip: String,
    /// client command; no command will run server.
    #[command(subcommand)]
    command: Option<Command>,
}

impl Flags {
    #[must_use]
    pub const fn is_server(&self) -> bool {
        self.command.is_none()
    }
}

#[derive(Subcommand)]
enum Command {
    /// check hobob is alive, return alive pid or -1
    Alive,
    /// restart hobob, return pid
    Restart {
        /// force kill option
        #[arg(short, long)]
        force: bool,
        /// binary path
        #[arg(short, long, default_value = "hobob")]
        binary: String,
        /// passthrough hobob options
        args: Vec<String>,
    },
}

#[tarpc::service]
pub trait Dbgconn {
    /// check hobob is alive, return alive pid or -1
    async fn alive() -> i32;
    /// restart hobob, return pid
    async fn restart(force_kill: bool, binary: String, args: Vec<String>) -> i32;
}

mod client;
mod server;

pub use client::main as client_main;
pub use server::main as server_main;

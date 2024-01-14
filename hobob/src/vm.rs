use crate::bench::Bench;
use im::{HashMap, OrdSet};
use tokio::sync::{mpsc, watch};

pub struct Machine {
    pub bench: Bench,
    pub bench_rx: watch::Receiver<Bench>,
    bench_tx: watch::Sender<Bench>,
    pub trunk_tx: mpsc::Sender<String>,
    trunk_rx: mpsc::Sender<String>,
    timer_cb: HashMap<String, String>,
    next_timer: OrdSet<(u64, String)>,
}

impl Machine {
    pub async fn infer(&mut self, trunk: String) {
        todo!();
    }

    pub async fn step(&mut self) {
        todo!();
    }
}

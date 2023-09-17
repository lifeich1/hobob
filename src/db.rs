use anyhow::{anyhow, Result};
use chrono::{DateTime, TimeZone, Utc};
use serde_derive::{Deserialize, Serialize};
use serde_json::Value;
use std::convert::TryFrom;
use std::fs::File;
use std::io::BufReader;
use std::ops::Deref;
use std::path::Path;
use std::sync::mpsc;
use std::sync::mpsc::{Receiver, Sender};

pub type UpInfo = im::HashMap<String, Value>;
pub type UpJoinGroup = im::HashMap<String, im::HashSet<String>>;
pub type Events = im::Vector<Value>;
pub type GroupInfo = im::HashMap<String, Value>;
pub type LogRecords = im::Vector<Value>;
pub type RuntimeCfg = im::HashMap<String, Value>;

#[derive(Debug, Deserialize, Serialize, Clone, Default, PartialEq)]
pub struct FullBench {
    pub up_info: UpInfo,
    pub up_by_fid: im::Vector<String>,
    pub up_join_group: UpJoinGroup,
    pub events: Events,
    pub group_info: GroupInfo,
    pub logs: LogRecords,
    pub runtime: RuntimeCfg,
}

pub struct BenchUpdate(FullBench, FullBench);

#[derive(Debug, Default)]
pub struct WeiYuanHui {
    updates: Vec<Receiver<BenchUpdate>>,
    publish: Vec<Sender<FullBench>>,
    bench: FullBench,
}

pub struct WeiYuan {
    update: Sender<BenchUpdate>,
    fetch: Receiver<FullBench>,
    bench: FullBench,
}

impl WeiYuanHui {
    pub fn load<P: AsRef<Path>>(path: P) -> Self {
        let loaded = Self::load_check(path);
        if let Err(e) = loaded {
            log::error!(
                "load full bench from file {} failed: {}",
                path.as_ref().display(),
                e
            );
        }
        loaded.ok().unwrap_or_else(Self::default)
    }

    fn load_check<P: AsRef<Path>>(path: P) -> Result<Self> {
        let file = File::open(path)?;
        let reader = BufReader::new(file);
        let bench: FullBench = serde_json::from_reader(reader)?;
        Ok(Self {
            bench,
            ..Default::default()
        })
    }

    pub fn new_chair(&mut self) -> WeiYuan {
        let (update, rx) = mpsc::channel();
        self.updates.push(rx);
        let (tx, fetch) = mpsc::channel();
        self.publish.push(tx);
        let bench = self.bench.clone();
        WeiYuan {
            update,
            fetch,
            bench,
        }
    }

    pub fn run(&mut self) {
        while self.try_update() {
            if self.bench.runtime_dump_now() {
                self.save_disk();
            }
        }
    }

    fn try_update(&mut self) -> bool {
        // TODO
        true
    }

    fn save_disk(&mut self) {
        // TODO
    }
}

impl WeiYuan {
    pub fn recv(&mut self) -> &FullBench {
        loop {
            let res = self.fetch.try_recv();
            match res {
                Ok(data) => self.bench = data,
                Err(mpsc::TryRecvError::Disconnected) => panic!("this WeiYuan DISCONNECTED !!!"),
                _ => break,
            }
        }
        &self.bench
    }

    pub fn update<F: Fn(&FullBench) -> FullBench>(&mut self, f: F) {
        let msg: BenchUpdate;
        loop {
            let old = self.recv().clone();
            let new = f(&old);
            if old == *self.recv() {
                msg = BenchUpdate(old, new);
                break;
            }
        }
        self.update.send(msg);
    }
}

impl FullBench {
    pub fn runtime_dump_now(&self) -> bool {
        im::get_in!(self.runtime, "db")
            .and_then(|v| serde_json::from_value::<DateTime<Utc>>(v["dump_time"]).ok())
            .map(|t| t > Utc::now())
            .unwrap_or(true)
    }

    pub fn set_runtime_next_dump(&self) -> Self {
        let r = self.clone();
        // TODO
        r
    }
}

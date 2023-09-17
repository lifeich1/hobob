use anyhow::{anyhow, Result};
use chrono::{DateTime, Duration, Utc};
use serde_derive::{Deserialize, Serialize};
use serde_json::{from_value, to_value, Value};
use std::fs::File;
use std::io::{BufReader, BufWriter};
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
    savepath: Option<Box<Path>>,
}

pub struct WeiYuan {
    update: Sender<BenchUpdate>,
    fetch: Receiver<FullBench>,
    bench: FullBench,
}

impl WeiYuanHui {
    pub fn load<P: AsRef<Path>>(path: P) -> Self {
        log::info!("loading full bench from {}", path.as_ref().display());
        let loaded = Self::load_check(path);
        if let Err(e) = loaded.as_ref() {
            log::error!("load full bench from file failed: {}", e);
        }
        loaded.ok().unwrap_or_else(Self::default)
    }

    fn load_check<P: AsRef<Path>>(path: P) -> Result<Self> {
        let savepath: Option<Box<Path>> = Some(path.as_ref().into());
        let file = File::open(path)?;
        let reader = BufReader::new(file);
        let bench: FullBench = serde_json::from_reader(reader)?;
        Ok(Self {
            bench,
            savepath,
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

    pub fn nonblocking_run(&mut self) {
        while self.try_update() {
            if self.bench.runtime_dump_now() {
                if let Err(e) = self.save_disk() {
                    log::error!("save_disk error: {}", e);
                }
            }
        }
    }

    fn try_update(&mut self) -> bool {
        let msgs: Vec<_> = self.updates.iter_mut().map(|rx| rx.try_recv()).collect();
        for msg in msgs {
            match msg {
                Ok(msg) => {
                    if self.try_push(msg) {
                        return true;
                    }
                }
                // FIXME disconn is shut not panic
                Err(mpsc::TryRecvError::Disconnected) => panic!("WeiYuanHui receiver offline !!!"),
                _ => (),
            }
        }
        false
    }

    fn try_push(&mut self, upd: BenchUpdate) -> bool {
        if upd.0 == self.bench {
            self.push(upd.1);
            true
        } else {
            false
        }
    }

    fn save_disk(&mut self) -> Result<()> {
        self.push(self.bench.set_runtime_next_dump());
        let path = self
            .savepath
            .as_ref()
            .ok_or_else(|| anyhow!("savepath not config"))?;
        let file = File::create(path)?;
        let writer = BufWriter::new(file);
        serde_json::to_writer(writer, &self.bench)?;
        Ok(())
    }

    fn push(&mut self, next: FullBench) {
        self.bench = next;
        for tx in self.publish.iter() {
            let data = self.bench.clone();
            if let Err(e) = tx.send(data) {
                log::error!("send update err: {}", e);
            }
        }
    }
}

impl WeiYuan {
    pub fn recv(&mut self) -> &FullBench {
        loop {
            let res = self.fetch.try_recv();
            match res {
                Ok(data) => self.bench = data,
                // FIXME disconn is shut not panic
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
        if let Err(e) = self.update.send(msg) {
            log::error!("send update failed: {}", e);
        }
    }
}

impl FullBench {
    fn mut_runtime_field<F: Fn(&mut Value)>(&self, key: &str, f: F) -> Self {
        let mut v = self
            .runtime
            .get(key)
            .cloned()
            .unwrap_or_else(|| Value::default());
        f(&mut v);
        let mut r = self.clone();
        r.runtime = self.runtime.update(key.into(), v);
        r
    }

    pub fn runtime_dump_time(&self) -> Option<DateTime<Utc>> {
        im::get_in!(self.runtime, "db")
            .and_then(|v| from_value::<DateTime<Utc>>(v["dump_time"].clone()).ok())
    }

    pub fn runtime_dump_now(&self) -> bool {
        self.runtime_dump_time()
            .map(|t| t < Utc::now())
            .unwrap_or(true)
    }

    pub fn set_runtime_next_dump(&self) -> Self {
        self.mut_runtime_field("db", |v| {
            v["dump_time"] =
                to_value(Utc::now() + Duration::minutes(self.runtime_dump_timeout_min() as i64))
                    .unwrap()
        })
    }

    pub fn runtime_dump_timeout_min(&self) -> u64 {
        im::get_in!(self.runtime, "db")
            .and_then(|v| v["dump_timeout_min"].as_u64())
            .unwrap_or(720)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_runtime_dump_now_default() {
        let bench = FullBench::default();
        assert!(bench.runtime_dump_now());
    }

    #[test]
    fn test_runtime_dump_timeout_min_default() {
        let bench = FullBench::default();
        assert_eq!(bench.runtime_dump_timeout_min(), 720_u64);
        assert!(bench.runtime_dump_now());
        let next = bench.set_runtime_next_dump();
        assert!(!next.runtime_dump_now());
    }

    #[test]
    fn test_two_chairs() {
        // TODO
        assert!(true);
    }
}

use anyhow::{anyhow, Result};
use chrono::{DateTime, Duration, Utc};
use serde_derive::{Deserialize, Serialize};
use serde_json::json;
use serde_json::{from_value, to_value, Value};
use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::path::Path;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{mpsc, watch};

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

#[derive(Debug)]
pub struct WeiYuanHui {
    updates: mpsc::Receiver<BenchUpdate>,
    updates_src: mpsc::Sender<BenchUpdate>,
    publish: watch::Sender<FullBench>,
    publish_dst: watch::Receiver<FullBench>,
    bench: FullBench,
    savepath: Option<Box<Path>>,
}

pub struct WeiYuan {
    update: mpsc::Sender<BenchUpdate>,
    fetch: watch::Receiver<FullBench>,
    bench: FullBench,
    buf_logs: Vec<Value>,
    buf_events: Vec<Value>,
    logs_dump_time: DateTime<Utc>,
    events_dump_time: DateTime<Utc>,
}

impl Default for WeiYuanHui {
    fn default() -> Self {
        let (updates_src, updates) = mpsc::channel(64);
        let (publish, publish_dst) = watch::channel(FullBench::default());
        Self {
            updates,
            updates_src,
            publish,
            publish_dst,
            bench: Default::default(),
            savepath: Default::default(),
        }
    }
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
        WeiYuan {
            update: self.updates_src.clone(),
            fetch: self.publish_dst.clone(),
            bench: self.bench.clone(),
            buf_logs: Default::default(),
            buf_events: Default::default(),
            logs_dump_time: Utc::now(),
            events_dump_time: Utc::now(),
        }
    }

    pub fn nonblocking_run(&mut self) {
        while self.try_update() {
            if self.bench.runtime_dump_now() {
                if let Err(e) = self.save_disk() {
                    // TODO update dump_err_ts
                    log::error!("save_disk error: {}", e);
                }
            }
        }
    }

    fn try_update(&mut self) -> bool {
        let msg = self.updates.try_recv();
        match msg {
            Ok(msg) => {
                if self.try_push(msg) {
                    return true;
                }
            }
            // FIXME disconn is shut not panic
            Err(mpsc::error::TryRecvError::Disconnected) => {
                panic!("WeiYuanHui receiver offline !!!")
            }
            _ => (),
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
        self.bench = next.clone();
        self.publish.send_modify(move |v| *v = next);
    }
}

impl WeiYuan {
    pub fn recv(&mut self) -> &FullBench {
        match self.fetch.has_changed() {
            Ok(true) => self.bench = self.fetch.borrow_and_update().clone(),
            Err(_) => panic!("FIXME: it should be shutdown signal"),
            _ => (),
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
        if let Err(e) = self.update.try_send(msg) {
            if let TrySendError::Closed(_) = e {
                panic!("FIXME: it should be shutdown signal");
            } else {
                // TODO push in buflogqueue
                log::error!("send update failed: {}", e);
            }
        }
    }

    pub fn log<S: ToString>(&mut self, level: i32, msg: S) {
        self.buf_logs.push(
            json!({"ts": to_value(Utc::now()).unwrap(), "level": level, "msg": msg.to_string()}),
        );
    }

    pub fn event(&mut self, ev: Value) {
        self.buf_events.push(ev);
    }

    pub async fn step_sidecar() {
        // select dumpers
    }

    async fn dump_logs() {}
    async fn dump_events() {}
}

impl FullBench {
    fn mut_runtime_field<F: FnOnce(&mut Value)>(&self, key: &str, f: F) -> Self {
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

    /// General api, for www use
    pub fn runtime_field(&self, key: &str, path: &str) -> Value {
        im::get_in!(self.runtime, key)
            .and_then(|v| {
                let mut t: &Value = v;
                for p in path.split('/') {
                    match t.get(p) {
                        Some(r) => t = r,
                        None => return None,
                    }
                }
                Some(t.clone())
            })
            .unwrap_or(Value::Null)
    }

    /// General api, for www use
    pub fn runtime_set_field(&self, key: &str, path: &str, val: Value) -> Self {
        self.mut_runtime_field(key, |mut v| {
            for p in path.split('/') {
                if v.get(p).is_none() {
                    v[p] = Value::Object(Default::default());
                }
                v = v.get_mut(p).unwrap();
            }
            *v = val;
        })
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
    fn test_runtime_field_set_n_get() {
        let mut bench = FullBench::default();
        bench = bench.runtime_set_field("db", "bucket/min_gap", json!(42));
        assert_eq!(
            bench.runtime.get("db"),
            Some(&json!({"bucket":{"min_gap":42}}))
        );
        assert_eq!(bench.runtime_field("db", "bucket/min_gap"), json!(42));
    }

    #[test]
    fn test_two_chairs() {
        let mut center = WeiYuanHui::default();
        let mut chair = center.new_chair();
        let mut chair_rx = center.new_chair();
        chair.update(|b| b.runtime_set_field("bucket", "min_gap", json!(23)));
        center.nonblocking_run();
        let cur = chair_rx.recv();
        assert_eq!(cur.runtime_field("bucket", "min_gap"), json!(23));
    }
}

use anyhow::{anyhow, Result};
use chrono::{DateTime, Duration, Utc};
use serde_derive::{Deserialize, Serialize};
use serde_json::json;
use serde_json::{from_value, to_value, Value};
use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::ops::Not;
use std::path::Path;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{mpsc, watch};

const FLAG_CLOSING: &str = "#CLOSING#";

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

#[derive(Debug)]
pub struct BenchUpdate(FullBench, FullBench);

#[derive(Default, Debug)]
struct VCounter {
    last_dump_ts: Option<DateTime<Utc>>,
    push_miss_cnt: u64,
}

#[derive(Debug)]
pub struct WeiYuanHui {
    updates: mpsc::Receiver<BenchUpdate>,
    updates_src: Option<mpsc::Sender<BenchUpdate>>,
    publish: watch::Sender<FullBench>,
    publish_dst: Option<watch::Receiver<FullBench>>,
    bench: FullBench,
    savepath: Option<Box<Path>>,
    counter: VCounter,
}

#[derive(Clone)]
pub struct WeiYuan {
    update: mpsc::Sender<BenchUpdate>,
    fetch: watch::Receiver<FullBench>,
    bench: FullBench,
}

impl Default for WeiYuanHui {
    fn default() -> Self {
        let (updates_src, updates) = mpsc::channel(64);
        let (publish, publish_dst) = watch::channel(FullBench::default());
        let updates_src = Some(updates_src);
        let publish_dst = Some(publish_dst);
        Self {
            updates,
            updates_src,
            publish,
            publish_dst,
            bench: Default::default(),
            savepath: Default::default(),
            counter: Default::default(),
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
        let mut bench: FullBench = serde_json::from_reader(reader)?;
        bench.runtime_rm_closing();
        Ok(Self {
            bench,
            savepath,
            ..Default::default()
        })
    }

    pub fn new_chair(&mut self) -> WeiYuan {
        WeiYuan {
            update: self
                .updates_src
                .as_ref()
                .expect("new_chair in closing")
                .clone(),
            fetch: self
                .publish_dst
                .as_ref()
                .expect("new_chair in closing")
                .clone(),
            bench: self.bench.clone(),
        }
    }

    pub fn close(&mut self) {
        self.updates_src = None;
        self.publish_dst = None;
        self.save_disk().ok();
        self.push(self.bench.runtime_set_closing());
        self.updates.close();
    }

    pub async fn closed(&self) {
        self.publish.closed().await;
    }

    /// @return is running
    pub async fn run(&mut self) -> bool {
        if !self.try_update().await {
            return false;
        }
        if self.bench.runtime_dump_now() {
            if let Err(e) = self.save_disk() {
                self.push(
                    self.bench
                        .add_log(0, format!("#WeiYuanHui# save_disk error: {}", &e)),
                );
                log::error!("save_disk error: {}", e);
            }
        }
        true
    }

    pub async fn run_for(&mut self, duration: Duration) -> Result<bool> {
        Ok(tokio::time::timeout(duration.to_std()?, self.run()).await?)
    }

    /// @return is running
    async fn try_update(&mut self) -> bool {
        let msg = self.updates.recv().await;
        match msg {
            Some(msg) => {
                self.try_push(msg);
                true
            }
            None => false,
        }
    }

    fn try_push(&mut self, upd: BenchUpdate) {
        if upd.0.ptr_eq(&self.bench) {
            self.push(upd.1);
        } else {
            self.counter.push_miss_cnt += 1;
            if let Some(msg) = self.counter.try_log(&self.bench) {
                self.push(self.bench.add_log(1, msg));
            }
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
    /// @return None for closing
    pub fn recv(&mut self) -> Result<&FullBench> {
        match self.fetch.has_changed() {
            Ok(true) => self.bench = self.fetch.borrow_and_update().clone(),
            Err(_) => panic!("watch chan: WeiYuanHui drop too fast"),
            _ => (),
        }
        self.bench
            .runtime_is_closing()
            .not()
            .then_some(&self.bench)
            .ok_or_else(|| anyhow!("WeiYuanHui closing"))
    }

    pub fn update<F: Fn(&FullBench) -> FullBench>(&mut self, f: F) -> Result<()> {
        let msg: BenchUpdate;
        loop {
            let old = self.recv()?.clone();
            let new = f(&old);
            if old.ptr_eq(self.recv()?) {
                msg = BenchUpdate(old, new);
                break;
            }
        }
        match self.update.try_send(msg) {
            Ok(_) => Ok(()),
            Err(e) => {
                if let TrySendError::Closed(_) = &e {
                    self.recv()
                        .map(|_| {
                            panic!("Update channel disconnected without WeiYuanHui closing flag !!")
                        })
                        .ok();
                } else {
                    log::error!("send update failed: {}, will treat as closing", e);
                }
                Err(e.into())
            }
        }
    }

    pub fn apply<F: Fn(&mut FullBench)>(&mut self, f: F) -> Result<()> {
        self.update(|b| {
            let mut v = b.clone();
            f(&mut v);
            v
        })
    }

    pub fn log<S: ToString>(&mut self, level: i32, msg: S) {
        self.update(|b| b.add_log(level, msg.to_string())).ok();
    }

    pub async fn until_closing(&mut self) {
        self.fetch
            .wait_for(|b| b.runtime_is_closing())
            .await
            .map_err(|e| panic!("fetch channel unexpected closed: {}", e))
            .ok();
    }
}

fn im_vector_p_eq<A: Clone + Eq>(lhs: &im::Vector<A>, rhs: &im::Vector<A>) -> bool {
    match (lhs.is_inline(), rhs.is_inline()) {
        (true, true) => *lhs == *rhs,
        (false, false) => lhs.ptr_eq(rhs),
        _ => false,
    }
}

impl FullBench {
    fn ptr_eq(&self, other: &Self) -> bool {
        self.up_info.ptr_eq(&other.up_info)
            && im_vector_p_eq(&self.up_by_fid, &other.up_by_fid)
            && self.up_join_group.ptr_eq(&other.up_join_group)
            && im_vector_p_eq(&self.events, &other.events)
            && self.group_info.ptr_eq(&other.group_info)
            && im_vector_p_eq(&self.logs, &other.logs)
            && self.runtime.ptr_eq(&other.runtime)
    }

    fn add_log(&self, level: i32, msg: String) -> Self {
        let mut r = self.clone();
        let cf = self.runtime.get("log_filter").unwrap_or(&Value::Null);
        let maxlv = cf["maxlevel"].as_i64().unwrap_or(3) as i32;
        if level > maxlv {
            return r;
        }
        r.logs
            .push_back(json!({"ts": to_value(Utc::now()).unwrap(), "level": level, "msg": msg}));
        let bufl = cf["buffer_lines"].as_u64().unwrap_or(2048) as usize;
        if r.logs.len() > bufl {
            let fitl = cf["fit_lines"].as_u64().unwrap_or(16);
            for _ in 0..=fitl {
                r.logs.pop_front();
            }
        }
        r
    }

    fn mut_runtime_field<F: FnOnce(&mut Value)>(&self, key: &str, f: F) -> Self {
        let mut r = self.clone();
        if r.runtime.get(key).is_none() {
            r.runtime.insert(key.into(), Value::default());
        }
        f(r.runtime.get_mut(key).unwrap());
        r
    }

    fn runtime_dump_time(&self) -> Option<DateTime<Utc>> {
        im::get_in!(self.runtime, "db")
            .and_then(|v| from_value::<DateTime<Utc>>(v["dump_time"].clone()).ok())
    }

    fn runtime_dump_now(&self) -> bool {
        self.runtime_dump_time()
            .map(|t| t < Utc::now())
            .unwrap_or(true)
    }

    fn set_runtime_next_dump(&self) -> Self {
        self.mut_runtime_field("db", |v| {
            v["dump_time"] =
                to_value(Utc::now() + Duration::minutes(self.runtime_dump_timeout_min() as i64))
                    .unwrap()
        })
    }

    fn runtime_dump_timeout_min(&self) -> u64 {
        im::get_in!(self.runtime, "db")
            .and_then(|v| v["dump_timeout_min"].as_u64())
            .unwrap_or(720)
    }

    fn runtime_vlog_dump_gap(&self) -> Duration {
        self.runtime
            .get("db")
            .and_then(|v| v["vlog_dump_gap_sec"].as_u64())
            .map(|sec| sec as i64)
            .map(Duration::seconds)
            .unwrap_or_else(|| Duration::seconds(10))
    }

    fn runtime_set_closing(&self) -> Self {
        self.mut_runtime_field(FLAG_CLOSING, |v| *v = Value::Null)
    }

    fn runtime_rm_closing(&mut self) {
        self.runtime.remove(FLAG_CLOSING);
    }

    fn runtime_is_closing(&self) -> bool {
        self.runtime.get(FLAG_CLOSING).is_some()
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

impl VCounter {
    pub fn try_log(&mut self, bench: &FullBench) -> Option<String> {
        if self.last_dump_ts.map(|t| Utc::now() > t).unwrap_or(true) {
            self.last_dump_ts = Some(Utc::now() + bench.runtime_vlog_dump_gap());
            Some(self.do_log())
        } else {
            None
        }
    }

    fn do_log(&self) -> String {
        format!("push_miss_cnt: {},", self.push_miss_cnt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem;
    use std::time::Duration as Dur;
    use tokio::time::timeout;

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

    async fn run_1s(center: &mut WeiYuanHui) -> bool {
        center
            .run_for(Duration::seconds(1))
            .await
            .ok()
            .unwrap_or(false)
    }

    async fn check_closed(center: &WeiYuanHui) {
        assert!(timeout(Dur::from_secs(1), center.closed()).await.is_ok());
    }

    #[tokio::test]
    async fn test_two_chairs() {
        let mut center = WeiYuanHui::default();
        assert_eq!(center.bench.runtime.get(FLAG_CLOSING), None);
        assert_eq!(center.bench.runtime_is_closing(), false);
        {
            let mut chair = center.new_chair();
            let mut chair_rx = chair.clone();
            assert_eq!(
                chair
                    .update(|b| b.runtime_set_field("bucket", "min_gap", json!(23)))
                    .err()
                    .map(|e| format!("{:?}", e)),
                None
            );
            assert!(run_1s(&mut center).await);
            let cur = chair_rx.recv().unwrap();
            assert_eq!(cur.runtime_field("bucket", "min_gap"), json!(23));
        }
        center.close();
        assert!(!run_1s(&mut center).await);
        check_closed(&center).await;
    }

    #[tokio::test]
    #[should_panic(expected = "watch chan: WeiYuanHui drop too fast")]
    async fn test_weiyuanhui_drop_too_fast() {
        let mut center = WeiYuanHui::default();
        let mut chair = center.new_chair();
        center.close();
        mem::drop(center);
        chair.recv().ok();
    }

    #[tokio::test]
    async fn test_weiyuan_log() {
        let mut center = WeiYuanHui::default();
        let mut chair = center.new_chair();
        chair.log(3, "Ooga-Chaka Ooga-Ooga");
        assert_eq!(center.bench.logs.len(), 0);
        assert!(center.run().await);
        assert_ne!(center.bench.logs.len(), 0);
        let mut v = center.bench.logs[0].clone();
        v["ts"] = json!(null);
        assert_eq!(
            v,
            json!({
                "ts": null,
                "level": 3,
                "msg": "Ooga-Chaka Ooga-Ooga",
            })
        );
    }

    #[test]
    fn test_circular_log() {
        let mut bench = FullBench::default();
        for i in 0..2048 {
            bench = bench.add_log(2, format!("test log {}", i));
        }
        assert_eq!(bench.logs.len(), 2048);
        bench = bench.add_log(4, "will discard log".into());
        assert_eq!(bench.logs.len(), 2048);
        bench = bench.add_log(-1, "this log trigger buffer shorten".into());
        assert_eq!(bench.logs.len(), 2048 - 16);
    }

    #[tokio::test]
    #[should_panic(expected = "Update channel disconnected without WeiYuanHui closing flag !!")]
    async fn test_weiyuanhui_channel_error() {
        let mut center = WeiYuanHui::default();
        let mut chair = center.new_chair();
        center.updates.close();
        chair.log(3, "Ooga-Chaka Ooga-Ooga");
    }

    #[test]
    fn test_vcounter() {
        let mut c = VCounter::default();
        let b = FullBench::default();
        assert_ne!(c.try_log(&b), None);
        assert_eq!(c.try_log(&b), None);
    }

    #[test]
    #[should_panic(expected = "new_chair in closing")]
    fn test_panic_at_new_chair_in_closing() {
        let mut center = WeiYuanHui::default();
        center.close();
        let _ = center.new_chair();
    }

    #[test]
    fn test_weiyuan_notified_closing() {
        let mut center = WeiYuanHui::default();
        let mut chair = center.new_chair();
        center.close();
        assert!(chair.recv().is_err());
        assert!(chair.update(Clone::clone).is_err());
    }

    #[tokio::test]
    async fn test_weiyuanhui_closed_after_members_release() {
        let center = &mut WeiYuanHui::default();
        let mut chair = center.new_chair();
        assert!(timeout(Dur::from_millis(100), chair.until_closing())
            .await
            .is_err());
        center.close();
        assert!(!run_1s(center).await);
        assert!(timeout(Dur::from_millis(100), center.closed())
            .await
            .is_err());
        assert!(timeout(Dur::from_millis(100), chair.until_closing())
            .await
            .is_ok());
        mem::drop(chair);
        check_closed(center).await;
    }
}

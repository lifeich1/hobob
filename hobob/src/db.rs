use crate::data_schema::ChairData;
use anyhow::{anyhow, bail, Result};
use chrono::{DateTime, Duration, Utc};
use serde_derive::{Deserialize, Serialize};
use serde_json::json;
use serde_json::{from_value, to_value, Value};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::ops::Not;
use std::path::Path;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{broadcast, mpsc, watch};

const FLAG_CLOSING: &str = "#CLOSING#";
const COUNTER_TAG: &str = "#COUNTER#";

pub type UpInfo = im::HashMap<String, Value>;
pub type UpIndex = im::HashMap<String, im::OrdSet<(i64, String)>>;
pub type UpJoinGroup = im::HashMap<String, im::OrdSet<String>>;
pub type Events = im::Vector<Value>;
pub type GroupInfo = im::OrdMap<String, Value>;
pub type LogRecords = im::Vector<Value>;
pub type RuntimeCfg = im::HashMap<String, Value>;
pub type Commands = im::Vector<Value>;

#[derive(Debug, Deserialize, Serialize, Clone, Default, PartialEq, Eq)]
pub struct FullBench {
    pub up_info: UpInfo,
    pub up_index: UpIndex,
    pub up_by_fid: im::Vector<String>,
    pub up_join_group: UpJoinGroup,
    pub events: Events,
    pub group_info: GroupInfo,
    pub logs: LogRecords,
    pub runtime: RuntimeCfg,
    pub commands: Commands,
}

#[derive(Debug)]
pub struct BenchUpdate(FullBench, FullBench);

#[derive(Default, Debug)]
struct VCounter {
    last_dump_ts: Option<DateTime<Utc>>,
    push_miss_cnt: u64,
    broadcast_void_cnt: u64,
    ext: BTreeMap<String, u64>,
}

#[derive(Debug)]
pub struct WeiYuanHui {
    updates: mpsc::Receiver<BenchUpdate>,
    updates_src: Option<mpsc::Sender<BenchUpdate>>,
    publish: watch::Sender<FullBench>,
    publish_dst: Option<watch::Receiver<FullBench>>,
    ev_tx: Option<broadcast::Sender<Events>>,
    ev_rx: broadcast::Receiver<Events>,
    bench: FullBench,
    savepath: Option<Box<Path>>,
    counter: VCounter,
}

#[derive(Clone)]
pub struct WeiYuan {
    update: Option<mpsc::Sender<BenchUpdate>>,
    fetch: watch::Receiver<FullBench>,
    bench: FullBench,
}

impl Default for WeiYuanHui {
    fn default() -> Self {
        let (updates_src, updates) = mpsc::channel(64);
        let (ev_tx, ev_rx) = broadcast::channel(64);
        let (publish, publish_dst) = watch::channel(FullBench::default());
        let updates_src = Some(updates_src);
        let publish_dst = Some(publish_dst);
        let ev_tx = Some(ev_tx);
        let mut bench = FullBench::default();
        bench.init();
        Self {
            updates,
            updates_src,
            publish,
            publish_dst,
            ev_tx,
            ev_rx,
            bench,
            savepath: Option::default(),
            counter: VCounter::default(),
        }
    }
}

impl From<FullBench> for WeiYuanHui {
    fn from(bench: FullBench) -> Self {
        Self {
            bench,
            ..Default::default()
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
        loaded.ok().unwrap_or_default()
    }

    fn load_check<P: AsRef<Path>>(path: P) -> Result<Self> {
        let savepath: Option<Box<Path>> = Some(path.as_ref().into());
        let file = File::open(path)?;
        let reader = BufReader::new(file);
        let mut bench: FullBench = serde_json::from_reader(reader)?;
        bench.init();
        Ok(Self {
            bench,
            savepath,
            ..Default::default()
        })
    }

    #[must_use]
    pub fn listen_events(&self) -> broadcast::Receiver<Events> {
        self.ev_rx.resubscribe()
    }

    /// # Panics
    /// Panic on `WeiYuanHui` is closing.
    pub fn new_chair(&mut self) -> WeiYuan {
        WeiYuan {
            update: Some(
                self.updates_src
                    .as_ref()
                    .expect("new_chair in closing")
                    .clone(),
            ),
            fetch: self
                .publish_dst
                .as_ref()
                .expect("new_chair in closing")
                .clone(),
            bench: self.bench.clone(),
        }
    }

    #[must_use]
    pub const fn bench(&self) -> &FullBench {
        &self.bench
    }

    pub fn close(&mut self) {
        self.updates_src = None;
        self.publish_dst = None;
        self.ev_tx = None;
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
                        .add_log(0, &format!("#WeiYuanHui# save_disk error: {}", &e)),
                );
                log::error!("save_disk error: {}", e);
            }
        }
        true
    }

    /// # Errors
    /// Throw if duration poisoned.
    pub async fn run_until(&mut self, deadline: DateTime<Utc>) -> Result<bool> {
        loop {
            let now = Utc::now();
            if now > deadline {
                return Ok(true);
            }
            let duration = deadline - now;
            match tokio::time::timeout(duration.to_std()?, self.run()).await {
                Ok(false) => return Ok(false),
                Err(_) => return Ok(true),
                _ => (),
            }
        }
    }

    /// # Errors
    /// Throw if duration poisoned.
    pub async fn run_for(&mut self, duration: Duration) -> Result<bool> {
        self.run_until(Utc::now() + duration).await
    }

    /// @return is running
    async fn try_update(&mut self) -> bool {
        let msg = self.updates.recv().await;
        msg.map_or(false, |msg| {
            self.try_push(msg);
            true
        })
    }

    fn try_push(&mut self, upd: BenchUpdate) {
        if upd.0.ptr_eq(&self.bench) {
            log::trace!("WeiYuanHui#try_push ok");
            self.push(upd.1);
        } else {
            log::trace!("WeiYuanHui#try_push abort");
            self.counter.push_miss_cnt += 1;
            if let Some(msg) = self.counter.try_log(&self.bench) {
                self.push(self.bench.add_log(3, &msg));
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

    fn push(&mut self, mut next: FullBench) {
        if !next.events.is_empty() {
            let pass: Events = next
                .events
                .into_iter()
                .filter(|ev| {
                    ev[COUNTER_TAG]
                        .as_str()
                        .map(|s| *self.counter.ext.entry(s.into()).or_default() += 1)
                        .is_none()
                })
                .collect();
            if !pass.is_empty()
                && self
                    .ev_tx
                    .as_ref()
                    .map_or(true, |tx| tx.send(pass).is_err())
            {
                self.counter.broadcast_void_cnt += 1;
            }
            next.events = im::Vector::default();
        }
        self.bench = next.clone();
        self.publish.send_modify(move |v| *v = next);
    }
}

impl WeiYuan {
    #[must_use]
    pub fn readonly(&self) -> Self {
        Self {
            update: None,
            ..Clone::clone(self)
        }
    }

    pub async fn changed(&mut self) {
        self.fetch
            .changed()
            .await
            .is_ok()
            .then(|| self.bench = self.fetch.borrow().clone());
    }

    /// @return None for closing
    /// # Errors
    /// Throw if closing.
    /// # Panics
    /// Panic on `WeiYuanHui` drop too fast.
    pub fn recv(&mut self) -> Result<&FullBench> {
        match self.fetch.has_changed() {
            Ok(true) => self.bench = self.fetch.borrow_and_update().clone(),
            Err(e) => panic!("watch chan: WeiYuanHui drop too fast: {e:#}"),
            _ => (),
        }
        self.bench
            .runtime_is_closing()
            .not()
            .then_some(&self.bench)
            .ok_or_else(|| anyhow!("WeiYuanHui closing"))
    }

    /// # Errors
    /// Throw if closing or worker throw.
    /// # Panics
    /// Panic on poisoned state.
    pub fn update<F>(&mut self, f: F) -> Result<()>
    where
        F: Fn(&FullBench) -> Result<FullBench>,
    {
        let msg: BenchUpdate;
        loop {
            let old = self.recv()?.clone();
            let new = f(&old)?;
            if old.ptr_eq(self.recv()?) {
                msg = BenchUpdate(old, new);
                break;
            }
        }
        match self
            .update
            .as_ref()
            .expect("try update in READONLY WeiYuan")
            .try_send(msg)
        {
            Ok(()) => {
                log::trace!("WeiYuan#update sent ok");
                Ok(())
            }
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

    /// # Errors
    /// Throw if closing or worker throw.
    pub fn apply<F>(&mut self, f: F) -> Result<()>
    where
        F: Fn(&mut FullBench) -> Result<()>,
    {
        self.update(|b| {
            let mut v = b.clone();
            f(&mut v)?;
            Ok(v)
        })
    }

    pub fn log<S: ToString + ?Sized>(&mut self, level: i32, msg: &S) {
        self.update(|b| Ok(b.add_log(level, &msg.to_string()))).ok();
    }

    pub fn count<S: ToString + ?Sized>(&mut self, msg: &S) {
        self.apply(|b| {
            b.events.push_back(json!({COUNTER_TAG: msg.to_string()}));
            Ok(())
        })
        .ok();
    }

    /// # Panics
    /// Panic on `WeiYuanHui` drop too fast.
    pub async fn until_closing(&mut self) {
        self.fetch
            .wait_for(FullBench::runtime_is_closing)
            .await
            .map_err(|e| panic!("fetch channel unexpected closed: {e}"))
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

#[must_use]
pub fn now_timestamp() -> i64 {
    Utc::now().timestamp()
}

fn pending_up_info(id: i64, fid: usize, ban: bool) -> Value {
    json!({
        "pick": {
            "basic": {
                "face_url": "https://i2.hdslb.com/bfs/face/0badf24e42d23a14255ee3809866791a9080461e.jpg",
                "name": "pending ...",
                "ban": ban,
                "id": id,
                "fid": fid,
                "ctime": 0,
            }
        }
    })
}

fn live_entropy(v: &Value) -> i64 {
    v["pick"]["live"]["entropy"].as_i64().unwrap_or(-1)
}

fn new_video_ts(v: &Value) -> i64 {
    v["pick"]["video"]["ts"].as_i64().unwrap_or(0)
}

fn info_ctime(v: &Value) -> i64 {
    v["pick"]["basic"]["ctime"].as_i64().unwrap_or(0)
}

fn default_bucket() -> Value {
    json!({
        "atime": to_value(Utc::now()).unwrap(),
        "min_gap": 10,
        "min_change_gap": 10,
        "gap": 30,
    })
}

impl FullBench {
    #[must_use]
    pub fn new() -> Self {
        let mut r = Self::default();
        r.init();
        r
    }

    fn ptr_eq(&self, other: &Self) -> bool {
        self.up_info.ptr_eq(&other.up_info)
            && self.up_index.ptr_eq(&other.up_index)
            && im_vector_p_eq(&self.up_by_fid, &other.up_by_fid)
            && self.up_join_group.ptr_eq(&other.up_join_group)
            && im_vector_p_eq(&self.events, &other.events)
            && self.group_info.ptr_eq(&other.group_info)
            && im_vector_p_eq(&self.logs, &other.logs)
            && self.runtime.ptr_eq(&other.runtime)
            && im_vector_p_eq(&self.commands, &other.commands)
    }

    fn add_log(&self, level: i32, msg: &str) -> Self {
        let mut r = self.clone();
        r.log(level, msg);
        r
    }

    fn log(&mut self, level: i32, msg: &str) {
        let cf = self.runtime.get("log_filter").unwrap_or(&Value::Null);
        let maxlv = cf["maxlevel"]
            .as_i64()
            .and_then(|x| i32::try_from(x).ok())
            .unwrap_or(3);
        if level > maxlv {
            return;
        }
        self.logs
            .push_back(json!({"ts": to_value(Utc::now()).unwrap(), "level": level, "msg": msg}));
        let bufl = cf["buffer_lines"]
            .as_u64()
            .and_then(|x| usize::try_from(x).ok())
            .unwrap_or(2048);
        if self.logs.len() > bufl {
            let fitl = cf["fit_lines"].as_u64().unwrap_or(16);
            for _ in 0..=fitl {
                self.logs.pop_front();
            }
        }
    }

    pub fn inspect<'a, T>(&mut self, res: &'a Result<T>) -> &'a Result<T> {
        if let Err(e) = res {
            self.log(1, &format!("inspect: {e:#}"));
        }
        res
    }

    fn init(&mut self) {
        self.runtime_rm_closing();
        self.touch_group_unchecked(&json!({
            "gid": 0,
            "name": "全部",
            "pin": true,
        }));
        self.touch_group_unchecked(&json!({
            "gid": 1,
            "name": "特殊关注",
            "pin": true,
        }));
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
        self.runtime_dump_time().map_or(true, |t| t < Utc::now())
    }

    fn set_runtime_next_dump(&self) -> Self {
        self.mut_runtime_field("db", |v| {
            v["dump_time"] =
                to_value(Utc::now() + Duration::minutes(self.runtime_dump_timeout_min())).unwrap();
        })
    }

    fn runtime_dump_timeout_min(&self) -> i64 {
        im::get_in!(self.runtime, "db")
            .and_then(|v| v["dump_timeout_min"].as_i64())
            .unwrap_or(720)
    }

    fn runtime_vlog_dump_gap(&self) -> Duration {
        Duration::seconds(
            self.runtime
                .get("db")
                .and_then(|v| v["vlog_dump_gap_sec"].as_i64())
                .unwrap_or(60),
        )
    }

    fn runtime_set_closing(&self) -> Self {
        self.mut_runtime_field(FLAG_CLOSING, |v| *v = Value::Bool(true))
    }

    fn runtime_rm_closing(&mut self) {
        self.runtime.remove(FLAG_CLOSING);
    }

    fn runtime_is_closing(&self) -> bool {
        self.runtime.get(FLAG_CLOSING).is_some()
    }

    fn bucket_checked(&mut self) -> &mut Value {
        self.runtime
            .entry("bucket".into())
            .or_insert_with(default_bucket)
    }

    fn bucket_or_default(&self) -> Value {
        self.runtime
            .get("bucket")
            .cloned()
            .unwrap_or_else(default_bucket)
    }

    /// # Panics
    /// Panic on storing value poisoned
    #[must_use]
    pub fn bucket_duration_to_next(&self) -> Duration {
        let v = self.bucket_or_default();
        let deadline = from_value::<DateTime<Utc>>(v["atime"].clone())
            .unwrap_or_else(|e| panic!("runtime.bucket.atime corrupted: {e}"))
            + Duration::seconds(v["gap"].as_i64().expect("runtime.bucket.gap SHOULD be i64"));
        std::cmp::max(deadline - Utc::now(), Duration::milliseconds(100))
    }

    fn bucket_access(&mut self) {
        let v = self.bucket_checked();
        v["atime"] = to_value(Utc::now()).unwrap();
    }

    /// # Panics
    /// Panic on storing value poisoned
    pub fn bucket_good(&mut self) {
        let v = self.bucket_checked();
        v["gap"] = std::cmp::max(
            v["gap"].as_i64().unwrap() - v["min_change_gap"].as_i64().unwrap(),
            v["min_gap"].as_i64().unwrap(),
        )
        .into();
    }

    /// # Panics
    /// Panic on storing value poisoned
    pub fn bucket_hang(&mut self) {
        let v = self.bucket_checked();
        let g = v["gap"].as_i64().unwrap();
        let t = v["atime"].as_i64().unwrap();
        v["gap"] = (g + v["min_change_gap"].as_i64().unwrap() + t % 7).into();
    }

    /// # Panics
    /// Panic on storing value poisoned
    pub fn bucket_double_gap(&mut self) {
        let v = self.bucket_checked();
        v["gap"] = (v["gap"].as_u64().unwrap() * 2).into();
    }

    /// General api, for www use
    ///
    /// # Errors
    /// Throw if storing value invalid.
    pub fn runtime_field(&self, key: &str, path: &str) -> Result<Value> {
        self.runtime
            .get(key)
            .ok_or_else(|| anyhow!("runtime miss field {}", key))
            .and_then(|v| {
                let mut t: &Value = v;
                // FIXME consider use default if broken
                ChairData::expect(schema_uri!("runtime", key), t)?;
                for p in path.split('/') {
                    match t.get(p) {
                        Some(r) => t = r,
                        None => return Ok(Value::Null),
                    }
                }
                Ok(t.clone())
            })
    }

    /// General api, for www use
    ///
    /// # Errors
    /// Throw if setting value invald.
    pub fn runtime_set_field(&mut self, key: &str, path: &str, val: Value) -> Result<()> {
        let mut o = self.runtime.get(key).cloned().unwrap_or(Value::Null);
        let ins = o.is_null();
        let mut v = &mut o;
        for p in path.split('/') {
            if v.get(p).is_none() {
                v[p] = Value::Object(serde_json::Map::default());
            }
            v = v
                .get_mut(p)
                .ok_or_else(|| anyhow!("internal error: cannot get inserted ref"))?;
        }
        *v = val;
        ChairData::expect(schema_uri!("runtime", key), &o)?;
        if ins {
            self.runtime.insert(key.into(), o);
        }
        Ok(())
    }

    /// # Errors
    /// Throw if input invalid.
    pub fn follow(&mut self, opt: &Value) -> Result<()> {
        log::trace!("bench#follow opt: {:?}", opt);
        ChairData::expect(schema_uri!("follow"), opt)?;
        let uid = opt["uid"]
            .as_i64()
            .ok_or_else(|| anyhow!("uid out of i64 range"))?;
        let enable = opt["enable"].as_bool().unwrap_or(true);
        self.log(2, &format!("follow uid:{uid} enable:{enable}"));
        if enable {
            self.commands.push_back(json!({
                "cmd": "fetch",
                "args": {
                    "uid": uid,
                }
            }));
            log::trace!("push cmd: {:?}", self.commands.back());
        }
        let id = &uid.to_string();
        self.up_info
            .get_mut(id)
            .map(|v| v["pick"]["basic"]["ban"] = (!enable).into())
            .is_none()
            .then(|| {
                self.up_info.insert(
                    id.into(),
                    pending_up_info(uid, self.up_by_fid.len(), !enable),
                );
                self.update_index("ctime", -1, 0, id);
                self.up_by_fid.push_back(id.into());
            });
        Ok(())
    }

    fn checked_uid(&mut self, opt: &Value, key: &str) -> Result<i64> {
        let uid = opt[key].as_i64().unwrap();
        let struid = uid.to_string();
        self.up_info
            .contains_key(&struid)
            .then_some(uid)
            .ok_or_else(|| anyhow!("operate on not tracing uid"))
    }

    /// # Errors
    /// Throw if input or uid invalid.
    pub fn refresh(&mut self, opt: &Value) -> Result<()> {
        ChairData::expect(schema_uri!("refresh"), opt)?;
        let uid = self.checked_uid(opt, "uid")?;
        self.commands.push_back(json!({
            "cmd": "fetch",
            "args": {
                "uid": uid,
            }
        }));
        Ok(())
    }

    /// # Errors
    /// Currently no errors in impl.
    pub fn force_silence(&mut self, _opt: &Value) -> Result<()> {
        self.bucket_double_gap();
        Ok(())
    }

    fn inited_gid(&mut self, opt: &Value, key: &str) -> String {
        let gid = opt[key].as_i64().unwrap().to_string();
        self.group_info.contains_key(&gid).not().then(|| {
            self.group_info.insert(
                gid.clone(),
                json!({
                    "name": "[placeholder]",
                    "removable": true,
                }),
            )
        });
        self.up_join_group.contains_key(&gid).not().then(|| {
            self.up_join_group
                .insert(gid.clone(), im::OrdSet::default());
        });
        gid
    }

    /// # Errors
    /// Throw if input invalid
    ///
    /// # Panics
    /// Panic on not tracing uid.
    pub fn toggle_group(&mut self, opt: &Value) -> Result<()> {
        ChairData::expect(schema_uri!("toggle_group"), opt)?;
        let suid = self.checked_uid(opt, "uid")?.to_string();
        let gid = self.inited_gid(opt, "gid");
        self.log(2, &format!("toggle_group uid:{} gid:{}", &suid, &gid));
        if self
            .up_join_group
            .get(&gid)
            .map_or(false, |s| s.contains(&suid))
        {
            self.up_join_group.get_mut(&gid).unwrap().remove(&suid);
        } else {
            self.up_join_group.get_mut(&gid).unwrap().insert(suid);
        }
        Ok(())
    }

    /// # Errors
    /// Throw if input invalid
    pub fn touch_group(&mut self, opt: &Value) -> Result<()> {
        ChairData::expect(schema_uri!("touch_group"), opt)?;
        self.touch_group_unchecked(opt);
        Ok(())
    }

    fn touch_group_unchecked(&mut self, opt: &Value) {
        let gid = self.inited_gid(opt, "gid");
        let info = self
            .group_info
            .get_mut(&gid)
            .expect("inited_gid SHOULD inited group info");
        if let Some(pin) = opt["pin"].as_bool() {
            info["removable"] = (!pin).into();
        }
        if let Some(name) = opt["name"].as_str() {
            info["name"] = name.into();
        }
    }

    /// # Errors
    /// Throw if input invalid
    pub fn users_pick(&self, opt: &Value) -> Result<Value> {
        ChairData::expect(schema_uri!("users_pick"), opt)?;
        let st = opt["range_start"]
            .as_u64()
            .and_then(|x| usize::try_from(x).ok())
            .unwrap_or(0);
        let len = opt["range_len"]
            .as_u64()
            .and_then(|x| usize::try_from(x).ok())
            .unwrap_or(10);
        let gid = opt["gid"].as_i64().unwrap_or(0);
        let Some(group) = self.up_join_group.get(&gid.to_string()) else {
            bail!("group {:?} not found", opt["gid"]);
        };
        let ids: Vec<&str> = match (
            gid,
            opt["order_desc"].as_str().is_some_and(|s| s == "default"),
        ) {
            (0, true) => self
                .up_by_fid
                .iter()
                .skip(st)
                .take(len)
                .map(AsRef::as_ref)
                .collect(),
            (_, true) => self
                .up_by_fid
                .iter()
                .filter(|s| group.contains(*s))
                .skip(st)
                .take(len)
                .map(AsRef::as_ref)
                .collect(),
            (0, false) => self
                .up_index
                .get(opt["order_desc"].as_str().unwrap_or("default"))
                .ok_or_else(|| anyhow!("index not found"))?
                .iter()
                .skip(st)
                .take(len)
                .map(|v| v.1.as_ref())
                .collect(),
            (_, false) => self
                .up_index
                .get(opt["order_desc"].as_str().unwrap_or("default"))
                .ok_or_else(|| anyhow!("index not found"))?
                .iter()
                .filter(|t| group.contains(&t.1))
                .skip(st)
                .take(len)
                .map(|v| v.1.as_ref())
                .collect(),
        };
        let a: Vec<_> = ids
            .into_iter()
            .filter_map(|id| self.up_info.get(id).map(|v| v["pick"].clone()))
            .collect();
        Ok(json!(a))
    }

    fn update_index(&mut self, typ: &str, old_value: i64, value: i64, uid: &str) {
        if old_value == value {
            return;
        }
        let index = self.up_index.entry(typ.into()).or_default();
        index.remove(&(old_value, uid.into()));
        index.insert((value, uid.into()));
        match typ {
            "video" | "live" => self.events.push_back(json!({
                "type": typ,
                typ: self.up_info.get(uid).expect("updating up_info SHOULD exists")["pick"][typ],
            })),
            _ => (),
        }
    }

    /// # Panics
    /// Panic on data poisoned
    pub fn modify_up_info<F>(&mut self, uid: &str, mut f: F)
    where
        F: FnMut(&mut Value),
    {
        let info = self
            .up_info
            .get_mut(uid)
            .expect("modifing up_info SHOULD be inited");
        let old_info = &info.clone();
        f(info);
        let video = new_video_ts(info);
        let live = live_entropy(info);
        let ctm = info_ctime(info);
        self.update_index("video", new_video_ts(old_info), video, uid);
        self.update_index("live", live_entropy(old_info), live, uid);
        self.update_index("ctime", info_ctime(old_info), ctm, uid);
        self.bucket_access();
    }
}

impl VCounter {
    pub fn try_log(&mut self, bench: &FullBench) -> Option<String> {
        if self.last_dump_ts.map_or(true, |t| Utc::now() > t) {
            let r = format!("VCounter: {self:?}");
            self.last_dump_ts = Some(Utc::now() + bench.runtime_vlog_dump_gap());
            Some(r)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem;
    use std::time::Duration as Dur;
    use tokio::time::timeout;

    fn init() {
        env_logger::builder()
            .is_test(true)
            .format_timestamp(Some(env_logger::fmt::TimestampPrecision::Micros))
            .try_init()
            .ok();
    }

    #[test]
    fn test_runtime_dump_now_default() {
        let bench = FullBench::default();
        assert!(bench.runtime_dump_now());
    }

    #[test]
    fn test_runtime_dump_timeout_min_default() {
        let bench = FullBench::default();
        assert_eq!(bench.runtime_dump_timeout_min(), 720_i64);
        assert!(bench.runtime_dump_now());
        let next = bench.set_runtime_next_dump();
        assert!(!next.runtime_dump_now());
    }

    #[test]
    fn test_runtime_field_set_n_get() {
        let mut bench = FullBench::default();
        assert_eq!(
            bench
                .runtime_set_field("db", "dump_timeout_min", json!(42))
                .as_ref()
                .map_err(ToString::to_string),
            Ok(&())
        );
        assert_eq!(
            bench.runtime.get("db"),
            Some(&json!({"dump_timeout_min":42}))
        );
        assert_eq!(
            bench.runtime_field("db", "dump_timeout_min").ok(),
            Some(json!(42))
        );
    }

    async fn run_1s(center: &mut WeiYuanHui) -> bool {
        center
            .run_for(Duration::milliseconds(100))
            .await
            .expect("should be in normal stat")
    }

    async fn run_ms(center: &mut WeiYuanHui, ms: i64, running: bool) {
        assert_eq!(
            center
                .run_for(chrono::Duration::milliseconds(ms))
                .await
                .ok(),
            Some(running)
        );
    }

    async fn check_closed(center: &WeiYuanHui) {
        assert!(timeout(Dur::from_secs(1), center.closed()).await.is_ok());
    }

    #[tokio::test]
    async fn test_two_chairs() {
        let mut center = WeiYuanHui::default();
        assert_eq!(center.bench.runtime.get(FLAG_CLOSING), None);
        assert!(!center.bench.runtime_is_closing());
        {
            let mut chair = center.new_chair();
            let mut chair_rx = chair.clone();
            assert_eq!(
                chair
                    .apply(|b| {
                        let r = b.runtime_set_field("bucket", "min_gap", json!(23));
                        println!("new bench: {b:?}");
                        r
                    })
                    .err()
                    .map(|e| format!("{e:?}")),
                None
            );
            assert!(run_1s(&mut center).await);
            assert!(chair_rx
                .recv()
                .as_ref()
                .map_err(ToString::to_string)
                .is_ok());
            let cur = chair_rx.recv().unwrap();
            println!("bucket: {:?}", cur.runtime);
            assert_eq!(
                cur.runtime_field("bucket", "min_gap")
                    .as_ref()
                    .map_err(ToString::to_string),
                Ok(&json!(23))
            );
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
            bench = bench.add_log(2, &format!("test log {i}"));
        }
        assert_eq!(bench.logs.len(), 2048);
        bench = bench.add_log(4, "will discard log");
        assert_eq!(bench.logs.len(), 2048);
        bench = bench.add_log(-1, "this log trigger buffer shorten");
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
        assert!(chair.update(|v| Ok(v.clone())).is_err());
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

    #[tokio::test]
    async fn test_broadcast_events() {
        init();
        let ls = vec![
            json!({
                "uid":12345,
                "live": {"isopen":"true"},
            }),
            json!({
                "uid":2233,
                "live": {"isopen":"true"},
                "video": {"ts":9977},
            }),
        ];
        let mut center = WeiYuanHui::default();
        let mut tx = center.new_chair();
        let mut rx = center.listen_events();
        let ls_rx = ls.clone();
        tokio::join!(
            async move {
                let mut it = ls_rx.iter();
                loop {
                    match rx.recv().await {
                        Ok(v) => {
                            assert_eq!(v.len(), 1);
                            assert_eq!(v.front(), it.next());
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            assert_eq!(None, it.next());
                            return;
                        }
                        _ => (),
                    }
                }
            },
            async move {
                run_ms(&mut center, 50, true).await;
                tx.count("ev_tx/test");
                for ev in ls {
                    run_ms(&mut center, 50, true).await;
                    assert!(tx
                        .apply(|b| {
                            b.events.push_back(ev.clone());
                            Ok(())
                        })
                        .is_ok());
                }
                run_ms(&mut center, 50, true).await;
                center.close();
                std::mem::drop(tx);
                check_closed(&center).await;
            }
        );
    }

    #[test]
    fn test_bench_update_index() {
        let mut bench = FullBench::default();
        bench.update_index("live", 9, 8, "12345");
        assert!(bench.up_index.get("live").is_some());
        assert_eq!(
            bench.up_index.get("live").unwrap().get_min(),
            Some(&(8i64, "12345".to_string()))
        );
        // TODO check events
        bench.update_index("live", 8, 117, "12345");
        assert_eq!(
            bench.up_index.get("live").unwrap().get_min(),
            Some(&(117i64, "12345".to_string()))
        );
    }

    // TODO test modify_up_info
    // 1. expect events
    // 2. index
}

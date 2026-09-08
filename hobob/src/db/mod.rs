//! 数据中枢：内存 ECS world + 快照并发协议（M1 · 方案 `.plans/m1-ecs-core.md` §4.3-4.4 / D2-D4）。
//!
//! 由 v1 的 `FullBench`（9 个 im 字段）演进为 `Snapshot { world: World, res: Resources }`：
//! - **world** 持有组件：up → `Brick`/`LivePost`/`VideoPost`/`CommentPost`/`RawInfo`；
//!   group → `GroupInfo`/`Members`；entity 1 = runtime（`RuntimeCfg`）。
//! - **res** 持有内存索引/队列（非组件，不落 redb，M0 §4.3 决议）：`up_index`/`up_by_fid`/
//!   `uid_index`/`gid_index`/`events`/`logs`/`commands`。
//! - hub（`WeiYuanHui`）持有权威 `Snapshot`，chair（`WeiYuan`）持快照副本；
//!   `apply/update` 走 mpsc 提交 `(base, next)`，hub 用 `ptr_eq`（world 两棵 map 根 +
//!   res 各 im 字段根）校验 base 仍是当前值，不匹配 abort——与 v1 逐行同构。
//!
//! 与方案文档的差异（实现备注）：
//! - `VCounter`（push_miss/broadcast_void 统计）保留为 hub 私有字段而非挪进
//!   `Resources`：它一旦进快照，push_miss 这类 hub 内部自更新就必须随 publish
//!   发布或与权威快照分叉；v1 语义（统计不外发、节流记日志）要求它留在 hub。
//! - 组件类型定义在本模块（内存模型），store 持久化信封（store.rs）与之同构，
//!   桥接在 `WeiYuanHui::open`（启动加载）/`push` 持久化 diff（运行期直写 + stage）完成。
//! - `WeiYuanHui::open(store)` 是唯一持久化入口（取代 v1 `load(bench.json)`，D12）；
//!   `close()` 停机强刷易变缓冲后释放 store（D8）。

use crate::data_schema::ChairData;
use crate::ecs::{Entity, World};
use crate::store::{self, Store};
use anyhow::{anyhow, bail, Result};
use chrono::{DateTime, Duration, Utc};
use serde_json::json;
use serde_json::{from_value, to_value, Value};
use std::collections::BTreeMap;
use std::ops::Not;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{broadcast, mpsc, watch};

const COUNTER_TAG: &str = "#COUNTER#";

/// 实体号布局（与 store `INITIAL_ENTITY_ID`/`alloc_entity_id` 语义衔接，D6）：
/// 1 = 全局 runtime；2/3 = 内置组「全部/特殊关注」（API gid 0/1）；≥4 普通实体。
pub const ENTITY_RUNTIME: u64 = 1;
const ENTITY_GROUP_ALL: u64 = 2;
const ENTITY_GROUP_SPECIAL: u64 = 3;

/// v1 `pending_up_info` 的默认头像。
const PENDING_FACE: &str =
    "https://i2.hdslb.com/bfs/face/0badf24e42d23a14255ee3809866791a9080461e.jpg";

// ============================== 组件（内存模型） ==============================
//
// 字段语义与 v1 JSON 的逐项校准见方案 §4.3 表；M1 只读/写这些字段，
// `pick_json` 负责反投影回 v1 的 `pick` JSON 形状（§4.8，字节级不变）。

/// up 基础资料 + 管理状态（对应 v1 `pick.basic` + 关注/分组状态；store Brick V2 信封镜像）。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Brick {
    pub uid: String,
    pub uname: String,
    pub face: String,
    /// v1 `pick.basic.ban`：取关保留实体（enable=false → ban=true）。
    pub ban: bool,
    /// v1 `up_by_fid` 序号（关注顺序，重启后按它重建索引）。
    pub fid: u64,
    /// 所属分组（group **entity id**，D7；成员关系权威，落盘）。
    pub groups: Vec<u64>,
    /// M1 恒 false，语义留给 M3 动态 system。
    pub silent: bool,
    /// unix 秒。v1 无对应字段（M1 语义补充）。
    pub followed_at: i64,
    /// 最近一次 fetch 成功时间 = v1 `pick.basic.ctime`（重建 ctime 索引）。
    pub updated_at: i64,
}

/// 直播状态（对应 v1 `pick.live`；store LivePostV1 信封 + D7 增量字段）。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LivePost {
    pub updated_at: i64,
    pub is_open: bool,
    pub title: String,
    pub url: String,
    /// 保底字段（与 store LivePostV1.ts 对应；M1 恒 0）。
    pub ts: i64,
    /// 观看人数（v1 `pick.live.entropy`，live 排序索引值，缺失 -1）。
    pub entropy: i64,
    pub entropy_txt: String,
}

/// 视频条目（对应 API `list.vlist` 一项）。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VideoItem {
    pub bvid: String,
    pub title: String,
    pub pubdate: i64,
}

/// 最新视频（对应 v1 `pick.video`；store VideoPostV1 信封镜像）。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VideoPost {
    pub updated_at: i64,
    /// = items[0].pubdate（无视频 0），video 排序索引值。
    pub latest_ts: i64,
    pub items: Vec<VideoItem>,
    /// API `episodic_button.uri`（"//www.bilibili.com/..."），反投影 `pick.video.url`
    /// 时补 `https:` 前缀（v1 `pick_video` 同款）。
    pub episodic: Option<String>,
}

/// 评论（v1 无数据源，M1 恒空；store CommentPostV1 信封镜像）。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CommentPost {
    pub updated_at: i64,
    pub items: Vec<CommentItem>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CommentItem {
    pub rpid: u64,
    pub msg: String,
    pub ts: i64,
}

/// fetch 原始 API 响应（v1 `up_info[uid].raw`）。**内存 only**，不落盘（D13）。
#[derive(Clone, Debug, PartialEq)]
pub struct RawInfo(pub Value);

/// 全局 runtime 配置（v1 `FullBench.runtime` JSON 原样收容：index/db/log_filter/bucket）。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RuntimeCfg(pub im::HashMap<String, Value>);

/// 分组资料（对应 v1 `group_info[gid]`；gid = API 可见 gid，entity id ≠ gid，D6）。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GroupInfo {
    pub gid: u64,
    pub name: String,
    /// v1 `removable = !pin`；内置组（0/1）pin=true。
    pub pin: bool,
}

/// 组 → 成员（up entity id）内存反索引（D5；由各 up 的 `Brick.groups` 重建/维护）。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Members(pub im::OrdSet<u64>);

// ============================== Resources ==============================

/// v1 同名类型（索引值 = (排序值, uid 字符串)）。
pub type UpIndex = im::HashMap<String, im::OrdSet<(i64, String)>>;
pub type Events = im::Vector<Value>;
pub type LogRecords = im::Vector<Value>;
pub type Commands = im::Vector<Value>;

/// 内存索引/队列（快照随发布传播；不落 redb）。
#[derive(Clone, Debug, Default)]
pub struct Resources {
    /// video/live/ctime 三个排序索引（v1 `up_index` 同构）。
    pub up_index: UpIndex,
    /// 关注顺序 = up **entity id** 列表（v1 `up_by_fid` 是 uid 字符串，M1 改 entity；D4）。
    pub up_by_fid: im::Vector<u64>,
    /// uid 字符串 → up entity。
    pub uid_index: im::HashMap<String, u64>,
    /// API gid → group entity。
    pub gid_index: im::HashMap<u64, u64>,
    /// SSE 事件队列（#COUNTER# 语义保留，hub push 时 drain→broadcast）。
    pub events: Events,
    /// 环形日志缓冲（M2 KV 化前保持 v1 语义）。
    pub logs: LogRecords,
    /// fetch 命令队列（engine take_cmds 消费）。
    pub commands: Commands,
    /// closing 标志（v1 runtime JSON `#CLOSING#` 挪入 Resources，§4.4；对外不可见）。
    pub closing: bool,
}

// ============================== Snapshot ==============================

/// 快照 = 权威/视图数据单元，替代 v1 `FullBench`。
#[derive(Clone)]
pub struct Snapshot {
    pub world: World,
    pub res: Resources,
}

/// 简易运行时错误日志（engine apply 闭包外用）。
pub struct BenchUpdate(Snapshot, Snapshot);

impl Default for Snapshot {
    fn default() -> Self {
        let mut world = World::default();
        // entity 1 = runtime：任何 Snapshot（含未 init）都有，runtime helper 免判空。
        world
            .spawn_at(ENTITY_RUNTIME)
            .expect("fresh world SHOULD have free id 1");
        world.insert(Entity(ENTITY_RUNTIME), RuntimeCfg::default());
        Self {
            world,
            res: Resources::default(),
        }
    }
}

fn im_vector_p_eq<A: Clone + Eq>(lhs: &im::Vector<A>, rhs: &im::Vector<A>) -> bool {
    match (lhs.is_inline(), rhs.is_inline()) {
        (true, true) => *lhs == *rhs,
        (false, false) => lhs.ptr_eq(rhs),
        _ => false,
    }
}

impl Snapshot {
    /// 空 hub 快照（runtime + 内置组 0/1）。
    #[must_use]
    pub fn new() -> Self {
        let mut r = Self::default();
        r.init();
        r
    }

    fn init(&mut self) {
        self.touch_group_internal(0, "全部", true);
        self.touch_group_internal(1, "特殊关注", true);
    }

    /// 结构共享判定：world 两棵 map 根 + res 各 im 字段根（v1 9 字段 ptr 比对的对等物）。
    #[must_use]
    pub fn ptr_eq(&self, other: &Self) -> bool {
        self.world.ptr_eq(&other.world)
            && self.res.up_index.ptr_eq(&other.res.up_index)
            && im_vector_p_eq(&self.res.up_by_fid, &other.res.up_by_fid)
            && self.res.uid_index.ptr_eq(&other.res.uid_index)
            && self.res.gid_index.ptr_eq(&other.res.gid_index)
            && im_vector_p_eq(&self.res.events, &other.res.events)
            && im_vector_p_eq(&self.res.logs, &other.res.logs)
            && im_vector_p_eq(&self.res.commands, &other.res.commands)
            && self.res.closing == other.res.closing
    }

    // ---------- runtime（entity 1 RuntimeCfg） ----------

    fn runtime_cfg(&self) -> &im::HashMap<String, Value> {
        &self
            .world
            .get::<RuntimeCfg>(Entity(ENTITY_RUNTIME))
            .expect("runtime entity SHOULD have RuntimeCfg")
            .0
    }

    fn runtime_cfg_mut(&mut self) -> &mut im::HashMap<String, Value> {
        &mut self
            .world
            .get_mut::<RuntimeCfg>(Entity(ENTITY_RUNTIME))
            .expect("runtime entity SHOULD have RuntimeCfg")
            .0
    }

    /// 读 runtime 顶层字段（www index 模板/测试用）。
    #[must_use]
    pub fn runtime_get(&self, key: &str) -> Option<&Value> {
        self.runtime_cfg().get(key)
    }

    /// v1 runtime dump 语义保留：M1 起落盘路径移除（T4 flush 接轨），
    /// 这些 helper 仅被对照测试（`test_runtime_dump_*`）与未来 flush 节流使用。
    #[allow(dead_code)]
    fn mut_runtime_field<F: FnOnce(&mut Value)>(&mut self, key: &str, f: F) {
        if self.runtime_cfg().get(key).is_none() {
            self.runtime_cfg_mut().insert(key.into(), Value::default());
        }
        f(self.runtime_cfg_mut().get_mut(key).expect("just inserted"));
    }

    /// 日志/事件过滤与环形缓冲参数（v1 原样，M1 仍由 runtime JSON 控制）。
    fn log_filter_cf(&self) -> (i32, usize, u64) {
        let cf = self.runtime_cfg().get("log_filter").unwrap_or(&Value::Null);
        let maxlv = cf["maxlevel"]
            .as_i64()
            .and_then(|x| i32::try_from(x).ok())
            .unwrap_or(3);
        let bufl = cf["buffer_lines"]
            .as_u64()
            .and_then(|x| usize::try_from(x).ok())
            .unwrap_or(2048);
        let fitl = cf["fit_lines"].as_u64().unwrap_or(16);
        (maxlv, bufl, fitl)
    }

    /// # Panics
    /// Panic on storing value poisoned.
    #[allow(dead_code)]
    fn runtime_dump_time(&self) -> Option<DateTime<Utc>> {
        im::get_in!(self.runtime_cfg(), "db")
            .and_then(|v| from_value::<DateTime<Utc>>(v["dump_time"].clone()).ok())
    }

    /// M1 起 save_disk 移除（T4 flush 接轨），dump 语义保留给 flush 节流对照测试。
    #[allow(dead_code)]
    fn runtime_dump_now(&self) -> bool {
        self.runtime_dump_time().is_none_or(|t| t < Utc::now())
    }

    #[allow(dead_code)]
    fn set_runtime_next_dump(&mut self) {
        let min = self.runtime_dump_timeout_min();
        self.mut_runtime_field("db", |v| {
            v["dump_time"] = to_value(Utc::now() + Duration::minutes(min)).unwrap();
        });
    }

    #[allow(dead_code)]
    fn runtime_dump_timeout_min(&self) -> i64 {
        im::get_in!(self.runtime_cfg(), "db")
            .and_then(|v| v["dump_timeout_min"].as_i64())
            .unwrap_or(720)
    }

    fn runtime_vlog_dump_gap(&self) -> Duration {
        Duration::seconds(
            self.runtime_cfg()
                .get("db")
                .and_then(|v| v["vlog_dump_gap_sec"].as_i64())
                .unwrap_or(60),
        )
    }

    // ---------- bucket（runtime JSON "bucket"，v1 同款 JSON 改写） ----------

    fn bucket_checked(&mut self) -> &mut Value {
        self.runtime_cfg_mut()
            .entry("bucket".into())
            .or_insert_with(default_bucket)
    }

    fn bucket_or_default(&self) -> Value {
        self.runtime_cfg()
            .get("bucket")
            .cloned()
            .unwrap_or_else(default_bucket)
    }

    /// # Panics
    /// Panic on storing value poisoned.
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
    /// Panic on storing value poisoned.
    pub fn bucket_good(&mut self) {
        let v = self.bucket_checked();
        v["gap"] = std::cmp::max(
            v["gap"].as_i64().unwrap() - v["min_change_gap"].as_i64().unwrap(),
            v["min_gap"].as_i64().unwrap(),
        )
        .into();
    }

    /// # Panics
    /// Panic on storing value poisoned.
    pub fn bucket_hang(&mut self) {
        let v = self.bucket_checked();
        let g = v["gap"].as_i64().unwrap();
        let t = v["atime"].as_i64().unwrap();
        v["gap"] = (g + v["min_change_gap"].as_i64().unwrap() + t % 7).into();
    }

    /// # Panics
    /// Panic on storing value poisoned.
    pub fn bucket_double_gap(&mut self) {
        let v = self.bucket_checked();
        v["gap"] = (v["gap"].as_u64().unwrap() * 2).into();
    }

    // ---------- runtime 通用字段 API ----------

    /// General api, for www use.
    ///
    /// # Errors
    /// Throw if storing value invalid.
    pub fn runtime_field(&self, key: &str, path: &str) -> Result<Value> {
        self.runtime_cfg()
            .get(key)
            .ok_or_else(|| anyhow!("runtime miss field {}", key))
            .and_then(|v| {
                let mut t: &Value = v;
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

    /// General api, for www use.
    ///
    /// # Errors
    /// Throw if setting value invalid.
    pub fn runtime_set_field(&mut self, key: &str, path: &str, val: Value) -> Result<()> {
        let mut o = self.runtime_cfg().get(key).cloned().unwrap_or(Value::Null);
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
        // v1 只在 key 原本缺失时 insert（对已存在 key 的修改被 clone 丢弃，系笔误；
        // 无调用路径依赖该 no-op），M1 统一写回。
        self.runtime_cfg_mut().insert(key.into(), o);
        Ok(())
    }

    // ---------- 日志 ----------

    fn log(&mut self, level: i32, msg: &str) {
        let (maxlv, bufl, fitl) = self.log_filter_cf();
        if level > maxlv {
            return;
        }
        self.res.logs.push_back(json!({
            "ts": to_value(Utc::now()).unwrap(),
            "level": level,
            "msg": msg,
        }));
        if self.res.logs.len() > bufl {
            for _ in 0..=fitl {
                self.res.logs.pop_front();
            }
        }
    }

    fn with_log(&self, level: i32, msg: &str) -> Snapshot {
        let mut r = self.clone();
        r.log(level, msg);
        r
    }

    pub fn inspect<'a, T>(&mut self, res: &'a Result<T>) -> &'a Result<T> {
        if let Err(e) = res {
            self.log(1, &format!("inspect: {e:#}"));
        }
        res
    }

    // ---------- 分组 ----------

    /// 内置/用户组实体建立（用户组经 `inited_gid` 走 spawn ≥4）。
    fn touch_group_internal(&mut self, gid: u64, name: &str, pin: bool) {
        let eid = match gid {
            0 => ENTITY_GROUP_ALL,
            1 => ENTITY_GROUP_SPECIAL,
            _ => unreachable!("internal groups only gid 0/1"),
        };
        if !self.world.is_alive(Entity(eid)) {
            self.world
                .spawn_at(eid)
                .expect("builtin group entity id SHOULD be free");
        }
        self.world.insert(
            Entity(eid),
            GroupInfo {
                gid,
                name: name.into(),
                pin,
            },
        );
        self.world
            .insert(Entity(eid), Members(im::OrdSet::default()));
        self.res.gid_index.insert(gid, eid);
    }

    /// v1 `inited_gid`：未知 gid 自动建 placeholder 组实体（`"[placeholder]"`, 可移除），
    /// 返回 group entity id。
    fn inited_gid(&mut self, opt: &Value, key: &str) -> u64 {
        let gid = opt[key]
            .as_u64()
            .expect("schema SHOULD ensure non-negative gid");
        if let Some(&e) = self.res.gid_index.get(&gid) {
            return e;
        }
        let ge = self.world.spawn().0;
        self.world.insert(
            Entity(ge),
            GroupInfo {
                gid,
                name: "[placeholder]".into(),
                pin: false,
            },
        );
        self.world
            .insert(Entity(ge), Members(im::OrdSet::default()));
        self.res.gid_index.insert(gid, ge);
        ge
    }

    fn touch_group_unchecked(&mut self, opt: &Value) {
        let ge = self.inited_gid(opt, "gid");
        if let Some(pin) = opt["pin"].as_bool() {
            if let Some(g) = self.world.get_mut::<GroupInfo>(Entity(ge)) {
                g.pin = pin;
            }
        }
        if let Some(name) = opt["name"].as_str() {
            if let Some(g) = self.world.get_mut::<GroupInfo>(Entity(ge)) {
                g.name = name.to_string();
            }
        }
    }

    // ---------- 索引 ----------

    fn update_index(&mut self, typ: &str, old_value: i64, value: i64, uid: &str) {
        if old_value == value {
            return;
        }
        let index = self.res.up_index.entry(typ.into()).or_default();
        index.remove(&(old_value, uid.to_string()));
        index.insert((value, uid.to_string()));
        match typ {
            "video" | "live" => {
                let payload =
                    self.res
                        .uid_index
                        .get(uid)
                        .copied()
                        .map_or(Value::Null, |e| match typ {
                            "video" => self.video_seg(Entity(e)),
                            _ => self.live_seg(Entity(e)),
                        });
                self.res
                    .events
                    .push_back(json!({ "type": typ, typ: payload }));
            }
            _ => (),
        }
    }

    fn checked_uid(&self, opt: &Value, key: &str) -> Result<i64> {
        let uid = opt[key].as_i64().unwrap();
        let struid = uid.to_string();
        self.res
            .uid_index
            .contains_key(&struid)
            .then_some(uid)
            .ok_or_else(|| anyhow!("operate on not tracing uid"))
    }

    // ---------- 组件段反投影（v1 `pick.live`/`pick.video` JSON） ----------

    fn live_seg(&self, e: Entity) -> Value {
        self.world.get::<LivePost>(e).map_or(Value::Null, |lp| {
            json!({
                "title": lp.title,
                "url": lp.url,
                "entropy": lp.entropy,
                "entropy_txt": lp.entropy_txt,
                "isopen": lp.is_open,
            })
        })
    }

    fn video_seg(&self, e: Entity) -> Value {
        self.world.get::<VideoPost>(e).map_or(Value::Null, |vp| {
            if vp.items.is_empty() {
                return Value::Null;
            }
            json!({
                "title": vp.items[0].title,
                "url": vp.episodic.as_ref().map(|u| format!("https:{u}")),
                "ts": vp.items[0].pubdate,
            })
        })
    }

    /// up 实体 → v1 `pick` JSON（`{basic, live?, video?, raw?}` 形状与 v1 输出一致；
    /// pending up 只有 basic，fetch 后 video 可为 null——模板/消费者语义同 v1）。
    fn pick_of_entity(&self, e: Entity) -> Option<Value> {
        let brick = self.world.get::<Brick>(e)?;
        let mut pick = json!({
            "basic": {
                "id": brick.uid.parse::<i64>().unwrap_or(0),
                "name": brick.uname,
                "face_url": brick.face,
                "ban": brick.ban,
                "fid": brick.fid,
                "ctime": brick.updated_at,
            },
        });
        if self.world.contains::<LivePost>(e) {
            pick["live"] = self.live_seg(e);
        }
        if self.world.contains::<VideoPost>(e) {
            pick["video"] = self.video_seg(e);
        }
        if let Some(raw) = self.world.get::<RawInfo>(e) {
            pick["raw"] = raw.0.clone();
        }
        Some(pick)
    }

    /// v1 `up_info[uid].pick` 的等价物：uid（i64）→ pick JSON。
    ///
    /// # Errors
    /// Throw if uid not traced.
    pub fn pick_of(&self, uid: i64) -> Result<Value> {
        let e = *self
            .res
            .uid_index
            .get(&uid.to_string())
            .ok_or_else(|| anyhow!("uid {} not found", uid))?;
        self.pick_of_entity(Entity(e))
            .ok_or_else(|| anyhow!("uid {} not found", uid))
    }

    // ---------- 业务 ops ----------

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
            self.res.commands.push_back(json!({
                "cmd": "fetch",
                "args": {
                    "uid": uid,
                }
            }));
            log::trace!("push cmd: {:?}", self.res.commands.back());
        }
        let uid_str = uid.to_string();
        if self.res.uid_index.contains_key(&uid_str) {
            // 已有 up：只改 ban（保留实体/索引/分组）
            if let Some(b) = self
                .world
                .get_mut::<Brick>(Entity(*self.res.uid_index.get(&uid_str).unwrap()))
            {
                b.ban = !enable;
            }
        } else {
            // 新 up：pending 占位
            let entity = self.world.spawn();
            let brick = Brick {
                fid: u64::try_from(self.res.up_by_fid.len()).expect("fid overflow"),
                uid: uid_str.clone(),
                uname: "pending ...".into(),
                face: PENDING_FACE.into(),
                ban: !enable,
                followed_at: now_timestamp(),
                ..Brick::default()
            };
            self.world.insert(entity, brick);
            self.res.uid_index.insert(uid_str.clone(), entity.0);
            self.update_index("ctime", -1, 0, &uid_str);
            self.res.up_by_fid.push_back(entity.0);
        }
        Ok(())
    }

    /// # Errors
    /// Throw if input or uid invalid.
    pub fn refresh(&mut self, opt: &Value) -> Result<()> {
        ChairData::expect(schema_uri!("refresh"), opt)?;
        let uid = self.checked_uid(opt, "uid")?;
        self.res.commands.push_back(json!({
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

    /// # Errors
    /// Throw if input invalid.
    ///
    /// # Panics
    /// Panic on not tracing uid.
    pub fn toggle_group(&mut self, opt: &Value) -> Result<()> {
        ChairData::expect(schema_uri!("toggle_group"), opt)?;
        let uid = self.checked_uid(opt, "uid")?;
        let uid_str = uid.to_string();
        let entity = Entity(
            *self
                .res
                .uid_index
                .get(&uid_str)
                .expect("checked_uid guarantees"),
        );
        let gid = opt["gid"].as_i64().unwrap();
        let ge = self.inited_gid(opt, "gid");
        self.log(2, &format!("toggle_group uid:{uid_str} gid:{gid}"));
        let in_group = self
            .world
            .get::<Brick>(entity)
            .is_some_and(|b| b.groups.contains(&ge));
        if in_group {
            if let Some(b) = self.world.get_mut::<Brick>(entity) {
                b.groups.retain(|&g| g != ge);
            }
            if let Some(m) = self.world.get_mut::<Members>(Entity(ge)) {
                m.0.remove(&entity.0);
            }
        } else {
            if let Some(b) = self.world.get_mut::<Brick>(entity) {
                b.groups.push(ge);
            }
            self.world
                .get_mut::<Members>(Entity(ge))
                .expect("inited_gid SHOULD init Members")
                .0
                .insert(entity.0);
        }
        Ok(())
    }

    /// # Errors
    /// Throw if input invalid.
    pub fn touch_group(&mut self, opt: &Value) -> Result<()> {
        ChairData::expect(schema_uri!("touch_group"), opt)?;
        self.touch_group_unchecked(opt);
        Ok(())
    }

    /// # Errors
    /// Throw if input invalid.
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
        let members = if gid == 0 {
            None
        } else if gid < 0 {
            // v1 用负 gid 查 group key（"-5"）不存在 → not found，同义保底。
            bail!("group {:?} not found", opt["gid"]);
        } else {
            let Some(ge) = self.res.gid_index.get(&gid_u64(gid)) else {
                bail!("group {:?} not found", opt["gid"]);
            };
            Some(
                self.world
                    .get::<Members>(Entity(*ge))
                    .map(|m| m.0.clone())
                    .unwrap_or_default(),
            )
        };
        let default_order = opt["order_desc"].as_str().is_some_and(|s| s == "default");
        let ids: Vec<u64> = if default_order {
            let it = self.res.up_by_fid.iter().copied();
            match &members {
                None => it.skip(st).take(len).collect(),
                Some(m) => it.filter(|e| m.contains(e)).skip(st).take(len).collect(),
            }
        } else {
            let index = self
                .res
                .up_index
                .get(opt["order_desc"].as_str().unwrap_or("default"))
                .ok_or_else(|| anyhow!("index not found"))?;
            let it = index
                .iter()
                .filter_map(|(_, s)| self.res.uid_index.get(s.as_str()).copied());
            match &members {
                None => it.skip(st).take(len).collect(),
                Some(m) => it.filter(|e| m.contains(e)).skip(st).take(len).collect(),
            }
        };
        let a: Vec<_> = ids
            .into_iter()
            .filter_map(|e| self.pick_of_entity(Entity(e)))
            .collect();
        Ok(json!(a))
    }

    /// v1 `group_info.iter()` 的等价物（filter_options 用，输出形状同 v1：
    /// `{filters: [{fid, name, removable}]}`，按 gid 字符串序）。
    #[must_use]
    pub fn filter_options(&self) -> Value {
        let mut a: Vec<Value> = self
            .world
            .iter::<GroupInfo>()
            .map(|(_, g)| {
                json!({
                    "fid": g.gid.to_string(),
                    "name": g.name,
                    "removable": !g.pin,
                })
            })
            .collect();
        a.sort_by(|l, r| l["fid"].as_str().cmp(&r["fid"].as_str()));
        json!({ "filters": a })
    }

    // ---------- fetch 结果写入（engine 用，v1 `modify_up_info` 等价物） ----------

    /// fetch 成功后按组件写入 + 索引/事件维护 + bucket_access（v1 `modify_up_info` 语义）。
    ///
    /// # Errors
    /// Throw if uid not tracing.
    pub fn apply_fetch(&mut self, uid: i64, info: &Value, videos: &Value) -> Result<()> {
        let uid_str = uid.to_string();
        let entity = Entity(
            *self
                .res
                .uid_index
                .get(&uid_str)
                .ok_or_else(|| anyhow!("not tracing uid"))?,
        );
        let now = now_timestamp();
        let old_brick = self
            .world
            .get::<Brick>(entity)
            .cloned()
            .ok_or_else(|| anyhow!("modifing up_info SHOULD be inited"))?;
        let old_live_entropy = self
            .world
            .get::<LivePost>(entity)
            .map(|l| l.entropy)
            .unwrap_or(-1);
        let old_video_ts = self
            .world
            .get::<VideoPost>(entity)
            .map(|v| v.latest_ts)
            .unwrap_or(0);

        // basic：uname/face/ctime 更新（ban/fid 等管理字段保留，v1 pick_basic 合并语义）
        let mut brick = old_brick.clone();
        if let Some(name) = info["name"].as_str() {
            brick.uname = name.to_string();
        }
        if let Some(face) = info["face"].as_str() {
            brick.face = face.to_string();
        }
        brick.updated_at = now;
        self.world.insert(entity, brick);

        // live：v1 `pick_live(info)` 整体覆写
        let live_pick = pick_live(info);
        let new_entropy = live_pick["entropy"].as_i64().unwrap_or(-1);
        self.world.insert(
            entity,
            LivePost {
                updated_at: now,
                is_open: live_pick["isopen"].as_bool().unwrap_or(false),
                title: live_pick["title"].as_str().unwrap_or_default().to_string(),
                url: live_pick["url"].as_str().unwrap_or_default().to_string(),
                ts: 0,
                entropy: new_entropy,
                entropy_txt: live_pick["entropy_txt"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            },
        );

        // video：全量 vlist + episodic uri
        let items: Vec<VideoItem> = videos["list"]["vlist"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| {
                Some(VideoItem {
                    bvid: v["bvid"].as_str()?.to_string(),
                    title: v["title"].as_str()?.to_string(),
                    pubdate: v["created"].as_i64()?,
                })
            })
            .collect();
        let new_video_ts = items.first().map_or(0, |i| i.pubdate);
        self.world.insert(
            entity,
            VideoPost {
                updated_at: now,
                latest_ts: new_video_ts,
                episodic: videos["episodic_button"]["uri"]
                    .as_str()
                    .map(str::to_string),
                items,
            },
        );

        // raw：内存 only
        self.world.insert(
            entity,
            RawInfo(json!({
                "videos": videos,
                "info": info,
            })),
        );

        self.update_index("video", old_video_ts, new_video_ts, &uid_str);
        self.update_index("live", old_live_entropy, new_entropy, &uid_str);
        self.update_index("ctime", old_brick.updated_at, now, &uid_str);
        self.bucket_access();
        Ok(())
    }
}

fn gid_u64(gid: i64) -> u64 {
    u64::try_from(gid).expect("users_pick gid SHOULD be non-negative")
}

fn default_bucket() -> Value {
    json!({
        "atime": to_value(Utc::now()).unwrap(),
        "min_gap": 10,
        "min_change_gap": 10,
        "gap": 30,
    })
}

#[must_use]
pub fn now_timestamp() -> i64 {
    Utc::now().timestamp()
}

// ============================== API 原始值 → pick 段（v1 engine.rs 迁入） ==============================

/// API `user.info()` → v1 `pick.basic` 合并结果（保留旧 ban/fid 等管理字段）。
pub fn pick_basic(a: &Value, b: &Value) -> Value {
    // TODO pick pendant
    let mut r = b.clone();
    let mut ap = json!({
        "id": a["mid"],
        "name": a["name"],
        "face_url": a["face"],
        "ctime": now_timestamp(),
    });
    r.as_object_mut()
        .expect("up_info SHOULD inited basic")
        .append(ap.as_object_mut().unwrap());
    r
}

/// API `user.info()` → v1 `pick.live` JSON。
pub fn pick_live(a: &Value) -> Value {
    let l = &a["live_room"];
    let w = &l["watched_show"];
    json!({
        "title": l["title"],
        "url": l["url"],
        "entropy": w["num"],
        "entropy_txt": w["text_large"],
        "isopen": w["roomStatus"].as_i64().filter(|i| *i > 0)
            .and(w["liveStatus"].as_i64())
            .filter(|i| *i > 0)
            .is_some(),
    })
}

/// API `user.latest_videos()` → v1 `pick.video` JSON（无 vlist 时 null）。
pub fn pick_video(a: &Value) -> Value {
    let v = if let Some(v) = a["list"]["vlist"].as_array().filter(|v| !v.is_empty()) {
        &v[0]
    } else {
        return Value::Null;
    };
    json!({
        "title": v["title"],
        "url": a["episodic_button"]["uri"].as_str().map(|s| format!("https:{s}")),
        "ts": v["created"],
    })
}

// ============================== VCounter ==============================

#[derive(Default, Debug)]
struct VCounter {
    last_dump_ts: Option<DateTime<Utc>>,
    push_miss_cnt: u64,
    broadcast_void_cnt: u64,
    ext: BTreeMap<String, u64>,
}

impl VCounter {
    pub fn try_log(&mut self, snap: &Snapshot) -> Option<String> {
        if self.last_dump_ts.is_none_or(|t| Utc::now() > t) {
            let r = format!("VCounter: {self:?}");
            self.last_dump_ts = Some(Utc::now() + snap.runtime_vlog_dump_gap());
            Some(r)
        } else {
            None
        }
    }
}

// ============================== store 桥接（T4 持久化接轨） ==============================
//
// 内存组件 ↔ store 信封逐字段搬运 + 启动全量加载 + patch 落盘 diff。
// 稳态零 redb 读：读路径只在 `load_snapshot`（启动）；运行期写路径仅两条——
// brick/group 直写（`persist_diff`，逐 op 一次写事务）、易变组件 stage（VolatileBuffer 批量 flush）。
// 组件字段与 store 信封的映射（M1 校准）：
// - Brick ↔ BrickV2 完全同构；GroupInfo ↔ GroupV1 同构（key = group entity id）。
// - LivePost ↔ LivePostV1：store 侧 `extra`（旧收容字段）写空、`entropy`/`entropy_txt`/`ts` 类型化。
// - VideoPost ↔ VideoPostV1：store 侧 `extra`（条目与顶层）写空、`episodic` 类型化（D7）。
// - CommentPost ↔ CommentPostV1：同上（M1 恒空）。
// - RuntimeCfg ↔ RuntimeV1.fields：v1 runtime JSON 对象文本 ↔ `im::HashMap<String, Value>`。

fn runtime_cfg_to_fields(cfg: &im::HashMap<String, Value>) -> String {
    let m: serde_json::Map<String, Value> = cfg
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    serde_json::to_string(&m).unwrap_or_else(|_| "{}".into())
}

fn runtime_cfg_from_fields(fields: &str) -> im::HashMap<String, Value> {
    serde_json::from_str::<serde_json::Map<String, Value>>(fields)
        .map(|m| m.into_iter().collect())
        .unwrap_or_default()
}

fn brick_to_store(b: &Brick) -> store::BrickV2 {
    store::BrickV2 {
        uid: b.uid.clone(),
        uname: b.uname.clone(),
        face: b.face.clone(),
        ban: b.ban,
        fid: b.fid,
        groups: b.groups.clone(),
        silent: b.silent,
        followed_at: b.followed_at,
        updated_at: b.updated_at,
    }
}

fn brick_from_store(b: store::BrickV2) -> Brick {
    Brick {
        uid: b.uid,
        uname: b.uname,
        face: b.face,
        ban: b.ban,
        fid: b.fid,
        groups: b.groups,
        silent: b.silent,
        followed_at: b.followed_at,
        updated_at: b.updated_at,
    }
}

fn live_to_store(l: &LivePost) -> store::LivePostV1 {
    store::LivePostV1 {
        updated_at: l.updated_at,
        is_open: l.is_open,
        title: l.title.clone(),
        url: l.url.clone(),
        ts: l.ts,
        extra: String::new(),
        entropy: l.entropy,
        entropy_txt: l.entropy_txt.clone(),
    }
}

fn live_from_store(l: store::LivePostV1) -> LivePost {
    LivePost {
        updated_at: l.updated_at,
        is_open: l.is_open,
        title: l.title,
        url: l.url,
        ts: l.ts,
        entropy: l.entropy,
        entropy_txt: l.entropy_txt,
    }
}

fn video_to_store(v: &VideoPost) -> store::VideoPostV1 {
    store::VideoPostV1 {
        updated_at: v.updated_at,
        latest_ts: v.latest_ts,
        items: v
            .items
            .iter()
            .map(|i| store::VideoItemV1 {
                bvid: i.bvid.clone(),
                title: i.title.clone(),
                pubdate: i.pubdate,
                extra: String::new(),
            })
            .collect(),
        extra: String::new(),
        episodic: v.episodic.clone(),
    }
}

fn video_from_store(v: store::VideoPostV1) -> VideoPost {
    VideoPost {
        updated_at: v.updated_at,
        latest_ts: v.latest_ts,
        items: v
            .items
            .into_iter()
            .map(|i| VideoItem {
                bvid: i.bvid,
                title: i.title,
                pubdate: i.pubdate,
            })
            .collect(),
        episodic: v.episodic,
    }
}

fn comment_to_store(c: &CommentPost) -> store::CommentPostV1 {
    store::CommentPostV1 {
        updated_at: c.updated_at,
        items: c
            .items
            .iter()
            .map(|i| store::CommentItemV1 {
                rpid: i.rpid,
                msg: i.msg.clone(),
                ts: i.ts,
                extra: String::new(),
            })
            .collect(),
        extra: String::new(),
    }
}

fn comment_from_store(c: store::CommentPostV1) -> CommentPost {
    CommentPost {
        updated_at: c.updated_at,
        items: c
            .items
            .into_iter()
            .map(|i| CommentItem {
                rpid: i.rpid,
                msg: i.msg,
                ts: i.ts,
            })
            .collect(),
    }
}

fn group_to_store(g: &GroupInfo) -> store::GroupV1 {
    store::GroupV1 {
        gid: g.gid,
        name: g.name.clone(),
        pin: g.pin,
    }
}

fn group_from_store(g: store::GroupV1) -> GroupInfo {
    GroupInfo {
        gid: g.gid,
        name: g.name,
        pin: g.pin,
    }
}

/// 启动全量加载：读 ec 各表 → 重建 world + 全部内存索引（uid/gid/up_by_fid/up_index/Members），
/// 并把 entity id 分配器推进到 max+1（只增不减）。失败以 Err 返回（调用方失败即退出）。
fn load_snapshot(store: &Store) -> Result<Snapshot> {
    let mut snap = Snapshot::new(); // entity 1 runtime + 内置组 2/3（init 固定重建）
    let mut max_id: u64 = ENTITY_GROUP_SPECIAL;

    // runtime（entity 1；缺行则保持空 RuntimeCfg，v1 booting 语义）
    for (e, rt) in store.list_runtimes()? {
        if e == ENTITY_RUNTIME {
            snap.world.insert(
                Entity(ENTITY_RUNTIME),
                RuntimeCfg(runtime_cfg_from_fields(&rt.fields)),
            );
        } else {
            log::warn!("ignore orphan runtime row for entity {e}");
        }
    }

    // 用户组（entity ≥4；内置 2/3 不落盘，防御性跳过异常落盘行）
    let mut group_cnt = 0usize;
    for (e, g) in store.list_groups()? {
        if e < 4 {
            log::warn!("ignore stored group row for builtin entity {e}");
            continue;
        }
        ensure_spawn(&mut snap.world, e)?;
        snap.res.gid_index.insert(g.gid, e);
        snap.world.insert(Entity(e), group_from_store(g));
        snap.world.insert(Entity(e), Members(im::OrdSet::default()));
        max_id = max_id.max(e);
        group_cnt += 1;
    }

    // up 实体 + brick 组件；易变组件/成员索引在其后补
    let mut uid_of: BTreeMap<u64, String> = BTreeMap::new(); // entity → uid
    let mut fid_order: Vec<(u64, u64)> = Vec::new(); // (fid, entity)
    let mut up_cnt = 0usize;
    for (e, b) in store.list_bricks()? {
        ensure_spawn(&mut snap.world, e)?;
        let brick = brick_from_store(b);
        fid_order.push((brick.fid, e));
        uid_of.insert(e, brick.uid.clone());
        max_id = max_id.max(e);
        up_cnt += 1;
        snap.res.uid_index.insert(brick.uid.clone(), e);
        // ctime 索引恒在（follow 即插 0，fetch 后 = updated_at；见 update_index 语义）
        snap.res
            .up_index
            .entry("ctime".into())
            .or_default()
            .insert((brick.updated_at, brick.uid.clone()));
        // 成员反索引：Brick.groups（group entity id）→ group 实体 Members
        for ge in &brick.groups {
            match snap.world.get_mut::<Members>(Entity(*ge)) {
                Some(m) => {
                    m.0.insert(e);
                }
                None => log::warn!("brick {e} groups points to unknown group entity {ge}"),
            }
        }
        snap.world.insert(Entity(e), brick);
    }
    // up_by_fid = 按 fid（关注顺序）升序；fid 相同时保持 key 升序（稳定排序）
    fid_order.sort_by_key(|&(fid, _)| fid);
    snap.res.up_by_fid = fid_order.into_iter().map(|(_, e)| e).collect();

    // 易变组件 + 排序索引重建（仅当值处于有效域才入索引，见 update_index 语义：
    // video latest_ts > 0、live entropy >= 0；空/无效 up 不出现在该排序维）
    for (e, v) in store.list_video_posts()? {
        let Some(uid) = uid_of.get(&e) else {
            log::warn!("ignore orphan video_post row for entity {e}");
            continue;
        };
        let vp = video_from_store(v);
        if vp.latest_ts > 0 {
            snap.res
                .up_index
                .entry("video".into())
                .or_default()
                .insert((vp.latest_ts, uid.clone()));
        }
        snap.world.insert(Entity(e), vp);
    }
    for (e, l) in store.list_live_posts()? {
        let Some(uid) = uid_of.get(&e) else {
            log::warn!("ignore orphan live_post row for entity {e}");
            continue;
        };
        let lp = live_from_store(l);
        if lp.entropy >= 0 {
            snap.res
                .up_index
                .entry("live".into())
                .or_default()
                .insert((lp.entropy, uid.clone()));
        }
        snap.world.insert(Entity(e), lp);
    }
    for (e, c) in store.list_comment_posts()? {
        if !uid_of.contains_key(&e) {
            log::warn!("ignore orphan comment_post row for entity {e}");
            continue;
        }
        snap.world.insert(Entity(e), comment_from_store(c));
    }

    store.ensure_next_entity_id(max_id + 1)?;
    log::info!(
        "state.redb loaded: {up_cnt} up, {group_cnt} user group(s), next entity id >= {}",
        max_id + 1
    );
    Ok(snap)
}

fn ensure_spawn(world: &mut World, e: u64) -> Result<()> {
    if !world.is_alive(Entity(e)) {
        world.spawn_at(e).ok_or_else(|| {
            anyhow!("store key {e} conflicts with alive entity (corrupt state db?)")
        })?;
    }
    Ok(())
}

// TODO(persist_diff 遗留优化方向，均以 T5 性能冒烟/对应阶段结论为准，勿在 M1 提前做；
// 决策出处：.plans/m1-ecs-core.md D8/D9/§7 指标表):
// 1. 直写粒度：brick/group 逐 op 一个写事务（put_ec 内 begin_write）。同 patch 多实体变更
//    先评估「同 patch 合并为单写事务」（需 store 增 batch API），再评估 D9 备选「专用 writer
//    线程 + ack」；触发条件：T5 冒烟 brick 直写 p99 > 100ms（§7 目标 p99 ≤ 50ms）。
// 2. diff 全扫描：每 patch 全遍历 base world 重建 BTreeMap 做删除检测（O(up 数)）。up 规模
//    大或热路径暴露后再上组件级脏追踪（dirty set）/per-entity 版本号比对，现阶段 200 up 无必要。
// 3. flush 阈值：stage 批量 flush 的 MAX_RECORDS=256 / MAX_AGE=5s 为 D8 占位值，
//    T5 冒烟定值（§7 flush 提交 p99 ≤ 200ms）。
// 4. 失败语义：persist 失败仅记日志继续（无补偿/重试），与 v1 save_disk().ok() 同款宽松度；
//    需要更强落盘保证时与 M4 停机强刷/强杀衔接一并评估。

/// hub patch 落盘 diff（`try_push` 校验通过后执行；hub 主循环同步写、单写者，D9）。
/// 直写（逐 op 一个写事务）：brick/group 变化；stage 批量 flush：video/live/comment/runtime。
/// 纯 res 变更（logs/events/commands/索引增量）不落盘（D4）；world 组件未变直接短路。
/// 失败以 Err 返回，由调用方记日志继续（宽松语义，等价 v1 save_disk().ok()）。
fn persist_diff(store: &mut Store, base: &Snapshot, next: &Snapshot) -> Result<()> {
    if base.world.ptr_eq(&next.world) {
        return Ok(());
    }

    // brick：逐实体比较，新增/修改 put、删除 remove
    let base_bricks: BTreeMap<u64, &Brick> = base
        .world
        .iter::<Brick>()
        .map(|(e, b)| (e.0, b))
        .collect();
    for (e, b) in next.world.iter::<Brick>() {
        match base_bricks.get(&e.0) {
            Some(bb) if *bb == b => continue,
            _ => store.put_brick(e.0, &brick_to_store(b))?,
        }
    }
    for e in base_bricks.keys() {
        if !next.world.contains::<Brick>(Entity(*e)) {
            store.delete_brick(*e)?;
        }
    }

    // group：仅用户组（≥4）直写；内置组由 init 固定重建，永不落盘
    let base_groups: BTreeMap<u64, &GroupInfo> = base
        .world
        .iter::<GroupInfo>()
        .filter(|(e, _)| e.0 >= 4)
        .map(|(e, g)| (e.0, g))
        .collect();
    for (e, g) in next.world.iter::<GroupInfo>() {
        if e.0 < 4 {
            continue;
        }
        match base_groups.get(&e.0) {
            Some(gg) if *gg == g => continue,
            _ => store.put_group(e.0, &group_to_store(g))?,
        }
    }
    for e in base_groups.keys() {
        if !next.world.contains::<GroupInfo>(Entity(*e)) {
            store.delete_group(*e)?;
        }
    }

    // 易变组件：组件变化才 stage（值比较；extra 收容字段由 *_to_store 写空）
    for (e, v) in next.world.iter::<VideoPost>() {
        if base.world.get::<VideoPost>(e) != Some(v) {
            store.stage_video_post(e.0, &video_to_store(v))?;
        }
    }
    for (e, l) in next.world.iter::<LivePost>() {
        if base.world.get::<LivePost>(e) != Some(l) {
            store.stage_live_post(e.0, &live_to_store(l))?;
        }
    }
    for (e, c) in next.world.iter::<CommentPost>() {
        if base.world.get::<CommentPost>(e) != Some(c) {
            store.stage_comment_post(e.0, &comment_to_store(c))?;
        }
    }
    // runtime（entity 1）：bucket/字段 JSON 变化 stage（bucket_access 每 fetch 一次）
    let base_rt = base.world.get::<RuntimeCfg>(Entity(ENTITY_RUNTIME));
    let next_rt = next.world.get::<RuntimeCfg>(Entity(ENTITY_RUNTIME));
    if base_rt != next_rt {
        if let Some(rt) = next_rt {
            store.stage_runtime(
                ENTITY_RUNTIME,
                &store::RuntimeV1 {
                    updated_at: now_timestamp(),
                    fields: runtime_cfg_to_fields(&rt.0),
                },
            )?;
        }
    }
    Ok(())
}

// ============================== hub / chair ==============================

/// 数据中枢：权威 `Snapshot` + 三通道（mpsc 提交 / watch 发布 / broadcast 事件）。
pub struct WeiYuanHui {
    updates: mpsc::Receiver<BenchUpdate>,
    updates_src: Option<mpsc::Sender<BenchUpdate>>,
    publish: watch::Sender<Snapshot>,
    publish_dst: Option<watch::Receiver<Snapshot>>,
    ev_tx: Option<broadcast::Sender<Events>>,
    ev_rx: broadcast::Receiver<Events>,
    bench: Snapshot,
    counter: VCounter,
    /// 持久化（T4 接轨）：`open(&store)` 时 Some；`close()` 强刷后 take 置 None。
    /// 默认/`From<Snapshot>` 构造为 None（纯内存 hub，测试/无盘场景）。
    store: Option<Store>,
}

impl Default for WeiYuanHui {
    fn default() -> Self {
        let (updates_src, updates) = mpsc::channel(64);
        let (ev_tx, ev_rx) = broadcast::channel(64);
        let (publish, publish_dst) = watch::channel(Snapshot::new());
        Self {
            updates,
            updates_src: Some(updates_src),
            publish,
            publish_dst: Some(publish_dst),
            ev_tx: Some(ev_tx),
            ev_rx,
            bench: Snapshot::new(),
            counter: VCounter::default(),
            store: None,
        }
    }
}

impl From<Snapshot> for WeiYuanHui {
    fn from(bench: Snapshot) -> Self {
        Self {
            bench,
            ..Default::default()
        }
    }
}

impl WeiYuanHui {
    /// 从 state.redb 全量加载重建权威快照（T4 持久化接轨，取代 v1 bench.json load）。
    /// 加载失败返回 Err——调用方（lib.rs `main_loop`）失败即退出（M1 探针升级语义，D12）。
    pub fn open(store: Store) -> Result<Self> {
        let snap = load_snapshot(&store)?;
        let mut h: Self = snap.into();
        h.store = Some(store);
        Ok(h)
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
    pub const fn bench(&self) -> &Snapshot {
        &self.bench
    }

    pub fn close(&mut self) {
        // T4（D8）停机强刷：易变缓冲 flush + store close，顺序在发布 closing 标志之前
        // （等价 v1 `save_disk` 位置）；失败记日志并继续退出（v1 save_disk().ok() 同款宽松度）。
        if let Some(store) = self.store.take() {
            let mut store = store;
            if let Err(e) = store.flush() {
                log::error!("close: store final flush failed: {e:#}");
            }
            if let Err(e) = store.close() {
                log::error!("close: store close failed: {e:#}");
            }
        }
        self.updates_src = None;
        self.publish_dst = None;
        self.ev_tx = None;
        let mut next = self.bench.clone();
        next.res.closing = true;
        self.push(next);
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
        // T4：易变缓冲每轮兜底 flush（≥MAX_RECORDS 或距上次 ≥MAX_AGE 才真写，VolatileBuffer 内节流）。
        if let Some(store) = &mut self.store {
            if let Err(e) = store.maybe_flush() {
                log::error!("run: store maybe_flush failed: {e:#}");
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
        msg.is_some_and(|msg| {
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
                self.push(self.bench.with_log(3, &msg));
            }
        }
    }

    fn push(&mut self, mut next: Snapshot) {
        if !next.res.events.is_empty() {
            let events = std::mem::take(&mut next.res.events);
            let pass: Events = events
                .into_iter()
                .filter(|ev| {
                    ev[COUNTER_TAG]
                        .as_str()
                        .map(|s| *self.counter.ext.entry(s.into()).or_default() += 1)
                        .is_none()
                })
                .collect();
            if !pass.is_empty() && self.ev_tx.as_ref().is_none_or(|tx| tx.send(pass).is_err()) {
                self.counter.broadcast_void_cnt += 1;
            }
        }
        // T4 持久化钩子：patch 落盘在 bench 替换前执行（hub 主循环同步写，D9）。
        // 失败记日志继续（宽松，v1 save_disk().ok() 语义）；store=None（内存 hub）跳过。
        if let Some(store) = &mut self.store {
            if let Err(e) = persist_diff(store, &self.bench, &next) {
                log::error!("persist patch failed (will retry on next diff): {e:#}");
            }
        }
        self.bench = next.clone();
        self.publish.send_modify(move |v| *v = next);
    }
}

/// 快照视图（chair）：读本地副本，写经 mpsc 提交。
#[derive(Clone)]
pub struct WeiYuan {
    update: Option<mpsc::Sender<BenchUpdate>>,
    fetch: watch::Receiver<Snapshot>,
    bench: Snapshot,
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
    pub fn recv(&mut self) -> Result<&Snapshot> {
        match self.fetch.has_changed() {
            Ok(true) => self.bench = self.fetch.borrow_and_update().clone(),
            Err(e) => panic!("watch chan: WeiYuanHui drop too fast: {e:#}"),
            _ => (),
        }
        self.bench
            .res
            .closing
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
        F: Fn(&Snapshot) -> Result<Snapshot>,
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
        F: Fn(&mut Snapshot) -> Result<()>,
    {
        self.update(|b| {
            let mut v = b.clone();
            f(&mut v)?;
            Ok(v)
        })
    }

    pub fn log<S: ToString + ?Sized>(&mut self, level: i32, msg: &S) {
        self.update(|b| Ok(b.with_log(level, &msg.to_string())))
            .ok();
    }

    pub fn count<S: ToString + ?Sized>(&mut self, msg: &S) {
        self.apply(|b| {
            b.res
                .events
                .push_back(json!({COUNTER_TAG: msg.to_string()}));
            Ok(())
        })
        .ok();
    }

    /// # Panics
    /// Panic on `WeiYuanHui` drop too fast.
    pub async fn until_closing(&mut self) {
        self.fetch
            .wait_for(|s| s.res.closing)
            .await
            .map_err(|e| panic!("fetch channel unexpected closed: {e}"))
            .ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ecs::World;
    use crate::store::StoreConfig;
    use std::mem;
    use std::time::Duration as Dur;
    use tempfile::tempdir;
    use tokio::time::timeout;

    fn init() {
        env_logger::builder()
            .is_test(true)
            .format_timestamp(Some(env_logger::fmt::TimestampPrecision::Micros))
            .try_init()
            .ok();
    }

    /// up 实体 id 收集（测试辅助，勿依赖迭代序语义）。
    fn up_ids(snap: &Snapshot) -> Vec<u64> {
        let mut v: Vec<u64> = snap.world.iter::<Brick>().map(|(e, _)| e.0).collect();
        v.sort_unstable();
        v
    }

    #[test]
    fn test_runtime_dump_now_default() {
        let snap = Snapshot::default();
        assert!(snap.runtime_dump_now());
    }

    #[test]
    fn test_runtime_dump_timeout_min_default() {
        let mut snap = Snapshot::default();
        assert_eq!(snap.runtime_dump_timeout_min(), 720_i64);
        assert!(snap.runtime_dump_now());
        snap.set_runtime_next_dump();
        assert!(!snap.runtime_dump_now());
    }

    #[test]
    fn test_runtime_field_set_n_get() {
        let mut snap = Snapshot::default();
        assert_eq!(
            snap.runtime_set_field("db", "dump_timeout_min", json!(42))
                .as_ref()
                .map_err(ToString::to_string),
            Ok(&())
        );
        assert_eq!(
            snap.runtime_get("db"),
            Some(&json!({"dump_timeout_min":42}))
        );
        assert_eq!(
            snap.runtime_field("db", "dump_timeout_min").ok(),
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
        assert!(!center.bench.res.closing);
        {
            let mut chair = center.new_chair();
            let mut chair_rx = chair.clone();
            assert_eq!(
                chair
                    .apply(|b| b.runtime_set_field("bucket", "min_gap", json!(23)))
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
            println!("bucket: {:?}", cur.runtime_get("bucket"));
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
        assert_eq!(center.bench.res.logs.len(), 0);
        assert!(center.run().await);
        assert_ne!(center.bench.res.logs.len(), 0);
        let mut v = center.bench.res.logs[0].clone();
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
        let mut snap = Snapshot::default();
        for i in 0..2048 {
            snap = snap.with_log(2, &format!("test log {i}"));
        }
        assert_eq!(snap.res.logs.len(), 2048);
        snap = snap.with_log(4, "will discard log");
        assert_eq!(snap.res.logs.len(), 2048);
        snap = snap.with_log(-1, "this log trigger buffer shorten");
        assert_eq!(snap.res.logs.len(), 2048 - 16);
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
        let snap = Snapshot::default();
        assert_ne!(c.try_log(&snap), None);
        assert_eq!(c.try_log(&snap), None);
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
                            b.res.events.push_back(ev.clone());
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
    fn test_snapshot_follow_toggle_users_pick() {
        let mut snap = Snapshot::new();
        assert!(snap.follow(&json!({"uid": 12345, "enable": true})).is_ok());
        assert!(snap.follow(&json!({"uid": 2233, "enable": true})).is_ok());
        assert_eq!(snap.res.uid_index.len(), 2);
        assert_eq!(
            up_ids(&snap),
            vec![4, 5],
            "up 实体从 4 起（1=runtime, 2/3=内置组）"
        );
        assert_eq!(snap.res.up_by_fid.len(), 2);

        // toggle_group：加组再移除，users_pick 走 Members 过滤
        assert!(snap.toggle_group(&json!({"uid": 12345, "gid": 5})).is_ok());
        let g5 = *snap.res.gid_index.get(&5).unwrap();
        let pick = snap
            .users_pick(
                &json!({"gid": 5, "order_desc": "default", "range_start": 0, "range_len": 10}),
            )
            .unwrap();
        assert_eq!(pick.as_array().unwrap().len(), 1);
        assert_eq!(pick[0]["basic"]["id"], json!(12345));

        // 组件三方一致：Brick.groups / Members / gid_index
        let e = *snap.res.uid_index.get("12345").unwrap();
        assert_eq!(snap.world.get::<Brick>(Entity(e)).unwrap().groups, vec![g5]);
        assert!(snap
            .world
            .get::<Members>(Entity(g5))
            .unwrap()
            .0
            .contains(&e));

        assert!(snap.toggle_group(&json!({"uid": 12345, "gid": 5})).is_ok());
        let pick = snap
            .users_pick(
                &json!({"gid": 5, "order_desc": "default", "range_start": 0, "range_len": 10}),
            )
            .unwrap();
        assert_eq!(pick.as_array().unwrap().len(), 0);
    }

    #[test]
    fn test_snapshot_touch_group_and_pick_json() {
        let mut snap = Snapshot::new();
        assert!(snap
            .touch_group(&json!({"gid": 7, "name": "g7", "pin": false}))
            .is_ok());
        let g7 = *snap.res.gid_index.get(&7).unwrap();
        let gi = snap.world.get::<GroupInfo>(Entity(g7)).unwrap();
        assert_eq!((gi.name.as_str(), gi.pin), ("g7", false));

        // filter_options：内置 0/1 + 用户组（按 gid 字符串序，removable=!pin）
        let fo = snap.filter_options();
        let fids: Vec<&str> = fo["filters"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["fid"].as_str().unwrap())
            .collect();
        assert_eq!(fids, vec!["0", "1", "7"]);

        // pending up 的 pick_json 形状与 v1 pending_up_info 一致（id 数字/name/ctime 0）
        assert!(snap.follow(&json!({"uid": 12345})).is_ok());
        let pick = snap.pick_of(12345).unwrap();
        assert_eq!(pick["basic"]["id"], json!(12345));
        assert_eq!(pick["basic"]["name"], json!("pending ..."));
        assert_eq!(pick["basic"]["ban"], json!(false));
        assert_eq!(pick["basic"]["ctime"], json!(0));
        assert!(pick.get("live").is_none());
        assert!(pick.get("video").is_none());
    }

    #[test]
    fn test_snapshot_apply_fetch() {
        let mut snap = Snapshot::new();
        assert!(snap.follow(&json!({"uid": 210_628})).is_ok());
        let info = json!({
            "mid": 210_628,
            "name": "MKiiiiii",
            "face": "https://i1.hdslb.com/bfs/face/x.jpg",
            "live_room": {
                "roomStatus": 1,
                "liveStatus": 1,
                "url": "https://live.bilibili.com/5229",
                "title": "【鑒賞會】就打一关",
                "watched_show": {
                    "num": 14,
                    "text_large": "14人看过",
                    "roomStatus": 1,
                    "liveStatus": 1,
                },
            },
        });
        let videos = json!({
            "list": { "vlist": [
                { "play": 1, "title": "四分鐘畫個機", "created": 1_695_871_800, "bvid": "BV1" },
            ]},
            "episodic_button": { "uri": "//www.bilibili.com/medialist/play/210628" },
        });
        assert!(snap.apply_fetch(210_628, &info, &videos).is_ok());
        let e = *snap.res.uid_index.get("210628").unwrap();
        // 组件 + 索引 + 事件
        let brick = snap.world.get::<Brick>(Entity(e)).unwrap();
        assert_eq!(brick.uname, "MKiiiiii");
        assert!(brick.updated_at > 0);
        let live = snap.world.get::<LivePost>(Entity(e)).unwrap();
        assert!(live.is_open);
        assert_eq!(live.entropy, 14);
        assert_eq!(
            snap.res.up_index.get("live").unwrap().get_max(),
            Some(&(14, "210628".into()))
        );
        let pick = snap.pick_of(210_628).unwrap();
        assert_eq!(pick["basic"]["ctime"], json!(brick.updated_at));
        assert_eq!(
            pick["video"]["url"],
            json!("https://www.bilibili.com/medialist/play/210628")
        );
        assert_eq!(pick["video"]["ts"], json!(1_695_871_800));
        assert_eq!(pick["live"]["entropy_txt"], json!("14人看过"));
        assert!(pick.get("raw").is_some());
        // ctime 索引从 0 挪到 updated_at
        assert_eq!(
            snap.res.up_index.get("ctime").unwrap().get_min(),
            Some(&(brick.updated_at, "210628".into()))
        );
    }

    #[test]
    fn test_snapshot_update_index() {
        let mut snap = Snapshot::default();
        assert!(snap.follow(&json!({"uid": 12345})).is_ok());
        snap.update_index("live", 9, 8, "12345");
        assert!(snap.res.up_index.get("live").is_some());
        assert_eq!(
            snap.res.up_index.get("live").unwrap().get_min(),
            Some(&(8i64, "12345".to_string()))
        );
        snap.update_index("live", 8, 117, "12345");
        assert_eq!(
            snap.res.up_index.get("live").unwrap().get_min(),
            Some(&(117i64, "12345".to_string()))
        );
        // 事件在 live/video 值变化时 push（无组件 → payload null，v1 同款）
        assert_eq!(snap.res.events.len(), 2);
    }

    #[test]
    fn test_snapshot_ptr_eq_semantics() {
        let mut snap = Snapshot::new();
        let base = snap.clone();
        assert!(snap.ptr_eq(&base), "clone 是结构共享");
        assert!(snap.follow(&json!({"uid": 12345})).is_ok());
        assert!(!snap.ptr_eq(&base), "follow 后结构必变");
        let base = snap.clone();
        assert!(snap
            .users_pick(&json!({
                "gid": 0,
                "order_desc": "default",
                "range_start": 0,
                "range_len": 10,
            }))
            .is_ok());
        assert!(snap.ptr_eq(&base), "只读查询不改变结构");
        assert_eq!(snap.world.get::<RawInfo>(Entity(999)), None);
        assert!(snap.ptr_eq(&base), "零变更 no-op 保持 ptr_eq");
    }

    #[test]
    fn test_snapshot_bucket_semantics() {
        let mut snap = Snapshot::new();
        // 线上 bucket.atime 是 epoch 数字（utils/ts schema），引擎 tick 路径（bucket_hang
        // 的 as_i64 读取）依赖该形态；default_bucket 的 RFC3339 atime 只在 access 前存在
        //（bucket_duration_to_next 的 from_value 路径由 engine test_next_deadline 覆盖）。
        snap.world
            .get_mut::<RuntimeCfg>(Entity(ENTITY_RUNTIME))
            .unwrap()
            .0
            .insert(
                "bucket".into(),
                json!({"atime": now_timestamp(), "min_gap": 10, "min_change_gap": 10, "gap": 30}),
            );
        let mut b = snap.clone();
        assert_eq!(b.bucket_or_default()["gap"], json!(30));
        b.bucket_hang();
        let g = b.bucket_or_default()["gap"].as_i64().unwrap();
        assert!(g > 30, "bucket_hang 增大 gap");
        assert!(!b.ptr_eq(&snap));
        b.bucket_double_gap();
        assert_eq!(b.bucket_or_default()["gap"], json!(g * 2));
        b.bucket_good();
        assert_eq!(
            b.bucket_or_default()["gap"].as_i64().unwrap(),
            std::cmp::max(g * 2 - 10, 10),
            "bucket_good 回落 min_change_gap，下限 min_gap"
        );
    }

    #[test]
    fn test_default_world_layout() {
        // runtime entity 1 恒在；new() 补齐内置组 2/3（gid 0/1）
        let d = Snapshot::default();
        assert!(d.world.is_alive(Entity(1)));
        assert!(!d.world.is_alive(Entity(2)));
        let n = Snapshot::new();
        assert!(n.world.is_alive(Entity(2)));
        assert!(n.world.is_alive(Entity(3)));
        assert_eq!(*n.res.gid_index.get(&0).unwrap(), 2);
        assert_eq!(*n.res.gid_index.get(&1).unwrap(), 3);
        // 空 world 基本操作 sanity（ecs 自身另有单测，这里只防误用）
        let mut w = World::default();
        let e = w.spawn();
        w.insert(e, Brick::default());
        assert!(w.contains::<Brick>(e));
    }

    // TODO test modify_up_info
    // 1. expect events
    // 2. index

    // ---------- T4 持久化接轨 ----------

    /// reopen 前后快照等价断言：实体/组件/索引/组成员/顺序逐项一致；
    /// `RawInfo` 内存 only，reopen 后必须不存在（D13：raw 不再落盘）。
    fn assert_reloaded_eq(orig: &Snapshot, rel: &Snapshot) {
        assert_eq!(orig.res.uid_index.len(), rel.res.uid_index.len());
        for (uid, e) in &orig.res.uid_index {
            assert_eq!(rel.res.uid_index.get(uid), Some(e), "uid_index {uid}");
            let e = Entity(*e);
            assert_eq!(
                rel.world.get::<Brick>(e),
                orig.world.get::<Brick>(e),
                "Brick {uid}"
            );
            assert_eq!(
                rel.world.get::<LivePost>(e),
                orig.world.get::<LivePost>(e),
                "LivePost {uid}"
            );
            assert_eq!(
                rel.world.get::<VideoPost>(e),
                orig.world.get::<VideoPost>(e),
                "VideoPost {uid}"
            );
            assert!(rel.world.get::<RawInfo>(e).is_none(), "raw 不落盘");
        }
        assert_eq!(rel.res.up_by_fid, orig.res.up_by_fid, "up_by_fid 关注顺序");
        for typ in ["ctime", "video", "live"] {
            let collect = |s: &Snapshot| {
                s.res
                    .up_index
                    .get(typ)
                    .map(|os| os.iter().cloned().collect::<Vec<_>>())
            };
            assert_eq!(collect(rel), collect(orig), "up_index[{typ}]");
        }
        assert_eq!(orig.res.gid_index.len(), rel.res.gid_index.len());
        for (gid, ge) in &orig.res.gid_index {
            assert_eq!(rel.res.gid_index.get(gid), Some(ge), "gid_index {gid}");
            let ge = Entity(*ge);
            assert_eq!(
                rel.world.get::<GroupInfo>(ge),
                orig.world.get::<GroupInfo>(ge),
                "GroupInfo {gid}"
            );
            assert_eq!(
                rel.world.get::<Members>(ge),
                orig.world.get::<Members>(ge),
                "Members {gid}"
            );
        }
        assert_eq!(
            rel.world.get::<RuntimeCfg>(Entity(ENTITY_RUNTIME)),
            orig.world.get::<RuntimeCfg>(Entity(ENTITY_RUNTIME)),
            "RuntimeCfg"
        );
    }

    async fn apply_then_run(
        center: &mut WeiYuanHui,
        chair: &mut WeiYuan,
        f: impl Fn(&mut Snapshot) -> Result<()>,
    ) {
        chair.apply(f).unwrap();
        assert!(center.run().await, "hub run SHOULD process one patch");
    }

    #[tokio::test]
    async fn test_store_roundtrip_persist_reopen() {
        init();
        let dir = tempdir().unwrap();
        let cfg = StoreConfig::new(dir.path().join("state.redb"));
        let info = json!({
            "mid": 12345,
            "name": "MKiiiiii",
            "face": "https://i1.hdslb.com/bfs/face/x.jpg",
            "live_room": {
                "roomStatus": 1,
                "liveStatus": 1,
                "url": "https://live.bilibili.com/5229",
                "title": "【鑒賞會】就打一关",
                "watched_show": { "num": 14, "text_large": "14人看过" },
            },
        });
        let videos = json!({
            "list": { "vlist": [
                { "play": 1, "title": "四分鐘畫個機", "created": 1_695_871_800, "bvid": "BV1" },
            ]},
            "episodic_button": { "uri": "//www.bilibili.com/medialist/play/12345" },
        });
        // ---- 第一代：hub 经 chair ops 驱动 store 写入 ----
        let mut center =
            WeiYuanHui::open(Store::open_or_create(&cfg).unwrap()).unwrap();
        let mut chair = center.new_chair();
        apply_then_run(&mut center, &mut chair, |b| {
            b.follow(&json!({"uid": 12345, "enable": true}))
        })
        .await;
        apply_then_run(&mut center, &mut chair, |b| {
            b.follow(&json!({"uid": 2233, "enable": true}))
        })
        .await;
        // toggle 自动建 placeholder 组 → touch 改名（组直写）
        apply_then_run(&mut center, &mut chair, |b| {
            b.toggle_group(&json!({"uid": 12345, "gid": 5}))
        })
        .await;
        apply_then_run(&mut center, &mut chair, |b| {
            b.touch_group(&json!({"gid": 5, "name": "组5", "pin": false}))
        })
        .await;
        // fetch：brick/live/video 组件 + bucket（易变 stage）
        apply_then_run(&mut center, &mut chair, |b| {
            b.apply_fetch(12345, &info, &videos)
        })
        .await;
        // 取关 2233（enable=false → ban=true，实体/索引保留）
        apply_then_run(&mut center, &mut chair, |b| {
            b.follow(&json!({"uid": 2233, "enable": false}))
        })
        .await;
        let orig = center.bench().clone();
        assert_eq!(orig.res.uid_index.len(), 2);
        center.close();

        // ---- 第二代：reopen 全量加载，世界/索引/顺序完全一致 ----
        let mut center2 =
            WeiYuanHui::open(Store::open_or_create(&cfg).unwrap()).unwrap();
        assert_reloaded_eq(&orig, center2.bench());
        // 行为级抽查：pending/fetch/ban 均正确恢复
        let s = center2.bench();
        assert_eq!(s.pick_of(12345).unwrap()["basic"]["name"], json!("MKiiiiii"));
        assert_eq!(
            s.pick_of(12345).unwrap()["video"]["url"],
            json!("https://www.bilibili.com/medialist/play/12345")
        );
        assert!(s.pick_of(12345).unwrap().get("raw").is_none(), "raw 重启丢失（D13）");
        assert_eq!(s.pick_of(2233).unwrap()["basic"]["ban"], json!(true));
        assert_eq!(
            s.filter_options()["filters"].as_array().unwrap().len(),
            3,
            "内置 0/1 + 用户组 5"
        );
        // 实体号推进：新 follow 分配原 max+1（store ensure_next_entity_id 已回填；
        // up 实体 4/5 + 组实体 6 → 777 取 7）
        let mut chair2 = center2.new_chair();
        apply_then_run(&mut center2, &mut chair2, |b| {
            b.follow(&json!({"uid": 777}))
        })
        .await;
        assert_eq!(up_ids(center2.bench()), vec![4, 5, 7]);
        center2.close();
    }
}

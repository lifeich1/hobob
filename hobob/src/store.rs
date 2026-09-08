//! v2 持久化地基：redb + bincode（M0）。
//!
//! 设计约束（见 `.plans/m0-redb-bincode-storage.md`）：
//! - 全同步 API，单写者假设；redb 文件锁保证不会双开同一文件。
//! - 全库统一 bincode 根函数（fixint + 小端）编解码，见 `encode_bincode`。
//! - 业务表 value 为 `VersionedRecord` 信封；每表独立版本链 + 迁移钩子；
//!   旧版本结构体永久保留在 `legacy` 子模块。
//! - `ec:brick`/`ec:group` 直写；易变表（video/live/comment/runtime）走 `VolatileBuffer` 批量 flush。
//! - entity id：`0` 非法、`1` 预留给全局 runtime 实体（M1 确认），分配从 `2` 开始。

use anyhow::{anyhow, bail, Context, Result};
use redb::{
    Database, ReadTransaction, ReadableTable, ReadableTableMetadata, TableDefinition,
    TableHandle, WriteTransaction,
};
use serde::{Deserialize, Serialize};
use std::cell::Cell;
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::time::{Duration, Instant};

// ============================== 编解码 ==============================

/// 全库统一编解码入口。
///
/// 使用 bincode 根函数 `serialize`/`deserialize`：其默认配置为 **fixint + 小端**。
/// ⚠️ 禁止改走 `bincode::DefaultOptions`（其默认是 varint）或
/// `with_varint_encoding` 等配置，否则磁盘上的旧数据不可读。
fn encode_bincode<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>> {
    bincode::serialize(value).map_err(|e| anyhow!("bincode encode failed: {e}"))
}

fn decode_bincode<'a, T: Deserialize<'a>>(bytes: &'a [u8]) -> Result<T> {
    bincode::deserialize(bytes).map_err(|e| anyhow!("bincode decode failed: {e}"))
}

/// 业务表记录的磁盘信封。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VersionedRecord {
    pub version: u32,
    pub payload: Vec<u8>,
}

fn encode_record(version: u32, payload: Vec<u8>) -> Result<Vec<u8>> {
    encode_bincode(&VersionedRecord { version, payload })
}

/// 迁移钩子：旧版本 payload → 新版本 payload。
pub type MigrationFn = fn(&[u8]) -> anyhow::Result<Vec<u8>>;

/// 每表独立版本链。`migrations` 为 `(from_version, hook)`，按 from 升序。
#[derive(Clone, Copy)]
pub struct TableSchema {
    pub current_version: u32,
    pub migrations: &'static [(u32, MigrationFn)],
}

impl TableSchema {
    /// M0 所有表均为 v1、无迁移链。
    pub const fn v1() -> Self {
        Self {
            current_version: 1,
            migrations: &[],
        }
    }
}

/// 逐级执行迁移链，返回 current_version 的 payload。
fn migrate_payload(schema: TableSchema, mut version: u32, mut payload: Vec<u8>) -> Result<Vec<u8>> {
    while version < schema.current_version {
        let hook = schema
            .migrations
            .iter()
            .find(|(from, _)| *from == version)
            .ok_or_else(|| anyhow!("missing migration hook: from version {version}"))?
            .1;
        payload = hook(&payload).with_context(|| format!("migration {version} -> {} failed", version + 1))?;
        version += 1;
    }
    Ok(payload)
}

/// 解信封；版本高于当前直接报错（数据文件比二进制新）；低于当前则迁移（不写回）。
fn decode_envelope_to_current(schema: TableSchema, raw: &[u8], what: &str) -> Result<Vec<u8>> {
    let rec: VersionedRecord =
        decode_bincode(raw).with_context(|| format!("decode record envelope {what}"))?;
    if rec.version > schema.current_version {
        bail!(
            "{what}: record version {} > current {} (数据文件比二进制新，拒绝读取)",
            rec.version,
            schema.current_version
        );
    }
    if rec.version == schema.current_version {
        return Ok(rec.payload);
    }
    migrate_payload(schema, rec.version, rec.payload).with_context(|| format!("migrate {what}"))
}

// ============================== 表定义 ==============================

const META: TableDefinition<&str, u64> = TableDefinition::new("meta");
const SYSTEMS: TableDefinition<&str, &[u8]> = TableDefinition::new("systems");
const EC_BRICK: TableDefinition<u64, &[u8]> = TableDefinition::new("ec:brick");
const EC_VIDEO_POST: TableDefinition<u64, &[u8]> = TableDefinition::new("ec:video_post");
const EC_LIVE_POST: TableDefinition<u64, &[u8]> = TableDefinition::new("ec:live_post");
const EC_COMMENT_POST: TableDefinition<u64, &[u8]> = TableDefinition::new("ec:comment_post");
const EC_RUNTIME: TableDefinition<u64, &[u8]> = TableDefinition::new("ec:runtime");
const EC_GROUP: TableDefinition<u64, &[u8]> = TableDefinition::new("ec:group");
const KV_LOG: TableDefinition<u64, &[u8]> = TableDefinition::new("kv:log");

/// 文件格式魔数：ASCII `"HOBB"`。
pub const FORMAT_MAGIC: u64 = 0x484F4242;
/// 布局级版本：表集合/键编码变化才 bump（与每表记录版本是两个维度）。
/// M0 = 1；M1 = 2（新增 `ec:group` 表，升级钩子见 `upgrade_layout_1_to_2`）；
/// M2 = 3（新增 `kv:log` 表，升级钩子见 `upgrade_layout_2_to_3`）。
pub const SCHEMA_VERSION: u64 = 3;
const META_FORMAT_MAGIC: &str = "format_magic";
const META_SCHEMA_VERSION: &str = "schema_version";
const META_NEXT_ENTITY_ID: &str = "next_entity_id";
/// entity id：0 非法、1 预留给全局 runtime 实体。
const INITIAL_ENTITY_ID: u64 = 2;

/// 业务表标识（`meta` 表不带版本信封，不在此列）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TableId {
    Systems,
    Brick,
    VideoPost,
    LivePost,
    CommentPost,
    Runtime,
    /// M1 新增（布局 2）：分组资料表（直写）。
    Group,
}

impl TableId {
    /// 每表独立版本链。Brick M1 升 V2（含 V1→V2 迁移钩子）；其余仍 v1。
    pub const fn schema(self) -> TableSchema {
        match self {
            TableId::Brick => TableSchema {
                current_version: 2,
                migrations: &[(1, legacy::brick_v1_to_v2 as MigrationFn)],
            },
            _ => TableSchema::v1(),
        }
    }

    const fn ec_def(self) -> Option<TableDefinition<'static, u64, &'static [u8]>> {
        match self {
            TableId::Brick => Some(EC_BRICK),
            TableId::VideoPost => Some(EC_VIDEO_POST),
            TableId::LivePost => Some(EC_LIVE_POST),
            TableId::CommentPost => Some(EC_COMMENT_POST),
            TableId::Runtime => Some(EC_RUNTIME),
            TableId::Group => Some(EC_GROUP),
            TableId::Systems => None,
        }
    }
}

// ============================== 组件类型（磁盘信封） ==============================

/// up 基础资料 + 管理状态（M1 起；与 `db::Brick` 组件同构，T4 `open` 桥接逐字段搬运）。
/// V2 相对 V1：`groups: Vec<String>` → `Vec<u64>`（group entity id，D7）、新增 `ban`/`fid`、
/// 删除 `sign`（db 组件无此字段，v1 JSON 亦不输出）。
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BrickV2 {
    /// bilibili uid（up 实体必填）
    pub uid: String,
    pub uname: String,
    pub face: String,
    /// v1 `pick.basic.ban`：取关保留实体（enable=false → ban=true）。
    pub ban: bool,
    /// v1 `up_by_fid` 序号（关注顺序，重启后按它重建 `up_by_fid`）。
    pub fid: u64,
    /// 所属分组（group **entity id**；成员关系权威，落盘）。
    pub groups: Vec<u64>,
    /// 静默（M1 恒 false，语义留给 M3 动态 system）。
    pub silent: bool,
    /// unix 秒
    pub followed_at: i64,
    /// 最近 fetch 成功时间（v1 `pick.basic.ctime`，重建 ctime 索引用）。
    pub updated_at: i64,
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VideoPostV1 {
    pub updated_at: i64,
    pub latest_ts: i64,
    pub items: Vec<VideoItemV1>,
    /// 保底字段：v1 JSON 里未类型化的部分先塞这里。
    /// JSON 文本（bincode 不支持 `serde_json::Value` 反序列化，按字符串收容；M1 校准语义）。
    pub extra: String,
    /// API `episodic_button.uri`（`"//www.bilibili.com/..."` 无 `https:` 前缀）；
    /// `db::VideoPost.episodic` 镜像（D7 增量字段，不 bump）。
    pub episodic: Option<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VideoItemV1 {
    pub bvid: String,
    pub title: String,
    pub pubdate: i64,
    /// JSON 文本（同上）。
    pub extra: String,
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LivePostV1 {
    pub updated_at: i64,
    pub is_open: bool,
    pub title: String,
    pub url: String,
    pub ts: i64,
    /// JSON 文本（同上）。
    pub extra: String,
    /// 观看人数（v1 `pick.live.entropy`，live 排序索引值；无观看 -1）；
    /// `db::LivePost.entropy/entropy_txt` 镜像（D7 增量字段，不 bump）。
    pub entropy: i64,
    pub entropy_txt: String,
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CommentPostV1 {
    pub updated_at: i64,
    pub items: Vec<CommentItemV1>,
    /// JSON 文本（同上）。
    pub extra: String,
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CommentItemV1 {
    pub rpid: u64,
    pub msg: String,
    pub ts: i64,
    /// JSON 文本（同上）。
    pub extra: String,
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RuntimeV1 {
    pub updated_at: i64,
    /// v1 runtime 原样收容（JSON 对象文本；bincode 限制同上，M1 校准语义）。
    pub fields: String,
}

/// 分组资料（M1 新增 `ec:group` 表；与 `db::GroupInfo` 组件同构）。
/// key = group **entity id**（D6：≠ API gid，映射走 `gid_index` 资源）。
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GroupV1 {
    /// API 可见 gid（v1 `group_info` key：0=全部、1=特殊关注、其余客户端指定）。
    pub gid: u64,
    pub name: String,
    /// v1 `removable = !pin`；内置组（gid 0/1）pin=true。
    pub pin: bool,
}

/// KV 日志记录（M2 新增 `kv:log` 表；与 log4rs appender 同构）。
/// key = 全局单调日志序号 seq（u64，0 起，跨重启续号）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LogRecordV1 {
    /// Unix 毫秒（append 时取 Utc::now；未使用 chrono 以保持 store 无 chrono 依赖）。
    pub ts_ms: i64,
    /// 1=ERROR 2=WARN 3=INFO 4=DEBUG 5=TRACE（log4rs 直接映射）。
    pub level: u8,
    /// 模块路径（log4rs `record.target()`）；业务镜像 = `"op"`。
    pub target: String,
    pub msg: String,
    /// `"file:line"`（可选，业务镜像恒 None）。
    pub loc: Option<String>,
    /// 业务上下文 JSON 文本（本期恒空串；M3/M5 填充点）。
    pub ctx: String,
}

impl Default for LogRecordV1 {
    fn default() -> Self {
        Self {
            ts_ms: 0,
            level: 3, // INFO
            target: String::new(),
            msg: String::new(),
            loc: None,
            ctx: String::new(),
        }
    }
}

/// KV 日志查询参数（供 M5 UI 日志页；本期不接 UI/HTTP，验收以单测为准）。
/// 默认：limit=200、最新在前（seq 降序）。
#[derive(Debug, Clone)]
pub struct LogQuery<'a> {
    pub limit: usize,
    pub before_seq: Option<u64>,
    pub min_level: Option<u8>,
    pub target_prefix: Option<&'a str>,
    pub since_ms: Option<i64>,
    pub until_ms: Option<i64>,
}

impl<'a> Default for LogQuery<'a> {
    fn default() -> Self {
        Self {
            limit: 200,
            before_seq: None,
            min_level: None,
            target_prefix: None,
            since_ms: None,
            until_ms: None,
        }
    }
}

/// KV 日志统计（供 `log_stats()`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogStats {
    /// 当前 `kv:log` 表总条数。
    pub total: u64,
    /// 通道满丢弃 + 写失败累计（logkv 模块写入，store 层合并读出）。
    pub dropped: u64,
}

/// KV 日志丢弃计数（通道满 + 写失败；logkv 模块写入，store 层读取）。
pub static LOG_DROPPED: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SystemSpecV1 {
    pub name: String,
    /// 回调源码；M0 按不透明字符串存取，语法与执行语义 M3 定
    pub lua: String,
    /// 触发条件；同上
    pub condition: String,
}

/// 旧版结构体保留区：schema 变更后旧类型移入这里，禁止直接删字段后不迁移。
/// M1 起收留 v1 的 brick 信封（V2 引入，含 V1→V2 迁移钩子）；旧类型定义不可改动，
/// 磁盘上的旧字节要靠它反序列化。
pub mod legacy {
    use super::{decode_bincode, encode_bincode, BrickV2, Deserialize, Result, Serialize};

    /// v1 的 brick 信封（M0 布局）：分组是 `Vec<String>`、无 `ban`/`fid`、含 `sign`。
    #[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(default)]
    pub struct BrickV1 {
        pub uid: String,
        pub uname: String,
        pub face: String,
        pub sign: String,
        /// 分组**名**（v1）；M1 组实体化后废弃。
        pub groups: Vec<String>,
        pub silent: bool,
        /// unix 秒
        pub followed_at: i64,
        pub updated_at: i64,
    }

    /// V1 → V2：`groups` 置空（v1 分组名无法映射到 group entity id，M1 起组关系从空重建）、
    /// `ban=false`、`fid=0`（取关/关注序语义重来）、`sign` 丢弃（`db::Brick` 无此字段）。
    pub fn brick_v1_to_v2(old: &[u8]) -> Result<Vec<u8>> {
        let v1: BrickV1 = decode_bincode(old)?;
        let v2 = BrickV2 {
            uid: v1.uid,
            uname: v1.uname,
            face: v1.face,
            ban: false,
            fid: 0,
            groups: Vec::new(),
            silent: v1.silent,
            followed_at: v1.followed_at,
            updated_at: v1.updated_at,
        };
        encode_bincode(&v2)
    }
}

// ============================== meta ==============================

/// 新库初始化：写魔数/布局版本/entity id 分配器。
fn init_meta(db: &Database) -> Result<()> {
    let txn = db.begin_write().context("begin write for meta init")?;
    {
        let mut meta = txn
            .open_table(META)
            .map_err(|e| anyhow!("open meta table: {e}"))?;
        meta.insert(META_FORMAT_MAGIC, FORMAT_MAGIC)
            .map_err(|e| anyhow!("insert format_magic: {e}"))?;
        meta.insert(META_SCHEMA_VERSION, SCHEMA_VERSION)
            .map_err(|e| anyhow!("insert schema_version: {e}"))?;
        meta.insert(META_NEXT_ENTITY_ID, INITIAL_ENTITY_ID)
            .map_err(|e| anyhow!("insert next_entity_id: {e}"))?;
    }
    txn.commit().context("commit meta init")?;
    Ok(())
}

/// 已存在的库：只读校验魔数/版本，通过后执行布局升级（如有）并补缺的 meta key。
/// 校验失败不产生任何写入，保证「错文件不被写坏」。
fn validate_and_repair_meta(db: &Database, path: &std::path::Path) -> Result<()> {
    let read = db.begin_read().context("begin read for meta validation")?;
    let meta = match read.open_table(META) {
        Err(redb::TableError::TableDoesNotExist(_)) => {
            bail!("{path:?} is not a hobob state db (meta table missing)")
        }
        Err(e) => return Err(e).with_context(|| format!("open meta table of {path:?}")),
        Ok(t) => t,
    };
    match meta.get(META_FORMAT_MAGIC).map_err(|e| anyhow!("read format_magic: {e}"))? {
        None => bail!("{path:?} is not a hobob state db (format_magic missing)"),
        Some(g) if g.value() != FORMAT_MAGIC => bail!(
            "{path:?} format magic mismatch: got 0x{:X}, expected 0x{:X}",
            g.value(),
            FORMAT_MAGIC
        ),
        Some(_) => {}
    }
    // 布局版本：高于当前拒绝；低于当前由下方升级链逐级补齐。
    // magic 已校验通过，schema_version 理论上与 magic 同事务写入、必然存在；
    // 缺失视作 1（M0 最早布局，只差 1→2 一步）而非 0（0 无历史钩子）。
    let cur = match meta
        .get(META_SCHEMA_VERSION)
        .map_err(|e| anyhow!("read schema_version: {e}"))?
    {
        None => 1,
        Some(g) if g.value() > SCHEMA_VERSION => {
            bail!(
                "{path:?} layout version {} > binary {} (数据文件比二进制新，拒绝打开)",
                g.value(),
                SCHEMA_VERSION
            );
        }
        Some(g) => g.value(),
    };
    drop(meta);
    drop(read);

    let txn = db.begin_write().context("begin write for meta repair")?;
    {
        let mut cur = cur;
        while cur < SCHEMA_VERSION {
            match cur {
                1 => upgrade_layout_1_to_2(&txn)?,
                2 => upgrade_layout_2_to_3(&txn)?,
                _ => bail!("missing layout upgrade hook from {cur}"),
            }
            cur += 1;
        }
        let mut meta = txn
            .open_table(META)
            .map_err(|e| anyhow!("open meta table: {e}"))?;
        meta.insert(META_SCHEMA_VERSION, SCHEMA_VERSION)
            .map_err(|e| anyhow!("insert schema_version: {e}"))?;
        if meta.get(META_NEXT_ENTITY_ID).map_err(|e| anyhow!("read next_entity_id: {e}"))?.is_none() {
            meta.insert(META_NEXT_ENTITY_ID, INITIAL_ENTITY_ID)
                .map_err(|e| anyhow!("insert next_entity_id: {e}"))?;
        }
    }
    txn.commit().context("commit meta repair")?;
    Ok(())
}

/// 布局 1 → 2：新增 `ec:group` 表。redb 写事务 `open_table` 即建表（空表随 commit 落盘）。
/// 幂等：已升到 2 的库不会走到这里；旧 v1 库无业务数据（M0 仅探针建库），无数据搬迁。
/// 未来布局变化沿用该模式：while 链内按 from 版本注册钩子。
fn upgrade_layout_1_to_2(txn: &WriteTransaction) -> Result<()> {
    txn.open_table(EC_GROUP)
        .map_err(|e| anyhow!("upgrade layout 1 -> 2: open ec:group table: {e}"))?;
    Ok(())
}

/// 布局 2 → 3：新增 `kv:log` 表。幂等：已升到 3 的库不会走到这里。
fn upgrade_layout_2_to_3(txn: &WriteTransaction) -> Result<()> {
    txn.open_table(KV_LOG)
        .map_err(|e| anyhow!("upgrade layout 2 -> 3: open kv:log table: {e}"))?;
    Ok(())
}

// ============================== 配置 ==============================

/// store 路径配置。解析优先级：CLI `--state` > env `HOBOB_STATE`（由 clap 完成）> 默认路径。
#[derive(Debug, Clone, PartialEq)]
pub struct StoreConfig {
    pub path: PathBuf,
}

impl StoreConfig {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// 纯函数，便于单测（不直接读进程环境）。
    /// `home` 为 `None` 时回退 `default_path()`（与 v1 `vpath!` 一致，直接 panic）。
    pub fn resolve(cli: Option<PathBuf>, home: Option<PathBuf>) -> PathBuf {
        cli.unwrap_or_else(|| {
            home.map(|h| h.join(".hobob").join("state.redb"))
                .unwrap_or_else(Self::default_path)
        })
    }

    /// `$HOME/.hobob/state.redb`。
    #[allow(deprecated)]
    pub fn default_path() -> PathBuf {
        let home = std::env::home_dir().expect("not found home dir");
        home.join(".hobob").join("state.redb")
    }
}

// ============================== VolatileBuffer ==============================

/// 易变表批量 flush 缓冲。阈值/间隔为占位参数（M1 性能冒烟后定）。
pub struct VolatileBuffer {
    pending: BTreeMap<(TableId, u64), Vec<u8>>,
    last_flush: Instant,
    max_records: usize,
    max_age: Duration,
}

/// 占位：待写记录数阈值（M1 定）。
pub const MAX_RECORDS: usize = 256;
/// 占位：距上次 flush 间隔阈值（M1 定）。
pub const MAX_AGE: Duration = Duration::from_secs(5);

impl VolatileBuffer {
    pub fn new() -> Self {
        Self::with_params(MAX_RECORDS, MAX_AGE)
    }

    /// 测试/调参入口；M1 性能冒烟后定值。
    pub fn with_params(max_records: usize, max_age: Duration) -> Self {
        Self {
            pending: BTreeMap::new(),
            last_flush: Instant::now(),
            max_records,
            max_age,
        }
    }

    fn stage(&mut self, table: TableId, key: u64, value: Vec<u8>) {
        self.pending.insert((table, key), value);
    }

    fn pending_len(&self) -> usize {
        self.pending.len()
    }

    fn should_flush(&self) -> bool {
        self.pending_len() >= self.max_records
            || (!self.pending.is_empty() && self.last_flush.elapsed() >= self.max_age)
    }

    /// 触发条件满足则 flush；不满足不动作。
    pub fn maybe_flush(&mut self, db: &Database) -> Result<usize> {
        if self.should_flush() {
            self.flush(db)
        } else {
            Ok(0)
        }
    }

    /// 显式 flush；写盘条数。失败时 pending 保留（事务未提交即回滚）。
    pub fn flush(&mut self, db: &Database) -> Result<usize> {
        if self.pending.is_empty() {
            return Ok(0);
        }
        let txn = db.begin_write().context("begin write for buffer flush")?;
        {
            for ((tid, key), value) in &self.pending {
                let def = tid
                    .ec_def()
                    .ok_or_else(|| anyhow!("{tid:?} is not an ec table, cannot flush"))?;
                let mut table = txn
                    .open_table(def)
                    .map_err(|e| anyhow!("open {}: {e}", def.name()))?;
                table
                    .insert(*key, value.as_slice())
                    .map_err(|e| anyhow!("flush {} key {key}: {e}", def.name()))?;
            }
        }
        txn.commit().context("commit buffer flush")?;
        let n = self.pending.len();
        self.pending.clear();
        self.last_flush = Instant::now();
        Ok(n)
    }
}

impl Default for VolatileBuffer {
    fn default() -> Self {
        Self::new()
    }
}

// ============================== Store ==============================

pub struct Store {
    db: Database,
    buffer: VolatileBuffer,
    /// 冒烟/观测埋点（T5 §7）：直写与批量 flush 的 commit 计数，进程内单调累加。
    /// 写方法多为 `&self`，计数用 `Cell`（Store 单写者持有，hub 独占线程）。
    pub stats: StoreStats,
}

/// 写路径 commit 统计（`fsync` 频率指标采集用，M1 T5 / M6 设备冒烟）。
#[derive(Debug, Default, Clone)]
pub struct StoreStats {
    /// 直写（brick/group 等 put/remove）事务 commit 次数。
    pub direct_commits: Cell<u64>,
    /// 易变批量 flush 事务 commit 次数（仅阈值/间隔/显式触发时 +1）。
    pub volatile_commits: Cell<u64>,
}

impl StoreStats {
    /// (直写 commits, 批量 flush commits)。
    pub fn get(&self) -> (u64, u64) {
        (self.direct_commits.get(), self.volatile_commits.get())
    }

    fn count_direct(&self, n: u64) {
        self.direct_commits.set(self.direct_commits.get() + n);
    }

    fn count_volatile(&self, n: u64) {
        self.volatile_commits.set(self.volatile_commits.get() + n);
    }
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store").finish_non_exhaustive()
    }
}

impl Store {
    /// 存在则 open（校验魔数、补缺 meta），否则 create + 初始化 meta。
    pub fn open_or_create(cfg: &StoreConfig) -> Result<Self> {
        let path = &cfg.path;
        if path.exists() {
            let db = Database::open(path)
                .with_context(|| format!("failed to open state db {path:?} (not a redb file?)"))?;
            validate_and_repair_meta(&db, path)?;
            Ok(Self {
                db,
                buffer: VolatileBuffer::new(),
                stats: StoreStats::default(),
            })
        } else {
            if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("failed create_dir_all {}", parent.display()))?;
            }
            let db = Database::create(path)
                .with_context(|| format!("failed to create state db {path:?}"))?;
            init_meta(&db).with_context(|| format!("init meta of {path:?}"))?;
            Ok(Self {
                db,
                buffer: VolatileBuffer::new(),
                stats: StoreStats::default(),
            })
        }
    }

    /// 启动探针：open_or_create + 完整性检查 + close。
    pub fn probe(cfg: &StoreConfig) -> Result<PathBuf> {
        probe(cfg)
    }

    /// 强刷易变表缓冲后关闭。
    pub fn close(mut self) -> Result<()> {
        self.buffer.flush(&self.db).context("final flush on close")?;
        Ok(())
    }

    /// 分配 entity id（0 非法、1 预留，从 2 起）；同事务 +1。
    pub fn alloc_entity_id(&self) -> Result<u64> {
        let txn = self.db.begin_write().context("begin write for alloc_entity_id")?;
        let id = {
            let mut meta = txn
                .open_table(META)
                .map_err(|e| anyhow!("open meta table: {e}"))?;
            let cur = meta
                .get(META_NEXT_ENTITY_ID)
                .map_err(|e| anyhow!("read next_entity_id: {e}"))?
                .map(|g| g.value())
                .ok_or_else(|| anyhow!("meta.next_entity_id missing (corrupt state db)"))?;
            if cur == 0 {
                bail!("meta.next_entity_id == 0 (corrupt state db)");
            }
            meta.insert(META_NEXT_ENTITY_ID, cur + 1)
                .map_err(|e| anyhow!("bump next_entity_id: {e}"))?;
            cur
        };
        txn.commit().context("commit alloc_entity_id")?;
        Ok(id)
    }

    /// M1 全量加载后回填 max+1；只增不减。
    pub fn ensure_next_entity_id(&self, next: u64) -> Result<()> {
        if next == 0 {
            bail!("next entity id must not be 0");
        }
        let txn = self.db.begin_write().context("begin write for ensure_next_entity_id")?;
        {
            let mut meta = txn
                .open_table(META)
                .map_err(|e| anyhow!("open meta table: {e}"))?;
            let cur = meta
                .get(META_NEXT_ENTITY_ID)
                .map_err(|e| anyhow!("read next_entity_id: {e}"))?
                .map(|g| g.value())
                .ok_or_else(|| anyhow!("meta.next_entity_id missing (corrupt state db)"))?;
            if next > cur {
                meta.insert(META_NEXT_ENTITY_ID, next)
                    .map_err(|e| anyhow!("write next_entity_id: {e}"))?;
            }
        }
        txn.commit().context("commit ensure_next_entity_id")?;
        Ok(())
    }

    // ---- systems（直写，低频） ----

    pub fn put_system(&self, spec: &SystemSpecV1) -> Result<()> {
        if spec.name.is_empty() {
            bail!("system name must not be empty");
        }
        let payload = encode_bincode(spec)?;
        let raw = encode_record(TableId::Systems.schema().current_version, payload)?;
        let txn = self.db.begin_write().context("begin write for put_system")?;
        {
            let mut table = txn
                .open_table(SYSTEMS)
                .map_err(|e| anyhow!("open systems table: {e}"))?;
            table
                .insert(spec.name.as_str(), raw.as_slice())
                .map_err(|e| anyhow!("put system {:?}: {e}", spec.name))?;
        }
        txn.commit().context("commit put_system")?;
        Ok(())
    }

    pub fn get_system(&self, name: &str) -> Result<Option<SystemSpecV1>> {
        if name.is_empty() {
            bail!("system name must not be empty");
        }
        match self.get_system_payload(name)? {
            None => Ok(None),
            Some(payload) => {
                let spec: SystemSpecV1 = decode_bincode(&payload)
                    .with_context(|| format!("decode system spec {name:?}"))?;
                if spec.name != name {
                    bail!(
                        "systems table corrupted: key {name:?} holds spec.name {:?}",
                        spec.name
                    );
                }
                Ok(Some(spec))
            }
        }
    }

    pub fn delete_system(&self, name: &str) -> Result<()> {
        if name.is_empty() {
            bail!("system name must not be empty");
        }
        let txn = self.db.begin_write().context("begin write for delete_system")?;
        {
            let mut table = txn
                .open_table(SYSTEMS)
                .map_err(|e| anyhow!("open systems table: {e}"))?;
            table
                .remove(name)
                .map_err(|e| anyhow!("delete system {name:?}: {e}"))?;
        }
        txn.commit().context("commit delete_system")?;
        Ok(())
    }

    pub fn list_systems(&self) -> Result<Vec<SystemSpecV1>> {
        let read = self.db.begin_read().context("begin read for list_systems")?;
        let table = match read.open_table(SYSTEMS) {
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(Vec::new()),
            Err(e) => return Err(anyhow!("open systems table: {e}")),
            Ok(t) => t,
        };
        let mut out = Vec::new();
        for entry in table
            .iter()
            .map_err(|e| anyhow!("iterate systems table: {e}"))?
        {
            let (key, value) = entry.map_err(|e| anyhow!("iterate systems entry: {e}"))?;
            let payload = decode_envelope_to_current(
                TableId::Systems.schema(),
                value.value(),
                &format!("system {:?}", key.value()),
            )?;
            let spec: SystemSpecV1 = decode_bincode(&payload)
                .with_context(|| format!("decode system spec {:?}", key.value()))?;
            if spec.name != key.value() {
                bail!(
                    "systems table corrupted: key {:?} holds spec.name {:?}",
                    key.value(),
                    spec.name
                );
            }
            out.push(spec);
        }
        Ok(out)
    }

    fn get_system_payload(&self, name: &str) -> Result<Option<Vec<u8>>> {
        let schema = TableId::Systems.schema();
        let read = self.db.begin_read().context("begin read for get_system")?;
        let raw = match read.open_table(SYSTEMS) {
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
            Err(e) => return Err(anyhow!("open systems table: {e}")),
            Ok(table) => match table
                .get(name)
                .map_err(|e| anyhow!("read system {name:?}: {e}"))?
            {
                None => return Ok(None),
                Some(g) => g.value().to_vec(),
            },
        };
        let rec: VersionedRecord = decode_bincode(&raw)
            .with_context(|| format!("decode envelope of system {name:?}"))?;
        match rec.version.cmp(&schema.current_version) {
            Ordering::Equal => Ok(Some(rec.payload)),
            Ordering::Greater => bail!(
                "system {name:?}: record version {} > current {} (数据文件比二进制新，拒绝读取)",
                rec.version,
                schema.current_version
            ),
            Ordering::Less => {
                drop(read);
                let txn = self
                    .db
                    .begin_write()
                    .context("begin write for system migration")?;
                let payload = migrate_payload(schema, rec.version, rec.payload)
                    .with_context(|| format!("migrate system {name:?}"))?;
                let new_raw = encode_record(schema.current_version, payload.clone())?;
                {
                    let mut table = txn
                        .open_table(SYSTEMS)
                        .map_err(|e| anyhow!("open systems table: {e}"))?;
                    table
                        .insert(name, new_raw.as_slice())
                        .map_err(|e| anyhow!("write back migrated system {name:?}: {e}"))?;
                }
                txn.commit().context("commit system migration write-back")?;
                Ok(Some(payload))
            }
        }
    }

    // ---- ec 表 typed CRUD ----

    pub fn put_brick(&self, entity: u64, brick: &BrickV2) -> Result<()> {
        ensure_entity_id(entity)?;
        let payload = encode_bincode(brick)?;
        self.put_ec(TableId::Brick, entity, payload)
    }

    pub fn get_brick(&self, entity: u64) -> Result<Option<BrickV2>> {
        match self.get_ec(TableId::Brick, entity)? {
            None => Ok(None),
            Some(payload) => Ok(Some(decode_bincode(&payload).with_context(|| {
                format!("decode brick payload of entity {entity}")
            })?)),
        }
    }

    pub fn delete_brick(&self, entity: u64) -> Result<()> {
        self.remove_ec(TableId::Brick, entity)
    }

    // ---- group（M1 新增 `ec:group` 表；直写，低频） ----

    pub fn put_group(&self, entity: u64, group: &GroupV1) -> Result<()> {
        ensure_entity_id(entity)?;
        let payload = encode_bincode(group)?;
        self.put_ec(TableId::Group, entity, payload)
    }

    pub fn get_group(&self, entity: u64) -> Result<Option<GroupV1>> {
        match self.get_ec(TableId::Group, entity)? {
            None => Ok(None),
            Some(payload) => Ok(Some(decode_bincode(&payload).with_context(|| {
                format!("decode group payload of entity {entity}")
            })?)),
        }
    }

    pub fn delete_group(&self, entity: u64) -> Result<()> {
        self.remove_ec(TableId::Group, entity)
    }

    pub fn list_groups(&self) -> Result<Vec<(u64, GroupV1)>> {
        self.list_ec(TableId::Group)
    }

    pub fn stage_video_post(&mut self, entity: u64, v: &VideoPostV1) -> Result<()> {
        ensure_entity_id(entity)?;
        let payload = encode_bincode(v)?;
        let raw = encode_record(TableId::VideoPost.schema().current_version, payload)?;
        self.buffer.stage(TableId::VideoPost, entity, raw);
        self.note_stage_flush()
    }

    pub fn stage_live_post(&mut self, entity: u64, v: &LivePostV1) -> Result<()> {
        ensure_entity_id(entity)?;
        let payload = encode_bincode(v)?;
        let raw = encode_record(TableId::LivePost.schema().current_version, payload)?;
        self.buffer.stage(TableId::LivePost, entity, raw);
        self.note_stage_flush()
    }

    pub fn stage_comment_post(&mut self, entity: u64, v: &CommentPostV1) -> Result<()> {
        ensure_entity_id(entity)?;
        let payload = encode_bincode(v)?;
        let raw = encode_record(TableId::CommentPost.schema().current_version, payload)?;
        self.buffer.stage(TableId::CommentPost, entity, raw);
        self.note_stage_flush()
    }

    pub fn stage_runtime(&mut self, entity: u64, v: &RuntimeV1) -> Result<()> {
        ensure_entity_id(entity)?;
        let payload = encode_bincode(v)?;
        let raw = encode_record(TableId::Runtime.schema().current_version, payload)?;
        self.buffer.stage(TableId::Runtime, entity, raw);
        self.note_stage_flush()
    }

    /// stage 后按阈值/间隔触发 flush；真 flush 时计 volatile_commits（1 次 commit）。
    fn note_stage_flush(&mut self) -> Result<()> {
        let n = self.buffer.maybe_flush(&self.db)?;
        if n > 0 {
            self.stats.count_volatile(1);
        }
        Ok(())
    }

    /// 显式强刷易变表缓冲。
    pub fn flush(&mut self) -> Result<()> {
        let n = self.buffer.flush(&self.db)?;
        if n > 0 {
            self.stats.count_volatile(1);
        }
        Ok(())
    }

    /// 阈值/间隔触发条件满足才 flush（run 循环每轮兜底；不满足为 no-op）。
    pub fn maybe_flush(&mut self) -> Result<usize> {
        let n = self.buffer.maybe_flush(&self.db)?;
        if n > 0 {
            self.stats.count_volatile(1);
        }
        Ok(n)
    }

    /// 调参入口（测试/后续性能冒烟用）。
    #[cfg(test)]
    pub(crate) fn set_buffer_params(&mut self, max_records: usize, max_age: Duration) {
        self.buffer = VolatileBuffer::with_params(max_records, max_age);
    }

    // ---- ec 底层原语（typed 包装层之下；M1 换组件类型不改磁盘逻辑） ----

    fn put_ec(&self, tid: TableId, key: u64, payload: Vec<u8>) -> Result<()> {
        let def = ec_table_def(tid)?;
        let raw = encode_record(tid.schema().current_version, payload)?;
        let txn = self.db.begin_write().context("begin write for put_ec")?;
        {
            let mut table = txn
                .open_table(def)
                .map_err(|e| anyhow!("open {}: {e}", def.name()))?;
            table
                .insert(key, raw.as_slice())
                .map_err(|e| anyhow!("insert {} key {key}: {e}", def.name()))?;
        }
        txn.commit().context("commit put_ec")?;
        self.stats.count_direct(1);
        Ok(())
    }

    fn remove_ec(&self, tid: TableId, key: u64) -> Result<()> {
        let def = ec_table_def(tid)?;
        let txn = self.db.begin_write().context("begin write for remove_ec")?;
        {
            let mut table = txn
                .open_table(def)
                .map_err(|e| anyhow!("open {}: {e}", def.name()))?;
            table
                .remove(key)
                .map_err(|e| anyhow!("remove {} key {key}: {e}", def.name()))?;
        }
        txn.commit().context("commit remove_ec")?;
        self.stats.count_direct(1);
        Ok(())
    }

    fn get_ec(&self, tid: TableId, key: u64) -> Result<Option<Vec<u8>>> {
        let def = ec_table_def(tid)?;
        let schema = tid.schema();
        let read = self.db.begin_read().context("begin read for get_ec")?;
        let Some(raw) = read_ec_envelope(&read, def, key)? else {
            return Ok(None);
        };
        let rec: VersionedRecord = decode_bincode(&raw)
            .with_context(|| format!("decode envelope of {} key {key}", def.name()))?;
        match rec.version.cmp(&schema.current_version) {
            Ordering::Equal => Ok(Some(rec.payload)),
            Ordering::Greater => bail!(
                "{} key {key}: record version {} > current {} (数据文件比二进制新，拒绝读取)",
                def.name(),
                rec.version,
                schema.current_version
            ),
            Ordering::Less => {
                drop(read);
                let txn = self
                    .db
                    .begin_write()
                    .context("begin write for migrate-on-read")?;
                let out = read_ec_writeback(&txn, def, schema, key)?;
                txn.commit().context("commit migrate-on-read write-back")?;
                Ok(out)
            }
        }
    }

    // ---- ec 表全量迭代（M1 启动加载用；只读事务内 migrate-on-read，不写回） ----

    pub fn list_bricks(&self) -> Result<Vec<(u64, BrickV2)>> {
        self.list_ec(TableId::Brick)
    }

    pub fn list_video_posts(&self) -> Result<Vec<(u64, VideoPostV1)>> {
        self.list_ec(TableId::VideoPost)
    }

    pub fn list_live_posts(&self) -> Result<Vec<(u64, LivePostV1)>> {
        self.list_ec(TableId::LivePost)
    }

    pub fn list_comment_posts(&self) -> Result<Vec<(u64, CommentPostV1)>> {
        self.list_ec(TableId::CommentPost)
    }

    pub fn list_runtimes(&self) -> Result<Vec<(u64, RuntimeV1)>> {
        self.list_ec(TableId::Runtime)
    }

    /// key 升序遍历（redb u64 key 有序）；值经信封解码并迁移到 current_version
    /// （`decode_envelope_to_current`，纯内存迁移；启动加载后由 hub 直写/重开兜底写回）。
    fn list_ec<T: serde::de::DeserializeOwned>(&self, tid: TableId) -> Result<Vec<(u64, T)>> {
        let def = ec_table_def(tid)?;
        let schema = tid.schema();
        let read = self.db.begin_read().context("begin read for list_ec")?;
        let table = match read.open_table(def) {
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(Vec::new()),
            Err(e) => return Err(anyhow!("open {}: {e}", def.name())),
            Ok(t) => t,
        };
        let mut out = Vec::new();
        for entry in table
            .iter()
            .map_err(|e| anyhow!("iterate {}: {e}", def.name()))?
        {
            let (key, value) = entry.map_err(|e| anyhow!("iterate {} entry: {e}", def.name()))?;
            let payload = decode_envelope_to_current(
                schema,
                value.value(),
                &format!("{} key {}", def.name(), key.value()),
            )?;
            let v: T = decode_bincode(&payload)
                .with_context(|| format!("decode {} payload of key {}", def.name(), key.value()))?;
            out.push((key.value(), v));
        }
        Ok(out)
    }

    // ---- kv:log 表（M2 追加表；不复用 VolatileBuffer，独立写事务） ----

    /// 批量追加日志记录（单写事务，seq 由调用方保证单调）。
    pub fn append_logs(&self, batch: &[(u64, LogRecordV1)]) -> Result<()> {
        if batch.is_empty() {
            return Ok(());
        }
        let schema = TableSchema::v1();
        let txn = self
            .db
            .begin_write()
            .context("begin write for append_logs")?;
        {
            let mut table = txn
                .open_table(KV_LOG)
                .map_err(|e| anyhow!("open kv:log: {e}"))?;
            for (seq, rec) in batch {
                let payload = encode_bincode(rec)?;
                let raw = encode_record(schema.current_version, payload)?;
                table
                    .insert(*seq, raw.as_slice())
                    .map_err(|e| anyhow!("kv:log insert seq {seq}: {e}"))?;
            }
        }
        txn.commit().context("commit append_logs")?;
        Ok(())
    }

    /// 裁剪最旧记录，保留最近 `max_records` 条。返回删除条数。
    pub fn trim_logs(&self, max_records: u64) -> Result<u64> {
        let len = self.log_len()?;
        if len <= max_records {
            return Ok(0);
        }
        let delete_count = len - max_records;
        // 先收集待删 key（最旧 = 最小 seq）
        let to_remove: Vec<u64> = {
            let read = self.db.begin_read().context("begin read for trim_logs collect")?;
            let table = match read.open_table(KV_LOG) {
                Err(redb::TableError::TableDoesNotExist(_)) => return Ok(0),
                Err(e) => return Err(anyhow!("open kv:log: {e}")),
                Ok(t) => t,
            };
            let mut keys = Vec::with_capacity(delete_count as usize);
            for entry in table
                .iter()
                .map_err(|e| anyhow!("iterate kv:log for trim: {e}"))?
            {
                let (key, _) = entry.map_err(|e| anyhow!("kv:log trim entry: {e}"))?;
                keys.push(key.value());
                if keys.len() as u64 >= delete_count {
                    break;
                }
            }
            keys
        };
        if to_remove.is_empty() {
            return Ok(0);
        }
        let txn = self
            .db
            .begin_write()
            .context("begin write for trim_logs")?;
        {
            let mut table = txn
                .open_table(KV_LOG)
                .map_err(|e| anyhow!("open kv:log: {e}"))?;
            for seq in &to_remove {
                table
                    .remove(*seq)
                    .map_err(|e| anyhow!("kv:log trim remove seq {seq}: {e}"))?;
            }
        }
        txn.commit().context("commit trim_logs")?;
        Ok(to_remove.len() as u64)
    }

    /// 查询日志（草案 API，供 M5 UI；seq 降序，最新在前）。
    pub fn query_logs(&self, q: &LogQuery) -> Result<Vec<(u64, LogRecordV1)>> {
        let schema = TableSchema::v1();
        let read = self.db.begin_read().context("begin read for query_logs")?;
        let table = match read.open_table(KV_LOG) {
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(Vec::new()),
            Err(e) => return Err(anyhow!("open kv:log: {e}")),
            Ok(t) => t,
        };
        let mut out = Vec::new();
        // 倒序遍历（redb 默认升序，倒序需手动实现）
        let iter = table
            .iter()
            .map_err(|e| anyhow!("iterate kv:log: {e}"))?;
        // 收集所有 → 倒序 → 过滤
        let mut all: Vec<(u64, Vec<u8>)> = Vec::new();
        for entry in iter {
            let (key, value) = entry.map_err(|e| anyhow!("kv:log query entry: {e}"))?;
            let seq = key.value();
            // before_seq 游标：只取 seq < before_seq
            if let Some(before) = q.before_seq {
                if seq >= before {
                    continue;
                }
            }
            all.push((seq, value.value().to_vec()));
        }
        drop(table);
        drop(read);

        // 倒序（最新在前）
        all.reverse();
        for (seq, raw) in all {
            let payload = decode_envelope_to_current(
                schema,
                &raw,
                &format!("kv:log seq {seq}"),
            )?;
            let rec: LogRecordV1 = decode_bincode(&payload)
                .with_context(|| format!("decode kv:log seq {seq}"))?;

            // 过滤
            if let Some(min_lv) = q.min_level {
                if rec.level > min_lv {
                    continue;
                }
            }
            if let Some(prefix) = q.target_prefix {
                if !rec.target.starts_with(prefix) {
                    continue;
                }
            }
            if let Some(since) = q.since_ms {
                if rec.ts_ms < since {
                    continue;
                }
            }
            if let Some(until) = q.until_ms {
                if rec.ts_ms > until {
                    continue;
                }
            }

            out.push((seq, rec));
            if out.len() >= q.limit {
                break;
            }
        }
        Ok(out)
    }

    /// 返回 `kv:log` 表当前记录数。表不存在返回 0。
    pub fn log_len(&self) -> Result<u64> {
        let read = self.db.begin_read().context("begin read for log_len")?;
        let table = match read.open_table(KV_LOG) {
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(0),
            Err(e) => return Err(anyhow!("open kv:log: {e}")),
            Ok(t) => t,
        };
        let len = table.len().map_err(|e| anyhow!("kv:log len: {e}"))?;
        Ok(len)
    }

    /// 日志统计：总条数 + 丢弃计数。
    pub fn log_stats(&self) -> Result<LogStats> {
        Ok(LogStats {
            total: self.log_len()?,
            dropped: LOG_DROPPED.load(std::sync::atomic::Ordering::Relaxed),
        })
    }

    /// 返回 `kv:log` 表最大 key（最新 seq）；表不存在或为空返回 0。
    /// T3 供 seq allocator 初始化：起始 seq = max + 1（重启续号，空洞无害）。
    pub fn max_log_seq(&self) -> Result<u64> {
        let read = self.db.begin_read().context("begin read for max_log_seq")?;
        let table = match read.open_table(KV_LOG) {
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(0),
            Err(e) => return Err(anyhow!("open kv:log: {e}")),
            Ok(t) => t,
        };
        let last = table.last().map_err(|e| anyhow!("kv:log last: {e}"))?;
        Ok(last.map(|(k, _)| k.value()).unwrap_or(0))
    }
}

fn ec_table_def(tid: TableId) -> Result<TableDefinition<'static, u64, &'static [u8]>> {
    tid.ec_def()
        .ok_or_else(|| anyhow!("{tid:?} is not an ec table"))
}

fn ensure_entity_id(entity: u64) -> Result<()> {
    if entity == 0 {
        bail!("entity id must not be 0");
    }
    Ok(())
}

fn read_ec_envelope(
    txn: &ReadTransaction,
    def: TableDefinition<u64, &[u8]>,
    key: u64,
) -> Result<Option<Vec<u8>>> {
    let table = match txn.open_table(def) {
        Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
        Err(e) => return Err(anyhow!("open {}: {e}", def.name())),
        Ok(t) => t,
    };
    match table
        .get(key)
        .map_err(|e| anyhow!("read {} key {key}: {e}", def.name()))?
    {
        None => Ok(None),
        Some(g) => Ok(Some(g.value().to_vec())),
    }
}

/// 在写事务里读 + 迁移 + 写回；返回 current_version 的 payload。
/// 与 `read_ec_envelope` 一样，表不存在视为无记录（不建表）。
fn read_ec_writeback(
    txn: &WriteTransaction,
    def: TableDefinition<u64, &[u8]>,
    schema: TableSchema,
    key: u64,
) -> Result<Option<Vec<u8>>> {
    let table = match txn.open_table(def) {
        Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
        Err(e) => return Err(anyhow!("open {}: {e}", def.name())),
        Ok(t) => t,
    };
    let raw = match table
        .get(key)
        .map_err(|e| anyhow!("read {} key {key}: {e}", def.name()))?
    {
        None => return Ok(None),
        Some(g) => g.value().to_vec(),
    };
    let payload = decode_envelope_to_current(schema, &raw, &format!("{} key {key}", def.name()))?;
    let rec: VersionedRecord = decode_bincode(&raw)?;
    if rec.version != schema.current_version {
        let new_raw = encode_record(schema.current_version, payload.clone())?;
        let mut table = table;
        table
            .insert(key, new_raw.as_slice())
            .map_err(|e| anyhow!("write back migrated {} key {key}: {e}", def.name()))?;
    }
    Ok(Some(payload))
}

/// 启动探针：open_or_create + 完整性检查 + close，返回最终路径。
pub fn probe(cfg: &StoreConfig) -> Result<PathBuf> {
    let store = Store::open_or_create(cfg)?;
    store.close()?;
    Ok(cfg.path.clone())
}

impl Drop for Store {
    /// 尽力 flush；失败只 log（M1 起停机强刷挂到 `WeiYuanHui::close` 语义）。
    fn drop(&mut self) {
        if let Err(e) = self.buffer.flush(&self.db) {
            log::error!("store drop: final flush failed (pending data may be lost): {e:#}");
        }
    }
}

// ============================== tests ==============================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    fn cfg_in(dir: &std::path::Path) -> StoreConfig {
        StoreConfig::new(dir.join("state.redb"))
    }

    fn sample_brick(uid: &str) -> BrickV2 {
        BrickV2 {
            uid: uid.to_string(),
            uname: format!("up-{uid}"),
            face: format!("https://face/{uid}"),
            ban: false,
            fid: 0,
            groups: vec![],
            silent: false,
            followed_at: 1700000000,
            updated_at: 1700000001,
        }
    }

    fn sample_group(gid: u64) -> GroupV1 {
        GroupV1 {
            gid,
            name: format!("组{gid}"),
            pin: false,
        }
    }

    // ---------- T1 空库初始化 ----------

    #[test]
    fn t1_empty_db_init_and_reopen() {
        let dir = tempdir().unwrap();
        let cfg = cfg_in(dir.path());
        let store = Store::open_or_create(&cfg).unwrap();
        store.close().unwrap();

        let store = Store::open_or_create(&cfg).unwrap();
        let read = store.db.begin_read().unwrap();
        let meta = read.open_table(META).unwrap();
        assert_eq!(meta.get(META_FORMAT_MAGIC).unwrap().unwrap().value(), FORMAT_MAGIC);
        assert_eq!(meta.get(META_SCHEMA_VERSION).unwrap().unwrap().value(), SCHEMA_VERSION);
        assert_eq!(meta.get(META_NEXT_ENTITY_ID).unwrap().unwrap().value(), INITIAL_ENTITY_ID);
        for def in [EC_BRICK, EC_VIDEO_POST, EC_LIVE_POST, EC_COMMENT_POST, EC_RUNTIME, EC_GROUP] {
            match read.open_table(def) {
                Ok(t) => assert_eq!(t.len().unwrap(), 0, "table {} should be empty", def.name()),
                Err(redb::TableError::TableDoesNotExist(_)) => {}
                Err(e) => panic!("unexpected error opening {}: {e}", def.name()),
            }
        }
        match read.open_table(SYSTEMS) {
            Ok(t) => assert_eq!(t.len().unwrap(), 0, "table systems should be empty"),
            Err(redb::TableError::TableDoesNotExist(_)) => {}
            Err(e) => panic!("unexpected error opening systems: {e}"),
        }
    }

    // ---------- T2 样例数据读写 ----------

    #[test]
    fn t2_sample_data_roundtrip() {
        let dir = tempdir().unwrap();
        let cfg = cfg_in(dir.path());
        {
            let mut store = Store::open_or_create(&cfg).unwrap();
            store
                .put_system(&SystemSpecV1 {
                    name: "sys1".to_string(),
                    lua: "print('hi')".to_string(),
                    condition: "always".to_string(),
                })
                .unwrap();
            let e1 = store.alloc_entity_id().unwrap();
            let e2 = store.alloc_entity_id().unwrap();
            store.put_brick(e1, &sample_brick("10001")).unwrap();
            store.put_brick(e2, &sample_brick("10002")).unwrap();
            store
                .stage_video_post(
                    e1,
                    &VideoPostV1 {
                        updated_at: 1,
                        latest_ts: 2,
                        items: vec![VideoItemV1 {
                            bvid: "BV1".to_string(),
                            title: "t".to_string(),
                            pubdate: 3,
                            extra: json!({"part": 1}).to_string(),
                        }],
                        extra: json!({"n": 2}).to_string(),
                        episodic: None,
                    },
                )
                .unwrap();
            store
                .stage_runtime(
                    e2,
                    &RuntimeV1 {
                        updated_at: 4,
                        fields: json!({"bucket": {"left": 5}}).to_string(),
                    },
                )
                .unwrap();
            store.close().unwrap();
        }
        let store = Store::open_or_create(&cfg).unwrap();
        let sys = store.get_system("sys1").unwrap().unwrap();
        assert_eq!(sys.name, "sys1");
        assert_eq!(sys.lua, "print('hi')");
        assert_eq!(sys.condition, "always");
        let bricks: Vec<BrickV2> = (2..4)
            .map(|e| store.get_brick(e).unwrap().unwrap())
            .collect();
        assert_eq!(bricks[0].uid, "10001");
        assert_eq!(bricks[1].uid, "10002");
        let video = store
            .get_ec(TableId::VideoPost, 2)
            .unwrap()
            .unwrap();
        let video: VideoPostV1 = decode_bincode(&video).unwrap();
        assert_eq!(video.items.len(), 1);
        assert_eq!(video.items[0].bvid, "BV1");
        assert_eq!(video.extra, json!({"n": 2}).to_string());
        let runtime = store
            .get_ec(TableId::Runtime, 3)
            .unwrap()
            .unwrap();
        let runtime: RuntimeV1 = decode_bincode(&runtime).unwrap();
        assert_eq!(runtime.fields, json!({"bucket": {"left": 5}}).to_string());
        assert_eq!(store.list_systems().unwrap().len(), 1);
    }

    // ---------- T3 错误文件守卫 ----------

    #[test]
    fn t3_garbage_file_rejected_and_untouched() {
        let dir = tempdir().unwrap();
        let cfg = cfg_in(dir.path());
        std::fs::write(&cfg.path, b"this is not a redb file at all").unwrap();
        let err = Store::open_or_create(&cfg).unwrap_err();
        assert!(format!("{err:#}").contains("state db"));
        // 原文件未被写坏
        assert_eq!(
            std::fs::read(&cfg.path).unwrap(),
            b"this is not a redb file at all"
        );
    }

    #[test]
    fn t3_wrong_magic_rejected_and_untouched() {
        let dir = tempdir().unwrap();
        let cfg = cfg_in(dir.path());
        {
            let db = Database::create(&cfg.path).unwrap();
            let txn = db.begin_write().unwrap();
            {
                let mut meta = txn.open_table(META).unwrap();
                meta.insert(META_FORMAT_MAGIC, 0xBAD_u64).unwrap();
            }
            txn.commit().unwrap();
        }
        let err = Store::open_or_create(&cfg).unwrap_err();
        assert!(format!("{err:#}").contains("magic mismatch"));
        // 原文件未被写坏：魔数仍是 0xBAD，且没有 next_entity_id 被补上
        let db = Database::open(&cfg.path).unwrap();
        let read = db.begin_read().unwrap();
        let meta = read.open_table(META).unwrap();
        assert_eq!(meta.get(META_FORMAT_MAGIC).unwrap().unwrap().value(), 0xBAD);
        assert!(meta.get(META_NEXT_ENTITY_ID).unwrap().is_none());
    }

    // ---------- T4 codec roundtrip ----------

    #[test]
    fn t4_codec_roundtrip_all_types() {
        let brick = sample_brick("1");
        assert_eq!(decode_bincode::<BrickV2>(&encode_bincode(&brick).unwrap()).unwrap(), brick);

        let video = VideoPostV1 {
            updated_at: 1,
            latest_ts: 2,
            items: vec![VideoItemV1 {
                bvid: "BV1".to_string(),
                title: "t".to_string(),
                pubdate: 3,
                extra: json!([1, "two", null, {"k": true}]).to_string(),
            }],
            extra: json!({"mixed": [1, 2.5, "x", null]}).to_string(),
            episodic: None,
        };
        assert_eq!(
            decode_bincode::<VideoPostV1>(&encode_bincode(&video).unwrap()).unwrap(),
            video
        );

        let live = LivePostV1 {
            updated_at: 1,
            is_open: true,
            title: "live".to_string(),
            url: "https://live".to_string(),
            ts: 9,
            extra: json!(null).to_string(),
            entropy: 0,
            entropy_txt: String::new(),
        };
        assert_eq!(decode_bincode::<LivePostV1>(&encode_bincode(&live).unwrap()).unwrap(), live);

        let comment = CommentPostV1 {
            updated_at: 1,
            items: vec![CommentItemV1 {
                rpid: 42,
                msg: "hi".to_string(),
                ts: 5,
                extra: json!({"ok": false}).to_string(),
            }],
            extra: json!({}).to_string(),
        };
        assert_eq!(
            decode_bincode::<CommentPostV1>(&encode_bincode(&comment).unwrap()).unwrap(),
            comment
        );

        let runtime = RuntimeV1 {
            updated_at: 7,
            fields: json!({"a": {"b": [1, 2]}}).to_string(),
        };
        assert_eq!(
            decode_bincode::<RuntimeV1>(&encode_bincode(&runtime).unwrap()).unwrap(),
            runtime
        );

        let group = GroupV1 {
            gid: 42,
            name: "默认分组".to_string(),
            pin: true,
        };
        assert_eq!(
            decode_bincode::<GroupV1>(&encode_bincode(&group).unwrap()).unwrap(),
            group
        );

        let system = SystemSpecV1 {
            name: "s".to_string(),
            lua: "lua".to_string(),
            condition: "cond".to_string(),
        };
        assert_eq!(
            decode_bincode::<SystemSpecV1>(&encode_bincode(&system).unwrap()).unwrap(),
            system
        );

        let rec = VersionedRecord {
            version: 1,
            payload: vec![1, 2, 3],
        };
        assert_eq!(
            decode_bincode::<VersionedRecord>(&encode_bincode(&rec).unwrap()).unwrap(),
            rec
        );
    }

    // ---------- T5 迁移链 ----------

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct MigV1 {
        a: u32,
    }
    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct MigV2 {
        a: u32,
        b: u64,
    }

    fn mig_v1_to_v2(old: &[u8]) -> Result<Vec<u8>> {
        let v1: MigV1 = decode_bincode(old)?;
        encode_bincode(&MigV2 { a: v1.a, b: 100 })
    }

    #[test]
    fn t5_migration_chain_and_writeback() {
        static MIGS: [(u32, MigrationFn); 1] = [(1, mig_v1_to_v2 as MigrationFn)];
        let schema = TableSchema {
            current_version: 2,
            migrations: &MIGS,
        };
        const TEST_DEF: TableDefinition<u64, &[u8]> = TableDefinition::new("t:migration");

        let dir = tempdir().unwrap();
        let path = dir.path().join("mig.redb");
        {
            let db = Database::create(&path).unwrap();
            let txn = db.begin_write().unwrap();
            {
                let mut table = txn.open_table(TEST_DEF).unwrap();
                let v1_raw = encode_record(1, encode_bincode(&MigV1 { a: 42 }).unwrap()).unwrap();
                table.insert(7_u64, v1_raw.as_slice()).unwrap();
            }
            txn.commit().unwrap();
        }
        {
            let db = Database::open(&path).unwrap();
            let txn = db.begin_write().unwrap();
            let out = read_ec_writeback(&txn, TEST_DEF, schema, 7).unwrap().unwrap();
            assert_eq!(decode_bincode::<MigV2>(&out).unwrap(), MigV2 { a: 42, b: 100 });
            txn.commit().unwrap();
        }
        // 已写回：磁盘上的信封 version == 2
        {
            let db = Database::open(&path).unwrap();
            let read = db.begin_read().unwrap();
            let table = read.open_table(TEST_DEF).unwrap();
            let raw = table.get(7_u64).unwrap().unwrap().value().to_vec();
            let rec: VersionedRecord = decode_bincode(&raw).unwrap();
            assert_eq!(rec.version, 2);
            assert_eq!(decode_bincode::<MigV2>(&rec.payload).unwrap(), MigV2 { a: 42, b: 100 });
        }

        // 缺钩子报错
        let broken = TableSchema {
            current_version: 3,
            migrations: &[],
        };
        {
            let db = Database::open(&path).unwrap();
            let txn = db.begin_write().unwrap();
            let err = read_ec_writeback(&txn, TEST_DEF, broken, 7).unwrap_err();
            assert!(format!("{err:#}").contains("missing migration hook"));
        }
    }

    // ---------- T6 版本过高拒绝 ----------

    #[test]
    fn t6_newer_record_version_rejected() {
        let dir = tempdir().unwrap();
        let cfg = cfg_in(dir.path());
        let store = Store::open_or_create(&cfg).unwrap();
        store.close().unwrap();
        {
            let db = Database::open(&cfg.path).unwrap();
            let txn = db.begin_write().unwrap();
            {
                let mut table = txn.open_table(EC_VIDEO_POST).unwrap();
                let raw = encode_record(2, vec![]).unwrap();
                table.insert(9_u64, raw.as_slice()).unwrap();
            }
            txn.commit().unwrap();
        }
        let store = Store::open_or_create(&cfg).unwrap();
        let err = store.get_ec(TableId::VideoPost, 9).unwrap_err();
        assert!(format!("{err:#}").contains("数据文件比二进制新"));
    }

    // ---------- T7 entity id ----------

    #[test]
    fn t7_entity_id_alloc_and_monotonic() {
        let dir = tempdir().unwrap();
        let cfg = cfg_in(dir.path());
        let store = Store::open_or_create(&cfg).unwrap();
        assert_eq!(store.alloc_entity_id().unwrap(), 2);
        assert_eq!(store.alloc_entity_id().unwrap(), 3);
        assert!(store.put_brick(0, &sample_brick("0")).is_err());
        store.ensure_next_entity_id(10).unwrap();
        assert_eq!(store.alloc_entity_id().unwrap(), 10);
        store.ensure_next_entity_id(5).unwrap(); // 只增不减
        assert_eq!(store.alloc_entity_id().unwrap(), 11);
        store.close().unwrap();

        let store = Store::open_or_create(&cfg).unwrap();
        assert_eq!(store.alloc_entity_id().unwrap(), 12, "reopen 后不重复");
    }

    // ---------- T8 直写语义 ----------

    #[test]
    fn t8_brick_direct_write_visibility() {
        let dir = tempdir().unwrap();
        let cfg = cfg_in(dir.path());
        let store = Store::open_or_create(&cfg).unwrap();
        store.put_brick(2, &sample_brick("1")).unwrap();
        // 不 flush，新读事务立即可见
        assert_eq!(store.get_brick(2).unwrap().unwrap().uid, "1");
        store.delete_brick(2).unwrap();
        assert!(store.get_brick(2).unwrap().is_none());
    }

    // ---------- T9 批量 flush 四路径 ----------

    #[test]
    fn t9_flush_threshold_and_reopen() {
        let dir = tempdir().unwrap();
        let cfg = cfg_in(dir.path());
        {
            let mut store = Store::open_or_create(&cfg).unwrap();
            store.set_buffer_params(2, Duration::from_secs(3600));
            store
                .stage_video_post(2, &VideoPostV1 {
                    updated_at: 1,
                    latest_ts: 0,
                    items: vec![],
                    extra: "{}".to_string(),
                    episodic: None,
                })
                .unwrap();
            assert!(store.get_ec(TableId::VideoPost, 2).unwrap().is_none());
            // 第 2 条触发阈值 flush
            store
                .stage_video_post(3, &VideoPostV1 {
                    updated_at: 2,
                    latest_ts: 0,
                    items: vec![],
                    extra: "{}".to_string(),
                    episodic: None,
                })
                .unwrap();
            assert!(store.get_ec(TableId::VideoPost, 2).unwrap().is_some());
            assert!(store.get_ec(TableId::VideoPost, 3).unwrap().is_some());
        }
        let store = Store::open_or_create(&cfg).unwrap();
        assert!(store.get_ec(TableId::VideoPost, 2).unwrap().is_some());
        assert!(store.get_ec(TableId::VideoPost, 3).unwrap().is_some());
    }

    #[test]
    fn t9_flush_age_trigger() {
        let dir = tempdir().unwrap();
        let cfg = cfg_in(dir.path());
        let mut store = Store::open_or_create(&cfg).unwrap();
        store.set_buffer_params(100, Duration::from_millis(10));
        store
            .stage_video_post(2, &VideoPostV1 {
                updated_at: 1,
                latest_ts: 0,
                items: vec![],
                extra: "{}".to_string(),
                episodic: None,
            })
            .unwrap();
        assert!(store.get_ec(TableId::VideoPost, 2).unwrap().is_none());
        std::thread::sleep(Duration::from_millis(30));
        // 下一次 stage 时发现超龄 → 两条一起 flush
        store
            .stage_video_post(3, &VideoPostV1 {
                updated_at: 2,
                latest_ts: 0,
                items: vec![],
                extra: "{}".to_string(),
                episodic: None,
            })
            .unwrap();
        assert!(store.get_ec(TableId::VideoPost, 2).unwrap().is_some());
        assert!(store.get_ec(TableId::VideoPost, 3).unwrap().is_some());
    }

    #[test]
    fn t9_flush_explicit_and_drop() {
        let dir = tempdir().unwrap();
        let cfg = cfg_in(dir.path());
        // 显式 flush
        {
            let mut store = Store::open_or_create(&cfg).unwrap();
            store
                .stage_video_post(2, &VideoPostV1 {
                    updated_at: 1,
                    latest_ts: 0,
                    items: vec![],
                    extra: "{}".to_string(),
                    episodic: None,
                })
                .unwrap();
            store.flush().unwrap();
            assert!(store.get_ec(TableId::VideoPost, 2).unwrap().is_some());
        }
        // Drop 尽力 flush
        {
            let mut store = Store::open_or_create(&cfg).unwrap();
            store
                .stage_video_post(3, &VideoPostV1 {
                    updated_at: 1,
                    latest_ts: 0,
                    items: vec![],
                    extra: "{}".to_string(),
                    episodic: None,
                })
                .unwrap();
            assert!(store.get_ec(TableId::VideoPost, 3).unwrap().is_none());
            drop(store);
        }
        let store = Store::open_or_create(&cfg).unwrap();
        assert!(store.get_ec(TableId::VideoPost, 2).unwrap().is_some());
        assert!(store.get_ec(TableId::VideoPost, 3).unwrap().is_some());
    }

    // ---------- T10 systems 表语义 ----------

    #[test]
    fn t10_systems_key_name_mismatch_rejected() {
        let dir = tempdir().unwrap();
        let cfg = cfg_in(dir.path());
        let store = Store::open_or_create(&cfg).unwrap();
        let spec = SystemSpecV1 {
            name: "a".to_string(),
            lua: "l".to_string(),
            condition: "c".to_string(),
        };
        store.put_system(&spec).unwrap();
        assert!(store.put_system(&SystemSpecV1 {
            name: String::new(),
            ..spec.clone()
        })
        .is_err());
        assert_eq!(store.list_systems().unwrap(), vec![spec]);
        assert_eq!(store.get_system("a").unwrap().unwrap().name, "a");
        store.delete_system("a").unwrap();
        assert!(store.get_system("a").unwrap().is_none());
        assert!(store.list_systems().unwrap().is_empty());

        // 通过 raw 制造 key 与 value.name 不一致
        let mismatched = SystemSpecV1 {
            name: "other".to_string(),
            lua: "l".to_string(),
            condition: "c".to_string(),
        };
        let raw = encode_record(
            TableId::Systems.schema().current_version,
            encode_bincode(&mismatched).unwrap(),
        )
        .unwrap();
        {
            let txn = store.db.begin_write().unwrap();
            {
                let mut table = txn.open_table(SYSTEMS).unwrap();
                table.insert("k", raw.as_slice()).unwrap();
            }
            txn.commit().unwrap();
        }
        let err = store.get_system("k").unwrap_err();
        assert!(format!("{err:#}").contains("corrupted"));
    }

    // ---------- T11 配置优先级 ----------

    #[test]
    fn t11_config_resolution() {
        let cli = PathBuf::from("/cli/state.redb");
        let home = PathBuf::from("/home/user");
        assert_eq!(
            StoreConfig::resolve(Some(cli.clone()), Some(home.clone())),
            cli,
            "CLI 优先"
        );
        assert_eq!(
            StoreConfig::resolve(None, Some(home)),
            PathBuf::from("/home/user/.hobob/state.redb"),
            "无 CLI 时 home 派生默认值"
        );
        assert!(StoreConfig::default_path().ends_with(".hobob/state.redb"));
    }

    // ---------- probe ----------

    #[test]
    fn t12_probe_creates_and_revalidates() {
        let dir = tempdir().unwrap();
        let cfg = cfg_in(dir.path());
        assert_eq!(probe(&cfg).unwrap(), cfg.path);
        assert!(cfg.path.exists());
        assert_eq!(probe(&cfg).unwrap(), cfg.path, "二次 probe 应幂等");
    }

    // ---------- T13 group 表（M1 `ec:group`；重启可见） ----------

    #[test]
    fn t13_group_crud_roundtrip_and_reopen() {
        let dir = tempdir().unwrap();
        let cfg = cfg_in(dir.path());
        {
            let store = Store::open_or_create(&cfg).unwrap();
            store
                .put_group(2, &GroupV1 { gid: 0, name: "全部".into(), pin: true })
                .unwrap();
            store.put_group(4, &sample_group(9)).unwrap();
            assert_eq!(store.get_group(4).unwrap().unwrap().gid, 9);
            assert!(store.get_group(2).unwrap().unwrap().pin);
            assert_eq!(store.list_groups().unwrap().len(), 2);
            // entity 0 非法
            assert!(store.put_group(0, &sample_group(1)).is_err());
            store.delete_group(4).unwrap();
            assert!(store.get_group(4).unwrap().is_none());
            store.close().unwrap();
        }
        let store = Store::open_or_create(&cfg).unwrap();
        // 重启可见；已删组不复活
        assert_eq!(store.get_group(2).unwrap().unwrap().gid, 0);
        assert!(store.get_group(4).unwrap().is_none());
        assert_eq!(
            store.list_groups().unwrap(),
            vec![(2, GroupV1 { gid: 0, name: "全部".into(), pin: true })]
        );
    }

    // ---------- T14 全量 list_*（M1 启动加载） ----------

    #[test]
    fn t14_list_all_ec_tables_full_iteration() {
        let dir = tempdir().unwrap();
        let cfg = cfg_in(dir.path());
        let mut store = Store::open_or_create(&cfg).unwrap();
        let e2 = store.alloc_entity_id().unwrap();
        let e3 = store.alloc_entity_id().unwrap();
        store.put_brick(e2, &sample_brick("10001")).unwrap();
        store.put_brick(e3, &sample_brick("10002")).unwrap();
        store.put_group(e2, &sample_group(7)).unwrap();
        store
            .stage_video_post(
                e2,
                &VideoPostV1 {
                    updated_at: 1,
                    latest_ts: 2,
                    items: vec![VideoItemV1 {
                        bvid: "BV1".into(),
                        title: "t".into(),
                        pubdate: 3,
                        extra: "{}".into(),
                    }],
                    extra: "{}".into(),
                    episodic: None,
                },
            )
            .unwrap();
        store
            .stage_live_post(
                e3,
                &LivePostV1 {
                    updated_at: 1,
                    is_open: true,
                    title: "live".into(),
                    url: "https://live".into(),
                    ts: 0,
                    extra: "{}".into(),
                    entropy: 0,
                    entropy_txt: String::new(),
                },
            )
            .unwrap();
        store
            .stage_comment_post(
                e3,
                &CommentPostV1 {
                    updated_at: 1,
                    items: vec![CommentItemV1 {
                        rpid: 42,
                        msg: "hi".into(),
                        ts: 5,
                        extra: "{}".into(),
                    }],
                    extra: "{}".into(),
                },
            )
            .unwrap();
        store
            .stage_runtime(
                e3,
                &RuntimeV1 {
                    updated_at: 4,
                    fields: "{}".into(),
                },
            )
            .unwrap();
        store.flush().unwrap();

        // key 升序（redb u64 有序），全量不遗漏
        let bricks = store.list_bricks().unwrap();
        assert_eq!(bricks.len(), 2);
        assert_eq!((bricks[0].0, bricks[0].1.uid.as_str()), (e2, "10001"));
        assert_eq!((bricks[1].0, bricks[1].1.uid.as_str()), (e3, "10002"));
        let groups = store.list_groups().unwrap();
        assert_eq!(groups, vec![(e2, sample_group(7))]);
        let videos = store.list_video_posts().unwrap();
        assert_eq!(videos.len(), 1);
        assert_eq!((videos[0].0, videos[0].1.items[0].bvid.as_str()), (e2, "BV1"));
        let lives = store.list_live_posts().unwrap();
        assert_eq!((lives[0].0, lives[0].1.title.as_str()), (e3, "live"));
        let comments = store.list_comment_posts().unwrap();
        assert_eq!((comments[0].0, comments[0].1.items[0].rpid), (e3, 42));
        let runtimes = store.list_runtimes().unwrap();
        assert_eq!((runtimes[0].0, runtimes[0].1.updated_at), (e3, 4));
    }

    // ---------- T15 布局 1→2 升级（N11） ----------

    #[test]
    fn t15_layout_v1_db_auto_upgrade_idempotent() {
        let dir = tempdir().unwrap();
        let cfg = cfg_in(dir.path());
        // 手工构造 v1 布局库（M0：magic + schema_version=1 + next_entity_id，无 ec:group 表）
        {
            let db = Database::create(&cfg.path).unwrap();
            let txn = db.begin_write().unwrap();
            {
                let mut meta = txn.open_table(META).unwrap();
                meta.insert(META_FORMAT_MAGIC, FORMAT_MAGIC).unwrap();
                meta.insert(META_SCHEMA_VERSION, 1).unwrap();
                meta.insert(META_NEXT_ENTITY_ID, 2).unwrap();
            }
            txn.commit().unwrap();
        }
        // 首次 open：自动升到 SCHEMA_VERSION=2 且 ec:group 表可开（空表）
        let store = Store::open_or_create(&cfg).unwrap();
        store.close().unwrap();
        {
            let db = Database::open(&cfg.path).unwrap();
            let read = db.begin_read().unwrap();
            let meta = read.open_table(META).unwrap();
            assert_eq!(
                meta.get(META_SCHEMA_VERSION).unwrap().unwrap().value(),
                SCHEMA_VERSION
            );
            let group_table = read.open_table(EC_GROUP).unwrap();
            assert_eq!(group_table.len().unwrap(), 0);
        }
        // 幂等：二次 open 不再升级/报错，且 ec:group 可写
        let store = Store::open_or_create(&cfg).unwrap();
        store.put_group(2, &sample_group(1)).unwrap();
        assert_eq!(store.get_group(2).unwrap().unwrap().gid, 1);
        store.close().unwrap();
    }

    // ---------- T16 brick V1→V2 迁移链（N4；legacy 字节样例可迁 + 写回） ----------

    #[test]
    fn t16_brick_v1_bytes_migrate_to_v2_and_writeback() {
        let dir = tempdir().unwrap();
        let cfg = cfg_in(dir.path());
        let store = Store::open_or_create(&cfg).unwrap();
        // 手工落 v1 信封字节（legacy::BrickV1 序列化，模拟 M0 磁盘数据）
        let v1 = legacy::BrickV1 {
            uid: "42".into(),
            uname: "旧 up".into(),
            face: "https://face/42".into(),
            sign: "签名".into(),
            groups: vec!["g1".into(), "g2".into()],
            silent: true,
            followed_at: 1700000000,
            updated_at: 1700000001,
        };
        let raw = encode_record(1, encode_bincode(&v1).unwrap()).unwrap();
        {
            let txn = store.db.begin_write().unwrap();
            {
                let mut table = txn.open_table(EC_BRICK).unwrap();
                table.insert(2_u64, raw.as_slice()).unwrap();
            }
            txn.commit().unwrap();
        }
        // 读：migrate-on-read 返回 V2（groups 空、ban=false、fid=0；其余字段保留）
        let brick = store.get_brick(2).unwrap().unwrap();
        assert_eq!(brick.uid, "42");
        assert_eq!(brick.uname, "旧 up");
        assert_eq!(brick.face, "https://face/42");
        assert!(!brick.ban);
        assert_eq!(brick.fid, 0);
        assert!(brick.groups.is_empty());
        assert!(brick.silent);
        assert_eq!(brick.followed_at, 1700000000);
        assert_eq!(brick.updated_at, 1700000001);
        // 已写回：磁盘信封 version == 2
        {
            let read = store.db.begin_read().unwrap();
            let table = read.open_table(EC_BRICK).unwrap();
            let raw = table.get(2_u64).unwrap().unwrap().value().to_vec();
            let rec: VersionedRecord = decode_bincode(&raw).unwrap();
            assert_eq!(rec.version, 2);
        }
        // 二次读不再迁移；list_* 亦按 V2 全量返回
        assert_eq!(store.get_brick(2).unwrap().unwrap().uid, "42");
        assert_eq!(store.list_bricks().unwrap().len(), 1);
        store.close().unwrap();
    }

    // ---------- T17 kv:log 追加/持久化/重启可见（N1） ----------

    fn sample_log(seq: u64, msg: &str, level: u8, target: &str) -> (u64, LogRecordV1) {
        (
            seq,
            LogRecordV1 {
                ts_ms: 1700000000000 + seq as i64,
                level,
                target: target.to_string(),
                msg: msg.to_string(),
                loc: Some("test.rs:1".to_string()),
                ctx: String::new(),
            },
        )
    }

    #[test]
    fn t17_append_logs_reopen_visible_and_seq_continues() {
        let dir = tempdir().unwrap();
        let cfg = cfg_in(dir.path());
        {
            let store = Store::open_or_create(&cfg).unwrap();
            let batch: Vec<_> = (0..100)
                .map(|i| sample_log(i, &format!("msg-{i}"), 3, "test::mod"))
                .collect();
            store.append_logs(&batch).unwrap();
            assert_eq!(store.log_len().unwrap(), 100);
            // 同进程读事务可见
            let results = store
                .query_logs(&LogQuery {
                    limit: 200,
                    ..Default::default()
                })
                .unwrap();
            assert_eq!(results.len(), 100);
            // 最新在前
            assert_eq!(results[0].0, 99);
            assert_eq!(results[0].1.msg, "msg-99");
            assert_eq!(results[99].0, 0);
            store.close().unwrap();
        }
        // reopen：全量可查，seq 续号不重复
        let store = Store::open_or_create(&cfg).unwrap();
        assert_eq!(store.log_len().unwrap(), 100);
        let results = store
            .query_logs(&LogQuery::default())
            .unwrap();
        assert_eq!(results.len(), 100);
        // 追加新批次：seq 从 100 起（调用方分配，此处验证 reopen 后表不丢数据）
        let batch2: Vec<_> = (100..110)
            .map(|i| sample_log(i, &format!("msg-{i}"), 2, "test::mod"))
            .collect();
        store.append_logs(&batch2).unwrap();
        assert_eq!(store.log_len().unwrap(), 110);
        store.close().unwrap();
    }

    // ---------- T18 裁剪边界（N2） ----------

    #[test]
    fn t18_trim_logs_keeps_newest() {
        let dir = tempdir().unwrap();
        let cfg = cfg_in(dir.path());
        let store = Store::open_or_create(&cfg).unwrap();
        let batch: Vec<_> = (0..10)
            .map(|i| sample_log(i, &format!("m{i}"), 3, "t"))
            .collect();
        store.append_logs(&batch).unwrap();
        assert_eq!(store.log_len().unwrap(), 10);

        // 上限 = 10，恰好等于：不删
        assert_eq!(store.trim_logs(10).unwrap(), 0);
        assert_eq!(store.log_len().unwrap(), 10);

        // 上限 = 5：删最旧 5 条（seq 0..5），保留 5..10
        assert_eq!(store.trim_logs(5).unwrap(), 5);
        assert_eq!(store.log_len().unwrap(), 5);
        let results = store
            .query_logs(&LogQuery {
                limit: 20,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(results.len(), 5);
        let seqs: Vec<u64> = results.iter().map(|(s, _)| *s).collect();
        assert_eq!(seqs, vec![9, 8, 7, 6, 5]);

        // 上限 = 0：全删
        assert_eq!(store.trim_logs(0).unwrap(), 5);
        assert_eq!(store.log_len().unwrap(), 0);

        // 空表 trim 不报错
        assert_eq!(store.trim_logs(5).unwrap(), 0);
        store.close().unwrap();
    }

    // ---------- T19 查询过滤（N3） ----------

    #[test]
    fn t19_query_logs_filters() {
        let dir = tempdir().unwrap();
        let cfg = cfg_in(dir.path());
        let store = Store::open_or_create(&cfg).unwrap();
        let mut batch = Vec::new();
        for i in 0..50 {
            let level = match i % 5 {
                0 => 1u8, // ERROR
                1 => 2,   // WARN
                2 => 3,   // INFO
                3 => 4,   // DEBUG
                _ => 5,   // TRACE
            };
            let target = if i < 25 { "hobob::db" } else { "op" };
            batch.push((
                i as u64,
                LogRecordV1 {
                    ts_ms: 1000 + i as i64 * 100,
                    level,
                    target: target.to_string(),
                    msg: format!("msg-{i}"),
                    loc: None,
                    ctx: String::new(),
                },
            ));
        }
        store.append_logs(&batch).unwrap();

        // limit 截断
        let r = store
            .query_logs(&LogQuery {
                limit: 5,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(r.len(), 5);
        assert_eq!(r[0].0, 49); // 最新在前

        // before_seq 翻页
        let r = store
            .query_logs(&LogQuery {
                limit: 100,
                before_seq: Some(5),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(r.len(), 5);
        assert_eq!(r[0].0, 4);
        assert_eq!(r[4].0, 0);

        // min_level: 只取 ERROR(1)+WARN(2)
        let r = store
            .query_logs(&LogQuery {
                limit: 100,
                min_level: Some(2),
                ..Default::default()
            })
            .unwrap();
        assert!(r.iter().all(|(_, rec)| rec.level <= 2));
        // 级别 1+2 各占 1/5，50 条共 20 条
        assert_eq!(r.len(), 20);

        // target_prefix
        let r = store
            .query_logs(&LogQuery {
                limit: 100,
                target_prefix: Some("op"),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(r.len(), 25);
        assert!(r.iter().all(|(_, rec)| rec.target == "op"));

        // 时间窗
        let r = store
            .query_logs(&LogQuery {
                limit: 100,
                since_ms: Some(2000),
                until_ms: Some(3000),
                ..Default::default()
            })
            .unwrap();
        assert!(r.iter().all(|(_, rec)| rec.ts_ms >= 2000 && rec.ts_ms <= 3000));

        // 空结果
        let r = store
            .query_logs(&LogQuery {
                target_prefix: Some("nonexistent"),
                ..Default::default()
            })
            .unwrap();
        assert!(r.is_empty());

        store.close().unwrap();
    }

    // ---------- T20 布局 2→3 升级（N9） ----------

    #[test]
    fn t20_layout_v2_db_auto_upgrade_to_v3_idempotent() {
        let dir = tempdir().unwrap();
        let cfg = cfg_in(dir.path());
        // 手工构造 v2 布局库（M1：magic + schema_version=2 + next_entity_id，有 ec:group 表，无 kv:log 表）
        {
            let db = Database::create(&cfg.path).unwrap();
            let txn = db.begin_write().unwrap();
            {
                let mut meta = txn.open_table(META).unwrap();
                meta.insert(META_FORMAT_MAGIC, FORMAT_MAGIC).unwrap();
                meta.insert(META_SCHEMA_VERSION, 2).unwrap();
                meta.insert(META_NEXT_ENTITY_ID, 2).unwrap();
            }
            // 建 ec:group 表（v2 特征）
            txn.open_table(EC_GROUP).unwrap();
            txn.commit().unwrap();
        }
        // 首次 open：自动升到 SCHEMA_VERSION=3 且 kv:log 表可开（空表）
        let store = Store::open_or_create(&cfg).unwrap();
        store.close().unwrap();
        {
            let db = Database::open(&cfg.path).unwrap();
            let read = db.begin_read().unwrap();
            let meta = read.open_table(META).unwrap();
            assert_eq!(
                meta.get(META_SCHEMA_VERSION).unwrap().unwrap().value(),
                SCHEMA_VERSION
            );
            let log_table = read.open_table(KV_LOG).unwrap();
            assert_eq!(log_table.len().unwrap(), 0);
        }
        // 幂等：二次 open 不再升级/报错，且 kv:log 可写
        let store = Store::open_or_create(&cfg).unwrap();
        store
            .append_logs(&[sample_log(0, "hello", 3, "test")])
            .unwrap();
        assert_eq!(store.log_len().unwrap(), 1);
        store.close().unwrap();

        // 版本过高（schema > 3）仍拒开
        {
            let db = Database::open(&cfg.path).unwrap();
            let txn = db.begin_write().unwrap();
            {
                let mut meta = txn.open_table(META).unwrap();
                meta.insert(META_SCHEMA_VERSION, 99).unwrap();
            }
            txn.commit().unwrap();
        }
        let err = Store::open_or_create(&cfg).unwrap_err();
        assert!(format!("{err:#}").contains("数据文件比二进制新"));
    }

    // ---------- T21 v1 库升到 v3（跨两级升级） ----------

    #[test]
    fn t21_layout_v1_db_upgrades_to_v3() {
        let dir = tempdir().unwrap();
        let cfg = cfg_in(dir.path());
        {
            let db = Database::create(&cfg.path).unwrap();
            let txn = db.begin_write().unwrap();
            {
                let mut meta = txn.open_table(META).unwrap();
                meta.insert(META_FORMAT_MAGIC, FORMAT_MAGIC).unwrap();
                meta.insert(META_SCHEMA_VERSION, 1).unwrap();
                meta.insert(META_NEXT_ENTITY_ID, 2).unwrap();
            }
            txn.commit().unwrap();
        }
        let store = Store::open_or_create(&cfg).unwrap();
        // 两级升级：1→2→3，ec:group 和 kv:log 表都存在
        store.close().unwrap();
        {
            let db = Database::open(&cfg.path).unwrap();
            let read = db.begin_read().unwrap();
            let meta = read.open_table(META).unwrap();
            assert_eq!(
                meta.get(META_SCHEMA_VERSION).unwrap().unwrap().value(),
                SCHEMA_VERSION
            );
            // 两表都可开
            assert!(read.open_table(EC_GROUP).is_ok());
            assert!(read.open_table(KV_LOG).is_ok());
        }
    }

    // ---------- T22 max_log_seq（T3 seq allocator 初始化） ----------

    #[test]
    fn t22_max_log_seq_empty_and_after_append() {
        let dir = tempdir().unwrap();
        let cfg = cfg_in(dir.path());
        let store = Store::open_or_create(&cfg).unwrap();
        // 空表 → 0
        assert_eq!(store.max_log_seq().unwrap(), 0, "empty table → 0");
        // 追加 3 条 → max = 2
        let batch: Vec<_> = (0..3)
            .map(|i| sample_log(i, &format!("msg-{i}"), 3, "test"))
            .collect();
        store.append_logs(&batch).unwrap();
        assert_eq!(store.max_log_seq().unwrap(), 2, "max after seq 0..2");
        // 追加 seq 100..102 → max = 101
        let batch2: Vec<_> = (100..102)
            .map(|i| sample_log(i, &format!("msg-{i}"), 3, "test"))
            .collect();
        store.append_logs(&batch2).unwrap();
        assert_eq!(store.max_log_seq().unwrap(), 101, "max after seq 100..101");
        store.close().unwrap();
    }
}

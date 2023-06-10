use anyhow::{anyhow, Result};
use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{params, Connection, Row};
use serde_derive::{Deserialize, Serialize};
use std::convert::TryFrom;
use std::ops::Deref;
use std::sync::Mutex;

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct UserInfo {
    pub id: i64,
    pub name: String,
    pub face_url: String,
    pub live_room_url: Option<String>,
    pub live_room_title: Option<String>,
    pub live_open: Option<bool>,
    pub live_entropy: Option<i64>,
}

#[derive(Clone)]
pub struct UserSync {
    pub id: i64,
    pub enable: bool,
    pub ctime: DateTime<Utc>,
    pub ctimestamp: i64,
    pub new_video_ts: i64,
    pub new_video_title: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct VideoInfo {
    pub vid: String,
    pub title: String,
    pub pic_url: String,
    pub utime: DateTime<Utc>,
}

#[derive(Debug)]
pub struct VideoOwner {
    pub uid: i64,
    pub vid: String,
    pub timestamp: i64,
}

#[derive(Debug)]
pub struct UserFilter {
    pub uid: i64,
    pub fid: i64,
    pub priority: i64,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct FilterMeta {
    pub fid: i64,
    pub name: String,
}

impl Default for VideoInfo {
    fn default() -> Self {
        Self {
            vid: Default::default(),
            title: Default::default(),
            pic_url: Default::default(),
            utime: Utc::now(),
        }
    }
}

pub trait FromRow: Sized {
    fn from_row(row: &Row) -> rusqlite::Result<Self>;
}

impl FromRow for UserInfo {
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            name: row.get(1)?,
            face_url: row.get(2)?,
            live_room_url: row.get(3)?,
            live_room_title: row.get(4)?,
            live_open: row.get(5)?,
            live_entropy: row.get(6)?,
        })
    }
}

impl FromRow for UserSync {
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            enable: row.get(1)?,
            ctime: row.get(2)?,
            ctimestamp: row.get(3)?,
            new_video_ts: row.get(4)?,
            new_video_title: row.get(5)?,
        })
    }
}

impl FromRow for VideoInfo {
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            vid: row.get(0)?,
            title: row.get(1)?,
            pic_url: row.get(2)?,
            utime: row.get(3)?,
        })
    }
}

impl FromRow for VideoOwner {
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            uid: row.get(0)?,
            vid: row.get(1)?,
            timestamp: row.get(2)?,
        })
    }
}

impl FromRow for UserFilter {
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            uid: row.get(0)?,
            fid: row.get(1)?,
            priority: row.get(2)?,
        })
    }
}

impl FromRow for FilterMeta {
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            fid: row.get(0)?,
            name: row.get(1)?,
        })
    }
}

impl TryFrom<serde_json::Value> for UserInfo {
    type Error = anyhow::Error;

    fn try_from(v: serde_json::Value) -> Result<Self, Self::Error> {
        Ok(Self {
            id: v["mid"].as_i64().ok_or(anyhow!("mid not found"))?,
            name: v["name"]
                .as_str()
                .map(ToString::to_string)
                .ok_or(anyhow!("name not found"))?,
            face_url: v["face"]
                .as_str()
                .map(ToString::to_string)
                .ok_or(anyhow!("face not found"))?,
            live_room_url: v["live_room"]["url"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(ToString::to_string),
            live_room_title: v["live_room"]["title"].as_str().map(ToString::to_string),
            live_open: v["live_room"]["liveStatus"].as_i64().map(|s| s != 0),
            live_entropy: v["live_room"]["watched_show"]["num"].as_i64(),
        })
    }
}

pub struct VideoVector(Vec<VideoInfo>);

impl Deref for VideoVector {
    type Target = Vec<VideoInfo>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl TryFrom<serde_json::Value> for VideoVector {
    type Error = anyhow::Error;
    fn try_from(v: serde_json::Value) -> Result<Self, Self::Error> {
        let mut r = Vec::new();
        if let Some(a) = v["list"]["vlist"].as_array() {
            for (i, v) in a.iter().enumerate() {
                r.push(VideoInfo {
                    vid: v["bvid"]
                        .as_str()
                        .map(ToString::to_string)
                        .ok_or_else(|| anyhow!("list.vlist.{}.bvid not found", i))?,
                    title: v["title"]
                        .as_str()
                        .map(ToString::to_string)
                        .ok_or_else(|| anyhow!("list.vlist.{}.title not found", i))?,
                    pic_url: v["pic"]
                        .as_str()
                        .map(ToString::to_string)
                        .ok_or_else(|| anyhow!("list.vlist.{}.pic not found", i))?,
                    utime: v["created"]
                        .as_i64()
                        .map(|x| Utc.timestamp_opt(x, 0))
                        .ok_or_else(|| anyhow!("list.vlist.{}.created not found", i))?
                        .latest()
                        .unwrap_or(chrono::DateTime::<Utc>::MIN_UTC),
                })
            }
        }
        Ok(VideoVector(r))
    }
}

lazy_static::lazy_static! {
    static ref DBCON: Mutex<Option<Connection>> = {
        let path = "./.cache/cache.db3";
        let db = match Connection::open(&path) {
            Ok(r) => r,
            Err(e) => {
                log::error!("Open database error(s): {}", e);
                ::std::process::exit(1);
            }
        };
        let db_init_cmds = include_str!("../assets/db_init.sql");
        let create_result = db.execute_batch(db_init_cmds);
        match create_result {
            Ok(_) => log::info!("Database tables created!"),
            Err(e) => log::warn!("Database tables creation error(s): {}", e),
        }
        Mutex::new(Some(db))
    };
}

macro_rules! conn_db {
    ($name:ident, $mtxdb:ident) => {
        let _guard = $mtxdb
            .lock()
            .unwrap_or_else(|e| panic!("Database access error(s): {}", e));
        let $name = _guard
            .as_ref()
            .expect("Require db connection after shutdown");
    };
    ($name:ident) => {
        conn_db!($name, DBCON);
    };
}

pub fn blocking_shutdown() {
    log::info!("blocking_shutdown");
    let mut db = DBCON
        .lock()
        .unwrap_or_else(|e| panic!("Database access error(s): {}", e))
        .take()
        .expect("Initilizate DBCON failure or double shutdown");
    for _ in 0..6 {
        if let Err((con, e)) = db.close() {
            db = con;
            log::error!("Close db connection error(s): {}", e);
        } else {
            break;
        }
    }
}

#[derive(Debug)]
pub enum Order {
    Rowid,
    LatestVideo,
    LiveEntropy,
}

impl From<&str> for Order {
    fn from(s: &str) -> Self {
        match s {
            "video" => Self::LatestVideo,
            "live" => Self::LiveEntropy,
            _ => Self::Rowid,
        }
    }
}

#[derive(Clone)]
pub struct User {
    uid: i64,
}

type DbType<'a> = &'a rusqlite::Connection;

impl User {
    pub fn new(uid: i64) -> Self {
        Self { uid }
    }

    pub fn info(&self) -> Result<UserInfo> {
        conn_db!(db);
        self.db_info(db)
    }

    fn db_info(&self, db: DbType) -> Result<UserInfo> {
        Ok(db.query_row(
            "SELECT * FROM userinfo WHERE id=?1",
            params![self.uid],
            UserInfo::from_row,
        )?)
    }

    pub fn set_info(&self, info: &UserInfo) {
        if info.id != self.uid {
            log::error!("BUG: user {} set_info with info id {}", self.uid, info.id);
            return;
        }
        conn_db!(db);
        self.db_set_info(db, info);
    }

    fn db_set_info(&self, db: DbType, info: &UserInfo) {
        db.execute(
            "REPLACE INTO userinfo VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                info.id,
                info.name,
                info.face_url,
                info.live_room_url,
                info.live_room_title,
                info.live_open,
                info.live_entropy,
            ],
        )
        .map_err(|e| log::warn!("Replace into userinfo error(s): {}", e))
        .ok();
        self.db_upd_ctime(db, info.id);
    }

    pub fn force_upd_ctime(&self) {
        conn_db!(db);
        self.db_upd_ctime(db, self.id());
    }

    fn db_upd_ctime(&self, db: DbType, id: i64) {
        let now = Utc::now();
        db.execute(
            "UPDATE usersync SET ctime=?2, ctimestamp=?3 WHERE id=?1",
            params![id, now, now.timestamp(),],
        )
        .map_err(|e| log::warn!("Update usersync error(s): {}", e))
        .ok();
    }

    pub fn get_sync(&self) -> Result<UserSync> {
        conn_db!(db);
        self.db_get_sync(db)
    }

    fn db_get_sync(&self, db: DbType) -> Result<UserSync> {
        Ok(db.query_row(
            "SELECT * FROM usersync WHERE id=?1",
            params![self.uid],
            UserSync::from_row,
        )?)
    }

    pub fn recent_videos(&self, limit: i32) -> Result<Vec<VideoInfo>> {
        conn_db!(db);
        self.db_recent_videos(db, limit)
    }

    fn db_recent_videos(&self, db: DbType, limit: i32) -> Result<Vec<VideoInfo>> {
        let mut stmt = db.prepare_cached(
            "SELECT * FROM videoowner \
            WHERE uid=?1 \
            ORDER BY timestamp DESC \
            LIMIT ?2",
        )?;
        let iter = stmt.query_map(params![self.uid, limit], VideoOwner::from_row)?;
        let mut stmt = db.prepare_cached(
            "SELECT * FROM videoinfo \
            WHERE vid=?1",
        )?;
        Ok(iter
            .filter_map(|o| {
                o.and_then(|o| {
                    Ok(stmt
                        .query_map(params![o.vid], VideoInfo::from_row)?
                        .filter_map(|r| {
                            r.map_err(|e| log::warn!("Parse database video info error(s): {}", e))
                                .ok()
                        })
                        .collect::<Vec<VideoInfo>>())
                })
                .map_err(|e| log::warn!("Select video info error(s): {}", e))
                .ok()
            })
            .flatten()
            .collect())
    }

    pub fn update_videos<'a>(&self, videos: impl Iterator<Item = &'a VideoInfo>) {
        conn_db!(db);
        for v in videos {
            self.db_update_video(db, v);
        }
    }

    fn db_update_video(&self, db: DbType, info: &VideoInfo) {
        db.execute(
            "INSERT OR IGNORE INTO videoinfo VALUES \
            (?1, ?2, ?3, ?4)",
            params![info.vid, info.title, info.pic_url, info.utime],
        )
        .map_err(|e| log::warn!("Insert or ignore into videoinfo error(s): {}", e))
        .ok();
        db.execute(
            "INSERT OR IGNORE INTO videoowner VALUES \
            (?1, ?2, ?3)",
            params![self.uid, info.vid, info.utime.timestamp()],
        )
        .map_err(|e| log::warn!("Insert or ignore into videoowner error(s): {}", e))
        .ok();
        db.execute(
            "UPDATE usersync SET new_video_ts=?2, new_video_title=?3 WHERE id=?1 AND new_video_ts < ?2",
            params![self.uid, info.utime.timestamp(), info.title],
        )
        .map_err(|e| log::warn!("Update userinfo error(s): {}", e))
        .ok();
    }

    pub fn id(&self) -> i64 {
        self.uid
    }

    pub fn oldest_ctime_user() -> Result<Self> {
        conn_db!(db);
        let uid = Self::db_oldest_ctime_user(db)?;
        Ok(Self { uid })
    }

    fn db_oldest_ctime_user(db: DbType) -> Result<i64> {
        Ok(db.query_row(
            "SELECT id FROM usersync WHERE enable=1 ORDER BY ctimestamp ASC LIMIT 1",
            [],
            |row| row.get(0),
        )?)
    }

    pub fn enable(&self, b: bool) {
        conn_db!(db);
        self.db_disable(db, b);
    }

    fn db_disable(&self, db: DbType, b: bool) {
        let z = DateTime::<Utc>::MIN_UTC;
        db.execute(
            "REPLACE INTO usersync VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![self.uid, b, z, z.timestamp(), z.timestamp(), ""],
        )
        .map_err(|e| {
            log::error!(
                "Update usersync uid {} enable flag {} error(s): {}",
                self.uid,
                b,
                e
            )
        })
        .ok();
    }

    pub fn list(fid: i64, order: Order, start: i64, len: i64) -> Result<Vec<i64>> {
        conn_db!(db);
        if fid <= 0 {
            Self::db_list(db, order, start, len)
        } else {
            Self::db_filter_list(db, fid, order, start, len)
        }
    }

    fn db_list(db: DbType, order: Order, start: i64, len: i64) -> Result<Vec<i64>> {
        let mut stmt = db.prepare_cached(match order {
            Order::Rowid => {
                "SELECT id FROM usersync \
                    WHERE enable=1 \
                    ORDER BY rowid DESC LIMIT ?2 OFFSET ?1"
            }
            Order::LatestVideo => {
                "SELECT id FROM usersync \
                    WHERE enable=1 \
                    ORDER BY new_video_ts DESC LIMIT ?2 OFFSET ?1"
            }
            Order::LiveEntropy => {
                "SELECT userinfo.id FROM userinfo \
                    INNER JOIN usersync ON usersync.id=userinfo.id \
                    WHERE live_open=1 and enable=1 \
                    ORDER BY live_entropy DESC LIMIT ?2 OFFSET ?1"
            }
        })?;
        let iter = stmt.query_map(params![start, len], |row| row.get(0))?;
        Ok(iter.filter_map(|id| id.ok()).collect())
    }

    /// Modify user's priority in filter _fid_ . Priority of non-positive is equal to delete.
    pub fn mod_filter(&self, fid: i64, priority: i64) {
        conn_db!(db);
        self.db_mod_filter(db, fid, priority);
    }

    fn db_mod_filter(&self, db: DbType, fid: i64, priority: i64) {
        if priority > 0 {
            db.execute(
                "REPLACE INTO userfilters VALUES (?1, ?2, ?3)",
                params![self.uid, fid, priority,],
            )
            .map_err(|e| log::warn!("Replace into userfilters error(s): {}", e))
            .ok();
        } else {
            db.execute(
                "DELETE FROM userfilters WHERE uid=?1 and fid=?2",
                params![self.uid, fid],
            )
            .map_err(|e| log::warn!("Delete from userfilters error(s): {}", e))
            .ok();
        }
    }

    pub fn filter_list(fid: i64, start: i64, len: i64) -> Result<Vec<i64>> {
        conn_db!(db);
        Self::db_filter_list(db, fid, Order::Rowid, start, len)
    }

    fn db_filter_list(
        db: DbType,
        fid: i64,
        order: Order,
        start: i64,
        len: i64,
    ) -> Result<Vec<i64>> {
        let mut stmt = db.prepare_cached(match order {
            Order::Rowid => {
                "SELECT id FROM usersync \
                    INNER JOIN userfilters ON userfilters.uid=usersync.id and fid=?1 \
                    WHERE enable=1 \
                    ORDER BY priority DESC LIMIT ?3 OFFSET ?2"
            }
            Order::LatestVideo => {
                "SELECT id FROM usersync \
                    INNER JOIN userfilters ON userfilters.uid=usersync.id and fid=?1 \
                    WHERE enable=1 \
                    ORDER BY new_video_ts DESC LIMIT ?3 OFFSET ?2"
            }
            Order::LiveEntropy => {
                "SELECT usersync.id FROM usersync \
                    INNER JOIN userfilters ON userfilters.uid=usersync.id and fid=?1 \
                    INNER JOIN userinfo ON userinfo.id=usersync.id \
                    WHERE live_open=1 and enable=1 \
                    ORDER BY live_entropy DESC LIMIT ?3 OFFSET ?2"
            }
        })?;
        let iter = stmt.query_map(params![fid, start, len], |row| row.get(0))?;
        Ok(iter.filter_map(|id| id.ok()).collect())
    }
}

impl FilterMeta {
    pub fn new<T: ToString>(name: T) -> Result<Self> {
        conn_db!(db);
        db.execute(
            "INSERT INTO filtermeta (name) VALUES (?1)",
            params![name.to_string()],
        )?;
        Ok(Self {
            fid: db.query_row(
                "SELECT fid FROM filtermeta WHERE name=?1 ORDER BY fid DESC LIMIT 1",
                params![name.to_string()],
                |row| row.get(0),
            )?,
            name: name.to_string(),
        })
    }

    pub fn all() -> Result<Vec<Self>> {
        conn_db!(db);
        let mut stmt = db.prepare_cached("SELECT * FROM filtermeta ORDER BY fid ASC")?;
        let iter = stmt.query_map([], FilterMeta::from_row)?;
        Ok(iter.filter_map(|o| o.ok()).collect())
    }
}

impl TryFrom<i64> for FilterMeta {
    type Error = anyhow::Error;

    fn try_from(fid: i64) -> Result<Self> {
        conn_db!(db);
        Ok(Self {
            fid,
            name: db.query_row(
                "SELECT name FROM filtermeta WHERE fid=?1",
                params![fid],
                |row| row.get(0),
            )?,
        })
    }
}

impl std::fmt::Display for User {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.uid.fmt(f)
    }
}

use crate::Result;
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

impl TryFrom<serde_json::Value> for UserInfo {
    type Error = &'static str;

    fn try_from(v: serde_json::Value) -> std::result::Result<Self, Self::Error> {
        Ok(Self {
            id: v["mid"].as_i64().ok_or("mid not found")?,
            name: v["name"]
                .as_str()
                .map(ToString::to_string)
                .ok_or("name not found")?,
            face_url: v["face"]
                .as_str()
                .map(ToString::to_string)
                .ok_or("face not found")?,
            live_room_url: v["live_room"]["url"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(ToString::to_string),
            live_room_title: v["live_room"]["title"].as_str().map(ToString::to_string),
            live_open: v["live_room"]["liveStatus"].as_i64().map(|s| s != 0),
            live_entropy: v["live_room"]["online"].as_i64(),
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
    type Error = String;

    fn try_from(v: serde_json::Value) -> std::result::Result<Self, Self::Error> {
        let mut r = Vec::new();
        if let Some(a) = v["list"]["vlist"].as_array() {
            for (i, v) in a.iter().enumerate() {
                r.push(VideoInfo {
                    vid: v["bvid"]
                        .as_str()
                        .map(ToString::to_string)
                        .ok_or_else(|| format!("list.vlist.{}.bvid not found", i))?,
                    title: v["title"]
                        .as_str()
                        .map(ToString::to_string)
                        .ok_or_else(|| format!("list.vlist.{}.title not found", i))?,
                    pic_url: v["pic"]
                        .as_str()
                        .map(ToString::to_string)
                        .ok_or_else(|| format!("list.vlist.{}.pic not found", i))?,
                    utime: v["created"]
                        .as_i64()
                        .map(|x| Utc.timestamp(x, 0))
                        .ok_or_else(|| format!("list.vlist.{}.created not found", i))?,
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
        let create_result = db.execute_batch("BEGIN;
                          CREATE TABLE IF NOT EXISTS userinfo(
                                                id INTEGER PRIMARY KEY, \
                                                name TEXT NOT NULL, \
                                                face_url TEXT NOT NULL, \
                                                live_room_url TEXT, \
                                                live_room_title TEXT, \
                                                live_open INTEGER, \
                                                live_entropy INTEGER);
                          CREATE TABLE IF NOT EXISTS usersync(\
                                                id INTEGER NOT NULL UNIQUE, \
                                                enable INTEGER NOT NULL DEFAULT 1, \
                                                ctime TEXT NOT NULL, \
                                                ctimestamp INTEGER NOT NULL DEFAULT 0, \
                                                new_video_ts INTEGER NOT NULL DEFAULT 0, \
                                                new_video_title TEXT NOT NULL);
                          CREATE TABLE IF NOT EXISTS videoinfo(\
                                                 vid TEXT PRIMARY KEY, \
                                                 title TEXT NOT NULL, \
                                                 pic_url TEXT NOT NULL, \
                                                 utime TEXT NOT NULL);
                          CREATE TABLE IF NOT EXISTS videoowner(\
                                                  uid INTEGER NOT NULL, \
                                                  vid TEXT NOT NULL, \
                                                  timestamp INTEGER NOT NULL, \
                                                  UNIQUE(vid, uid));
                          COMMIT;");
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
        let now = Utc::now();
        db.execute(
            "UPDATE usersync SET ctime=?2, ctimestamp=?3 WHERE id=?1",
            params![info.id, now, now.timestamp(),],
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
        let z = Utc.timestamp(0, 0);
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

    pub fn list(order: Order, start: i64, len: i64) -> Result<Vec<i64>> {
        conn_db!(db);
        Self::db_list(db, order, start, len)
    }

    fn db_list(db: DbType, order: Order, start: i64, len: i64) -> Result<Vec<i64>> {
        let mut stmt = match order {
            Order::Rowid => db.prepare_cached("SELECT id FROM usersync ORDER BY rowid DESC LIMIT ?2 OFFSET ?1"),
            Order::LatestVideo => db.prepare_cached("SELECT id FROM usersync ORDER BY new_video_ts DESC LIMIT ?2 OFFSET ?1"),
            Order::LiveEntropy => db.prepare_cached("SELECT id FROM userinfo WHERE live_open=1 ORDER BY live_entropy DESC LIMIT ?2 OFFSET ?1"),
        }?;
        let iter = stmt.query_map(params![start, len], |row| row.get(0))?;
        Ok(iter.filter_map(|id| id.ok()).collect())
    }
}

impl std::fmt::Display for User {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.uid.fmt(f)
    }
}

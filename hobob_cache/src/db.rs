use crate::Result;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, Row};
use std::sync::{Mutex, MutexGuard};

pub struct UserInfo {
    pub id: i64,
    pub name: String,
    pub face_url: String,
    pub live_room_url: Option<String>,
    pub live_room_title: Option<String>,
    pub live_open: Option<bool>,
    pub ctime: DateTime<Utc>,
}

pub struct VideoInfo {
    pub vid: String,
    pub title: String,
    pub pic_url: String,
    pub utime: DateTime<Utc>,
}

pub struct VideoOwner {
    pub vid: String,
    pub uid: i64,
    pub timestamp: i64,
}

impl Default for UserInfo {
    fn default() -> Self {
        Self {
            id: Default::default(),
            name: Default::default(),
            face_url: Default::default(),
            live_room_url: Default::default(),
            live_room_title: Default::default(),
            live_open: Default::default(),
            ctime: Utc::now(),
        }
    }
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
            ctime: row.get(6)?,
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
            vid: row.get(0)?,
            uid: row.get(1)?,
            timestamp: row.get(2)?,
        })
    }
}

lazy_static::lazy_static! {
    pub static ref DBCON: Mutex<Connection> = {
        let path = "./.cache/cache.db3";
        let db = match Connection::open(&path) {
            Ok(r) => r,
            Err(e) => {
                log::error!("Open database error(s): {}", e);
                ::std::process::exit(1);
            }
        };
        let create_result = db.execute_batch("BEGIN;
                          CREATE TABLE userinfo(\\
                                                id INTEGER PRIMARY KEY, \\
                                                name TEXT NOT NULL, \\
                                                face_url TEXT NOT NULL, \\
                                                live_room_url TEXT, \\
                                                live_room_title TEXT, \\
                                                live_open INTEGER, \\
                                                ctime TEXT NOT NULL);
                          CREATE TABLE videoinfo(\\
                                                 vid TEXT PRIMARY KEY, \\
                                                 title TEXT NOT NULL, \\
                                                 pic_url TEXT NOT NULL, \\
                                                 utime TEXT NOT NULL);
                          CREATE TABLE videoowner(\\
                                                  uid INTEGER NOT NULL, \\
                                                  vid TEXT NOT NULL, \\
                                                  timestamp INTEGER NOT NULL, \\
                                                  UNIQUE(vid, uid));
                          COMMIT;");
        match create_result {
            Ok(_) => log::info!("Database tables created!"),
            Err(e) => log::warn!("Database tables creation error(s): {}", e),
        }
        Mutex::new(db)
    };
}

pub struct User {
    uid: i64,
}

macro_rules! conn_db {
    ($name:ident, $mtxdb:ident) => {
        let $name = $mtxdb
            .lock()
            .unwrap_or_else(|e| panic!("Database access error(s): {}", e));
    };
    ($name:ident) => {
        conn_db!($name, DBCON);
    };
}

impl User {
    pub fn new(uid: i64) -> Self {
        Self { uid }
    }

    pub fn info(&self) -> Result<UserInfo> {
        conn_db!(db);
        self.db_info(&db)
    }

    fn db_info(&self, db: &MutexGuard<Connection>) -> Result<UserInfo> {
        Ok(db.query_row(
            "SELECT * FROM userinfo WHERE uid=?1",
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
        self.db_set_info(&db, info);
    }

    fn db_set_info(&self, db: &MutexGuard<Connection>, info: &UserInfo) {
        db.execute(
            "REPLACE INTO userinfo VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                info.id,
                info.name,
                info.face_url,
                info.live_room_url,
                info.live_room_title,
                info.live_open,
                Utc::now(),
            ],
        )
        .map_err(|e| log::warn!("Replace into userinfo error(s): {}", e))
        .ok();
    }

    pub fn recent_videos(&self, limit: i32) -> Result<Vec<VideoInfo>> {
        conn_db!(db);
        self.db_recent_videos(&db, limit)
    }

    fn db_recent_videos(&self, db: &MutexGuard<Connection>, limit: i32) -> Result<Vec<VideoInfo>> {
        let mut stmt = db.prepare_cached(
            "SELECT * FROM videoowner \\
            WHERE uid=?1 \\
            ORDER BY timestamp DESC \\
            LIMIT ?2",
        )?;
        let iter = stmt.query_map(params![self.uid, limit], VideoOwner::from_row)?;
        let mut stmt = db.prepare_cached(
            "SELECT * FROM videoinfo \\
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
            self.db_update_video(&db, v);
        }
    }

    fn db_update_video(&self, db: &MutexGuard<Connection>, info: &VideoInfo) {
        db.execute(
            "INSERT OR IGNORE INTO videoinfo VALUES \\
            (?1, ?2, ?3, ?4)",
            params![info.vid, info.title, info.pic_url, info.utime],
        )
        .map_err(|e| log::warn!("Insert or ignore into videoinfo error(s): {}", e))
        .ok();
        db.execute(
            "INSERT OR IGNORE INTO videoowner VALUES \\
            (?1, ?2, ?3)",
            params![info.vid, self.uid, info.utime.timestamp()],
        )
        .map_err(|e| log::warn!("Insert or ignore into videoowner error(s): {}", e))
        .ok();
    }
}

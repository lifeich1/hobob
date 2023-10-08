BEGIN;

CREATE TABLE IF NOT EXISTS userinfo(
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    face_url TEXT NOT NULL,
    live_room_url TEXT,
    live_room_title TEXT,
    live_open INTEGER,
    live_entropy INTEGER);
CREATE TABLE IF NOT EXISTS usersync(
    id INTEGER NOT NULL UNIQUE,
    enable INTEGER NOT NULL DEFAULT 1,
    ctime TEXT NOT NULL,
    ctimestamp INTEGER NOT NULL DEFAULT 0,
    new_video_ts INTEGER NOT NULL DEFAULT 0,
    new_video_title TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS videoinfo(
    vid TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    pic_url TEXT NOT NULL,
    utime TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS videoowner(
    uid INTEGER NOT NULL,
    vid TEXT NOT NULL,
    timestamp INTEGER NOT NULL,
    UNIQUE(vid, uid));
CREATE TABLE IF NOT EXISTS userfilters(
    uid INTEGER NOT NULL,
    fid INTEGER NOT NULL,
    priority INTEGER NOT NULL,
    UNIQUE(uid, fid));
CREATE TABLE IF NOT EXISTS filtermeta(
    fid INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL);

INSERT OR IGNORE INTO filtermeta VALUES (0,"全部\");
INSERT OR IGNORE INTO filtermeta VALUES (1,"特别关注\");

COMMIT;

use crate::{
    db,
    Result,
    engine::{self, Command},
};
use serde_derive::{Deserialize, Serialize};
use tera::{Context as TeraContext, Tera};
use warp::{http::StatusCode, Filter};
use chrono::{Utc, TimeZone};
use std::convert::From;

lazy_static::lazy_static! {
    pub static ref TEMPLATES: Tera = {
        let tera = match Tera::new("templates/**/*.html") {
            Ok(t) => t,
            Err(e) => {
                log::error!("Parsing error(s): {}", e);
                ::std::process::exit(1);
            }
        };
        tera
    };
}

macro_rules! render {
    (@errhtml $kind:expr, $reason:expr) => {
        warp::reply::html(render!(@err TEMPLATES, $kind, $reason))
    };
    (@err $tera:ident, $kind:expr, $reason:expr) => {
        {
            let mut ctx = TeraContext::new();
            ctx.insert("kind", $kind);
            ctx.insert("reason", $reason);
            $tera.render("failure.html", &ctx).unwrap()
        }
    };

    ($name:expr, $ctx:expr) => {
        render!(TEMPLATES, $name, $ctx)
    };
    ($tera:ident, $name:expr, $ctx:expr) => {
        warp::reply::html($tera.render($name, $ctx).unwrap_or_else(|e|
            render!(@err $tera, "Tera engine", &format!("Error: tera: {}", e))
        ))
    };
}

macro_rules! jsnapi {
    ($expr:expr) => {{
        tokio::spawn(async move {
            $expr;
        });
        warp::reply::json(&String::from("success"))
    }};
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct FollowOptions {
    enable: bool,
    uid: i64,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct RefreshOptions {
    uid: i64,
}

#[derive(Serialize, Debug)]
struct OpMessage {
    code: u16,
    message: String,
}

impl OpMessage {
    pub fn ok() -> Self {
        Self {
            code: 200,
            message: String::from("Success"),
        }
    }
}

impl warp::reject::Reject for OpMessage {}

macro_rules! req_type {
    (@post) => {
        warp::post()
            .and(warp::body::content_length_limit(1024 * 16))
            .and(warp::body::json())
    };
}


macro_rules! reply_json_result {
    (@err $e:expr, $c:expr) => {
        warp::reply::with_status(warp::reply::json(&format!("Err: {}", $e)), $c)
    };

    ($expr:expr) => {
        match $expr {
            Ok(ok) => warp::reply::with_status(warp::reply::json(&ok), StatusCode::OK),
            Err(e) => reply_json_result!(@err e, StatusCode::INTERNAL_SERVER_ERROR),
        }
    };
}

#[derive(Debug, Serialize)]
struct UserExt {
    card_id: String,
    space_link: String,
    live_link: String,
    live_link_cls: String,
    live_open: bool,
    new_video_ts: i64,
    new_video_title: String,
    new_video_tsrepr: String,
    live_entropy: String,
}

#[derive(Debug, Serialize)]
struct UserPack {
    data: db::UserInfo,
    ext: UserExt,
}

impl From<Result<db::UserInfo>> for UserPack {
    fn from(data: Result<db::UserInfo>) -> Self {
        match data {
            Ok(data) => {
                let sync = db::User::new(data.id).get_sync();
                Self {
                    ext: UserExt {
                        card_id: format!("user-card-{}", data.id),
                        space_link: format!("https://space.bilibili.com/{}/", data.id),
                        live_link: data.live_room_url.clone().unwrap_or_else(|| String::from("https://live.bilibili.com/")),
                        live_open: matches!(data.live_open, Some(true)),
                        live_link_cls: String::from(if matches!(data.live_open, Some(true)) {
                            "btn btn-success"
                        } else {
                            "btn btn-secondary"
                        }),
                        new_video_ts: sync.as_ref().map(|s| s.new_video_ts).unwrap_or(0),
                        new_video_title: sync.as_ref().map(|s| s.new_video_title.clone()).unwrap_or_else(|_| String::default()),
                        new_video_tsrepr: sync.map(
                            |s| Utc.timestamp(s.new_video_ts, 0)
                                .with_timezone(&chrono::Local)
                                .format("%Y-%m-%d %H:%M:%S")
                                .to_string()
                            ).unwrap_or_else(|_| String::default()),
                        live_entropy: match data.live_entropy {
                            Some(e @ 0..=1000) => format!("{}", e),
                            Some(e @ 1001..=1000_000) => format!("{:.1}K", e as f32 / 1000f32),
                            Some(e) => format!("{:.1}M", e as f32 / 1000_000f32),
                            _ => String::from("0"),
                        },
                    },
                    data,
                }
            },
            Err(e) => Self {
                ext: UserExt {
                    card_id: String::from("user-card-0"),
                    space_link: String::from("https://www.bilibili.com/"),
                    live_link: String::from("https://live.bilibili.com/"),
                    live_open: false,
                    live_link_cls: String::from("btn btn-danger"),
                    new_video_ts: 0,
                    new_video_title: Default::default(),
                    new_video_tsrepr: Default::default(),
                    live_entropy: Default::default(),
                },
                data: db::UserInfo {
                    name: format!("Err: {}", e),
                    face_url: String::from("https://i0.hdslb.com/bfs/face/member/noface.jpg"),
                    ..Default::default()
                },
            }
        }
    }
}

pub async fn run() {
    let index = warp::path::end().map(|| render!("index.html", &TeraContext::new()));

    let evrx = engine::event_rx();

    let op_follow = warp::path!("follow")
        .and(req_type!(@post))
        .map(|opt: FollowOptions| {
            log::debug!("op_follow arg: {:?}", opt);
            jsnapi!(engine::handle()
                .send(Command::Follow(opt.enable, opt.uid))
                .await
                .ok())
        });
    let op_refresh = warp::path!("refresh")
        .and(req_type!(@post))
        .map(|opt: RefreshOptions| {
            jsnapi!(engine::handle().send(Command::Refresh(opt.uid)).await.ok())
        });
    let op = warp::path("op");

    let get_user = warp::path!("user" / i64)
        .map(|uid| {
            reply_json_result!( db::User::new(uid).info())
        });
    let get_vlist = warp::path!("vlist" / i64)
        .map(|uid| {
            reply_json_result!(db::User::new(uid).recent_videos(30))
        });
    let get = warp::path("get").and(warp::get());

    let list = warp::path!("list" / String / i64 / i64)
        .and(warp::get())
        .map(|typ: String, start, len| {
            reply_json_result!(db::User::list(typ.as_str().into(), start, len))
        });

    let card_ulist = warp::path!("ulist" / String / i64 / i64)
        .map(|typ: String, start, len| {
            let uids = db::User::list(typ.as_str().into(), start, len);
            if let Err(e) = uids {
                return render!(@errhtml "Database", &format!("Db error(s): {}", e));
            }
            let users: Vec<UserPack> = uids.unwrap()
                .iter()
                .map(|uid| {
                    UserPack::from(db::User::new(*uid).info())
                })
                .collect();
            let mut ctx = TeraContext::new();
            ctx.insert("users", &users);
            render!("user_cards.html", &ctx)
        });
    let card = warp::path("card");

    let static_files = warp::path("static")
        .and(warp::fs::dir("./static"));
    let favicon = warp::path!("favicon.ico")
        .and(warp::fs::file("./static/favicon.ico"));

    let app = index.or(op.and(op_follow)).or(op.and(op_refresh))
        .or(get.and(get_user)).or(get.and(get_vlist))
        .or(list)
        .or(static_files)
        .or(card.and(card_ulist))
        .or(favicon);
    log::info!("www running");
    warp::serve(app).run(([0, 0, 0, 0], 3731)).await;
}

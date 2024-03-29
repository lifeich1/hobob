use crate::{
    db,
    engine::{self, Command},
};
use anyhow::Result;
use chrono::{TimeZone, Utc};
use futures::StreamExt;
use serde_derive::{Deserialize, Serialize};
use std::convert::From;
use std::convert::Infallible;
use tera::{Context as TeraContext, Tera};
use tokio::sync::oneshot;
use tokio_stream::wrappers::WatchStream;
use warp::{http::StatusCode, sse::Event, Filter};

lazy_static::lazy_static! {
    pub static ref TEMPLATES: Tera = {
        match Tera::new("templates/**/*.html") {
            Ok(t) => t,
            Err(e) => {
                log::error!("Parsing error(s): {}", e);
                ::std::process::exit(1);
            }
        }
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

macro_rules! async_command {
    ($expr:expr) => {
        tokio::spawn(async move { engine::handle().send($expr).await.ok() })
    };
}

macro_rules! ulist_render {
    (@pack $packs:ident, $in_div:expr) => {{
        let mut ctx = TeraContext::new();
        ctx.insert("users", &$packs);
        ctx.insert("in_div", &$in_div);
        render!("user_cards.html", &ctx)
    }};

    ($uids:ident, $in_div:expr) => {{
        if let Err(e) = $uids {
            return render!(@errhtml "Database", &format!("Db error(s): {}", e));
        }
        let users: Vec<UserPack> = $uids
            .unwrap()
            .iter()
            .map(|uid| UserPack::from(db::User::new(*uid).info()))
            .collect();
        async_command!(Command::Activate);
        ulist_render!(@pack users, $in_div)
    }};
}

macro_rules! www_try {
    (@hdl $expr:expr, $err:ident, $errhdl:expr) => {
        match $expr {
            Ok(ok) => ok,
            Err($err) => return $errhdl,
        }
    };

    (@db $expr:expr) => {
        www_try!(@hdl $expr, e, render!(@errhtml "Database", &format!("Db error(s): {}", e)))
    };
}

macro_rules! jsnapi {
    (@ok) => {
        warp::reply::json(&String::from("success"))
    };

    (@err $why:expr) => {
        warp::reply::json(&$why)
    };

    (@try $expr:expr; $err:ident; $why:expr) => {
        match $expr {
            Ok(_) => jsnapi!(@ok),
            Err($err) => jsnapi!(@err $why),
        }
    };

    ($expr:expr) => {{
        tokio::spawn(async move {
            $expr;
        });
        jsnapi!(@ok)
    }};

    (@cmd $expr:expr) => {
        jsnapi!(engine::handle().send($expr).await.ok())
    };
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

#[derive(Debug, Deserialize, Serialize, Clone)]
struct ForceSilenceOptions {
    silence: bool,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct ModFilterOptions {
    uid: i64,
    fid: i64,
    priority: i64,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct NewFilterOptions {
    name: String,
}

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
    ctimestamp: i64,
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
                        live_link: data
                            .live_room_url
                            .clone()
                            .unwrap_or_else(|| String::from("https://live.bilibili.com/")),
                        live_open: matches!(data.live_open, Some(true)),
                        live_link_cls: String::from(if matches!(data.live_open, Some(true)) {
                            "btn btn-success"
                        } else {
                            "btn btn-secondary"
                        }),
                        new_video_ts: sync.as_ref().map(|s| s.new_video_ts).unwrap_or(0),
                        ctimestamp: sync.as_ref().map(|s| s.ctimestamp).unwrap_or(0),
                        new_video_title: sync
                            .as_ref()
                            .map(|s| s.new_video_title.clone())
                            .unwrap_or_else(|_| String::default()),
                        new_video_tsrepr: sync
                            .map(|s| {
                                Utc.timestamp_opt(s.new_video_ts, 0)
                                    .latest()
                                    .unwrap_or(chrono::DateTime::<Utc>::MIN_UTC)
                                    .with_timezone(&chrono::Local)
                                    .format("%Y-%m-%d %H:%M:%S")
                                    .to_string()
                            })
                            .unwrap_or_else(|_| String::default()),
                        live_entropy: match data.live_entropy {
                            Some(e @ 0..=1000) => format!("{}", e),
                            Some(e @ 1001..=1_000_000) => format!("{:.1}K", e as f32 / 1000f32),
                            Some(e) => format!("{:.1}M", e as f32 / 1_000_000_f32),
                            _ => String::from("0"),
                        },
                    },
                    data,
                }
            }
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
                    ctimestamp: 0,
                },
                data: db::UserInfo {
                    name: format!("Err: {}", e),
                    face_url: String::from("https://i0.hdslb.com/bfs/face/member/noface.jpg"),
                    ..Default::default()
                },
            },
        }
    }
}

#[derive(Debug, Serialize)]
struct IndexData {
    status: String,
}

impl IndexData {
    pub fn now() -> Self {
        Self {
            status: engine::event_rx().borrow().status.to_string(),
        }
    }
}

fn sse_ev_engine(e: engine::Event) -> std::result::Result<Event, Infallible> {
    Ok(Event::default()
        .json_data(e)
        .expect("engine event json-stringify should never fail"))
}

pub async fn run(shutdown: oneshot::Receiver<i32>) {
    let _running = engine::will_shutdown();

    let index = warp::path::end().map(|| {
        let mut ctx = TeraContext::new();
        ctx.insert("data", &IndexData::now());
        render!("index.html", &ctx)
    });

    let op_follow = warp::path!("follow")
        .and(req_type!(@post))
        .map(|opt: FollowOptions| {
            log::debug!("op_follow arg: {:?}", opt);
            jsnapi!(@cmd Command::Follow(opt.enable, opt.uid))
        });
    let op_refresh = warp::path!("refresh")
        .and(req_type!(@post))
        .map(|opt: RefreshOptions| jsnapi!(@cmd Command::Refresh(opt.uid)));
    let op_silence = warp::path!("silence")
        .and(req_type!(@post))
        .map(|opt: ForceSilenceOptions| jsnapi!(@cmd Command::ForceSilence(opt.silence)));
    let op_mod_filter =
        warp::path!("mod" / "filter")
            .and(req_type!(@post))
            .map(|opt: ModFilterOptions| {
                db::User::new(opt.uid).mod_filter(opt.fid, opt.priority);
                jsnapi!(@ok)
            });
    let op_new_filter =
        warp::path!("new" / "filter")
            .and(req_type!(@post))
            .map(|opt: NewFilterOptions| {
                jsnapi!(@try db::FilterMeta::new(&opt.name); e; {
                    log::error!("new filter error(s): {}", e);
                    format!("Db error: {}", e)
                })
            });
    let op = warp::path("op");

    let get_user =
        warp::path!("user" / i64).map(|uid| reply_json_result!(db::User::new(uid).info()));
    let get_vlist = warp::path!("vlist" / i64)
        .map(|uid| reply_json_result!(db::User::new(uid).recent_videos(30)));
    let get_flist = warp::path!("flist").map(|| reply_json_result!(db::FilterMeta::all()));
    let get = warp::path("get").and(warp::get());

    let list = warp::path!("list" / i64 / String / i64 / i64)
        .and(warp::get())
        .map(|fid, typ: String, start, len| {
            reply_json_result!(db::User::list(fid, typ.as_str().into(), start, len))
        });

    let card_ulist =
        warp::path!("ulist" / i64 / String / i64 / i64).map(|fid, typ: String, start, len| {
            let uids = db::User::list(fid, typ.as_str().into(), start, len);
            ulist_render!(uids, true)
        });
    let card_one = warp::path!("one" / i64).map(|uid| {
        let users = vec![UserPack::from(db::User::new(uid).info())];
        ulist_render!(@pack users, false)
    });
    let card_filter_options = warp::path!("filter" / "options").map(|| {
        let filters = www_try!(@db db::FilterMeta::all());
        let mut ctx = TeraContext::new();
        ctx.insert("filters", &filters);
        render!("filter_options.html", &ctx)
    });
    let card = warp::path("card");

    let ev_engine = warp::path!("engine").map(|| {
        warp::sse::reply(
            warp::sse::keep_alive().stream(WatchStream::new(engine::event_rx()).map(sse_ev_engine)),
        )
    });
    let ev = warp::path("ev");

    let static_files = warp::path("static").and(warp::fs::dir("./static"));
    let favicon = warp::path!("favicon.ico").and(warp::fs::file("./static/favicon.ico"));

    let app = index
        .or(op.and(op_follow))
        .or(op.and(op_refresh))
        .or(op.and(op_silence))
        .or(op.and(op_mod_filter))
        .or(op.and(op_new_filter))
        .or(get.and(get_user))
        .or(get.and(get_vlist))
        .or(get.and(get_flist))
        .or(list)
        .or(static_files)
        .or(card.and(card_ulist))
        .or(card.and(card_one))
        .or(card.and(card_filter_options))
        .or(ev.and(ev_engine))
        .or(favicon);
    log::info!("www running");
    let (_, run) = warp::serve(app).bind_with_graceful_shutdown(([0, 0, 0, 0], 3731), async move {
        shutdown.await.ok();
    });
    run.await;
    log::info!("www stopped");
}

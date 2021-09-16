use crate::{
    db,
    engine::{self, Command},
};
use serde_derive::{Deserialize, Serialize};
use std::convert::Infallible;
use tera::{Context as TeraContext, Tera};
use warp::{http::StatusCode, reject::Rejection, reply::Reply, Filter};

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
    ($name:expr, $ctx:expr) => {
        render!(TEMPLATES, $name, $ctx)
    };
    ($tera:ident, $name:expr, $ctx:expr) => {
        warp::reply::html($tera.render($name, $ctx).unwrap_or_else(|e| {
            let mut ctx = TeraContext::new();
            ctx.insert("kind", "Tera engine");
            ctx.insert("reason", &format!("Error: tera: {}", e));
            $tera.render("failure.html", &ctx).unwrap()
        }))
    };
}

macro_rules! jsnapi {
    ($expr:expr) => {{
        tokio::spawn(async move {
            $expr;
        });
        StatusCode::OK
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

pub async fn run() {
    let index = warp::path::end().map(|| render!("index.html", &TeraContext::new()));

    let evrx = engine::event_rx();

    let op_follow = warp::path!("follow")
        .and(req_type!(@post))
        .map(|opt: FollowOptions| {
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
    let op = warp::path!("op");

    let app = index.or(op.and(op_follow)).or(op.and(op_refresh));
    log::info!("www running");
    warp::serve(app).run(([127, 0, 0, 1], 3000)).await;
}

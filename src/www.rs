use crate::db::{FullBench, WeiYuan};
use anyhow::Result;
use chrono::{TimeZone, Utc};
use futures::StreamExt;
use serde_derive::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::convert::From;
use std::convert::Infallible;
use tera::{Context, Tera};
use tokio::sync::oneshot;
use tokio_stream::wrappers::WatchStream;
use warp::{filters::BoxedFilter, http::StatusCode, path, reply, sse::Event, Filter};

lazy_static::lazy_static! {
    pub static ref TERA: Tera = {
        match Tera::new("templates/**/*.html") {
            Ok(t) => t,
            Err(e) => {
                log::error!("Parsing error(s): {}", e);
                ::std::process::exit(1);
            }
        }
    };
}

#[derive(Debug, Deserialize)]
struct UnparsableQuery();

impl warp::reject::Reject for UnparsableQuery {}

/*
fn sse_ev_engine(e: engine::Event) -> std::result::Result<Event, Infallible> {
    Ok(Event::default()
        .json_data(e)
        .expect("engine event json-stringify should never fail"))
}
*/

pub fn build_app(runner: WeiYuan) -> BoxedFilter<(impl warp::Reply,)> {
    fn render_fail<E: std::fmt::Display>(page: &str, e: E) -> String {
        let mut c = Context::new();
        c.insert("kind", "render process");
        c.insert(
            "reason",
            &format!("rendering {} get unexpected error: {}", page, e),
        );
        TERA.render("failure.html", &c)
            .expect("render failure.html MUST be safe")
    }
    fn render(page: &str, value: Result<Value>) -> String {
        value
            .and_then(|v| Ok(Context::from_value(v)?))
            .and_then(|c| Ok(TERA.render(page, &c)?))
            .unwrap_or_else(|e| render_fail(page, e))
    }
    fn log(mut hdl: WeiYuan, lv: i32, msg: String) -> WeiYuan {
        hdl.log(lv, msg);
        hdl
    }
    fn info(hdl: WeiYuan, msg: String) -> WeiYuan {
        log(hdl, 2, msg)
    }

    let index = {
        let hdl = runner.clone();
        path::end().map(move || {
            reply::html(render(
                "index.html",
                hdl.clone().recv().map(|v| {
                    v.runtime
                        .get("index")
                        .cloned()
                        .unwrap_or_else(|| json!({"status":"booting"}))
                }),
            ))
        })
    };

    fn simpleapi() -> BoxedFilter<(Value,)> {
        warp::post()
            .and(warp::body::content_length_limit(1024 * 16))
            .and(warp::body::bytes())
            .and_then(|bytes: bytes::Bytes| async move {
                serde_json::to_value(bytes.as_ref())
                    .map_err(|_| warp::reject::custom(UnparsableQuery()))
            })
            .boxed()
    }
    fn do_api<F>(hdl: &WeiYuan, name: &str, opt: Value, f: F) -> reply::Json
    where
        F: Fn(&mut FullBench, &Value) -> Result<()>,
    {
        let msg = format!("{} arg: {:?}", name, &opt);
        reply::json(
            &info(hdl.clone(), msg)
                .apply(|b| f(b, &opt))
                .map(|_| json!("success"))
                .unwrap_or_else(|e| json!({"err": e.to_string()})),
        )
    }
    fn create_op<F>(runner: &WeiYuan, name: &'static str, f: F) -> BoxedFilter<(impl reply::Reply,)>
    where
        F: Fn(&mut FullBench, &Value) -> Result<()>
            + std::marker::Sync
            + std::marker::Send
            + Copy
            + 'static,
    {
        let hdl = runner.clone();
        simpleapi()
            .map(move |opt: Value| do_api(&hdl, name, opt, f))
            .boxed()
    }

    let op_follow = warp::path!("follow").and(create_op(&runner, "follow", |b, opt| b.follow(opt)));
    let op_refresh =
        warp::path!("refresh").and(create_op(&runner, "refresh", |b, opt| b.refresh(opt)));
    let op_silence =
        warp::path!("silence").and(create_op(&runner, "silence", |b, opt| b.force_silence(opt)));
    let op_toggle_group =
        warp::path!("toggle" / "group").and(create_op(&runner, "toggle/group", |b, opt| {
            b.toggle_group(opt)
        }));
    let op_new_group =
        warp::path!("touch" / "group").and(create_op(&runner, "touch/group", |b, opt| {
            b.touch_group(opt)
        }));
    /*

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
    */
    let static_files = warp::path("static").and(warp::fs::dir("./static"));
    let favicon = warp::path!("favicon.ico").and(warp::fs::file("./static/favicon.ico"));

    let app = index
        .or(warp::path("op").and(
            op_follow
                .or(op_refresh)
                .or(op_silence)
                .or(op_toggle_group)
                .or(op_new_group),
        ))
        .or(static_files)
        .or(favicon);
    app.boxed()
    /*
    let (_, run) = warp::serve(app).bind_with_graceful_shutdown(([0, 0, 0, 0], 3731), async move {
        runner.clone().until_closing().await
    });
    run.await;
    log::info!("www stopped");
        */
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::WeiYuanHui;
    use chrono::Duration;
    use hyper::body::{to_bytes, Buf};
    use warp::reply::Reply;

    async fn resp_to_st(resp: warp::reply::Response) -> String {
        std::str::from_utf8(to_bytes(resp.into_body()).await.unwrap().chunk())
            .unwrap()
            .into()
    }

    async fn check_n_step(center: &mut WeiYuanHui) {
        assert!(matches!(
            center
                .run_for(Duration::milliseconds(20))
                .await
                .as_ref()
                .map_err(ToString::to_string),
            Ok(true)
        ));
    }

    fn bench(center: &mut WeiYuanHui) -> FullBench {
        center
            .new_chair()
            .recv()
            .cloned()
            .unwrap_or_else(|e| panic!("get bench err: {:?}", e))
    }

    #[tokio::test]
    async fn test_index() {
        let mut center = WeiYuanHui::default();
        let app = build_app(center.new_chair());
        let index = warp::test::request()
            .path("/")
            .filter(&app)
            .await
            .unwrap()
            .into_response();
        println!("index: {:?}", &index);
        assert_eq!(index.status(), StatusCode::OK);
        let s = resp_to_st(index).await;
        println!("body: {:?}", &s);
        assert!(!s.contains("render process failure"));
    }

    #[tokio::test]
    async fn test_op_follow() {
        let center = &mut WeiYuanHui::default();
        let app = build_app(center.new_chair());
        let resp = warp::test::request()
            .method("POST")
            .path("/op/follow")
            .json(&json!({
                "uid": 12345,
                "enable": true,
            }))
            .filter(&app)
            .await
            .unwrap()
            .into_response();
        assert_eq!(resp.status(), StatusCode::OK);
        let s = resp_to_st(resp).await;
        assert_eq!(s, "success");
        check_n_step(center).await;
        let b = bench(center);
        assert_eq!(b.commands.len(), 1);
        assert_eq!(
            b.commands.front(),
            Some(&json!({
                "cmd": "fetch",
                "args": { "uid": 12345, },
            }))
        );
        assert_eq!(b.up_info.len(), 1);
        assert_eq!(
            b.up_info.get("12345"),
            Some(&json!({
                "pick":{"basic":{"ban":false}}
            }))
        );
    }

    // TODO test other op
}

use crate::data_schema::ChairData;
use crate::db::{FullBench, WeiYuan};
use anyhow::{anyhow, Result};
use futures::StreamExt;
use serde_derive::Deserialize;
use serde_json::{json, Value};
use tera::{Context, Tera};
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

    let index = {
        let hdl = runner.readonly();
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
                let r = std::str::from_utf8(bytes.as_ref())
                    .ok()
                    .and_then(|s| serde_json::from_str(s).ok())
                    .ok_or_else(|| warp::reject::custom(UnparsableQuery()));
                log::trace!("simpleapi()#parsed: {:?}", &r);
                r
            })
            .boxed()
    }
    fn do_api<F>(hdl: &WeiYuan, opt: Value, f: F) -> reply::Json
    where
        F: Fn(&mut FullBench, &Value) -> Result<()>,
    {
        reply::json(
            &hdl.clone()
                .apply(|b| f(b, &opt))
                .map(|_| json!("success"))
                .unwrap_or_else(|e| json!({"err": e.to_string()})),
        )
    }
    fn create_op<F>(runner: &WeiYuan, f: F) -> BoxedFilter<(impl reply::Reply,)>
    where
        F: Fn(&mut FullBench, &Value) -> Result<()>
            + std::marker::Sync
            + std::marker::Send
            + Copy
            + 'static,
    {
        let hdl = runner.clone();
        simpleapi()
            .map(move |opt: Value| do_api(&hdl, opt, f))
            .boxed()
    }

    let op_follow = warp::path!("follow").and(create_op(&runner, |b, opt| b.follow(opt)));
    let op_refresh = warp::path!("refresh").and(create_op(&runner, |b, opt| b.refresh(opt)));
    let op_silence = warp::path!("silence").and(create_op(&runner, |b, opt| b.force_silence(opt)));
    let op_toggle_group =
        warp::path!("toggle" / "group").and(create_op(&runner, |b, opt| b.toggle_group(opt)));
    let op_new_group =
        warp::path!("touch" / "group").and(create_op(&runner, |b, opt| b.touch_group(opt)));

    let card_one = {
        let hdl = runner.readonly();
        warp::path!("one" / i64).map(move |uid: i64| {
            let r = hdl
                .clone()
                .recv()
                .and_then(|b| {
                    b.up_info
                        .get(&uid.to_string())
                        .ok_or_else(|| anyhow!("uid {} not found", uid))
                })
                .map(|v| json!({"users": [v["pick"]]}))
                .and_then(ChairData::checker(schema_uri!("user_cards")));
            reply::html(render("user_cards.html", r))
        })
    };

    let card_ulist = {
        let hdl = runner.readonly();
        warp::path!("ulist" / i64 / String / i64 / i64).map(move |gid, typ: String, start, len| {
            let r = hdl
                .clone()
                .recv()
                .and_then(|b| {
                    b.users_pick(&json!({
                        "gid": gid,
                        "order_desc": typ,
                        "range_start": start,
                        "range_len": len,
                    }))
                })
                .map(|v| {
                    json!({
                        "users": v,
                        "in_div": true,
                    })
                })
                .and_then(ChairData::checker(schema_uri!("user_cards")));
            reply::html(render("user_cards.html", r))
        })
    };

    /*
    let card_filter_options = warp::path!("filter" / "options").map(|| {
        let filters = www_try!(@db db::FilterMeta::all());
        let mut ctx = TeraContext::new();
        ctx.insert("filters", &filters);
        render!("filter_options.html", &ctx)
    });

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
        .or(warp::path("card").and(card_one.or(card_ulist)))
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

    fn init() {
        env_logger::builder().is_test(true).try_init().ok();
    }

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

    async fn do_op3(
        mut center: WeiYuanHui,
        path: &str,
        jsn: Value,
    ) -> (
        WeiYuanHui,
        warp::reply::Response,
        BoxedFilter<(impl warp::Reply,)>,
    ) {
        let mut init = center.new_chair();
        init.log(0, "trigger first save disk");
        assert!(center.run().await);
        let app = build_app(center.new_chair());
        let resp = warp::test::request()
            .method("POST")
            .path(path)
            .json(&jsn)
            .filter(&app)
            .await
            .unwrap()
            .into_response();
        (center, resp, app)
    }
    async fn do_op(
        path: &str,
        jsn: Value,
    ) -> (
        WeiYuanHui,
        warp::reply::Response,
        BoxedFilter<(impl warp::Reply,)>,
    ) {
        do_op3(Default::default(), path, jsn).await
    }
    async fn do_get(
        mut center: WeiYuanHui,
        path: &str,
    ) -> (
        WeiYuanHui,
        warp::reply::Response,
        BoxedFilter<(impl warp::Reply,)>,
    ) {
        let mut init = center.new_chair();
        init.log(0, "trigger first save disk");
        assert!(center.run().await);
        let app = build_app(center.new_chair());
        let resp = warp::test::request()
            .method("GET")
            .path(path)
            .filter(&app)
            .await
            .unwrap()
            .into_response();
        (center, resp, app)
    }

    #[tokio::test]
    async fn test_op_follow() {
        init();
        let (mut center, resp, _app) = do_op(
            "/op/follow",
            json!({
                "uid": 12345,
                "enable": true,
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let s = resp_to_st(resp).await;
        assert_eq!(serde_json::from_str(&s).ok(), Some(json!("success")));

        check_n_step(&mut center).await;
        let b = bench(&mut center);
        println!("cur bench: {:?}", b);
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
            b.up_info
                .get("12345")
                .map(|v| v["pick"]["basic"]["ban"].clone()),
            Some(json!(false))
        );
        assert_eq!(b.up_by_fid.len(), 1);
        assert_eq!(b.up_by_fid.front(), Some(&"12345".into()));
    }

    #[tokio::test]
    async fn test_op_refresh() {
        init();
        let mut b = FullBench::default();
        assert!(b.follow(&json!({"uid": 12345})).is_ok());
        let (mut center, resp, _app) = do_op3(
            b.into(),
            "/op/refresh",
            json!({
                "uid": 12345,
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let s = resp_to_st(resp).await;
        assert_eq!(serde_json::from_str(&s).ok(), Some(json!("success")));

        check_n_step(&mut center).await;
        let b = bench(&mut center);
        assert_eq!(b.commands.len(), 2);
        assert_eq!(
            b.commands.back(),
            Some(&json!({
                "cmd": "fetch",
                "args": { "uid": 12345, },
            }))
        );
        assert_eq!(b.up_info.len(), 1);
    }

    #[tokio::test]
    async fn test_op_silence() {
        init();
        let mut b = FullBench::default();
        assert_eq!(b.runtime.insert("bucket".into(), json!({"gap": 21})), None);
        let (mut center, resp, _app) = do_op3(b.into(), "/op/silence", json!({})).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let s = resp_to_st(resp).await;
        assert_eq!(serde_json::from_str(&s).ok(), Some(json!("success")));

        check_n_step(&mut center).await;
        let b = bench(&mut center);
        assert_eq!(b.commands.len(), 0);
        assert_eq!(
            b.runtime.get("bucket"),
            Some(&json!({
                "gap": 42,
            }))
        );
    }

    #[tokio::test]
    async fn test_op_toggle_group_is_insert() {
        init();
        let mut b = FullBench::default();
        assert!(b.follow(&json!({"uid": 12345})).is_ok());
        assert_eq!(
            b.group_info.insert("5".into(), json!({ "name": "test" })),
            None
        );
        let (mut center, resp, _app) = do_op3(
            b.into(),
            "/op/toggle/group",
            json!({
                "uid": 12345,
                "gid": 5,
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let s = resp_to_st(resp).await;
        assert_eq!(serde_json::from_str(&s).ok(), Some(json!("success")));

        check_n_step(&mut center).await;
        let b = bench(&mut center);
        assert_eq!(b.up_info.len(), 1);
        assert_eq!(b.up_join_group.get("5").map(|l| l.len()), Some(1));
        assert_eq!(
            b.up_join_group.get("5").unwrap().get_min(),
            Some(&"12345".into())
        );
    }

    #[tokio::test]
    async fn test_op_toggle_group_is_remove() {
        init();
        let mut b = FullBench::default();
        assert!(b.follow(&json!({"uid": 12345})).is_ok());
        assert_eq!(b.up_join_group.insert("5".into(), Default::default()), None);
        assert_eq!(
            b.up_join_group.get_mut("5").unwrap().insert("12345".into()),
            None
        );
        let (mut center, resp, _app) = do_op3(
            b.into(),
            "/op/toggle/group",
            json!({
                "uid": 12345,
                "gid": 5,
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let s = resp_to_st(resp).await;
        assert_eq!(serde_json::from_str(&s).ok(), Some(json!("success")));

        check_n_step(&mut center).await;
        let b = bench(&mut center);
        assert_eq!(b.up_info.len(), 1);
        assert_eq!(b.up_join_group.get("5").map(|l| l.len()), Some(0));
    }

    #[tokio::test]
    async fn test_op_new_group() {
        init();
        let (mut center, resp, _app) = do_op(
            "/op/touch/group",
            json!({
                "gid": 5,
                "pin": true,
                "name": "test2",
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let s = resp_to_st(resp).await;
        assert_eq!(serde_json::from_str(&s).ok(), Some(json!("success")));

        check_n_step(&mut center).await;
        let b = bench(&mut center);
        assert_eq!(
            b.group_info.get("5"),
            Some(&json!({
                "name": "test2",
                "removable": false,
            }))
        );
    }

    #[tokio::test]
    async fn test_card_one() {
        init();
        let mut b = FullBench::default();
        assert!(b.follow(&json!({"uid": 12345})).is_ok());
        let (_center, resp, _app) = do_get(b.into(), "/card/one/12345").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let s = resp_to_st(resp).await;
        println!("{}", &s);
        assert!(s.contains(r#"<a href="https://space.bilibili.com/12345" target="_blank">"#));
        assert!(!s.contains(r#"failure</title>"#));
        assert!(!s.contains(r#"<div class="card m-2 p-1 shadow" id="#));
    }

    // TODO test card_ulist
}

use crate::data_schema::ChairData;
use crate::db::{FullBench, WeiYuan, WeiYuanHui};
use anyhow::{anyhow, Result};
use futures::StreamExt;
use serde_derive::Deserialize;
use serde_json::{json, Value};
use std::convert::Infallible;
use std::sync::Arc;
use tera::{Context, Tera};
use tokio_stream::wrappers::{errors::BroadcastStreamRecvError, BroadcastStream};
use warp::{filters::BoxedFilter, path, reply, sse::Event, Filter};

// TODO: use rocket_include_tera
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

pub fn build_app(weiyuanhui: &mut WeiYuanHui) -> BoxedFilter<(impl warp::Reply,)> {
    let runner = weiyuanhui.new_chair();
    fn render_fail<E: std::fmt::Display + std::fmt::Debug>(page: &str, e: E) -> String {
        log::debug!("render fail: {:?}", &e);
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

    let card_filter_options = {
        let hdl = runner.readonly();
        warp::path!("filter" / "options").map(move || {
            let r = hdl
                .clone()
                .recv()
                .map(|b| {
                    let a = b
                        .group_info
                        .iter()
                        .map(|(k, v)| {
                            json!({
                                "fid": k,
                                "name": v["name"],
                                "removable": v["removable"],
                            })
                        })
                        .collect::<Vec<_>>();
                    json!({"filters": a})
                })
                .and_then(ChairData::checker(schema_uri!("filter_options")));
            reply::html(render("filter_options.html", r))
        })
    };

    let ev_engine = {
        let rx = Arc::new(weiyuanhui.listen_events());
        warp::path!("engine").map(move || {
            warp::sse::reply(warp::sse::keep_alive().stream(
                BroadcastStream::new(rx.resubscribe()).map(|v| -> Result<Event, Infallible> {
                    Ok(match v {
                        Ok(ev) => Event::default()
                            .json_data(ev)
                            .expect("im::Vector<Value> SHOULD never serde fail"),
                        Err(BroadcastStreamRecvError::Lagged(l)) => {
                            Event::default().comment(format!("event rx lagged {}", l))
                        }
                    })
                }),
            ))
        })
    };

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
        .or(warp::path("card").and(card_one.or(card_ulist).or(card_filter_options)))
        .or(warp::path("ev").and(ev_engine))
        .or(static_files)
        .or(favicon);
    app.boxed()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::WeiYuanHui;
    use chrono::Duration;
    use hyper::body::{to_bytes, Buf};
    use warp::http::StatusCode;
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
        let app = build_app(&mut center);
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
        let app = build_app(&mut center);
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
        let app = build_app(&mut center);
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

    #[tokio::test]
    async fn test_card_ulist() {
        init();
        let mut b = FullBench::new();
        assert!(b.follow(&json!({"uid": 12345})).is_ok());
        assert!(b.follow(&json!({"uid": 2233})).is_ok());
        let (_center, resp, _app) = do_get(b.into(), "/card/ulist/0/default/0/10").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let s = resp_to_st(resp).await;
        println!("{}", &s);
        assert!(s.contains(r#"<a href="https://space.bilibili.com/12345" target="_blank">"#));
        assert!(s.contains(r#"<a href="https://space.bilibili.com/2233" target="_blank">"#));
        assert!(s.contains(r#"<div class="card m-2 p-1 shadow" id=user-card-12345>"#));
        assert!(s.contains(r#"<div class="card m-2 p-1 shadow" id=user-card-2233>"#));
        assert!(
            s.find(r#"<div class="card m-2 p-1 shadow" id=user-card-12345>"#)
                .unwrap()
                < s.find(r#"<div class="card m-2 p-1 shadow" id=user-card-2233>"#)
                    .unwrap()
        );
        assert!(!s.contains(r#"failure</title>"#));
    }

    #[tokio::test]
    async fn test_card_filter_options() {
        init();
        let mut b = FullBench::new();
        assert!(b.touch_group(&json!({"gid": 7, "name": "g7"})).is_ok());
        let (_center, resp, _app) = do_get(b.into(), "/card/filter/options").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let s = resp_to_st(resp).await;
        println!("{}", &s);
        assert!(s.contains(r#"<option value="0">"#));
        assert!(s.contains(r#"<option value="1">"#));
        assert!(s.contains(r#"<option value="7">g7<"#));
        assert!(!s.contains(r#"failure</title>"#));
    }

    async fn run_ms(center: &mut WeiYuanHui, ms: i64, running: bool) {
        assert_eq!(
            center
                .run_for(chrono::Duration::milliseconds(ms))
                .await
                .ok(),
            Some(running)
        );
    }

    #[tokio::test]
    async fn test_sse() {
        init();
        let (mut center, resp, app) = do_get(Default::default(), "/ev/engine").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let mut body = resp.into_body();
        let mut tx = center.new_chair();
        // FIXME use modify_up_info to trigger events
        let ls = vec![json!({
            "uid":12345,
            "live": {"isopen":"true"},
        })];
        let ls_rx = ls.clone();
        tokio::join!(
            async move {
                let mut it = ls_rx.iter().cloned();
                while let Some(msg) = body.next().await {
                    log::info!("get {:?}", &msg);
                    let buf = if let Ok(b) = msg {
                        b
                    } else {
                        continue;
                    };
                    const BOM: &[u8; 2] = b"\xFE\xFE";
                    let b: &[u8] = if buf.starts_with(BOM) {
                        buf.split_at(BOM.len()).1
                    } else {
                        buf.as_ref()
                    };
                    let s = std::str::from_utf8(b);
                    assert!(matches!(s, Ok(_)));
                    let s = s.unwrap();
                    for l in s.lines() {
                        if l.starts_with("data:") {
                            let v = serde_json::from_str(l.strip_prefix("data:").unwrap());
                            assert!(matches!(v, Ok(_)));
                            assert_eq!(v.ok(), it.next().map(|v| json!([v])));
                        }
                    }
                }
                assert_eq!(it.next(), None);
                std::mem::drop(app);
            },
            async {
                tx.log(3, "clear dump");
                for ev in ls.iter().cloned() {
                    run_ms(&mut center, 50, true).await;
                    log::info!("sending {:?}", &ev);
                    assert!(tx.apply(|b| Ok(b.events.push_back(ev.clone()))).is_ok());
                }
                run_ms(&mut center, 50, true).await;
                center.close();
                std::mem::drop(tx);
                run_ms(&mut center, 50, false).await;
                assert!(tokio::time::timeout(
                    tokio::time::Duration::from_millis(200),
                    center.closed()
                )
                .await
                .is_ok());
            }
        );
    }
}

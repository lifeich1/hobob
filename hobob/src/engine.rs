//! engine 后台循环（M1 T2 适配版：读 `Snapshot`，fetch 写组件；T6 整体迁入 systems.rs）。
use crate::db::{Commands, Snapshot, WeiYuan};
use anyhow::Context;
use anyhow::{anyhow, Result};
use bilibili_api_rs::Client;
use serde_json::{json, Value};
use std::time::Duration;
use tokio::time::Instant;

fn take_cmds(bench: &Snapshot, runner: &mut WeiYuan) -> Commands {
    let r = bench.res.commands.clone();
    let l = r.len();
    runner
        .apply(|b| {
            b.res
                .commands
                .len()
                .eq(&l)
                .then(|| b.res.commands.clear())
                .ok_or_else(|| {
                    anyhow!(
                        "mutate commands encounter: {} != {}",
                        b.res.commands.len(),
                        l
                    )
                })
        })
        .map_err(|e| log::debug!("{}", e))
        .and(Ok(r))
        .unwrap_or_else(|()| im::Vector::default())
}

async fn do_fetch(cli: &mut Client, runner: &mut WeiYuan, args: &Value) -> Result<()> {
    let uid = args["uid"]
        .as_i64()
        .ok_or_else(|| anyhow!("bad args: {:?}", args))?;
    let info = cli.user(uid).info().await;
    let video = cli.user(uid).latest_videos().await;
    runner.apply(|b| {
        let info = b.inspect(&info).as_ref().ok();
        let video = b.inspect(&video).as_ref().ok();
        if info.is_none() || video.is_none() {
            b.bucket_double_gap();
            return Ok(());
        }
        let info = info.unwrap();
        let video = video.unwrap();
        b.apply_fetch(uid, info, video)?;
        b.bucket_good();
        Ok(())
    })?;
    info?;
    video?;
    log::info!("do fetch uid:{} ok", uid);
    Ok(())
}

async fn exec_cmd(cmd: Value, runner: &mut WeiYuan, cli: &mut Client) {
    log::debug!("exec_cmd: {:?}", &cmd);
    match cmd["cmd"].as_str() {
        Some("fetch") => {
            do_fetch(cli, runner, &cmd["args"])
                .await
                .with_context(|| format!("failed do_fetch args: {:?}", cmd["args"]))
                .map_err(|e| log::error!("{:?}", e))
                .ok();
        }
        Some("livelist") => todo!(),
        _ => log::error!("unimplemented cmd: {:?}", &cmd),
    }
}

fn exec_timers(bench: &Snapshot, runner: &mut WeiYuan) {
    let uid: i64 = if let Some(suid) = bench.res.up_index.get("ctime").and_then(|i| i.get_min()) {
        suid.1
            .parse()
            .unwrap_or_else(|e| panic!("suid SHOULD be valid integer: {e}"))
    } else {
        log::error!("empty 'ctime' index");
        return;
    };
    runner
        .apply(|b| {
            // TODO query xlive timer
            b.res.commands.push_back(json!({
                "cmd": "fetch",
                "args": { "uid": uid, },
            }));
            b.bucket_hang();
            Ok(())
        })
        .map_err(|e| log::error!("exec_timers error: {}", e))
        .ok();
}

fn next_deadline(runner: &mut WeiYuan) -> Instant {
    Instant::now()
        + runner.recv().ok().map_or_else(
            || Duration::from_secs(1),
            |b| {
                b.bucket_duration_to_next()
                    .to_std()
                    .unwrap_or_else(|e| panic!("unexpected out_of_range: {e}"))
            },
        )
}

pub async fn main_loop(mut runner: WeiYuan) {
    let mut client = Client::new();
    while let Ok(bench) = runner.recv().cloned() {
        log::trace!("engine_loop wake");
        if bench.res.commands.is_empty() {
            exec_timers(&bench, &mut runner);
        } else {
            let cmds = take_cmds(&bench, &mut runner);

            for cmd in cmds {
                exec_cmd(cmd, &mut runner, &mut client).await;
            }
        }

        let deadline = next_deadline(&mut runner);
        log::trace!("engine_loop sleep, deadline: {:?}", &deadline);
        tokio::time::timeout_at(deadline, runner.changed())
            .await
            .ok();
    }
    log::error!("closing");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::WeiYuanHui;
    use tokio::time::timeout;

    fn init() {
        env_logger::builder()
            .is_test(true)
            .format_timestamp(Some(env_logger::fmt::TimestampPrecision::Micros))
            .try_init()
            .ok();
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
    async fn test_close_engine() {
        init();
        let mut center = WeiYuanHui::default();
        let mut runner = center.new_chair();
        tokio::join!(
            async {
                run_ms(&mut center, 100, true).await;
                center.close();
                log::info!("emit closing");
                run_ms(&mut center, 100, false).await;
                assert!(timeout(Duration::from_millis(200), center.closed())
                    .await
                    .is_ok());
            },
            async {
                assert!(runner.recv().is_ok());
                log::info!("start engine");
                assert!(timeout(Duration::from_millis(400), main_loop(runner))
                    .await
                    .is_ok());
            }
        );
    }

    #[test]
    fn test_next_deadline() {
        init();
        let mut center = WeiYuanHui::default();
        let mut runner = center.new_chair();
        assert!(Instant::now() < next_deadline(&mut runner));
    }

    #[test]
    fn test_take_cmds() {
        init();
        let mut snap = Snapshot::default();
        snap.res.commands.push_back(json!({
            "cmd": "cmd:for_test",
            "args": {"c":1},
        }));
        snap.res.commands.push_back(json!({
            "cmd": "cmd:for_test",
            "args": {"c":2},
        }));
        let mut center = WeiYuanHui::from(snap.clone());
        let mut runner = center.new_chair();
        let out = take_cmds(&snap, &mut runner);
        assert_eq!(out.len(), 2);
        assert_eq!(
            out[0],
            json!({
                "cmd": "cmd:for_test",
                "args": {"c":1},
            })
        );
        assert_eq!(
            out[1],
            json!({
                "cmd": "cmd:for_test",
                "args": {"c":2},
            })
        );
    }

    fn mkiiiiii_info() -> Value {
        json!({
            "mid": 210_628,
            "name": "MKiiiiii",
            "sex": "保密",
            "face": "https://i1.hdslb.com/bfs/face/83343d35792eeb0924ae27bf882a72fe38b2e335.jpg",
            "pendant": {
                "image": "https://i1.hdslb.com/bfs/garb/item/63db246f6a657190d79415af47fa0478013f9c05.png",
                "image_enhance": "https://i1.hdslb.com/bfs/garb/item/76b0cb6c1a7cdaaa64a9626a31796025c8aae89b.webp",
            },
            "live_room": {
                "roomStatus": 1,
                "liveStatus": 0,
                "url": "https://live.bilibili.com/5229?broadcast_type=0\u{0026}is_room_feed=1",
                "title": "【鑒賞會】就打一关",
                "watched_show": {
                    "num": 14,
                    "text_large": "14人看过",
                }
            },
        })
    }

    #[test]
    fn test_pick_basic() {
        let a = mkiiiiii_info();
        let b = json!({});
        let mut b = crate::db::pick_basic(&a, &b);
        assert!(b["ctime"].is_i64());
        b["ctime"] = Value::Null;
        assert_eq!(
            b,
            json!({
                "id": 210_628,
                "name": "MKiiiiii",
                "face_url":  "https://i1.hdslb.com/bfs/face/83343d35792eeb0924ae27bf882a72fe38b2e335.jpg",
                "ctime": null,
            })
        );
    }

    #[test]
    fn test_pick_live() {
        let a = mkiiiiii_info();
        assert_eq!(
            crate::db::pick_live(&a),
            json!({
                "title": "【鑒賞會】就打一关",
                "url": "https://live.bilibili.com/5229?broadcast_type=0\u{0026}is_room_feed=1",
                "entropy": 14,
                "entropy_txt": "14人看过",
                "isopen": false,
            })
        );
    }

    fn mkiiiiii_videos() -> Value {
        json!({
            "list": {
                "tlist": {},
                "vlist": [
                {
                    "play": 13010,
                    "pic": "http://i2.hdslb.com/bfs/archive/8075cc1b875a27de57b499cf231e3d131b8192ba.jpg",
                    "title": "四分鐘畫個機 室友版",
                    "author": "MKiiiiii",
                    "created": 1_695_871_800,
                    "bvid": "BV15N4y1f7cN",
                },
                {
                    "description": "慶祝戰艦少女R的賀圖，角色為風大師負責的不惧，一個加速繪畫過程，沒什麼營養，他也懶得加bgm，乾巴巴的，我也懶得幫他加。就這。",
                    "title": "一分鐘畫個煙花發射圖 室友版",
                },
                ]
            },
            "episodic_button": {
                "text": "播放全部",
                "uri": "//www.bilibili.com/medialist/play/210628?from=space"
            },
        })
    }

    #[test]
    fn test_pick_video() {
        let a = mkiiiiii_videos();
        assert_eq!(
            crate::db::pick_video(&a),
            json!({
                "title": "四分鐘畫個機 室友版",
                "url": "https://www.bilibili.com/medialist/play/210628?from=space",
                "ts": 1_695_871_800,
            })
        );
    }
    // TODO test do_fetch
    // TODO test exec_cmd
    // TODO test exec_timers
}

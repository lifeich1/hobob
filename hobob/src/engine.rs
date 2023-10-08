use crate::db::{now_timestamp, Commands, FullBench, WeiYuan};
use anyhow::Context;
use anyhow::{anyhow, Result};
use bilibili_api_rs::Client;
use serde_json::{json, Value};
use std::time::Duration;
use tokio::time::Instant;

fn take_cmds(bench: &FullBench, runner: &mut WeiYuan) -> Commands {
    let r = bench.commands.clone();
    let l = r.len();
    runner
        .apply(|b| {
            b.commands
                .len()
                .eq(&l)
                .then(|| b.commands.clear())
                .ok_or_else(|| anyhow!("mutate commands encounter: {} != {}", b.commands.len(), l))
        })
        .map_err(|e| log::debug!("{}", e))
        .and(Ok(r))
        .unwrap_or_else(|_| Default::default())
}

fn pick_basic(a: &Value, b: &Value) -> Value {
    let mut r = b.clone();
    let mut ap = json!({
        "id": a["mid"],
        "name": a["name"],
        "face_url": a["face"],
        "ctime": now_timestamp(),
    });
    r.as_object_mut()
        .expect("up_info SHOULD inited basic")
        .append(ap.as_object_mut().unwrap());
    r
}

fn pick_live(a: &Value) -> Value {
    let l = &a["live_room"];
    let w = &l["watched_show"];
    json!({
        "title": l["title"],
        "url": l["url"],
        "entropy": w["num"],
        "entropy_txt": w["text_large"],
        "isopen": w["roomStatus"].as_i64().filter(|i| *i > 0)
            .and(w["liveStatus"].as_i64())
            .filter(|i| *i > 0)
            .is_some(),
    })
}

fn pick_video(a: &Value) -> Value {
    let v = if let Some(v) = a["vlist"].as_array().filter(|v| !v.is_empty()) {
        &v[0]
    } else {
        return Value::Null;
    };
    json!({
        "title": v["title"],
        "url": a["episodic_button"]["uri"].as_str().map(|s| format!("https:{}", s)),
        "ts": v["created"],
    })
}

async fn do_fetch(cli: &Client, runner: &mut WeiYuan, args: &Value) -> Result<()> {
    let uid = args["uid"]
        .as_i64()
        .ok_or_else(|| anyhow!("bad args: {:?}", args))?;
    let wbi = cli.user(uid);
    let info = wbi.info().await;
    let vid = wbi.latest_videos().await;
    runner.apply(|b| {
        let info = b.inspect(&info).as_ref().ok();
        let vid = b.inspect(&vid).as_ref().ok();
        if info.is_none() || vid.is_none() {
            b.bucket_double_gap();
            return Ok(());
        }
        let info = info.unwrap();
        let vid = vid.unwrap();
        b.modify_up_info(&uid.to_string(), |v| {
            v["raw"] = json!({
                "videos": vid,
                "info": info,
            });
            v["pick"] = json!({
                "basic": pick_basic(info, &v["pick"]["basic"]),
                "live": pick_live(info),
                "video": pick_video(vid),
            });
        });
        b.bucket_good();
        Ok(())
    })?;
    info?;
    vid?;
    log::info!("do fetch uid:{} ok", uid);
    Ok(())
}

async fn exec_cmd(cmd: Value, runner: &mut WeiYuan, cli: &Client) {
    log::debug!("exec_cmd: {:?}", &cmd);
    match cmd["cmd"].as_str() {
        Some("fetch") => {
            do_fetch(cli, runner, &cmd["args"])
                .await
                .with_context(|| format!("failed do_fetch args: {:?}", cmd["args"]))
                .map_err(|e| log::error!("{:?}", e))
                .ok();
        }
        _ => log::error!("unimplemented cmd: {:?}", &cmd),
    }
}

async fn exec_timers(bench: &FullBench, runner: &mut WeiYuan) {
    let uid: i64 = if let Some(suid) = bench.up_index.get("ctime").and_then(|i| i.get_min()) {
        suid.1
            .parse()
            .unwrap_or_else(|e| panic!("suid SHOULD be valid integer: {}", e))
    } else {
        log::error!("empty 'ctime' index");
        return;
    };
    runner
        .apply(|b| {
            // TODO query xlive timer
            b.commands.push_back(json!({
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
        + runner
            .recv()
            .ok()
            .map(|b| {
                b.bucket_duration_to_next()
                    .to_std()
                    .unwrap_or_else(|e| panic!("unexpected out_of_range: {}", e))
            })
            .unwrap_or_else(|| Duration::from_secs(1))
}

pub async fn engine_loop(mut runner: WeiYuan) {
    let client = &Client::new();
    while let Ok(bench) = runner.recv().cloned() {
        log::trace!("engine_loop wake");
        if !bench.commands.is_empty() {
            let cmds = take_cmds(&bench, &mut runner);

            for cmd in cmds {
                exec_cmd(cmd, &mut runner, client).await;
            }
        } else {
            exec_timers(&bench, &mut runner).await;
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
                log::info!("emit closing: {:?}", center.bench());
                run_ms(&mut center, 100, false).await;
                assert!(timeout(Duration::from_millis(200), center.closed())
                    .await
                    .is_ok());
            },
            async {
                assert!(runner.recv().is_ok());
                log::info!("start engine");
                assert!(timeout(Duration::from_millis(400), engine_loop(runner))
                    .await
                    .is_ok());
            }
        );
    }

    // TODO test next_deadline
    // TODO test take_cmds
    // TODO test pick_basic
    // TODO test pick_live
    // TODO test pick_video
    // TODO test do_fetch
    // TODO test exec_cmd
    // TODO test exec_timers
}

//! 基础 system（M1 §4.5，原 `engine.rs` 迁入 + 动态 system 骨架）。
//!
//! - **fetch 系统**（`fetch_loop`）：原 engine 主循环平移——消费 `commands`、网络抓取、
//!   组件写回走 `Snapshot::apply_fetch`；成功/失败经 `Snapshot::push_sys_event` 上报
//!   `FetchDone`/`FetchFailed` 事件（hub 分发，SSE 形状不变，见 db `SYS_EVENT_TAG`）。
//! - **tick（内置 `builtin.tick`）**：v1 `exec_timers` 语义平移为动态 system handler——
//!   commands 为空时选 ctime 最小 up 补抓 + `bucket_hang`。触发路径：fetch_loop 在
//!   commands 为空且 deadline 唤醒时发 `Tick` 事件 → hub 分发 → handler 在权威快照上
//!   补抓（v1 节奏/语义逐行保留：bucket 速率控制仍在 fetch_loop `next_deadline`）。
//! - **动态 system 框架**（M1 骨架，执行语义 M3）：`TriggerEvent` + `DynSystemRegistry`
//!   （BTreeMap 按注册名序分发，handler 报错记日志继续）。M1 只注册内置 `builtin.tick`，
//!   store `systems` 表不加载（lua/condition 语法 M3 定）。
//!
//! 与方案文档的偏差（实现备注）：
//! - fetch 系统保留**独立循环**（hub 串行执行会以网络等待阻塞 patch 处理，违背 v1
//!   解耦语义与 D13）；hub 仍是唯一分发点（try_push 尾），事件经 patch 通道上传。
//! - `DynSystemRegistry` 由 hub（`WeiYuanHui`）持有、不进 `Resources`（分发点唯一在
//!   hub，随快照传播只增 clone 成本且 chair 无用途）。

use crate::db::{Commands, Snapshot, WeiYuan};
use anyhow::Context;
use anyhow::{anyhow, Result};
use bilibili_api_rs::Client;
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::Instant;

// ============================== 动态 system 框架 ==============================

/// 触发事件（M1 骨架枚举；M3 定 condition 语法与 lua 回调时扩展）。
#[derive(Clone, Debug, PartialEq)]
pub enum TriggerEvent {
    /// hub 或 fetch 循环的节拍（v1 `exec_timers` 由 engine 每轮 deadline 唤醒触发）。
    Tick { at: DateTime<Utc> },
    FetchDone { entity: u64, uid: String },
    FetchFailed { entity: u64, uid: String, error: String },
    UpStateChanged { entity: u64, kind: UpStateKind },
}

/// up 管理状态变化类别（M1 只发 Followed/Unfollowed/GroupToggled；Silenced 留 M3）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpStateKind {
    Followed,
    Unfollowed,
    GroupToggled,
    Silenced,
}

/// 回调：`(&TriggerEvent, &mut Snapshot)`。错误由 registry 记日志继续（不中断后续 handler）。
pub type DynCallback = Arc<dyn Fn(&TriggerEvent, &mut Snapshot) -> Result<()> + Send + Sync>;

pub struct DynSystemSpec {
    pub name: String,
    /// M1 恒 `"always"`（占位）；condition 语法与求值 M3。
    pub condition: String,
    pub callback: DynCallback,
}

impl std::fmt::Debug for DynSystemSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DynSystemSpec")
            .field("name", &self.name)
            .field("condition", &self.condition)
            .finish_non_exhaustive()
    }
}

/// 动态 system 注册表（hub 持有）。按注册名（BTreeMap key）升序分发，串行执行。
#[derive(Debug, Default)]
pub struct DynSystemRegistry {
    specs: BTreeMap<String, DynSystemSpec>,
}

impl DynSystemRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册/覆盖（同名替换）。注册名即分发顺序 key。
    pub fn register(&mut self, spec: DynSystemSpec) {
        self.specs.insert(spec.name.clone(), spec);
    }

    pub fn len(&self) -> usize {
        self.specs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.specs.is_empty()
    }

    /// 按注册名升序分发；单个 handler 报错只记日志，不中断后续 handler。
    pub fn dispatch(&self, ev: &TriggerEvent, snap: &mut Snapshot) {
        for spec in self.specs.values() {
            if let Err(e) = (spec.callback)(ev, snap) {
                log::warn!("dyn system {:?} handler error: {e:#}", spec.name);
            }
        }
    }
}

/// 内置 tick：v1 `exec_timers` 语义（M1 唯一注册的内置 system，condition 恒真）。
pub fn builtin_tick() -> DynSystemSpec {
    DynSystemSpec {
        name: "builtin.tick".into(),
        condition: "always".into(),
        callback: Arc::new(|ev: &TriggerEvent, snap: &mut Snapshot| -> Result<()> {
            match ev {
                TriggerEvent::Tick { .. } => tick_impl(snap),
                _ => Ok(()),
            }
        }),
    }
}

/// v1 `exec_timers` 平移：commands 为空时选 ctime 最小 up 补抓 + `bucket_hang`。
fn tick_impl(snap: &mut Snapshot) -> Result<()> {
    if !snap.res.commands.is_empty() {
        return Ok(()); // 幂等防御：只在 commands 空时补抓（v1 调用侧保证）
    }
    let uid: i64 = if let Some(suid) = snap.res.up_index.get("ctime").and_then(|i| i.get_min()) {
        suid
            .1
            .parse()
            .unwrap_or_else(|e| panic!("suid SHOULD be valid integer: {e}"))
    } else {
        log::error!("empty 'ctime' index");
        return Ok(());
    };
    snap.res.commands.push_back(json!({
        "cmd": "fetch",
        "args": { "uid": uid, },
    }));
    snap.bucket_hang();
    Ok(())
}

// ============================== fetch 系统（原 engine.rs 平移） ==============================

/// 从权威快照取走全部 commands（v1 原样：apply 内校验长度一致后清空）。
pub(crate) fn take_cmds(bench: &Snapshot, runner: &mut WeiYuan) -> Commands {
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

/// fetch 事件上报（经 apply 内 `push_sys_event`；成功/失败都会触发 hub 分发）。
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
            b.push_sys_event(json!({
                "kind": "fetch_failed",
                "uid": uid,
                "error": "api error (details in log)",
            }));
            return Ok(());
        }
        let info = info.unwrap();
        let video = video.unwrap();
        b.apply_fetch(uid, info, video)?;
        b.bucket_good();
        b.push_sys_event(json!({
            "kind": "fetch_done",
            "uid": uid,
        }));
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

/// fetch 系统主循环（原 `engine::main_loop` 平移）：commands 有则执行，空则向 hub 发
/// `Tick` 事件（builtin.tick 补抓）；bucket 节奏（deadline）由 v1 语义保留。
pub async fn fetch_loop(mut runner: WeiYuan) {
    let mut client = Client::new();
    while let Ok(bench) = runner.recv().cloned() {
        log::trace!("engine_loop wake");
        if bench.res.commands.is_empty() {
            // v1 exec_timers 语义移交 builtin.tick：仅请求节拍，补抓由 hub 分发完成
            runner
                .apply(|b| {
                    b.push_sys_event(json!({ "kind": "tick" }));
                    Ok(())
                })
                .map_err(|e| log::error!("emit tick error: {}", e))
                .ok();
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

/// hub 分发用：events 中的 `#SYSEV#` 载荷 → `TriggerEvent`（uid 经快照 uid_index 反查 entity）。
/// 无法解析返回 None（调用方记日志）。
pub fn trigger_from_json(ev: &Value, snap: &Snapshot) -> Option<TriggerEvent> {
    let kind = ev["kind"].as_str()?;
    let at = Utc::now();
    match kind {
        "tick" => Some(TriggerEvent::Tick { at }),
        "fetch_done" | "fetch_failed" => {
            let uid = ev["uid"].as_i64()?.to_string();
            let entity = entity_of(snap, &uid);
            match kind {
                "fetch_done" => Some(TriggerEvent::FetchDone { entity, uid }),
                _ => Some(TriggerEvent::FetchFailed {
                    entity,
                    uid,
                    error: ev["error"]
                        .as_str()
                        .unwrap_or("fetch failed")
                        .to_string(),
                }),
            }
        }
        "up_state" => {
            let uid = ev["uid"].as_i64()?.to_string();
            let entity = entity_of(snap, &uid);
            let kind = match ev["state"].as_str() {
                Some("followed") => UpStateKind::Followed,
                Some("unfollowed") => UpStateKind::Unfollowed,
                Some("group_toggled") => UpStateKind::GroupToggled,
                Some("silenced") => UpStateKind::Silenced,
                _ => return None,
            };
            Some(TriggerEvent::UpStateChanged { entity, kind })
        }
        _ => None,
    }
}

fn entity_of(snap: &Snapshot, uid: &str) -> u64 {
    snap.res.uid_index.get(uid).copied().unwrap_or(0)
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

    // ---------- fetch 循环（原 engine.rs 测试迁移） ----------

    #[tokio::test]
    async fn test_close_fetch_loop() {
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
                assert!(timeout(Duration::from_millis(400), fetch_loop(runner))
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

    // ---------- pick_*（原 engine.rs fixtures，测 db 纯函数） ----------

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
    // TODO test do_fetch（无网络；fetch mock 与事件分发在 db tests N10 覆盖）
}

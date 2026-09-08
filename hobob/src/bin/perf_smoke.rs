//! T5 性能冒烟（M1 §7）：模拟 200 up × 连续 10 轮 fetch 突发，走真实 hub/持久化路径
//! （chair apply → hub run → `persist_diff` 直写 + stage），采集六项指标。**不联网**：
//! fetch 数据用内嵌 JSON mock（v1 测试同款先例）。
//!
//! 运行（release，本机 nix devShell）：
//! ```text
//! cargo run --release -p hobob --bin perf_smoke
//! ```
//! 输出指标由人工回填 `.plans/m1-ecs-core.md` §7 指标表与「实施记录」。

use anyhow::Result;
use hobob::db::{Snapshot, WeiYuan, WeiYuanHui};
use hobob::store::{Store, StoreConfig};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// 场景规模：up 数与 fetch 轮数（方案 §7：200 up × 10 轮突发）。
const UPS: u64 = 200;
const ROUNDS: usize = 10;

fn uid_of(i: u64) -> i64 {
    100_000 + i64::try_from(i).expect("i < 2^62")
}

/// 每轮变化的 fetch mock（name 恒定、直播/视频/观看数随 round 变 → 组件 diff 真实发生）。
fn fetch_payload(i: u64, round: usize) -> (Value, Value) {
    let uid = uid_of(i);
    let info = json!({
        "mid": uid,
        "name": format!("up-{i}"),
        "face": format!("https://i1.hdslb.com/bfs/face/{uid}"),
        "live_room": {
            "roomStatus": 1,
            "liveStatus": 1,
            "url": format!("https://live.bilibili.com/{uid}"),
            "title": format!("直播-{i}-r{round}"),
            "watched_show": {
                "num": (round as i64) * 100 + (i as i64) % 50,
                "text_large": "N人看过",
            },
        },
    });
    let videos = json!({
        "list": { "vlist": [
            { "play": 1, "title": format!("视频-{i}-r{round}"),
              "created": 1_700_000_000 + round as i64, "bvid": format!("BV{i}r{round}") },
        ]},
        "episodic_button": { "uri": format!("//www.bilibili.com/medialist/play/{uid}") },
    });
    (info, videos)
}

async fn drive(center: &mut WeiYuanHui, chair: &mut WeiYuan, f: impl Fn(&mut Snapshot) -> Result<()>) {
    chair.apply(f).unwrap();
    assert!(center.run().await, "hub run SHOULD process one patch");
}

/// 百分位统计：排序后取 p（0.50/0.99）。
fn pct(mut xs: Vec<Duration>, p: f64) -> Duration {
    xs.sort_unstable();
    let idx = ((xs.len() as f64) * p).ceil().max(1.0) as usize - 1;
    xs[idx.min(xs.len() - 1)]
}

fn summary(tag: &str, xs: &[Duration]) {
    let n = xs.len();
    let total: Duration = xs.iter().sum();
    println!(
        "  {tag:<28} n={n:<4} p50={:>10.3?}  p99={:>10.3?}  sum={:>10.3?}",
        pct(xs.to_vec(), 0.50),
        pct(xs.to_vec(), 0.99),
        total
    );
}

fn temp_dir(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("hobob-perf-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&p).expect("create perf temp dir");
    p
}

/// store 层：易变批量 flush 提交延迟（256 条一次 commit，默认 MAX_RECORDS=256）。
/// 每组 stage 255 条（不触发阈值），第 256 条 stage 内部触发一次 256 条 flush → 计时该 stage。
fn bench_flush_latency(dir: &Path) -> Result<Vec<Duration>> {
    let cfg = StoreConfig::new(dir.join("flush.redb"));
    let mut store = Store::open_or_create(&cfg)?;
    let mut lat = Vec::new();
    for group in 0..10u64 {
        for j in 0..255u64 {
            store.stage_video_post(1000 + group * 1000 + j, &default_video())?;
        }
        let t0 = Instant::now();
        // 第 256 条 → 阈值触发一次 256 条 commit
        store.stage_video_post(1000 + group * 1000 + 255, &default_video())?;
        lat.push(t0.elapsed());
    }
    store.close()?;
    Ok(lat)
}

fn default_video() -> hobob::store::VideoPostV1 {
    hobob::store::VideoPostV1 {
        updated_at: 1_700_000_000,
        latest_ts: 1_700_000_000,
        items: vec![hobob::store::VideoItemV1 {
            bvid: "BV1x".into(),
            title: "视频".into(),
            pubdate: 1_700_000_000,
            extra: String::new(),
        }],
        extra: String::new(),
        episodic: None,
    }
}

/// 逻辑 payload 字节采样（写放大分母，bincode fixint 根编码与 store 一致）。
fn sample_payload_sizes() -> (usize, usize, usize) {
    let brick = hobob::store::BrickV2 {
        uid: "100123".into(),
        uname: "up-123".into(),
        face: "https://i1.hdslb.com/bfs/face/100123".into(),
        ban: false,
        fid: 0,
        groups: vec![],
        silent: false,
        followed_at: 1_700_000_000,
        updated_at: 1_700_000_001,
    };
    let video = default_video();
    let live = hobob::store::LivePostV1 {
        updated_at: 1,
        is_open: true,
        title: "直播".into(),
        url: "https://live.bilibili.com/100123".into(),
        ts: 0,
        extra: String::new(),
        entropy: 42,
        entropy_txt: "42人看过".into(),
    };
    (
        bincode::serialize(&brick).expect("brick serializable").len(),
        bincode::serialize(&video).expect("video serializable").len(),
        bincode::serialize(&live).expect("live serializable").len(),
    )
}

/// hub 场景：follow 200 + 组 ops + 10 轮 fetch 突发 → 返回 (统计/耗时, commit 计数, 文件字节)。
async fn hub_scenario(dir: &Path) -> Result<(usize, usize, usize, usize, (u64, u64), u64)> {
    let cfg = StoreConfig::new(dir.join("state.redb"));
    let file = cfg.path.clone();
    let size0 = file_size(&file);
    let mut center = WeiYuanHui::open(Store::open_or_create(&cfg)?)?;
    let mut chair = center.new_chair();

    // 1) brick 直写延迟：200 次 follow（每 patch 一次直写事务 + 协议 + diff 开销）
    let mut follow_lat = Vec::new();
    for i in 0..UPS {
        let t0 = Instant::now();
        drive(&mut center, &mut chair, |b| {
            b.follow(&json!({"uid": uid_of(i), "enable": true}))
        })
        .await;
        follow_lat.push(t0.elapsed());
    }
    println!("brick 直写延迟（follow 200，真实 hub 路径）:");
    summary("follow", &follow_lat);

    // 2) 分组 ops：10 组 toggle + touch（组直写 + brick 分组变更直写）
    for i in 0..10u64 {
        let gid = 1_000 + i;
        drive(&mut center, &mut chair, |b| {
            b.toggle_group(&json!({"uid": uid_of(i), "gid": gid}))
        })
        .await;
        drive(&mut center, &mut chair, |b| {
            b.touch_group(&json!({"gid": gid, "name": format!("组{i}"), "pin": false}))
        })
        .await;
    }

    // 3) fetch 突发：10 轮 × 200 up（组件更新走真实 stage 路径）。
    //    轮间 sleep 1s：`apply_fetch` 的 brick.updated_at 是秒粒度 now_timestamp，
    //    真实 bucket 节奏（gap≥10s）保证同 up 两次 fetch 跨秒 → brick 每轮直写；
    //    不加 sleep 会让 10 轮落在同一秒内、diff 把 brick 直写短路（模拟失真）。
    let t0 = Instant::now();
    let mut fetch_lat = Vec::new();
    for round in 0..ROUNDS {
        if round > 0 {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        for i in 0..UPS {
            let (info, videos) = fetch_payload(i, round);
            let t1 = Instant::now();
            drive(&mut center, &mut chair, |b| {
                b.apply_fetch(uid_of(i), &info, &videos)
            })
            .await;
            fetch_lat.push(t1.elapsed());
        }
    }
    println!("fetch 突发 10 轮 × 200 up（直写 + stage 真实路径）:");
    summary("apply_fetch patch", &fetch_lat);
    println!("  fetch 突发墙钟总耗时: {:?}", t0.elapsed());

    let stats = center.store_stats().expect("hub holds store");
    println!(
        "  commit 计数: direct={} volatile={}（stage 总条数≈{}，预期 volatile≈{}）",
        stats.0,
        stats.1,
        ROUNDS as u64 * UPS * 3 + 10,
        (ROUNDS as u64 * UPS * 3 + 10).div_ceil(256)
    );
    // 4) 稳态只读期：只读查询不打任何 redb 读（T4 代码审计 + roundtrip 保障）；此处不触发 store 读 API。
    let s = center.bench();
    let _ = s.pick_of(uid_of(0)).map(|p| p["basic"]["name"].clone());
    let _ = s.filter_options();
    let stats_after_read = center.store_stats().expect("hub holds store");
    assert_eq!(stats_after_read, stats, "只读查询不得产生写 commit");

    center.close();
    let size1 = file_size(&file);
    Ok((
        follow_lat.len(),
        fetch_lat.len(),
        ROUNDS * UPS as usize,
        follow_lat.len() + fetch_lat.len(),
        stats,
        size1.saturating_sub(size0),
    ))
}

fn file_size(p: &Path) -> u64 {
    std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)
}

/// 启动全量加载计时（200 up + video/live 组件 + 组）。
fn bench_reopen(dir: &Path) -> Result<Duration> {
    let cfg = StoreConfig::new(dir.join("state.redb"));
    let t0 = Instant::now();
    let store = Store::open_or_create(&cfg)?;
    let center = WeiYuanHui::open(store)?;
    let elapsed = t0.elapsed();
    let ups = center.bench().res.uid_index.len();
    drop(center);
    println!("  启动全量加载（reopen {ups} up）: {elapsed:?}");
    Ok(elapsed)
}

#[tokio::main]
async fn main() -> Result<()> {
    let dir = temp_dir("t5");
    println!("== M1 T5 性能冒烟（本机 {}，{UPS} up × {ROUNDS} 轮 fetch，不联网）==", std::env::consts::ARCH);
    println!("== MAX_RECORDS=256 / MAX_AGE=5s（store 占位参数，冒烟后定值）==\n");

    // flush 提交延迟（store 层 10 组）
    let flush_lat = bench_flush_latency(&dir)?;
    println!("易变批量 flush 提交延迟（256 条/次 commit，10 组）:");
    summary("256 条 flush", &flush_lat);
    println!();

    // hub 端到端场景
    let (n_follow, n_fetch, n_rounds, _, (direct, volatile), file_delta) =
        hub_scenario(&dir).await?;
    let (brick_b, video_b, live_b) = sample_payload_sizes();
    println!("  payload 采样: brick≈{brick_b}B video≈{video_b}B live≈{live_b}B");
    // 写放大 = 文件增量 / 逻辑 payload 字节（follow 200×brick + fetch 每 patch
    // brick + video + live + runtime≈200B；分组 ops 少量直写计入 direct 差）
    let logical_bytes = n_follow as u64 * brick_b as u64
        + (n_fetch as u64) * (brick_b as u64 + video_b as u64 + live_b as u64 + 200);
    println!("  写放大: 文件增量={file_delta}B 逻辑 payload≈{logical_bytes}B ratio={:.2}x", file_delta as f64 / logical_bytes.max(1) as f64);
    println!(
        "  fsync 频率: direct_commits={direct}（brick 每 op 1 commit≈{n_follow}+{n_rounds}×{UPS}），volatile_commits={volatile}（易变 {n_fetch}×3+ 条 → 仅批量 flush）\n"
    );

    // 启动全量加载
    let load = bench_reopen(&dir)?;
    println!("\n== 指标汇总 ==");
    println!("  flush p50/p99: {:?}/{:?}", pct(flush_lat.clone(), 0.50), pct(flush_lat.clone(), 0.99));
    println!("  启动加载: {load:?}");

    let _ = std::fs::remove_dir_all(&dir);
    println!("\ndone（临时目录已清理）");
    Ok(())
}

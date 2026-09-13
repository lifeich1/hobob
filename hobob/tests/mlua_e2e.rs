//! M3 T5：lua 动态 system 端到端（独立 binary，与 `tests/logkv_*.rs` 不共享进程）。
//!
//! 覆盖 lib 测试拿不到的**进程边界**：
//! - 场景 A「跨 store reopen 真实重启」：`close()` → 重新 `open_or_create` → `WeiYuanHui::open`
//!   从 systems 表自动重编译注册 + 分组成员关系持久化。
//! - 场景 B「www 路由投递链路」：`www::build_app` → POST `/op/reload_system` 返回**投递**结果
//!   → hub `run()` drain admin 通道 → reload 后 system 仍可正常执行；`/op/reload_all` 同理。
//!   注：本 binary 无法在线改库（`WeiYuanHui::store_mut` 是 `pub(crate)`），
//!   「改库换回调后 reload 生效」由 `src/www.rs::test_op_reload_system_and_all`（lib 测试）覆盖。
//! - 场景 C「沙箱拒绝汇总」：os/io/require/loadfile/dofile/debug/package 全部 error（非 panic）。
//!
//! 刻意不 `attach_logkv()`：log4rs / logkv 全局静态在进程内只能初始化一次，
//! 独立 binary + 不挂 KV appender 避免与 `tests/logkv_*.rs` 交叉污染。
//!
//! 事件源说明：本 binary 只能用公开 API 产生 `#SYSEV#`（`Snapshot::push_sys_event` 是
//! `pub(crate)`），故用 `Snapshot::follow` → `up_state/followed` 事件驱动 lua 回调。
//! condition 与回调都按 `event.state == "followed"` 过滤——`toggle_group` 会回踢
//! `up_state/group_toggled`，不过滤会命中自身造成事件自增。

use hobob::db::{Brick, GroupInfo, Snapshot, WeiYuanHui};
use hobob::ecs::Entity;
use hobob::libcall;
use hobob::store::{Store, StoreConfig, SystemSpecV1};
use hobob::www;
use serde_json::{json, Value};
use warp::reply::Reply;

/// 某 uid 所在分组的 gid 列表（排序后，便于断言）。
fn group_gids_of(snap: &Snapshot, uid: i64) -> Vec<u64> {
    let Some(&eid) = snap.res.uid_index.get(&uid.to_string()) else {
        return Vec::new();
    };
    let Some(brick) = snap.world.get::<Brick>(Entity(eid)) else {
        return Vec::new();
    };
    let mut out: Vec<u64> = brick
        .groups
        .iter()
        .filter_map(|&ge| snap.world.get::<GroupInfo>(Entity(ge)).map(|g| g.gid))
        .collect();
    out.sort_unstable();
    out
}

/// 写一条 `SystemSpecV1` 到 store（`open` 会全量加载）。
fn put_spec(store: &Store, name: &str, lua: &str, condition: &str) -> anyhow::Result<()> {
    store.put_system(&SystemSpecV1 {
        name: name.into(),
        lua: lua.into(),
        condition: condition.into(),
    })
}

/// 提交一条空 update，避免 `run()` 阻塞在 updates 队列上（reload 走独立 admin 通道）。
fn poke(center: &mut WeiYuanHui) {
    let mut chair = center.new_chair();
    chair
        .apply(|_: &mut Snapshot| -> anyhow::Result<()> { Ok(()) })
        .expect("poke update");
}

/// 场景 A：close → reopen 后 systems 表自动重编译注册 + 分组持久化。
#[tokio::test]
async fn e2e_reopen_recompiles_systems_and_persists_group() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cfg = StoreConfig::new(dir.path().join("state.redb"));

    // 首次：写 event spec（`up_state/followed` → 加入分组 2）
    let store = Store::open_or_create(&cfg).expect("open store");
    put_spec(
        &store,
        "on_followed",
        "function(event, ctx) \
         if event.state == 'followed' then \
           ctx.admin.toggle_group(event.uid, 2) \
         end end",
        "event.kind == 'up_state' and event.state == 'followed'",
    )
    .expect("put spec");

    let mut center = WeiYuanHui::open(store).expect("hub open");
    assert!(
        center.has_lua_system("on_followed"),
        "open 应从 systems 表加载并编译注册 lua system"
    );

    // follow → up_state/followed 事件（hub 分发）→ lua 回调改分组
    let mut chair = center.new_chair();
    chair
        .apply(|b| b.follow(&json!({"uid": 12345})))
        .expect("follow");
    assert!(center.run().await, "hub 处理 follow 并分发 lua 回调");
    assert_eq!(
        group_gids_of(center.bench(), 12345),
        vec![2],
        "lua 回调应把 uid 12345 加入分组 2"
    );

    center.close();

    // 真实重启：reopen 同一 state.redb → systems 表重编译注册 + 分组持久化
    let store2 = Store::open_or_create(&cfg).expect("reopen store");
    let mut center2 = WeiYuanHui::open(store2).expect("hub reopen");
    assert!(
        center2.has_lua_system("on_followed"),
        "重启后应从 systems 表重新编译注册"
    );
    assert_eq!(
        group_gids_of(center2.bench(), 12345),
        vec![2],
        "重启后分组成员关系持久化"
    );
    center2.close();
}

/// 场景 B：www 路由投递链路（`/op/reload_system`、`/op/reload_all`）+ reload 后 system 可用。
#[tokio::test]
async fn e2e_www_reload_route_delivers_and_system_keeps_working() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cfg = StoreConfig::new(dir.path().join("state.redb"));

    let store = Store::open_or_create(&cfg).expect("open store");
    put_spec(
        &store,
        "a.reload",
        "function(event, ctx) \
         if event.state == 'followed' then ctx.admin.refresh(event.uid) end end",
        "event.kind == 'up_state'",
    )
    .expect("put spec");

    let mut center = WeiYuanHui::open(store).expect("hub open");
    assert!(center.has_lua_system("a.reload"));

    let app = www::build_app(&mut center);

    // 1) 投递成功（fire-and-forget）：HTTP 返回 "success"
    let resp = warp::test::request()
        .method("POST")
        .path("/op/reload_system")
        .json(&json!({"name": "a.reload"}))
        .filter(&app)
        .await
        .expect("route")
        .into_response();
    assert_eq!(resp.status(), hyper::StatusCode::OK);
    let body = hyper::body::to_bytes(resp.into_body())
        .await
        .expect("body");
    assert_eq!(
        serde_json::from_slice::<Value>(&body).expect("json"),
        json!("success"),
        "路由应返回投递结果"
    );

    // 2) hub 消费 admin 通道并执行 reload
    poke(&mut center);
    assert!(center.run().await, "hub drain admin 请求并执行 reload");
    assert!(
        center.has_lua_system("a.reload"),
        "reload 后 system 仍注册（重新编译成功）"
    );

    // 3) reload 后 system 仍正常工作：follow → up_state → 回调 refresh → fetch 命令入队
    let mut chair = center.new_chair();
    chair
        .apply(|b| b.follow(&json!({"uid": 22222})))
        .expect("follow");
    assert!(center.run().await);
    assert!(
        center
            .bench()
            .res
            .commands
            .iter()
            .any(|c| *c == json!({"cmd": "fetch", "args": {"uid": 22222}})),
        "reload 后 lua 回调仍执行（refresh 产生 fetch 命令）: {:?}",
        center.bench().res.commands
    );

    // 4) 缺 name：路由层拒绝（不投递，返回 err）
    let resp = warp::test::request()
        .method("POST")
        .path("/op/reload_system")
        .json(&json!({}))
        .filter(&app)
        .await
        .expect("route")
        .into_response();
    let body = hyper::body::to_bytes(resp.into_body())
        .await
        .expect("body");
    assert!(
        String::from_utf8_lossy(&body).contains("missing string field"),
        "缺 name 应返回 err：{}",
        String::from_utf8_lossy(&body)
    );

    // 5) `/op/reload_all`：全量重载后 system 仍在
    let resp = warp::test::request()
        .method("POST")
        .path("/op/reload_all")
        .json(&json!({}))
        .filter(&app)
        .await
        .expect("route")
        .into_response();
    assert_eq!(resp.status(), hyper::StatusCode::OK);
    let body = hyper::body::to_bytes(resp.into_body())
        .await
        .expect("body");
    assert_eq!(
        serde_json::from_slice::<Value>(&body).expect("json"),
        json!("success")
    );
    poke(&mut center);
    assert!(center.run().await);
    assert!(
        center.has_lua_system("a.reload"),
        "reload_all 后仍注册（store 中 spec 未删）"
    );

    // 6) 重启后经 HTTP 触发的 reload 语义不受影响（registry 是进程内状态）
    center.close();
    let store2 = Store::open_or_create(&cfg).expect("reopen store");
    let mut center2 = WeiYuanHui::open(store2).expect("hub reopen");
    assert!(center2.has_lua_system("a.reload"), "重启后重新注册");
    center2.close();
}

/// 场景 C：沙箱拒绝 IO/OS/PACKAGE/DEBUG 能力（返回 error，不是 panic）。
#[test]
fn e2e_sandbox_denies_dangerous_capabilities() {
    let lua = libcall::new_sandbox().expect("sandbox lua");
    let cases = [
        ("os.execute", "os.execute('true')"),
        ("os.getenv", "os.getenv('HOME')"),
        ("io.open", "io.open('/etc/passwd')"),
        ("io.lines", "io.lines('/etc/passwd')"),
        ("require", "require('os')"),
        ("package.loaded", "package.loaded"),
        ("loadfile", "loadfile('/etc/passwd')"),
        ("dofile", "dofile('/etc/passwd')"),
        ("debug.getinfo", "debug.getinfo(1)"),
        ("debug.sethook", "debug.sethook(function() end, '', 1)"),
    ];
    for (label, src) in cases {
        assert!(
            lua.load(src).exec().is_err(),
            "{label} 应被沙箱拒绝（返回 error）"
        );
    }

    // 对照：安全标准库 + `json` 桥仍可用
    assert!(
        lua.load("return string.upper('ok'), math.floor(1.5), table.concat({'a','b'}, ',')")
            .exec()
            .is_ok(),
        "安全标准库应可用"
    );
    assert!(
        lua.load("return json.encode({a = 1})").exec().is_ok(),
        "json.encode 应可用"
    );
}

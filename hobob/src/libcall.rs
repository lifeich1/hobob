//! libcall 接口：lua 与 Rust 能力的桥梁
//!
//! 分两组：
//! - **admin**：管理操作（follow/unfollow/toggle_group/new_group/refresh/get_state/
//!   set_silent/register_system + last_error），直接改 `&mut Snapshot`
//! - **bapi**：B 站 API 封装（info/latest_videos/recent_posts/card/live_info/xlive_recommend），
//!   同步网络调用（`spawn_blocking` + 新 runtime，带本地 5s 兜底超时）
//!
//! 现状与缺口（T3 待补）：
//! - `register_system` 只落盘，尚未编译注册进 `DynSystemRegistry`
//! - `set_silent` 依赖的 `Snapshot::force_silence` 仍是 stub，故当前恒返回 `false` + `last_error`
//! - 尚无 `unregister_system`/`reload_system`/`reload_all`（计划 §4.2 / D4）
//!
//! 使用方式：
//! 1. `new_sandbox()` 创建沙箱 Lua 实例（已注册 `json.encode`/`json.decode` 到全局）
//! 2. `build_ctx_table(lua, scope, snap, store)` 在每次 lua 回调 call 前构建 ctx 表
//! 3. lua 回调内通过 `ctx.admin.follow(uid, enable)` 或 `ctx.bapi.info(uid)` 调用
//!
//! ctx 表不进 lua 全局环境（D14），每次 call 新建。

use std::cell::RefCell;
use std::rc::Rc;

use crate::db::Snapshot;
use crate::store::{Store, SystemSpecV1};
use anyhow::anyhow;
use bilibili_api_rs::Client;
use mlua::{Lua, LuaSerdeExt, StdLib, Table};
use serde_json::{json, Value as JValue};

// ============================== 沙箱与全局 ==============================

/// 创建沙箱 Lua 实例（禁止 IO/OS/PACKAGE，注册 json 编解码到全局）。
/// DEBUG 由 `new_with` 安全构造函数自动拒绝。
pub fn new_sandbox() -> mlua::Result<Lua> {
    // `^` 在此等价「差集」的前提是 IO/OS/PACKAGE 都在 ALL_SAFE 内（有测试钉住该前提）；
    // mlua 未给 StdLib 实现 `Not`/`Sub`，写不出真正的差集。
    let libs = StdLib::ALL_SAFE ^ (StdLib::IO | StdLib::OS | StdLib::PACKAGE);
    let lua = Lua::new_with(libs, Default::default())?;
    register_json_globals(&lua)?;
    Ok(lua)
}

/// 注册 `json.encode` / `json.decode` 到 `lua.globals()`（'static 无借用）。
pub fn register_json_globals(lua: &Lua) -> mlua::Result<()> {
    let json_tbl = lua.create_table()?;
    let encode_fn = lua.create_function(|lua, val: mlua::Value| -> mlua::Result<String> {
        let jv: JValue = lua
            .from_value(val)
            .map_err(|e| mlua::Error::external(format!("json.encode: {e}")))?;
        serde_json::to_string(&jv)
            .map_err(|e| mlua::Error::external(format!("json.encode: {e}")))
    })?;
    json_tbl.set("encode", encode_fn)?;

    let decode_fn = lua.create_function(|lua, s: String| -> mlua::Result<mlua::Value> {
        let jv: JValue = serde_json::from_str(&s)
            .map_err(|e| mlua::Error::external(format!("json.decode: {e}")))?;
        lua.to_value(&jv)
            .map_err(|e| mlua::Error::external(format!("json.decode: {e}")))
    })?;
    json_tbl.set("decode", decode_fn)?;
    lua.globals().set("json", json_tbl)?;
    Ok(())
}

// ============================== ctx 表构建 ==============================

/// 构建 ctx 表（scope 内，admin 借用 snap，bapi 无借用）。
///
/// 必须在 `lua.scope(...)` 内调用；scope 结束前必须使用完返回的 ctx 表。
pub fn build_ctx_table<'scope>(
    lua: &Lua,
    scope: &'scope mlua::Scope<'scope, '_>,
    snap: &'scope mut Snapshot,
    store: &'scope Store,
) -> mlua::Result<Table> {
    let snap_rc = Rc::new(RefCell::new(snap));
    let last_error = Rc::new(RefCell::new(String::new()));

    let admin = lua.create_table()?;
    let bapi = bapi_table(lua)?;

    // ---- admin：follow(uid: int, enable: bool) -> bool ----
    let snap1 = Rc::clone(&snap_rc);
    let err1 = Rc::clone(&last_error);
    let follow_fn = scope.create_function(
        move |_, (uid, enable): (i64, bool)| -> mlua::Result<bool> {
            if !valid_arg(&err1, "uid", uid) {
                return Ok(false);
            }
            let opt = json!({"uid": uid, "enable": enable});
            match with_snap(&snap1, |s| s.follow(&opt)) {
                Ok(()) => Ok(true),
                Err(e) => {
                    *err1.borrow_mut() = e.to_string();
                    Ok(false)
                }
            }
        },
    )?;
    admin.set("follow", follow_fn)?;

    // ---- admin：unfollow(uid: int) -> bool ----
    let snap1 = Rc::clone(&snap_rc);
    let err1 = Rc::clone(&last_error);
    let unfollow_fn = scope.create_function(move |_, uid: i64| -> mlua::Result<bool> {
        if !valid_arg(&err1, "uid", uid) {
            return Ok(false);
        }
        let opt = json!({"uid": uid, "enable": false});
        match with_snap(&snap1, |s| s.follow(&opt)) {
            Ok(()) => Ok(true),
            Err(e) => {
                *err1.borrow_mut() = e.to_string();
                Ok(false)
            }
        }
    })?;
    admin.set("unfollow", unfollow_fn)?;

    // ---- admin：toggle_group(uid: int, gid: int) -> bool ----
    let snap1 = Rc::clone(&snap_rc);
    let err1 = Rc::clone(&last_error);
    let toggle_group_fn = scope.create_function(
        move |_, (uid, gid): (i64, i64)| -> mlua::Result<bool> {
            if !valid_arg(&err1, "uid", uid) || !valid_arg(&err1, "gid", gid) {
                return Ok(false);
            }
            let opt = json!({"uid": uid, "gid": gid});
            match with_snap(&snap1, |s| s.toggle_group(&opt)) {
                Ok(()) => Ok(true),
                Err(e) => {
                    *err1.borrow_mut() = e.to_string();
                    Ok(false)
                }
            }
        },
    )?;
    admin.set("toggle_group", toggle_group_fn)?;

    // ---- admin：new_group(name: string, pin: bool) -> int|nil ----
    let snap1 = Rc::clone(&snap_rc);
    let err1 = Rc::clone(&last_error);
    let new_group_fn = scope.create_function(
        move |_, (name, pin): (String, bool)| -> mlua::Result<mlua::Value> {
            let gid = with_snap(&snap1, |s| {
                // 内置组 0/1 恒存在，用户组从 2 起；索引为空时也不能回落到 1（那是「特殊关注」）
                let new_gid = s
                    .res
                    .gid_index
                    .keys()
                    .copied()
                    .filter(|g| *g >= 2)
                    .max()
                    .unwrap_or(1)
                    + 1;
                let opt = json!({"gid": new_gid as i64, "pin": pin, "name": name});
                s.touch_group(&opt).map_err(|e| anyhow!("{e}"))?;
                Ok(new_gid as i64)
            });
            match gid {
                Ok(gid) => Ok(mlua::Value::Integer(gid)),
                Err(e) => {
                    *err1.borrow_mut() = e.to_string();
                    Ok(mlua::Value::Nil)
                }
            }
        },
    )?;
    admin.set("new_group", new_group_fn)?;

    // ---- admin：set_silent(uid: int, silent: bool) -> bool ----
    let snap1 = Rc::clone(&snap_rc);
    let err1 = Rc::clone(&last_error);
    let set_silent_fn = scope.create_function(
        move |_, (uid, silent): (i64, bool)| -> mlua::Result<bool> {
            if !valid_arg(&err1, "uid", uid) {
                return Ok(false);
            }
            // 参数透传，便于 `force_silence` 落地后直接生效；但 `Snapshot::force_silence`
            // 目前是 stub（忽略 opt，仅 bucket_double_gap），返回 Ok 不代表真的静音了，
            // 因此这里仍返回 false——不能给 lua 侧假成功。T6 实现后把 `Ok(())` 分支
            // 改成 `Ok(true)` 即可。
            match with_snap(&snap1, |s| {
                s.force_silence(&json!({"uid": uid, "silent": silent}))
            }) {
                Ok(()) => {
                    *err1.borrow_mut() = "set_silent: force_silence 尚未实现（M3 T6 待补）".into();
                    Ok(false)
                }
                Err(e) => {
                    *err1.borrow_mut() = e.to_string();
                    Ok(false)
                }
            }
        },
    )?;
    admin.set("set_silent", set_silent_fn)?;

    // ---- admin：refresh(uid: int) -> bool ----
    let snap1 = Rc::clone(&snap_rc);
    let err1 = Rc::clone(&last_error);
    let refresh_fn = scope.create_function(move |_, uid: i64| -> mlua::Result<bool> {
        if !valid_arg(&err1, "uid", uid) {
            return Ok(false);
        }
        let opt = json!({"uid": uid});
        match with_snap(&snap1, |s| s.refresh(&opt)) {
            Ok(()) => Ok(true),
            Err(e) => {
                *err1.borrow_mut() = e.to_string();
                Ok(false)
            }
        }
    })?;
    admin.set("refresh", refresh_fn)?;

    // ---- admin：get_state(uid: int) -> table|nil ----
    let snap1 = Rc::clone(&snap_rc);
    let err1 = Rc::clone(&last_error);
    let get_state_fn = scope.create_function(
        move |lua, uid: i64| -> mlua::Result<mlua::Value> {
            if !valid_arg(&err1, "uid", uid) {
                return Ok(mlua::Value::Nil);
            }
            let jv = with_snap(&snap1, |s| s.pick_of(uid));
            match jv {
                Ok(jv) => lua
                    .to_value(&jv)
                    .map_err(|e| mlua::Error::external(format!("get_state to_lua: {e}"))),
                Err(e) => {
                    *err1.borrow_mut() = e.to_string();
                    Ok(mlua::Value::Nil)
                }
            }
        },
    )?;
    admin.set("get_state", get_state_fn)?;

    // ---- admin：register_system(name, lua, condition) -> bool ----
    // 目前只落盘（store.put_system）；注册进 registry 与 `lib.` 前缀/condition 校验属 T3。
    let err1 = Rc::clone(&last_error);
    let register_system_fn = scope.create_function(
        move |_, (name, lua_src, condition): (String, String, String)| -> mlua::Result<bool> {
            let spec = SystemSpecV1 {
                name,
                lua: lua_src,
                condition,
            };
            match store.put_system(&spec) {
                Ok(()) => Ok(true),
                Err(e) => {
                    *err1.borrow_mut() = e.to_string();
                    Ok(false)
                }
            }
        },
    )?;
    admin.set("register_system", register_system_fn)?;

    // ---- admin：last_error() -> string ----
    let err1 = Rc::clone(&last_error);
    let last_error_fn =
        scope.create_function(move |_, ()| -> mlua::Result<String> { Ok(err1.borrow().clone()) })?;
    admin.set("last_error", last_error_fn)?;

    // ---- ctx 表 ----
    let ctx = lua.create_table()?;
    ctx.set("admin", admin)?;
    ctx.set("bapi", bapi)?;

    Ok(ctx)
}

// ============================== 辅助函数 ==============================

/// 校验 lua 传入的非负整数参数：非法时写 `last_error` 并返回 `false`，
/// 调用方据此直接 `return Ok(false)`（/`Ok(mlua::Value::Nil)`）。
///
/// lua 侧 `integer` 是 i64，负数会在 `Snapshot` 内部撞上 `as_u64().expect(...)`
/// 之类的断言，所以必须在这一层先挡（同时 schema 侧补 `minimum: 0` 兜底）。
fn valid_arg(err: &RefCell<String>, name: &str, v: i64) -> bool {
    if v < 0 {
        *err.borrow_mut() = format!("{name} 非法：{v}（需 >= 0）");
        false
    } else {
        true
    }
}

/// 在共享的可变快照借用上执行闭包。
fn with_snap<T>(
    snap_rc: &Rc<RefCell<&mut Snapshot>>,
    f: impl FnOnce(&mut Snapshot) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let mut guard = snap_rc
        .try_borrow_mut()
        .map_err(|_| anyhow!("snapshot already borrowed (re-entrant libcall?)"))?;
    f(&mut guard)
}

/// bapi 调用的本地兜底超时（对齐计划 D12 的「hub 冻结 ≤5s」）。
const BAPI_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// 同步阻塞执行 async API（方案 A：spawn_blocking + 新 runtime）。
///
/// 仅在 bapi 内部使用。`BAPI_TIMEOUT` 只是**本地兜底**，并不代表任务被取消：
/// `spawn_blocking` 任务不可 abort，超时后请求仍会在后台跑完，结果被丢弃。
/// 真正的隔离强度取决于 T3 dispatch 侧的超时策略。
fn bapi_block_on<T: Send + 'static>(
    fut: impl std::future::Future<Output = anyhow::Result<T>> + Send + 'static,
) -> anyhow::Result<T> {
    bapi_block_on_with(fut, BAPI_TIMEOUT)
}

/// `bapi_block_on` 的可注入超时版本（测试用短超时，避免真等 5s）。
fn bapi_block_on_with<T: Send + 'static>(
    fut: impl std::future::Future<Output = anyhow::Result<T>> + Send + 'static,
    timeout: std::time::Duration,
) -> anyhow::Result<T> {
    let (tx, rx) = std::sync::mpsc::channel();
    tokio::task::spawn_blocking(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("new current-thread rt for bapi");
        let result = rt.block_on(fut);
        let _ = tx.send(result);
    });
    rx.recv_timeout(timeout)
        .map_err(|e| match e {
            std::sync::mpsc::RecvTimeoutError::Timeout => {
                anyhow!("bapi: timeout after {timeout:?}（请求仍在后台执行，结果丢弃）")
            }
            std::sync::mpsc::RecvTimeoutError::Disconnected => {
                anyhow!("bapi: channel closed without result")
            }
        })?
}

/// 构建 bapi 表（'static 闭包，无 snap 借用）。
fn bapi_table(lua: &Lua) -> mlua::Result<Table> {
    let bapi = lua.create_table()?;

    let info_fn = bapi_user_fn(lua, "info", |client, uid| {
        let mut client = client;
        bapi_block_on(async move { client.user(uid).info().await.map_err(|e| anyhow!("{e}")) })
    })?;
    bapi.set("info", info_fn)?;

    let latest_videos_fn = bapi_user_fn(lua, "latest_videos", |client, uid| {
        let mut client = client;
        bapi_block_on(async move { client.user(uid).latest_videos().await.map_err(|e| anyhow!("{e}")) })
    })?;
    bapi.set("latest_videos", latest_videos_fn)?;

    let recent_posts_fn = bapi_user_fn(lua, "recent_posts", |client, uid| {
        let mut client = client;
        bapi_block_on(async move { client.user(uid).recent_posts().await.map_err(|e| anyhow!("{e}")) })
    })?;
    bapi.set("recent_posts", recent_posts_fn)?;

    let card_fn = bapi_user_fn(lua, "card", |client, uid| {
        let mut client = client;
        bapi_block_on(async move { client.user(uid).card().await.map_err(|e| anyhow!("{e}")) })
    })?;
    bapi.set("card", card_fn)?;

    let live_info_fn = bapi_user_fn(lua, "live_info", |client, uid| {
        let mut client = client;
        bapi_block_on(async move { client.user(uid).live_info().await.map_err(|e| anyhow!("{e}")) })
    })?;
    bapi.set("live_info", live_info_fn)?;

    let xlive_recommend_fn = lua.create_function(
        |lua, (area, sub, pn): (i64, i64, i64)| -> mlua::Result<mlua::MultiValue> {
            let mut client = Client::new();
            let result = bapi_block_on(async move {
                client
                    .xlive(area, sub)
                    .list(pn)
                    .await
                    .map_err(|e| anyhow!("{e}"))
            });
            match result {
                Ok(jv) => to_lua_result(lua, &jv).map(|v| mlua::MultiValue::from_vec(vec![v])),
                Err(e) => Ok(mlua::MultiValue::from_vec(vec![
                    mlua::Value::Nil,
                    mlua::Value::String(lua.create_string(format!("bapi.xlive_recommend: {e}"))?),
                ])),
            }
        },
    )?;
    bapi.set("xlive_recommend", xlive_recommend_fn)?;

    Ok(bapi)
}

/// 包装同步 bapi 调用为 lua 函数：成功 `(table)`，失败 `(nil, err_msg)`。
fn bapi_user_fn(
    lua: &Lua,
    name: &str,
    call: impl Fn(Client, i64) -> anyhow::Result<JValue> + Copy + 'static,
) -> mlua::Result<mlua::Function> {
    let name = format!("bapi.{name}");
    lua.create_function(move |lua, uid: i64| -> mlua::Result<mlua::MultiValue> {
        let client = Client::new();
        match call(client, uid) {
            Ok(jv) => to_lua_result(lua, &jv).map(|v| mlua::MultiValue::from_vec(vec![v])),
            Err(e) => Ok(mlua::MultiValue::from_vec(vec![
                mlua::Value::Nil,
                mlua::Value::String(lua.create_string(format!("{name}: {e}"))?),
            ])),
        }
    })
}

/// serde::Serialize → lua 值（经 serde_json 桥）。
fn to_lua_result(lua: &Lua, v: &impl serde::Serialize) -> mlua::Result<mlua::Value> {
    let jv = serde_json::to_value(v)
        .map_err(|e| mlua::Error::external(format!("serialize to json: {e}")))?;
    lua.to_value(&jv)
        .map_err(|e| mlua::Error::external(format!("to_lua: {e}")))
}

// ============================== 测试 ==============================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::StoreConfig;
    use tempfile::tempdir;

    /// 创建沙箱 Lua + 临时 Store。
    ///
    /// `TempDir` 必须由调用方持有：直接 drop 会把 redb 文件 unlink，断言就落在已删除文件上。
    fn setup() -> anyhow::Result<(Lua, Store, tempfile::TempDir)> {
        let lua = new_sandbox().map_err(|e| anyhow!("{e}"))?;
        let dir = tempdir()?;
        let cfg = StoreConfig::new(dir.path().join("test.redb"));
        let store = Store::open_or_create(&cfg)?;
        Ok((lua, store, dir))
    }

    #[test]
    fn test_admin_follow_uid() -> anyhow::Result<()> {
        let (lua, store, _dir) = setup()?;
        let mut snap = Snapshot::new();
        let uid: i64 = 12345;

        lua.scope(|scope| {
            let ctx = build_ctx_table(&lua, scope, &mut snap, &store)?;
            let chunk = lua
                .load(
                    r#"
                    local ctx = ...
                    local ok = ctx.admin.follow(12345, true)
                    assert(ok, "follow should succeed")
                    "#,
                )
                .into_function()?;
            chunk.call::<()>(ctx)?;
            Ok(())
        })
        .map_err(|e| anyhow!("{e}"))?;

        assert!(snap.res.uid_index.contains_key(&uid.to_string()));
        let pick = snap.pick_of(uid)?;
        assert_eq!(pick["basic"]["ban"], false);
        Ok(())
    }

    #[test]
    fn test_admin_unfollow_uid() -> anyhow::Result<()> {
        let (lua, store, _dir) = setup()?;
        let mut snap = Snapshot::new();
        let uid: i64 = 12346;

        lua.scope(|scope| {
            let ctx = build_ctx_table(&lua, scope, &mut snap, &store)?;
            let chunk = lua
                .load(
                    r#"
                    local ctx = ...
                    local ok = ctx.admin.follow(12346, true)
                    assert(ok, "follow should succeed")
                    "#,
                )
                .into_function()?;
            chunk.call::<()>(ctx)?;
            Ok(())
        })
        .map_err(|e| anyhow!("{e}"))?;

        lua.scope(|scope| {
            let ctx = build_ctx_table(&lua, scope, &mut snap, &store)?;
            let chunk = lua
                .load(
                    r#"
                    local ctx = ...
                    local ok = ctx.admin.unfollow(12346)
                    assert(ok, "unfollow should succeed")
                    "#,
                )
                .into_function()?;
            chunk.call::<()>(ctx)?;
            Ok(())
        })
        .map_err(|e| anyhow!("{e}"))?;

        let pick = snap.pick_of(uid)?;
        assert_eq!(pick["basic"]["ban"], true);
        Ok(())
    }

    #[test]
    fn test_admin_toggle_group() -> anyhow::Result<()> {
        let (lua, store, _dir) = setup()?;
        let mut snap = Snapshot::new();
        let uid: i64 = 12347;
        snap.follow(&json!({"uid": uid, "enable": true}))?;
        snap.touch_group(&json!({"gid": 100, "pin": false, "name": "test_group"}))?;

        lua.scope(|scope| {
            let ctx = build_ctx_table(&lua, scope, &mut snap, &store)?;
            let chunk = lua
                .load(
                    r#"
                    local ctx = ...
                    local ok = ctx.admin.toggle_group(12347, 100)
                    assert(ok, "toggle_group should succeed")
                    "#,
                )
                .into_function()?;
            chunk.call::<()>(ctx)?;
            Ok(())
        })
        .map_err(|e| anyhow!("{e}"))?;

        let eid = *snap.res.uid_index.get(&uid.to_string()).expect("uid traced");
        let ge = *snap.res.gid_index.get(&100).expect("group 100 exists");
        let brick = snap.world.get::<crate::db::Brick>(crate::ecs::Entity(eid)).expect("brick");
        assert!(brick.groups.contains(&ge), "group entity {ge} should be in brick.groups");
        Ok(())
    }

    #[test]
    fn test_admin_new_group() -> anyhow::Result<()> {
        let (lua, store, _dir) = setup()?;
        let mut snap = Snapshot::new();

        lua.scope(|scope| {
            let ctx = build_ctx_table(&lua, scope, &mut snap, &store)?;
            let chunk = lua
                .load(
                    r#"
                    local ctx = ...
                    local gid = ctx.admin.new_group("my_group", false)
                    assert(type(gid) == "number", "new_group should return a number, got " .. type(gid))
                    "#,
                )
                .into_function()?;
            chunk.call::<()>(ctx)?;
            Ok(())
        })
        .map_err(|e| anyhow!("{e}"))?;

        // gid 2 应存在（0=全部, 1=特殊关注, 2=新建组）
        assert!(snap.res.gid_index.contains_key(&2));
        Ok(())
    }

    #[test]
    fn test_admin_get_state() -> anyhow::Result<()> {
        let (lua, store, _dir) = setup()?;
        let mut snap = Snapshot::new();
        snap.follow(&json!({"uid": 12348, "enable": true}))?;

        lua.scope(|scope| {
            let ctx = build_ctx_table(&lua, scope, &mut snap, &store)?;
            let chunk = lua
                .load(
                    r#"
                    local ctx = ...
                    local state = ctx.admin.get_state(12348)
                    assert(state ~= nil, "get_state should return a table")
                    "#,
                )
                .into_function()?;
            chunk.call::<()>(ctx)?;
            Ok(())
        })
        .map_err(|e| anyhow!("{e}"))?;
        Ok(())
    }

    #[test]
    fn test_admin_last_error_on_missing_uid() -> anyhow::Result<()> {
        let (lua, store, _dir) = setup()?;
        let mut snap = Snapshot::new();

        lua.scope(|scope| {
            let ctx = build_ctx_table(&lua, scope, &mut snap, &store)?;
            let chunk = lua
                .load(
                    r#"
                    local ctx = ...
                    local ok = ctx.admin.toggle_group(99999, 100)
                    assert(ok == false, "toggle_group on unknown uid should fail")
                    local err = ctx.admin.last_error()
                    assert(#err > 0, "last_error should be non-empty")
                    "#,
                )
                .into_function()?;
            chunk.call::<()>(ctx)?;
            Ok(())
        })
        .map_err(|e| anyhow!("{e}"))?;
        Ok(())
    }

    #[test]
    fn test_admin_register_system() -> anyhow::Result<()> {
        let (lua, store, _dir) = setup()?;
        let mut snap = Snapshot::new();

        lua.scope(|scope| {
            let ctx = build_ctx_table(&lua, scope, &mut snap, &store)?;
            let chunk = lua
                .load(
                    r#"
                    local ctx = ...
                    local ok = ctx.admin.register_system("test_sys", "return 1", "")
                    assert(ok, "register_system should succeed")
                    "#,
                )
                .into_function()?;
            chunk.call::<()>(ctx)?;
            Ok(())
        })
        .map_err(|e| anyhow!("{e}"))?;

        let spec = store.get_system("test_sys")?.expect("should exist");
        assert_eq!(spec.name, "test_sys");
        assert_eq!(spec.lua, "return 1");
        Ok(())
    }

    #[test]
    fn test_json_globals() -> anyhow::Result<()> {
        let lua = new_sandbox().map_err(|e| anyhow!("{e}"))?;

        let result: String = lua
            .load(r#"return json.encode({a = 1, b = "hello"})"#)
            .eval().map_err(|e| anyhow!("{e}"))?;
        let parsed: JValue = serde_json::from_str(&result)?;
        assert_eq!(parsed["a"], 1);
        assert_eq!(parsed["b"], "hello");

        let decoded: mlua::Value = lua.load(r#"return json.decode('{"x": 42}')"#).eval().map_err(|e| anyhow!("{e}"))?;
        let tbl = decoded.as_table().expect("table");
        assert_eq!(tbl.get::<i64>("x").map_err(|e| anyhow!("{e}"))?, 42);
        Ok(())
    }

    #[test]
    fn test_sandbox_no_io_os_package() -> anyhow::Result<()> {
        let lua = new_sandbox().map_err(|e| anyhow!("{e}"))?;
        assert!(
            lua.load(r#"io.open("/etc/passwd")"#).eval::<mlua::Value>().is_err(),
            "io.open should be banned"
        );
        assert!(
            lua.load(r#"os.execute("echo test")"#).eval::<mlua::Value>().is_err(),
            "os.execute should be banned"
        );
        assert!(
            lua.load(r#"require("some_module")"#).eval::<mlua::Value>().is_err(),
            "require should be banned"
        );
        Ok(())
    }

    #[test]
    fn test_sandbox_debug_banned() -> anyhow::Result<()> {
        let lua = Lua::new_with(
            StdLib::ALL_SAFE ^ (StdLib::IO | StdLib::OS | StdLib::PACKAGE),
            Default::default(),
        )
        .map_err(|e| anyhow!("{e}"))?;
        // ALL_SAFE 不含 DEBUG（bit 31），安全构造函数也不会加载 unsafe 模块
        assert!(
            lua.load(r#"debug.getinfo(1)"#).eval::<mlua::Value>().is_err(),
            "debug should be banned"
        );
        Ok(())
    }

    #[test]
    fn test_sandbox_stdlib_available() -> anyhow::Result<()> {
        let lua = new_sandbox().map_err(|e| anyhow!("{e}"))?;
        let r: i64 = lua.load(r#"return math.floor(3.7)"#).eval().map_err(|e| anyhow!("{e}"))?;
        assert_eq!(r, 3);
        let r: String = lua.load(r#"return string.upper("hello")"#).eval().map_err(|e| anyhow!("{e}"))?;
        assert_eq!(r, "HELLO");
        let r: bool = lua.load(r#"return coroutine.isyieldable()"#).eval().map_err(|e| anyhow!("{e}"))?;
        assert!(!r);
        Ok(())
    }

    #[test]
    fn test_bapi_signature_present() -> anyhow::Result<()> {
        let (lua, store, _dir) = setup()?;
        let mut snap = Snapshot::new();
        lua.scope(|scope| {
            let ctx = build_ctx_table(&lua, scope, &mut snap, &store)?;
            let chunk = lua
                .load(
                    r#"
                    local ctx = ...
                    for _, name in ipairs({"info","latest_videos","recent_posts","card","live_info","xlive_recommend"}) do
                        assert(type(ctx.bapi[name]) == "function", "bapi." .. name .. " should be a function")
                    end
                    "#,
                )
                .into_function()?;
            chunk.call::<()>(ctx)?;
            Ok(())
        })
        .map_err(|e| anyhow!("{e}"))?;
        Ok(())
    }

    #[test]
    fn test_admin_negative_args_rejected() -> anyhow::Result<()> {
        let (lua, store, _dir) = setup()?;
        let mut snap = Snapshot::new();

        lua.scope(|scope| {
            let ctx = build_ctx_table(&lua, scope, &mut snap, &store)?;
            let chunk = lua
                .load(
                    r#"
                    local ctx = ...
                    -- 负 uid：全部 admin 入口拒绝，且不 panic
                    assert(ctx.admin.follow(-1, true) == false)
                    assert(#ctx.admin.last_error() > 0)
                    assert(ctx.admin.unfollow(-1) == false)
                    assert(ctx.admin.refresh(-1) == false)
                    assert(ctx.admin.get_state(-1) == nil)
                    assert(ctx.admin.toggle_group(-1, 2) == false)
                    assert(ctx.admin.set_silent(-1, true) == false)
                    -- 负 gid：只有 toggle_group 受影响
                    assert(ctx.admin.toggle_group(12345, -2) == false)
                    local err = ctx.admin.last_error()
                    assert(#err > 0, "last_error should be non-empty, got " .. err)
                    "#,
                )
                .into_function()?;
            chunk.call::<()>(ctx)?;
            Ok(())
        })
        .map_err(|e| anyhow!("{e}"))?;

        // 负参数不应产生任何实体
        assert!(snap.res.uid_index.is_empty(), "no uid should be traced");
        Ok(())
    }

    #[test]
    fn test_admin_set_silent_unimplemented() -> anyhow::Result<()> {
        let (lua, store, _dir) = setup()?;
        let mut snap = Snapshot::new();

        lua.scope(|scope| {
            let ctx = build_ctx_table(&lua, scope, &mut snap, &store)?;
            let chunk = lua
                .load(
                    r#"
                    local ctx = ...
                    -- force_silence 仍是 stub：必须返回 false，不能给假成功
                    assert(ctx.admin.set_silent(12345, true) == false)
                    local err = ctx.admin.last_error()
                    assert(#err > 0, "last_error should explain the stub, got " .. err)
                    "#,
                )
                .into_function()?;
            chunk.call::<()>(ctx)?;
            Ok(())
        })
        .map_err(|e| anyhow!("{e}"))?;
        Ok(())
    }

    // ---- bapi 同步桥（原先零覆盖） ----

    #[tokio::test]
    async fn test_bapi_block_on_bridge_ok() -> anyhow::Result<()> {
        let v = bapi_block_on(async { Ok::<_, anyhow::Error>(42_i64) })?;
        assert_eq!(v, 42);
        Ok(())
    }

    #[tokio::test]
    async fn test_bapi_block_on_bridge_err() -> anyhow::Result<()> {
        let e = bapi_block_on(async { Err::<i64, _>(anyhow!("boom")) }).unwrap_err();
        assert!(e.to_string().contains("boom"), "got {e}");
        Ok(())
    }

    /// 慢 future + 短超时 → 本地兜底生效。
    /// future 只睡 200ms：超时后任务仍在跑，测试退出时会等它收尾，别用长 sleep。
    #[tokio::test]
    async fn test_bapi_block_on_timeout() -> anyhow::Result<()> {
        let e = bapi_block_on_with(
            async {
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                Ok::<_, anyhow::Error>(1_i64)
            },
            std::time::Duration::from_millis(50),
        )
        .unwrap_err();
        assert!(e.to_string().contains("timeout"), "got {e}");
        Ok(())
    }

    /// 空 `gid_index`（无内置组）时也不能分配 1——那是「特殊关注」的保留号。
    #[test]
    fn test_admin_new_group_empty_index_starts_at_2() -> anyhow::Result<()> {
        let (lua, store, _dir) = setup()?;
        let mut snap = Snapshot::default();

        lua.scope(|scope| {
            let ctx = build_ctx_table(&lua, scope, &mut snap, &store)?;
            let chunk = lua
                .load(
                    r#"
                    local ctx = ...
                    local gid = ctx.admin.new_group("fresh", false)
                    assert(gid == 2, "expected gid 2, got " .. tostring(gid))
                    "#,
                )
                .into_function()?;
            chunk.call::<()>(ctx)?;
            Ok(())
        })
        .map_err(|e| anyhow!("{e}"))?;

        assert!(snap.res.gid_index.contains_key(&2), "gid 2 should exist");
        assert!(
            !snap.res.gid_index.contains_key(&1),
            "gid 1 must stay reserved"
        );
        Ok(())
    }

    /// 沙箱掩码前提：`ALL_SAFE ^ (IO|OS|PACKAGE)` 只有在三者都属于 ALL_SAFE 时才等价差集。
    #[test]
    fn test_sandbox_stdlib_mask_premise() {
        assert!(StdLib::ALL_SAFE.contains(StdLib::IO));
        assert!(StdLib::ALL_SAFE.contains(StdLib::OS));
        assert!(StdLib::ALL_SAFE.contains(StdLib::PACKAGE));
        assert!(!StdLib::ALL_SAFE.contains(StdLib::DEBUG));
        let masked = StdLib::ALL_SAFE ^ (StdLib::IO | StdLib::OS | StdLib::PACKAGE);
        assert!(!masked.contains(StdLib::IO));
        assert!(!masked.contains(StdLib::OS));
        assert!(!masked.contains(StdLib::PACKAGE));
        assert!(masked.contains(StdLib::MATH));
    }
}
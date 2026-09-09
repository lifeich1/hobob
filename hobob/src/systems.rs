//! 基础 system（M1 §4.5，原 `engine.rs` 迁入 + 动态 system 骨架）。
//!
//! - **fetch 系统**（`fetch_loop`）：原 engine 主循环平移——消费 `commands`、网络抓取、
//!   组件写回走 `Snapshot::apply_fetch`；成功/失败经 `Snapshot::push_sys_event` 上报
//!   `FetchDone`/`FetchFailed` 事件（hub 分发，SSE 形状不变，见 db `SYS_EVENT_TAG`）。
//! - **tick（内置 `builtin.tick`）**：v1 `exec_timers` 语义平移为动态 system handler——
//!   commands 为空时选 ctime 最小 up 补抓 + `bucket_hang`。触发路径：fetch_loop 在
//!   commands 为空且 deadline 唤醒时发 `Tick` 事件 → hub 分发 → handler 在权威快照上
//!   补抓（v1 节奏/语义逐行保留：bucket 速率控制仍在 fetch_loop `next_deadline`）。
//! - **动态 system 框架**（M1 骨架 + M3 执行语义）：`TriggerEvent` + `DynSystemRegistry`。
//!   M3 起 registry 两段式（D5）：`native_specs`（Rust 闭包，先执行）→ `lua_specs`
//!   （mlua 沙箱回调，后执行），各自按名序；handler 报错记日志继续，不中断后续 handler。
//!   store `systems` 表在 `WeiYuanHui::open` 时全量加载（D3），`lib.` 前缀 = oneshot
//!   库函数提供型 system（D16），其余 = 事件响应型。
//!
//! lua 执行模型（M3 T3 决议 B，见 `.plans/m3-mlua-dynsys.md` 执行偏差节）：
//! - **同步执行**在 hub 线程（不开 mlua `send` feature，`Lua` 为 `!Send`，与 libcall 的
//!   `Rc<RefCell<&mut Snapshot>>` ctx 构建天然配套）；因此**没有** D6 ② 的
//!   `spawn_blocking` + 5s 外层隔离。
//! - 超时只靠 D6 ①：`set_hook` 按指令计数（`LUA_INSTRUCTION_LIMIT`）中断死循环
//!   （1M 指令 ≈ 50ms）。回调内阻塞 Rust（bapi）由 libcall 自身 5s 兜底。
//! - 中断/报错后按 D13 实测点探针实例可用性：可复用则仅记日志，不可用且有 store 时
//!   全量重建 lua system（`load_from_store` 语义）。
//!
//! 与方案文档的偏差（实现备注）：
//! - fetch 系统保留**独立循环**（hub 串行执行会以网络等待阻塞 patch 处理，违背 v1
//!   解耦语义与 D13）；hub 仍是唯一分发点（try_push 尾），事件经 patch 通道上传。
//! - `DynSystemRegistry` 由 hub（`WeiYuanHui`）持有、不进 `Resources`（分发点唯一在
//!   hub，随快照传播只增 clone 成本且 chair 无用途）。
//! - registry 内部用 `Rc<RefCell<DynInner>>` 共享句柄（非 `Arc<Mutex>`）：dispatch 先把
//!   待执行 spec 收集成 `Vec` 再逐个调用，lua 回调内 `ctx.admin.register_system` 等可
//!   安全改写注册表（不持借用地重入）。句柄可克隆进 libcall ctx。
//! - **分发期新注册的 system 不参与当前事件**：先收集后执行意味着本事件的执行列表已固定，
//!   回调内 `register_system`/`unregister_system`/`reload_*` 从**下一个事件**起生效
//!   （同一事件的 condition/回调不会看到中途注册的 system）。

use crate::db::{Commands, Snapshot, WeiYuan};
use crate::libcall;
use crate::store::{Store, SystemSpecV1};
use anyhow::Context;
use anyhow::{anyhow, bail, Result};
use bilibili_api_rs::Client;
use chrono::{DateTime, Utc};
use mlua::{Function, HookTriggers, Lua, LuaSerdeExt, Table, Value as LuaValue, VmState};
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::Instant;

// ============================== 动态 system 框架 ==============================

/// 触发事件（M1 骨架；M3 为 lua 回调补 `uid`——lua 侧 `event.uid` 是主键）。
#[derive(Clone, Debug, PartialEq)]
pub enum TriggerEvent {
    /// hub 或 fetch 循环的节拍（v1 `exec_timers` 由 engine 每轮 deadline 唤醒触发）。
    Tick { at: DateTime<Utc> },
    FetchDone { entity: u64, uid: String },
    FetchFailed { entity: u64, uid: String, error: String },
    UpStateChanged {
        entity: u64,
        uid: String,
        kind: UpStateKind,
    },
}

/// up 管理状态变化类别（M1 只发 Followed/Unfollowed/GroupToggled；M3 起 `force_silence`
/// 发 Silenced——静音语义本身仍待 T6）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpStateKind {
    Followed,
    Unfollowed,
    GroupToggled,
    Silenced,
}

/// Rust 原生回调：`(&TriggerEvent, &mut Snapshot)`。错误由 registry 记日志继续。
pub type DynCallback = Arc<dyn Fn(&TriggerEvent, &mut Snapshot) -> Result<()> + Send + Sync>;

/// 原生（Rust）动态 system 描述。`condition` 对 native 无意义（M1 占位恒 `"always"`）。
#[derive(Clone)]
pub struct DynSystemSpec {
    pub name: String,
    /// M1 恒 `"always"`（占位）；condition 求值只对 lua system 生效（D15）。
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

/// lua 回调指令上限（D6 ①，≈50ms 纯计算）。
pub const LUA_INSTRUCTION_LIMIT: u64 = 1_000_000;
/// condition 求值指令上限（D15：condition 不该有循环）。
pub const LUA_CONDITION_INSTRUCTION_LIMIT: u64 = 100_000;
/// lua debug hook 采样间隔（指令数）：每 N 条指令回一次 Rust。
const LUA_HOOK_INTERVAL: u32 = 1_000;
/// oneshot（库函数提供型）system 的 name 前缀（D16）。
pub const ONESHOT_PREFIX: &str = "lib.";
/// 共享库表全局名（D16；所有 lua system 可读写，`load_from_store` 整表重置）。
pub const LIB_GLOBAL: &str = "_lib";

/// name 是否为 oneshot（`lib.` 前缀，D16）。
#[must_use]
pub fn is_oneshot(name: &str) -> bool {
    name.starts_with(ONESHOT_PREFIX)
}

/// lua 动态 system 编译产物（D1/D15/D16）。
///
/// `condition: None` = 恒真（`""`/`"always"`）；`Function` 是 mlua 的注册表引用句柄，
/// 与 `Lua` 实例同生命周期（无需再持有 `Arc<Lua>`，见模块文档执行偏差）。
#[derive(Clone)]
pub struct LuaSystemSpec {
    pub name: String,
    pub condition: Option<Function>,
    pub callback: Function,
}

impl std::fmt::Debug for LuaSystemSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LuaSystemSpec")
            .field("name", &self.name)
            .field("has_condition", &self.condition.is_some())
            .finish_non_exhaustive()
    }
}

/// 单个 system 的内存注册状态快照（回滚用，见 `DynSystemRegistry::saved_state`）。
///
/// 落盘失败时 `restore_state` 据此**恢复旧版本**——直接 `unregister` 会把原本可用的
/// 旧 spec 一并删掉（覆盖式注册已替换内存状态）。
pub enum SavedState {
    /// 原本无注册（回滚 = 删除）。
    Absent,
    /// event 型：旧编译产物。
    Lua(LuaSystemSpec),
    /// oneshot 型：`_lib.<短名>` 旧值（`Nil` = 原本无此条目）。
    Oneshot(LuaValue),
}

/// registry 可变状态（`Rc<RefCell<_>>` 共享句柄，见模块文档）。
struct DynInner {
    native_specs: BTreeMap<String, DynSystemSpec>,
    lua_specs: BTreeMap<String, LuaSystemSpec>,
    /// 共享库表 `_lib`（D16）。
    lib: Table,
}

/// 动态 system 注册表（hub 持有）。**两段式**：native（名序）→ lua（名序），串行执行。
///
/// 句柄可克隆（同一份 `DynInner` + 同一 Lua 实例），libcall ctx 借此支持 lua 内
/// `register_system`/`unregister_system`/`reload_system`/`reload_all`。
#[derive(Clone)]
pub struct DynSystemRegistry {
    inner: Rc<RefCell<DynInner>>,
    /// 共享沙箱 Lua 实例（所有 lua system 共用 `_G`，D14）。
    lua: Lua,
    /// 指令上限 hook 是否已激活（T3 修复）：mlua 的 hook 挂在 Lua 实例上是**全局唯一**
    /// 的，lua 回调内再调 `guarded_call`（`register_system`/`reload_*` → oneshot）若
    /// 无条件 `set_hook`/`remove_hook`，内层退出会把外层的 hook 一并拆掉，外层随后的
    /// 死循环就再也拦不住。故嵌套时**复用外层 hook 与预算**（见 `guarded_call`）。
    hook_active: Rc<Cell<bool>>,
}

impl std::fmt::Debug for DynSystemRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let inner = self.inner.borrow();
        f.debug_struct("DynSystemRegistry")
            .field("native_specs", &inner.native_specs.keys().collect::<Vec<_>>())
            .field("lua_specs", &inner.lua_specs.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

impl Default for DynSystemRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl DynSystemRegistry {
    /// 创建注册表：沙箱 Lua 实例（`libcall::new_sandbox`，D7）+ 空 `_lib` 表（D16）。
    ///
    /// # Panics
    /// Panic if the sandbox Lua instance / `_lib` table cannot be created (OOM).
    pub fn new() -> Self {
        let lua = libcall::new_sandbox().expect("create lua sandbox (out of memory?)");
        let lib = lua.create_table().expect("create _lib table");
        lua.globals().set(LIB_GLOBAL, &lib).expect("set _lib global");
        Self {
            inner: Rc::new(RefCell::new(DynInner {
                native_specs: BTreeMap::new(),
                lua_specs: BTreeMap::new(),
                lib,
            })),
            lua,
            hook_active: Rc::new(Cell::new(false)),
        }
    }

    /// 共享 Lua 沙箱实例（测试/诊断用；D14 共享 `_G`）。
    #[must_use]
    pub fn lua(&self) -> &Lua {
        &self.lua
    }

    /// 注册/覆盖 native spec（同名替换）。注册名即分发顺序 key。
    pub fn register(&self, spec: DynSystemSpec) {
        self.inner
            .borrow_mut()
            .native_specs
            .insert(spec.name.clone(), spec);
    }

    pub fn len(&self) -> usize {
        let i = self.inner.borrow();
        i.native_specs.len() + i.lua_specs.len()
    }

    pub fn is_empty(&self) -> bool {
        let i = self.inner.borrow();
        i.native_specs.is_empty() && i.lua_specs.is_empty()
    }

    /// 已注册 lua system 数（观测/测试）。
    #[must_use]
    pub fn lua_len(&self) -> usize {
        self.inner.borrow().lua_specs.len()
    }

    /// 是否已注册同名 lua system。
    #[must_use]
    pub fn contains_lua(&self, name: &str) -> bool {
        self.inner.borrow().lua_specs.contains_key(name)
    }

    // ---------- 编译 ----------

    /// 编译 lua 源码为可调用函数（沙箱环境；编译不执行用户代码）。
    ///
    /// 两种写法都接受（D1 示例与 §4.8 示例都用函数表达式）：
    /// 1. 函数表达式：`function(event, ctx) ... end` → `return (src)` 取回内层函数
    /// 2. 裸语句体：`ctx.admin.follow(event.uid, true)` → `return function(event, ctx) ... end`
    ///
    /// 注意：`lua.load("return (function ... end)").into_function()` 得到的是**外层 chunk**
    /// （调用它只会返回内层函数，不执行回调体），必须在这里执行一次 chunk 取出 `Function`。
    fn compile_callback(&self, src: &str) -> Result<Function> {
        let as_expr = format!("return ({src})");
        if let Ok(chunk) = self.lua.load(as_expr.as_str()).into_function() {
            match chunk.call::<LuaValue>(()) {
                Ok(LuaValue::Function(f)) => return Ok(f),
                // 表达式不是函数：按裸语句体再试
                Ok(_) => {}
                Err(e) => bail!("lua 回调求值失败：{e}"),
            }
        }
        let as_body = format!("return function(event, ctx)\n{src}\nend");
        let chunk = self
            .lua
            .load(as_body.as_str())
            .into_function()
            .map_err(|e| anyhow!("lua 回调编译失败（函数表达式/函数体两种写法均失败）：{e}"))?;
        match chunk.call::<LuaValue>(()) {
            Ok(LuaValue::Function(f)) => Ok(f),
            Ok(other) => bail!("lua 回调编译结果不是函数：{}", other.type_name()),
            Err(e) => bail!("lua 回调求值失败：{e}"),
        }
    }

    /// condition 编译（D15）：`""`/`"always"` → `None`（恒真），其余按**表达式**编译为
    /// `function(event) return (<expr>) end`——表达式内以形参 `event` 引用事件表。
    fn compile_condition(&self, cond: &str) -> Result<Option<Function>> {
        let c = cond.trim();
        if c.is_empty() || c == "always" {
            return Ok(None);
        }
        let src = format!("return function(event) return ({c}) end");
        let chunk = self
            .lua
            .load(src.as_str())
            .into_function()
            .map_err(|e| anyhow!("lua condition 编译失败（须是表达式）：{e}"))?;
        match chunk.call::<LuaValue>(()) {
            Ok(LuaValue::Function(f)) => Ok(Some(f)),
            Ok(other) => bail!("lua condition 编译结果不是函数：{}", other.type_name()),
            Err(e) => bail!("lua condition 求值失败：{e}"),
        }
    }

    // ---------- 注册 ----------

    /// 按 name 前缀分派注册（libcall `register_system` / 热加载共用入口，D16）。
    pub fn register_from_source(&self, name: &str, src: &str, condition: &str) -> Result<()> {
        if name.trim().is_empty() {
            bail!("system name 不能为空");
        }
        if is_oneshot(name) {
            self.register_oneshot(name, src, condition)
        } else {
            self.register_lua(name, src, condition)
        }
    }

    /// 注册/覆盖事件响应型 lua system：编译回调 + 预编译 condition（D15）。
    pub fn register_lua(&self, name: &str, src: &str, condition: &str) -> Result<()> {
        if is_oneshot(name) {
            bail!("lua system 名不能以 {ONESHOT_PREFIX:?} 开头（那是 oneshot 前缀，D16）");
        }
        let callback = self.compile_callback(src)?;
        let condition = self.compile_condition(condition)?;
        self.inner.borrow_mut().lua_specs.insert(
            name.to_string(),
            LuaSystemSpec {
                name: name.to_string(),
                condition,
                callback,
            },
        );
        Ok(())
    }

    /// 注册 oneshot（D16）：编译后**立即执行一次**，返回 table 存入 `_lib.<name 去前缀>`。
    ///
    /// `condition` 对 oneshot 无意义，非空即拒绝（§4.8）。
    pub fn register_oneshot(&self, name: &str, src: &str, condition: &str) -> Result<()> {
        if !is_oneshot(name) {
            bail!("oneshot system 名必须以 {ONESHOT_PREFIX:?} 开头（D16）：{name:?}");
        }
        let short = &name[ONESHOT_PREFIX.len()..];
        if short.is_empty() {
            bail!("oneshot system 名缺少库名：{name:?}");
        }
        if !condition.trim().is_empty() {
            bail!("oneshot system 不支持 condition（须为空）：{name:?}");
        }
        let chunk = self
            .lua
            .load(src)
            .into_function()
            .map_err(|e| anyhow!("oneshot {name:?} 编译失败：{e}"))?;
        let ret: LuaValue = self
            .guarded_call(&chunk, (), LUA_INSTRUCTION_LIMIT)
            .map_err(|e| anyhow!("oneshot {name:?} 执行失败：{e}"))?;
        match ret {
            LuaValue::Table(t) => {
                self.inner
                    .borrow()
                    .lib
                    .set(short, t)
                    .map_err(|e| anyhow!("oneshot {name:?} 写入 _lib.{short} 失败：{e}"))?;
            }
            LuaValue::Nil => {
                log::debug!("oneshot {name:?} 返回 nil（仅副作用，弃用风险自担）");
            }
            other => bail!("oneshot {name:?} 须返回 table 或 nil，得到 {}", other.type_name()),
        }
        Ok(())
    }

    /// 从注册表移除 lua system（native 不动）。返回是否真的移除了。
    ///
    /// oneshot（`lib.` 前缀）不进 `lua_specs`，而是 `_lib.<短名>` 条目——必须一并清掉，
    /// 否则 `unregister_system("lib.x")` 会「store 删了、内存还在」，落盘失败的回滚也成空操作。
    pub fn unregister(&self, name: &str) -> bool {
        if is_oneshot(name) {
            let short = &name[ONESHOT_PREFIX.len()..];
            let inner = self.inner.borrow();
            match inner.lib.contains_key(short) {
                Ok(false) => false,
                Ok(true) => match inner.lib.raw_remove(short) {
                    Ok(()) => true,
                    Err(e) => {
                        log::warn!("unregister {name:?}: 清理 _lib.{short} 失败：{e}");
                        false
                    }
                },
                Err(e) => {
                    log::warn!("unregister {name:?}: 探测 _lib.{short} 失败：{e}");
                    false
                }
            }
        } else {
            self.inner.borrow_mut().lua_specs.remove(name).is_some()
        }
    }

    /// 保存 `name` 当前的内存注册状态（供落盘失败回滚，libcall `register_system`）。
    ///
    /// 与 `unregister` 对偶：`Absent` 时回滚即删除，否则恢复旧编译产物 / 旧 `_lib` 值。
    #[must_use]
    pub fn saved_state(&self, name: &str) -> SavedState {
        if is_oneshot(name) {
            let short = &name[ONESHOT_PREFIX.len()..];
            let inner = self.inner.borrow();
            SavedState::Oneshot(inner.lib.raw_get(short).unwrap_or(LuaValue::Nil))
        } else {
            match self.inner.borrow().lua_specs.get(name) {
                Some(spec) => SavedState::Lua(spec.clone()),
                None => SavedState::Absent,
            }
        }
    }

    /// 回滚 `name` 到 `saved`（落盘失败时调用）。与 `unregister` 的差别：**恢复旧版本**
    /// 而非删除——覆盖式注册已经替换了内存状态，删除会连带丢掉原本可用的 spec。
    pub fn restore_state(&self, name: &str, saved: SavedState) {
        match saved {
            SavedState::Absent => {
                self.unregister(name);
            }
            SavedState::Lua(spec) => {
                self.inner.borrow_mut().lua_specs.insert(name.to_string(), spec);
            }
            SavedState::Oneshot(old) => {
                let short = &name[ONESHOT_PREFIX.len()..];
                let inner = self.inner.borrow();
                let r = match old {
                    LuaValue::Nil => inner.lib.raw_remove(short),
                    other => inner.lib.set(short, other),
                };
                if let Err(e) = r {
                    log::warn!("restore_state {name:?}: 回填 _lib.{short} 失败：{e}");
                }
            }
        }
    }

    // ---------- 加载 / 热加载 ----------

    /// D3：从 store 全量加载 systems 表并注册。
    ///
    /// 读取失败 = Err（调用方 `WeiYuanHui::open` 直接退出）；**单条 spec 编译/执行失败
    /// 只记日志跳过**，不阻断启动。
    pub fn load_from_store(&self, store: &Store) -> Result<()> {
        let specs = store.load_systems().context("load_systems for dyn systems")?;
        self.load_specs(&specs);
        Ok(())
    }

    /// §4.3 三步骤加载：① 重置 `_lib` → ② 名序执行全部 oneshot → ③ 注册 event system。
    ///
    /// 幂等（`reload_all` 语义）：先清空 lua_specs 再重建，native spec 保留。
    pub fn load_specs(&self, specs: &[SystemSpecV1]) {
        self.reset_lib();
        self.inner.borrow_mut().lua_specs.clear();
        let mut sorted: Vec<&SystemSpecV1> = specs.iter().collect();
        sorted.sort_by(|a, b| a.name.cmp(&b.name));
        for spec in sorted.iter().filter(|s| is_oneshot(&s.name)) {
            if let Err(e) = self.register_oneshot(&spec.name, &spec.lua, &spec.condition) {
                log::error!("oneshot system {:?} 加载失败（跳过）：{e:#}", spec.name);
            }
        }
        for spec in sorted.iter().filter(|s| !is_oneshot(&s.name)) {
            if let Err(e) = self.register_lua(&spec.name, &spec.lua, &spec.condition) {
                log::error!("lua system {:?} 加载失败（跳过）：{e:#}", spec.name);
            }
        }
    }

    /// D4 热加载单个 spec（从 store 读出的最新版本）：重编译替换。
    ///
    /// oneshot 只重跑该条并替换 `_lib.<短名>`，**不重置整表**（§4.8）。
    pub fn reload_one(&self, spec: &SystemSpecV1) -> Result<()> {
        self.register_from_source(&spec.name, &spec.lua, &spec.condition)
    }

    /// 重置共享库表 `_lib`（整表替换 + 更新全局，D16）。
    fn reset_lib(&self) {
        let lib = match self.lua.create_table() {
            Ok(t) => t,
            Err(e) => {
                log::error!("reset _lib failed: {e}");
                return;
            }
        };
        if let Err(e) = self.lua.globals().set(LIB_GLOBAL, &lib) {
            log::error!("reset _lib global failed: {e}");
            return;
        }
        self.inner.borrow_mut().lib = lib;
    }

    // ---------- 分发 ----------

    /// D5 两段式分发：先全部 native（名序），再全部 lua（名序）；中间不清空事件。
    ///
    /// `store` 供 lua 侧 `ctx.admin.register_system`/`reload_*` 落盘（内存 hub 为 None）。
    pub fn dispatch(&self, ev: &TriggerEvent, snap: &mut Snapshot, store: Option<&Store>) {
        self.dispatch_native(ev, snap);
        self.dispatch_lua(ev, snap, store);
    }

    /// Rust 原生 handler（名序；报错记日志继续）。
    pub fn dispatch_native(&self, ev: &TriggerEvent, snap: &mut Snapshot) {
        let specs: Vec<DynSystemSpec> = self
            .inner
            .borrow()
            .native_specs
            .values()
            .cloned()
            .collect();
        for spec in specs {
            if let Err(e) = (spec.callback)(ev, snap) {
                log::warn!("dyn system {:?} handler error: {e:#}", spec.name);
            }
        }
    }

    /// lua handler（名序）：event 序列化一次为**只读源**，每个 spec 取独立只读副本
    /// （D5 修复：共享一份会被上游 spec 改写污染）；condition 求值 → 回调执行。
    pub fn dispatch_lua(&self, ev: &TriggerEvent, snap: &mut Snapshot, store: Option<&Store>) {
        let specs: Vec<LuaSystemSpec> =
            self.inner.borrow().lua_specs.values().cloned().collect();
        if specs.is_empty() {
            return;
        }
        let event_val = trigger_event_to_json(ev);
        let event_src = match self.lua.to_value(&event_val) {
            Ok(LuaValue::Table(t)) => t,
            Ok(other) => {
                log::error!(
                    "lua event 序列化非 table（跳过全部 lua system）：{}",
                    other.type_name()
                );
                return;
            }
            Err(e) => {
                log::error!("lua event 序列化失败（跳过全部 lua system）：{e}");
                return;
            }
        };
        for spec in specs {
            // `specs` 是循环前克隆的快照：回调内 register/unregister/reload 以及
            // `probe_after_error` 的全量重建都不影响本轮剩余 spec（仍按旧编译产物执行）。
            // D5 修复：每个 spec 一份只读副本——共享同一 table 时，任一 system 改写
            // `event.*` 会污染名序在后的 system。
            let event_tbl = match readonly_event_copy(&self.lua, &event_src) {
                Ok(t) => t,
                Err(e) => {
                    log::error!("构造只读 event 副本失败（跳过 {:?}）：{e}", spec.name);
                    continue;
                }
            };
            if let Some(cond) = &spec.condition {
                // D15：恒真已在编译期折叠为 None；错误按「未通过」跳过，不阻断分发。
                match self.guarded_call::<bool>(
                    cond,
                    event_tbl.clone(),
                    LUA_CONDITION_INSTRUCTION_LIMIT,
                ) {
                    Ok(true) => {}
                    Ok(_) => continue,
                    Err(e) => {
                        log::warn!(
                            "lua system {:?} condition 求值失败（按未通过跳过）：{e}",
                            spec.name
                        );
                        self.probe_after_error(&spec.name, store);
                        continue;
                    }
                }
            }
            if let Err(e) = self.run_callback(&spec, &event_tbl, snap, store) {
                log::warn!("lua system {:?} 回调失败：{e:#}", spec.name);
                self.probe_after_error(&spec.name, store);
            }
        }
    }

    /// 单次 lua 回调：建 ctx（libcall）→ 指令上限保护下 call。
    fn run_callback(
        &self,
        spec: &LuaSystemSpec,
        event_tbl: &Table,
        snap: &mut Snapshot,
        store: Option<&Store>,
    ) -> Result<()> {
        let lua = &self.lua;
        let me = self.clone();
        let result = lua.scope(|scope| {
            let ctx = libcall::build_ctx_table(lua, scope, snap, store, Some(me.clone()))?;
            self.guarded_call::<()>(
                &spec.callback,
                (event_tbl.clone(), ctx),
                LUA_INSTRUCTION_LIMIT,
            )
        });
        result.map_err(|e| anyhow!("{e}"))
    }

    /// D13 实测点：lua 报错/中断后探针实例可用性。
    ///
    /// mlua 的 hook 中断是普通 Lua error（非 `AbortInto` 式实例失效），实测可复用；
    /// 若探针失败且有 store 则按 §4.3 全量重建（`reload_all` 语义）。
    fn probe_after_error(&self, name: &str, store: Option<&Store>) {
        match self.lua.load("return 1").eval::<i64>() {
            Ok(1) => log::debug!("lua instance probe ok after {name:?} error（实例可复用）"),
            _ => {
                log::error!("lua instance unusable after {name:?} error; rebuilding lua systems");
                match store {
                    Some(store) => {
                        if let Err(e) = self.load_from_store(store) {
                            log::error!("rebuild lua systems failed: {e:#}");
                        }
                    }
                    None => log::error!("no store attached: cannot rebuild lua systems"),
                }
            }
        }
    }

    /// 在指令上限保护下同步调用 lua 函数（D6 ①）。
    ///
    /// `set_hook` 每 `LUA_HOOK_INTERVAL` 条指令回调一次；累计超过 `limit` 时返回 Lua error
    /// 中断当前执行（`Function::call` 把错误带回 Rust），随后移除 hook。
    ///
    /// **嵌套复用**（T3 修复）：mlua 的 hook 挂在 Lua 实例上是全局唯一的，若外层已在保护
    /// 中（`hook_active`），本次调用直接执行、沿用外层 hook 与预算——绝不 `set_hook`/
    /// `remove_hook`，否则内层退出会拆掉外层保护（外层随后的死循环将永久卡死 hub 线程）。
    /// 代价：内层指令计入外层预算（lua 回调内 `register_system`/`reload_*` 触发的 oneshot
    /// 执行量算在外层 `LUA_INSTRUCTION_LIMIT` 里）。
    fn guarded_call<R: mlua::FromLuaMulti>(
        &self,
        f: &Function,
        args: impl mlua::IntoLuaMulti,
        limit: u64,
    ) -> mlua::Result<R> {
        if self.hook_active.get() {
            return f.call::<R>(args);
        }
        let counter = Arc::new(AtomicU64::new(0));
        let c = Arc::clone(&counter);
        self.lua.set_hook(
            HookTriggers::new().every_nth_instruction(LUA_HOOK_INTERVAL),
            move |_, _| {
                let n = c.fetch_add(u64::from(LUA_HOOK_INTERVAL), Ordering::Relaxed);
                if n >= limit {
                    Err(mlua::Error::runtime(format!(
                        "lua 指令数超限（> {limit}），已中断"
                    )))
                } else {
                    Ok(VmState::Continue)
                }
            },
        );
        self.hook_active.set(true);
        let _guard = HookGuard {
            lua: &self.lua,
            active: &self.hook_active,
        };
        f.call::<R>(args)
    }
}

/// `guarded_call` 的 hook 清理守卫：正常返回与 panic 展开都保证移除 hook + 复位标志。
struct HookGuard<'a> {
    lua: &'a Lua,
    active: &'a Cell<bool>,
}

impl Drop for HookGuard<'_> {
    fn drop(&mut self) {
        self.lua.remove_hook();
        self.active.set(false);
    }
}

/// 为单个 spec 构造只读 event 副本（D5 修复：跨 spec 共享一份会被上游改写污染）。
///
/// lua54 下 mlua 的 `Table::set_readonly` 不可用（仅 `luau` feature，见 mlua
/// `src/table.rs`），故用元方法实现：
/// - 字段浅拷贝到副本（读取 / `pairs` / `json.encode` 行为不变；字段均为标量）
/// - `__newindex` 拒绝**新增**键
/// - `__metatable` 锁住元表，防 lua 侧 `setmetatable(event, {})` 绕过
///
/// 已知边界：Lua 语义下改写**已有**字段（`event.kind = ...`）不触发 `__newindex`，
/// 该改写只落在本 spec 的副本上、不污染其他 spec；完全不可写需 luau 的 readonly 属性。
fn readonly_event_copy(lua: &Lua, src: &Table) -> mlua::Result<Table> {
    let copy = lua.create_table()?;
    for pair in src.pairs::<LuaValue, LuaValue>() {
        let (k, v) = pair?;
        copy.raw_set(k, v)?;
    }
    let meta = lua.create_table()?;
    meta.raw_set(
        "__newindex",
        lua.create_function(|_, _: mlua::MultiValue| -> mlua::Result<()> {
            Err(mlua::Error::runtime(
                "event 表只读（D5 跨 spec 复用）：禁止新增字段",
            ))
        })?,
    )?;
    meta.raw_set("__metatable", "event(readonly)")?;
    copy.set_metatable(Some(meta));
    Ok(copy)
}

/// `TriggerEvent` → lua 侧 event table（D1：`kind` + 事件专有字段）。
///
/// `uid` 能解析为整数时给 lua 数值（`event.uid > 1000` 这类 condition 需要），否则给字符串。
#[must_use]
pub fn trigger_event_to_json(ev: &TriggerEvent) -> Value {
    match ev {
        TriggerEvent::Tick { at } => json!({ "kind": "tick", "at": at.to_rfc3339() }),
        TriggerEvent::FetchDone { entity, uid } => {
            json!({ "kind": "fetch_done", "entity": entity, "uid": uid_value(uid) })
        }
        TriggerEvent::FetchFailed { entity, uid, error } => json!({
            "kind": "fetch_failed",
            "entity": entity,
            "uid": uid_value(uid),
            "error": error,
        }),
        TriggerEvent::UpStateChanged { entity, uid, kind } => json!({
            "kind": "up_state",
            "entity": entity,
            "uid": uid_value(uid),
            "state": match kind {
                UpStateKind::Followed => "followed",
                UpStateKind::Unfollowed => "unfollowed",
                UpStateKind::GroupToggled => "group_toggled",
                UpStateKind::Silenced => "silenced",
            },
        }),
    }
}

fn uid_value(uid: &str) -> Value {
    uid.parse::<i64>()
        .map_or_else(|_| Value::String(uid.to_string()), Value::from)
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
            Some(TriggerEvent::UpStateChanged { entity, uid, kind })
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
    // ---------- M3 T3：lua 动态 system ----------

    use crate::store::StoreConfig;
    use tempfile::tempdir;

    /// 临时 store + 写入 specs（`(name, lua, condition)`；TempDir 由调用方持有）。
    fn store_with_specs(specs: &[(&str, &str, &str)]) -> anyhow::Result<(Store, tempfile::TempDir)> {
        let dir = tempdir()?;
        let cfg = StoreConfig::new(dir.path().join("state.redb"));
        let store = Store::open_or_create(&cfg)?;
        for (name, lua, cond) in specs {
            store.put_system(&SystemSpecV1 {
                name: (*name).into(),
                lua: (*lua).into(),
                condition: (*cond).into(),
            })?;
        }
        Ok((store, dir))
    }

    /// 用户组 gid（>=2）列表，升序。
    fn group_ids(snap: &Snapshot) -> Vec<u64> {
        let mut v: Vec<u64> = snap
            .res
            .gid_index
            .keys()
            .copied()
            .filter(|g| *g >= 2)
            .collect();
        v.sort_unstable();
        v
    }

    /// D3/D16：oneshot（`lib.`）加载执行 → `_lib` 填充 → event system 复用其函数。
    #[test]
    fn test_load_from_store_oneshot_and_event() -> anyhow::Result<()> {
        init();
        let (store, _dir) = store_with_specs(&[
            (
                "lib.mylib",
                "return { is_big = function(ev) return ev.uid and ev.uid > 1000 end }",
                "",
            ),
            (
                "on_fetch",
                "function(event, ctx) \
                 if event.kind == 'fetch_done' and _lib.mylib.is_big(event) then \
                   ctx.admin.new_group('big', false) \
                 end end",
                "event.kind == 'fetch_done'",
            ),
        ])?;
        let reg = DynSystemRegistry::new();
        reg.load_from_store(&store)?;
        assert!(reg.contains_lua("on_fetch"));
        assert!(!reg.contains_lua("lib.mylib"), "oneshot 不参与 dispatch");
        assert_eq!(reg.lua_len(), 1);
        let lib_ok: bool = reg
            .lua()
            .load("return type(_lib.mylib) == 'table' and type(_lib.mylib.is_big) == 'function'")
            .eval()
            .map_err(|e| anyhow!("{e}"))?;
        assert!(lib_ok, "oneshot 返回值应存入 _lib.mylib");

        let mut snap = Snapshot::new();
        reg.dispatch(
            &TriggerEvent::FetchDone {
                entity: 0,
                uid: "12345".into(),
            },
            &mut snap,
            Some(&store),
        );
        assert_eq!(group_ids(&snap), vec![2], "uid > 1000 → 建组");
        let mut snap2 = Snapshot::new();
        reg.dispatch(
            &TriggerEvent::FetchDone {
                entity: 0,
                uid: "500".into(),
            },
            &mut snap2,
            Some(&store),
        );
        assert!(group_ids(&snap2).is_empty(), "uid <= 1000 → 不建组");
        Ok(())
    }

    /// 端到端核心：FetchDone → lua 回调 → `ctx.admin.toggle_group` 改分组。
    #[test]
    fn test_dispatch_lua_toggles_group() -> anyhow::Result<()> {
        init();
        let reg = DynSystemRegistry::new();
        reg.register_lua(
            "on_fetch",
            "function(event, ctx) \
             if event.kind == 'fetch_done' then ctx.admin.toggle_group(event.uid, 2) end end",
            "",
        )?;
        let uid: i64 = 12345;
        let mut snap = Snapshot::new();
        snap.follow(&json!({"uid": uid, "enable": true}))?;
        reg.dispatch(
            &TriggerEvent::FetchDone {
                entity: 0,
                uid: uid.to_string(),
            },
            &mut snap,
            None,
        );
        let eid = *snap.res.uid_index.get(&uid.to_string()).expect("uid traced");
        let ge = *snap.res.gid_index.get(&2).expect("gid 2 placeholder");
        let brick = snap
            .world
            .get::<crate::db::Brick>(crate::ecs::Entity(eid))
            .expect("brick");
        assert!(
            brick.groups.contains(&ge),
            "lua 回调应把 uid 加入分组实体 {ge}"
        );
        Ok(())
    }

    /// D15：condition 求值——表达式过滤 + `""`/`"always"` 恒真。
    #[test]
    fn test_condition_filters_lua_systems() -> anyhow::Result<()> {
        init();
        let reg = DynSystemRegistry::new();
        reg.register_lua(
            "a.tick_only",
            "function(event, ctx) ctx.admin.new_group('a', false) end",
            "event.kind == 'tick'",
        )?;
        reg.register_lua(
            "b.empty",
            "function(event, ctx) ctx.admin.new_group('b', false) end",
            "",
        )?;
        reg.register_lua(
            "c.always",
            "function(event, ctx) ctx.admin.new_group('c', false) end",
            "always",
        )?;
        let mut snap = Snapshot::new();
        reg.dispatch(
            &TriggerEvent::FetchDone {
                entity: 0,
                uid: "1".into(),
            },
            &mut snap,
            None,
        );
        assert_eq!(group_ids(&snap), vec![2, 3], "fetch_done 只命中 b/c");
        let mut snap2 = Snapshot::new();
        reg.dispatch(&TriggerEvent::Tick { at: Utc::now() }, &mut snap2, None);
        assert_eq!(group_ids(&snap2), vec![2, 3, 4], "tick 命中 a/b/c");
        Ok(())
    }

    /// D6 ①：死循环 lua 被指令上限斩杀（远早于 5s），后续 handler 正常执行。
    #[test]
    fn test_lua_instruction_limit_aborts_and_continues() -> anyhow::Result<()> {
        init();
        let reg = DynSystemRegistry::new();
        // 名序 a.loop < b.mark：先跑死循环（被斩杀），b.mark 必须照常执行
        reg.register_lua("a.loop", "function(event, ctx) while true do end end", "")?;
        reg.register_lua(
            "b.mark",
            "function(event, ctx) ctx.admin.new_group('after', false) end",
            "",
        )?;
        let mut snap = Snapshot::new();
        let started = std::time::Instant::now();
        reg.dispatch(&TriggerEvent::Tick { at: Utc::now() }, &mut snap, None);
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_secs(5),
            "指令上限应在 5s 内中断，实际 {elapsed:?}"
        );
        assert_eq!(group_ids(&snap), vec![2], "斩杀后后续 handler 仍执行");
        Ok(())
    }

    /// T3 修复：lua 回调内 `register_system`（→ 嵌套 `guarded_call`）不得拆掉外层指令上限。
    ///
    /// mlua 的 hook 是 Lua 实例全局唯一的：若内层 `guarded_call` 无条件
    /// `set_hook`/`remove_hook`，退出时会连带清掉外层 hook，外层随后的死循环便永久卡死。
    #[test]
    fn test_nested_guarded_call_keeps_outer_limit() -> anyhow::Result<()> {
        init();
        let reg = DynSystemRegistry::new();
        // 名序 a < b：a 先注册 oneshot（嵌套调用来源）再死循环，b 必须照常执行
        reg.register_lua(
            "a.nested_loop",
            "function(event, ctx) \
             ctx.admin.register_system('lib.h', 'return {}', '') \
             while true do end end",
            "",
        )?;
        reg.register_lua(
            "b.mark",
            "function(event, ctx) ctx.admin.new_group('after', false) end",
            "",
        )?;
        let mut snap = Snapshot::new();
        let started = std::time::Instant::now();
        reg.dispatch(&TriggerEvent::Tick { at: Utc::now() }, &mut snap, None);
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_secs(5),
            "嵌套 register_system 后外层仍应被指令上限斩杀，实际 {elapsed:?}"
        );
        assert_eq!(group_ids(&snap), vec![2], "斩杀后后续 handler 仍执行");
        let lib_ok: bool = reg
            .lua()
            .load("return type(_lib.h) == 'table'")
            .eval()
            .map_err(|e| anyhow!("{e}"))?;
        assert!(lib_ok, "内层 oneshot 确已执行（嵌套路径被覆盖）");
        Ok(())
    }

    /// D13 实测点：指令上限中断后 Lua 实例可复用（无需重建）。
    #[test]
    fn test_lua_instance_reusable_after_abort() -> anyhow::Result<()> {
        init();
        let reg = DynSystemRegistry::new();
        reg.register_lua("a.loop", "function(event, ctx) while true do end end", "")?;
        let mut snap = Snapshot::new();
        reg.dispatch(&TriggerEvent::Tick { at: Utc::now() }, &mut snap, None);
        // 复用同一实例注册并执行新回调
        reg.register_lua(
            "b.mark",
            "function(event, ctx) ctx.admin.new_group('reuse', false) end",
            "",
        )?;
        let mut snap2 = Snapshot::new();
        reg.dispatch(&TriggerEvent::Tick { at: Utc::now() }, &mut snap2, None);
        assert_eq!(group_ids(&snap2), vec![2], "中断后实例仍可用");
        Ok(())
    }

    /// 加载健壮性：语法错误/非法 oneshot 跳过；`load_from_store` 重置 `_lib`（幂等）。
    #[test]
    fn test_load_from_store_resets_lib_and_skips_bad_specs() -> anyhow::Result<()> {
        init();
        let (store, _dir) = store_with_specs(&[
            ("lib.mylib", "return { ping = function() return 1 end }", ""),
            (
                "a.ok",
                "function(event, ctx) \
                 if _lib.mylib.ping() == 1 then ctx.admin.new_group('ok', false) end end",
                "",
            ),
            ("b.bad", "function(event, ctx) this is not lua end", ""),
            ("lib.bad", "return {}", "always"),
        ])?;
        let reg = DynSystemRegistry::new();
        reg.load_from_store(&store)?;
        assert!(reg.contains_lua("a.ok"));
        assert!(!reg.contains_lua("b.bad"), "语法错误 spec 应跳过");
        assert!(!reg.contains_lua("lib.bad"), "带 condition 的 oneshot 应跳过");

        let mut snap = Snapshot::new();
        reg.dispatch(&TriggerEvent::Tick { at: Utc::now() }, &mut snap, Some(&store));
        assert_eq!(group_ids(&snap), vec![2], "oneshot 提供的函数可被 event system 调用");

        // 重载：删掉 lib.mylib 后再 load_from_store → `_lib` 整表重置
        store.delete_system("lib.mylib")?;
        reg.load_from_store(&store)?;
        let gone: bool = reg
            .lua()
            .load("return _lib.mylib == nil")
            .eval()
            .map_err(|e| anyhow!("{e}"))?;
        assert!(gone, "load_from_store 应重置 _lib");
        // 库函数缺失 → 回调报错只记日志（不 panic），后续分发照常
        let mut snap2 = Snapshot::new();
        reg.dispatch(&TriggerEvent::Tick { at: Utc::now() }, &mut snap2, Some(&store));
        assert!(group_ids(&snap2).is_empty(), "库函数缺失时回调跳过");
        Ok(())
    }

    /// D4：单条热加载替换回调（oneshot 只替换自己的 `_lib` 条目）。
    #[test]
    fn test_reload_one_replaces_callback() -> anyhow::Result<()> {
        init();
        let (store, _dir) = store_with_specs(&[(
            "a.reload",
            "function(event, ctx) ctx.admin.new_group('v1', false) end",
            "",
        )])?;
        let reg = DynSystemRegistry::new();
        reg.load_from_store(&store)?;
        let mut snap = Snapshot::new();
        reg.dispatch(&TriggerEvent::Tick { at: Utc::now() }, &mut snap, Some(&store));
        assert_eq!(group_ids(&snap), vec![2], "v1 回调建组");

        let spec = SystemSpecV1 {
            name: "a.reload".into(),
            lua: "function(event, ctx) ctx.admin.follow(999, true) end".into(),
            condition: String::new(),
        };
        reg.reload_one(&spec)?;
        let mut snap2 = Snapshot::new();
        reg.dispatch(&TriggerEvent::Tick { at: Utc::now() }, &mut snap2, Some(&store));
        assert!(snap2.res.uid_index.contains_key("999"), "热加载后执行新回调");
        assert!(group_ids(&snap2).is_empty(), "旧回调不再执行");
        Ok(())
    }

    /// D5：native handler 先于 lua handler（与注册名序无关）。
    #[test]
    fn test_dispatch_native_before_lua() -> anyhow::Result<()> {
        init();
        let reg = DynSystemRegistry::new();
        // 名序上 lua（a.lua）在前，但 native 必须先跑
        reg.register(DynSystemSpec {
            name: "z.native".into(),
            condition: "always".into(),
            callback: Arc::new(|_ev, snap| snap.follow(&json!({"uid": 1, "enable": true}))),
        });
        reg.register_lua(
            "a.lua",
            "function(event, ctx) native_seen = (ctx.admin.get_state(1) ~= nil) end",
            "",
        )?;
        let mut snap = Snapshot::new();
        reg.dispatch(&TriggerEvent::Tick { at: Utc::now() }, &mut snap, None);
        let seen: bool = reg
            .lua()
            .load("return native_seen")
            .eval()
            .map_err(|e| anyhow!("{e}"))?;
        assert!(seen, "native handler 必须先于 lua handler 执行");
        Ok(())
    }

    /// D16 命名规则：`lib.` 前缀与 condition 互斥，语法错误拒绝注册。
    #[test]
    fn test_oneshot_prefix_and_condition_validation() -> anyhow::Result<()> {
        init();
        let reg = DynSystemRegistry::new();
        assert!(
            reg.register_oneshot("a.plain", "return {}", "").is_err(),
            "非 lib. 前缀不能当 oneshot"
        );
        assert!(
            reg.register_oneshot("lib.x", "return {}", "always").is_err(),
            "oneshot 不允许 condition"
        );
        assert!(
            reg.register_lua("lib.x", "function(event, ctx) end", "").is_err(),
            "event system 不允许 lib. 前缀"
        );
        assert!(
            reg.register_lua("a.bad", "function(event, ctx) ] end", "").is_err(),
            "语法错误应拒绝"
        );
        assert!(reg
            .register_oneshot("lib.good", "return { f = function() return 1 end }", "")
            .is_ok());
        assert!(!reg.contains_lua("lib.good"), "oneshot 不入 lua_specs");
        Ok(())
    }

    /// T3 修复：`unregister` 须同时覆盖 oneshot（它不在 `lua_specs`，而在 `_lib.<短名>`）。
    #[test]
    fn test_unregister_oneshot_clears_lib() -> anyhow::Result<()> {
        init();
        let reg = DynSystemRegistry::new();
        reg.register_oneshot("lib.helper", "return { ping = function() return 1 end }", "")?;
        assert!(reg.unregister("lib.helper"), "已注册的 oneshot 应报告移除成功");
        let gone: bool = reg
            .lua()
            .load("return _lib.helper == nil")
            .eval()
            .map_err(|e| anyhow!("{e}"))?;
        assert!(gone, "_lib.helper 应被清除");
        assert!(!reg.unregister("lib.helper"), "重复注销返回 false");
        assert!(!reg.unregister("lib.never"), "未注册的 oneshot 返回 false");
        // event system 走原 lua_specs 路径
        reg.register_lua("a.x", "function(event, ctx) end", "")?;
        assert!(reg.unregister("a.x"), "event system 仍可注销");
        assert!(!reg.unregister("a.x"));
        Ok(())
    }

    /// T3 修复：落盘失败的回滚须恢复**旧版本**，而不是把新注册删掉（那会连带丢掉旧 spec）。
    #[test]
    fn test_restore_state_rolls_back_to_previous() -> anyhow::Result<()> {
        init();
        let reg = DynSystemRegistry::new();

        // 1) event 型：v1 → v2 → 回滚到 v1
        reg.register_lua(
            "a.x",
            "function(event, ctx) ctx.admin.follow(111, true) end",
            "",
        )?;
        let saved = reg.saved_state("a.x");
        reg.register_lua(
            "a.x",
            "function(event, ctx) ctx.admin.follow(222, true) end",
            "",
        )?;
        reg.restore_state("a.x", saved);
        let mut snap = Snapshot::new();
        reg.dispatch(&TriggerEvent::Tick { at: Utc::now() }, &mut snap, None);
        assert!(snap.res.uid_index.contains_key("111"), "回滚后执行旧回调");
        assert!(!snap.res.uid_index.contains_key("222"), "新回调不再执行");

        // 2) oneshot：`_lib.<短名>` 恢复旧值
        reg.register_oneshot("lib.h", "return { tag = 1 }", "")?;
        let saved = reg.saved_state("lib.h");
        reg.register_oneshot("lib.h", "return { tag = 2 }", "")?;
        reg.restore_state("lib.h", saved);
        let tag: i64 = reg
            .lua()
            .load("return _lib.h.tag")
            .eval()
            .map_err(|e| anyhow!("{e}"))?;
        assert_eq!(tag, 1, "oneshot 回滚应恢复旧 _lib 值");

        // 3) 原本不存在（event / oneshot 两条路径）→ 回滚即删除
        let absent = reg.saved_state("z.new");
        reg.register_lua("z.new", "function(event, ctx) end", "")?;
        reg.restore_state("z.new", absent);
        assert!(!reg.contains_lua("z.new"), "原本无注册 → 回滚即删除");
        let absent_lib = reg.saved_state("lib.none");
        reg.register_oneshot("lib.none", "return {}", "")?;
        reg.restore_state("lib.none", absent_lib);
        let gone: bool = reg
            .lua()
            .load("return _lib.none == nil")
            .eval()
            .map_err(|e| anyhow!("{e}"))?;
        assert!(gone, "原本无 _lib 条目 → 回滚即删除");
        Ok(())
    }

    /// T3 修复：event 表只读 + 每 spec 独立副本——上游 spec 不得污染后续 spec。
    #[test]
    fn test_event_table_readonly_and_isolated() -> anyhow::Result<()> {
        init();
        let reg = DynSystemRegistry::new();
        // a：新增字段应被 `__newindex` 拒绝（pcall 捕获）；改写已有字段只落在自己副本上
        reg.register_lua(
            "a.hack",
            "function(event, ctx) \
             local ok = pcall(function() event.hacked = true end) \
             if ok then ctx.admin.follow(999, true) end \
             event.kind = 'hacked' end",
            "",
        )?;
        // b：应看到原始 event（kind=tick、无 hacked）
        reg.register_lua(
            "b.read",
            "function(event, ctx) \
             if event.kind == 'tick' and event.hacked == nil then \
               ctx.admin.new_group('clean', false) end end",
            "",
        )?;
        let mut snap = Snapshot::new();
        reg.dispatch(&TriggerEvent::Tick { at: Utc::now() }, &mut snap, None);
        assert!(
            !snap.res.uid_index.contains_key("999"),
            "新增字段应被 __newindex 拒绝（否则 a 会 follow 999）"
        );
        assert_eq!(
            group_ids(&snap),
            vec![2],
            "b 应看到未被污染的 event 并建组（共享一份时 kind 已被 a 改写）"
        );
        Ok(())
    }


    // TODO test do_fetch（无网络；fetch mock 与事件分发在 db tests N10 覆盖）
}

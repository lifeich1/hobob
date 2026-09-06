//! 轻量 ECS 核心（M1 · 方案 `.plans/m1-ecs-core.md` §4.2 / D1）。
//!
//! - 无 archetype、无 sparse set：`Storage<T>` 内部是一张 `im::HashMap<Entity, T>`，
//!   靠结构共享获得 O(结构) 的快照克隆，靠 `ptr_eq` 获得 O(1) 的「结构未变」判定。
//! - 查询只有两级（`iter` / `iter2`），不做参数化 Query——本规模（数百 up）join 足够。
//! - 本模块不依赖 tokio / store，未来可独立提 crate。
//!
//! 与方案草图的差异（实现备注，均为硬需求）：
//! - `storages` 的值类型从 `Box<dyn Any + Send + Sync>` 换成包装在 `ErasedStorage`
//!   里的 `Box<dyn AnyStorage + Send + Sync>`（`AnyStorage: Any`）：`im::HashMap` 的
//!   `Clone` 要求 `V: Clone`，而 `Box<dyn Any>` 不可克隆；watch 通道要求
//!   `Snapshot: Clone`，所以必须给存储加 `clone_box`。顺带用 `remove_entity`
//!   支持 despawn 时清组件。
//! - 新增 `spawn_at`：`Store::alloc_entity_id`（store.rs，单调持久化）才是实体号
//!   分配权威，启动重建时 World 必须能按已分配的 id 建实体；`spawn` 保留为纯内存
//!   便捷分配器（同样单调、不复用 despawn 过的 id，与 store 语义一致）。
//! - `get_mut` 的 CoW 是**急切**的：只要存在共享者（快照 clone），调用即克隆
//!   （im 的 `make_mut` 语义），即使调用方没写入，`ptr_eq` 也已变 false——
//!   协议上等价于多一次 abort/重试，正确性不受影响（见方案 §8）。

use im::hashmap::HashMap;
use std::any::{Any, TypeId};

/// 实体 id。
///
/// - `0` 非法；
/// - `1` 全局 runtime 实体（由上层 `db` 模块固定，ecs 本身不预设）；
/// - `≥2` 普通实体。
///
/// 实际分配权威在 `store::Store::alloc_entity_id`（单调、持久化）；
/// `World::spawn` 是纯内存便捷分配器，语义与其保持一致（单调、不复用）。
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct Entity(pub u64);

/// 组件标记 trait：可克隆、可跨线程、静态。
pub trait Component: Clone + Send + Sync + 'static {}

impl<T: Clone + Send + Sync + 'static> Component for T {}

/// 单组件存储。`im::HashMap` 结构共享 + O(1) `ptr_eq`。
#[derive(Clone)]
pub struct Storage<T: Component> {
    map: HashMap<Entity, T>,
}

impl<T: Component> Storage<T> {
    /// 空存储。
    #[must_use]
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
        }
    }

    /// 组件数量（含已 despawn 实体的残留，由 despawn 清理）。
    #[must_use]
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// 是否为空。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// 读组件。
    #[must_use]
    pub fn get(&self, e: &Entity) -> Option<&T> {
        self.map.get(e)
    }

    /// 可变读组件（CoW，见模块文档）。
    #[must_use]
    pub fn get_mut(&mut self, e: &Entity) -> Option<&mut T> {
        self.map.get_mut(e)
    }

    /// 写入/替换组件，返回同类型旧值。
    pub fn insert(&mut self, e: Entity, c: T) -> Option<T> {
        self.map.insert(e, c)
    }

    /// 移除组件，返回旧值。
    pub fn remove(&mut self, e: &Entity) -> Option<T> {
        self.map.remove(e)
    }

    /// 是否含该实体的组件。
    #[must_use]
    pub fn contains_key(&self, e: &Entity) -> bool {
        self.map.contains_key(e)
    }

    /// 遍历。迭代序由 im 内部结构决定（**无顺序保证**，测试勿依赖顺序；
    /// 排序语义走上层索引）。
    pub fn iter(&self) -> impl Iterator<Item = (&Entity, &T)> + '_ {
        self.map.iter()
    }
}

impl<T: Component> Default for Storage<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// 实体记录。generation 字段预留（本版不用，避免 0 号歧义）。
#[derive(Clone)]
pub struct EntityRecord {
    alive: bool,
}

/// 类型擦除的存储：为 `World: Clone` 提供 `clone_box`（`Box<dyn Any>` 不可克隆），
/// 并给 despawn 提供跨类型的组件清理入口。`Any` 超 trait 保证可下行转换。
trait AnyStorage: Any + Send + Sync {
    fn clone_box(&self) -> Box<dyn AnyStorage + Send + Sync>;
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
    fn remove_entity(&mut self, e: Entity);
}

impl<T: Component> AnyStorage for Storage<T> {
    fn clone_box(&self) -> Box<dyn AnyStorage + Send + Sync> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn remove_entity(&mut self, e: Entity) {
        self.map.remove(&e);
    }
}

/// 可克隆的擦除存储包装：补上 `im::HashMap` 的 `V: Clone` 约束，
/// 克隆走 `clone_box`（`Storage<T>` 的 `Clone` 是 im 结构共享，O(1) 根节点 Arc clone，
/// `ptr_eq` 语义不受影响）。
struct ErasedStorage(Box<dyn AnyStorage + Send + Sync>);

impl Clone for ErasedStorage {
    fn clone(&self) -> Self {
        Self(self.0.clone_box())
    }
}

/// ECS 世界：实体表 + 按 `TypeId` 索引的组件存储。
///
/// 快照语义：`Clone` 是 O(结构) 浅拷贝（im 结构共享）；`ptr_eq` 判定「结构未变」，
/// 任何一次 `spawn/spawn_at/despawn/insert/remove/get_mut`（有共享者时）都会
/// 让它变 false——等价 v1 的逐字段 ptr 比对。
#[derive(Clone)]
pub struct World {
    entities: HashMap<Entity, EntityRecord>,
    storages: HashMap<TypeId, ErasedStorage>,
}

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}

impl World {
    /// 空世界。
    #[must_use]
    pub fn new() -> Self {
        Self {
            entities: HashMap::new(),
            storages: HashMap::new(),
        }
    }

    /// 分配最小未占 id 并创建实体（从 1 起，单调：despawn 过的 id 不复用，
    /// 与 `Store::alloc_entity_id` 语义一致）。
    pub fn spawn(&mut self) -> Entity {
        let mut id = 1u64;
        while self.entities.contains_key(&Entity(id)) {
            id += 1;
        }
        let e = Entity(id);
        self.entities.insert(e, EntityRecord { alive: true });
        e
    }

    /// 按显式 id 创建实体（启动重建用：id 权威在 store）。id 为 0 或已被占用
    /// 时返回 `None`，不产生任何变更。
    pub fn spawn_at(&mut self, id: u64) -> Option<Entity> {
        if id == 0 {
            return None;
        }
        let e = Entity(id);
        if self.entities.contains_key(&e) {
            return None;
        }
        self.entities.insert(e, EntityRecord { alive: true });
        Some(e)
    }

    /// 销毁实体：标记失活并清掉所有组件。返回是否确实销毁了活实体。
    pub fn despawn(&mut self, e: Entity) -> bool {
        if !self.is_alive(e) {
            return false;
        }
        self.entities.insert(e, EntityRecord { alive: false });
        for (_, storage) in self.storages.iter_mut() {
            storage.0.remove_entity(e);
        }
        true
    }

    /// 实体是否存活。
    #[must_use]
    pub fn is_alive(&self, e: Entity) -> bool {
        self.entities.get(&e).map(|r| r.alive).unwrap_or(false)
    }

    /// 写入/替换组件，返回同类型旧值。实体必须先 spawn（含 spawn_at），
    /// 否则 panic——内部不变量，早期暴露误用。
    pub fn insert<T: Component>(&mut self, e: Entity, c: T) -> Option<T> {
        assert!(
            self.is_alive(e),
            "insert component on non-alive entity {e:?}"
        );
        self.storage_mut::<T>().insert(e, c)
    }

    /// 移除组件，返回旧值。实体不存在/失活/没有该组件时**零变更**返回 `None`
    /// （不创建空 storage、不触发 CoW——保持 `ptr_eq` 的「结构未变」纯判定）。
    pub fn remove<T: Component>(&mut self, e: Entity) -> Option<T> {
        if !self.storage::<T>().is_some_and(|s| s.contains_key(&e)) {
            return None;
        }
        self.storages
            .get_mut(&TypeId::of::<T>())
            .and_then(|b| b.0.as_any_mut().downcast_mut::<Storage<T>>())
            .expect("storage type invariant broken")
            .remove(&e)
    }

    /// 读组件。实体失活时返回 `None`。
    #[must_use]
    pub fn get<T: Component>(&self, e: Entity) -> Option<&T> {
        if !self.is_alive(e) {
            return None;
        }
        self.storage::<T>()?.get(&e)
    }

    /// 可变读组件（CoW：见模块文档的急切克隆说明）。实体失活或没有该组件时
    /// **零变更**返回 `None`（不触发 CoW——保持 `ptr_eq` 的「结构未变」纯判定）。
    #[must_use]
    pub fn get_mut<T: Component>(&mut self, e: Entity) -> Option<&mut T> {
        if !self.is_alive(e) || !self.storage::<T>().is_some_and(|s| s.contains_key(&e)) {
            return None;
        }
        self.storages
            .get_mut(&TypeId::of::<T>())
            .and_then(|b| b.0.as_any_mut().downcast_mut::<Storage<T>>())
            .expect("storage type invariant broken")
            .get_mut(&e)
    }

    /// 实体是否携带某组件。
    #[must_use]
    pub fn contains<T: Component>(&self, e: Entity) -> bool {
        self.is_alive(e) && self.storage::<T>().is_some_and(|s| s.contains_key(&e))
    }

    /// 遍历携带 `T` 的所有活实体。顺序无保证（im 迭代序），排序语义走上层索引。
    pub fn iter<T: Component>(&self) -> impl Iterator<Item = (Entity, &T)> + '_ {
        let alive = &self.entities;
        let base: Box<dyn Iterator<Item = (Entity, &T)> + '_> = match self.storage::<T>() {
            Some(s) => Box::new(s.iter().map(|(e, t)| (*e, t))),
            None => Box::new(std::iter::empty()),
        };
        base.filter(move |(e, _)| alive.get(e).map(|r| r.alive).unwrap_or(false))
    }

    /// 遍历同时携带 `A` 和 `B` 的活实体（两级 join），以**较小存储为驱动集**。
    /// 顺序无保证。
    pub fn iter2<A: Component, B: Component>(&self) -> impl Iterator<Item = (Entity, &A, &B)> + '_ {
        let alive = &self.entities;
        let alive_filter = move |e: &Entity| alive.get(e).map(|r| r.alive).unwrap_or(false);
        let base: Box<dyn Iterator<Item = (Entity, &A, &B)> + '_> =
            match (self.storage::<A>(), self.storage::<B>()) {
                (Some(sa), Some(sb)) => {
                    if sa.len() <= sb.len() {
                        Box::new(
                            sa.iter()
                                .map(|(e, a)| (*e, a))
                                .filter(move |(e, _)| alive_filter(e))
                                .filter_map(move |(e, a)| sb.get(&e).map(|b| (e, a, b))),
                        )
                    } else {
                        Box::new(
                            sb.iter()
                                .map(|(e, b)| (*e, b))
                                .filter(move |(e, _)| alive_filter(e))
                                .filter_map(move |(e, b)| sa.get(&e).map(|a| (e, a, b))),
                        )
                    }
                }
                _ => Box::new(std::iter::empty()),
            };
        base
    }

    /// 「结构未变」判定：两个 map 的根节点指针均相同。任何写路径
    /// （spawn/despawn/insert/remove/get_mut）都会使其变 false。
    #[must_use]
    pub fn ptr_eq(&self, other: &Self) -> bool {
        self.entities.ptr_eq(&other.entities) && self.storages.ptr_eq(&other.storages)
    }

    fn storage<T: Component>(&self) -> Option<&Storage<T>> {
        self.storages.get(&TypeId::of::<T>()).map(|b| {
            b.0.as_any()
                .downcast_ref::<Storage<T>>()
                .expect("storage type invariant broken")
        })
    }

    fn storage_mut<T: Component>(&mut self) -> &mut Storage<T> {
        let tid = TypeId::of::<T>();
        if !self.storages.contains_key(&tid) {
            self.storages
                .insert(tid, ErasedStorage(Box::new(Storage::<T>::new())));
        }
        self.storages
            .get_mut(&tid)
            .and_then(|b| b.0.as_any_mut().downcast_mut::<Storage<T>>())
            .expect("storage type invariant broken")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct Pos(i32);

    #[derive(Clone, Debug, PartialEq)]
    struct Vel(f64);

    #[derive(Clone, Debug, PartialEq)]
    struct Tag(&'static str);

    fn ids<T: Component>(w: &World) -> Vec<u64> {
        let mut v: Vec<u64> = w.iter::<T>().map(|(e, _)| e.0).collect();
        v.sort_unstable();
        v
    }

    #[test]
    fn test_spawn_insert_replace_remove() {
        let mut w = World::new();
        let e = w.spawn();
        assert_eq!(e, Entity(1), "首实体必须是 1（0 非法）");

        // insert：新值无旧值；替换返回同类型旧值
        assert_eq!(w.insert(e, Pos(3)), None);
        assert_eq!(w.get::<Pos>(e), Some(&Pos(3)));
        assert_eq!(w.insert(e, Pos(4)), Some(Pos(3)));
        assert_eq!(w.get::<Pos>(e), Some(&Pos(4)));
        assert!(w.contains::<Pos>(e));

        // 同实体多组件互不干扰
        w.insert(e, Vel(1.5));
        assert_eq!(w.get::<Pos>(e), Some(&Pos(4)));
        assert_eq!(w.get::<Vel>(e), Some(&Vel(1.5)));

        // remove 只移除对应类型
        assert_eq!(w.remove::<Pos>(e), Some(Pos(4)));
        assert_eq!(w.get::<Pos>(e), None);
        assert!(!w.contains::<Pos>(e));
        assert_eq!(w.get::<Vel>(e), Some(&Vel(1.5)));
        assert_eq!(w.remove::<Pos>(e), None, "重复 remove 幂等返回 None");
    }

    #[test]
    fn test_get_mut_cow_and_ptr_eq() {
        let mut w = World::new();
        let e1 = w.spawn();
        let e2 = w.spawn();
        w.insert(e1, Pos(1));
        w.insert(e2, Pos(2));

        let snap = w.clone();
        assert!(w.ptr_eq(&snap), "clone 是结构共享，ptr_eq 必须为 true");

        // CoW 急切克隆：get_mut 一调用（有共享者时），ptr_eq 已变 false
        w.get_mut::<Pos>(e1).expect("e1 有 Pos");
        assert!(
            !w.ptr_eq(&snap),
            "get_mut（有共享者）后世界结构已变，ptr_eq 必须为 false"
        );
        // 写入不影响快照
        *w.get_mut::<Pos>(e1).expect("e1 有 Pos") = Pos(10);
        assert_eq!(w.get::<Pos>(e1), Some(&Pos(10)));
        assert_eq!(snap.get::<Pos>(e1), Some(&Pos(1)), "快照不受写影响");
        assert_eq!(snap.get::<Pos>(e2), Some(&Pos(2)));

        // 失活实体 / 无该组件：返回 None 且不 panic
        w.despawn(e2);
        assert_eq!(w.get_mut::<Pos>(e2), None);
        assert_eq!(w.get_mut::<Vel>(e1), None);
    }

    #[test]
    fn test_iter_and_iter2_join() {
        let mut w = World::new();
        let e1 = w.spawn();
        let e2 = w.spawn();
        let e3 = w.spawn();
        let e4 = w.spawn();
        let _e5 = w.spawn(); // 无组件实体：iter 不得产出
        w.insert(e1, Pos(1));
        w.insert(e1, Vel(0.1));
        w.insert(e2, Pos(2));
        w.insert(e3, Vel(0.3));
        w.insert(e4, Vel(0.4));

        assert_eq!(ids::<Pos>(&w), vec![1, 2]);
        assert_eq!(ids::<Vel>(&w), vec![1, 3, 4]);
        assert_eq!(
            ids::<Tag>(&w),
            Vec::<u64>::new(),
            "无该组件存储时 iter 为空"
        );

        // iter2 = join：只产出双组件实体；两个方向（驱动集=较小存储）结果一致
        let join12: Vec<u64> = w.iter2::<Pos, Vel>().map(|(e, _, _)| e.0).collect();
        assert_eq!(join12, vec![1]);
        let join21: Vec<u64> = w.iter2::<Vel, Pos>().map(|(e, _, _)| e.0).collect();
        assert_eq!(join21, vec![1]);
        let (e, p, v) = w.iter2::<Pos, Vel>().next().expect("有一个 join 实体");
        assert_eq!((e.0, p, v), (1, &Pos(1), &Vel(0.1)));

        // 同类型 iter2：各自产出（顺序无保证，排序后断言）
        let mut join_same: Vec<(u64, i32, i32)> = w
            .iter2::<Pos, Pos>()
            .map(|(e, a, b)| (e.0, a.0, b.0))
            .collect();
        join_same.sort_unstable();
        assert_eq!(join_same, vec![(1, 1, 1), (2, 2, 2)]);
    }

    #[test]
    fn test_despawn() {
        let mut w = World::new();
        let e = w.spawn();
        w.insert(e, Pos(7));
        w.insert(e, Vel(0.7));

        assert!(w.despawn(e));
        assert!(!w.despawn(e), "重复 despawn 返回 false");
        assert!(!w.is_alive(e));
        assert_eq!(w.get::<Pos>(e), None, "失活后查询不可见");
        assert_eq!(w.get::<Vel>(e), None);
        assert!(!w.contains::<Pos>(e));
        assert_eq!(ids::<Pos>(&w), Vec::<u64>::new(), "iter 过滤失活实体");
        assert_eq!(w.iter2::<Pos, Vel>().count(), 0);

        // 组件已清理：重新 spawn 新实体不残留旧数据
        let e2 = w.spawn();
        assert_eq!(ids::<Pos>(&w), Vec::<u64>::new());
        w.insert(e2, Pos(8));
        assert_eq!(ids::<Pos>(&w), vec![e2.0]);

        // spawn 单调：不复用 despawn 过的 id
        assert!(e2.0 > e.0);
        assert_eq!(w.remove::<Pos>(e), None, "失活实体的组件已随 despawn 清理");
    }

    #[test]
    fn test_spawn_at_explicit_id() {
        let mut w = World::new();
        // 启动重建：按 store 已分配的 id 建实体
        assert_eq!(w.spawn_at(2), Some(Entity(2)));
        assert_eq!(w.spawn_at(3), Some(Entity(3)));
        assert_eq!(w.spawn_at(2), None, "id 已占用");
        assert_eq!(w.spawn_at(0), None, "id 0 非法");
        assert!(!w.is_alive(Entity(0)));

        // spawn 继续取最小未占 id（1 仍空着）
        assert_eq!(w.spawn(), Entity(1));
        assert_eq!(w.spawn(), Entity(4));
        assert_eq!(w.spawn(), Entity(5));
    }

    #[test]
    fn test_ptr_eq_sensitivity() {
        let mut w = World::new();
        let e = w.spawn();
        w.insert(e, Pos(1));
        let snap = w.clone();
        assert!(w.ptr_eq(&snap));

        // 各写路径逐一使 ptr_eq 变 false
        let e2 = w.spawn();
        assert!(!w.ptr_eq(&snap));

        let snap = w.clone();
        w.insert(e2, Pos(2));
        assert!(!w.ptr_eq(&snap));

        let snap = w.clone();
        w.remove::<Pos>(e2);
        assert!(!w.ptr_eq(&snap));

        let snap = w.clone();
        w.despawn(e);
        assert!(!w.ptr_eq(&snap));

        // 零变更路径不触发 CoW：ptr_eq 保持
        let snap_nop = w.clone();
        assert_eq!(w.remove::<Tag>(e2), None, "e2 无 Tag 组件");
        assert_eq!(w.get_mut::<Tag>(e2), None);
        assert_eq!(w.get_mut::<Pos>(e), None, "e 已失活");
        assert_eq!(w.remove::<Pos>(Entity(999)), None);
        assert!(w.ptr_eq(&snap_nop), "no-op 不改变结构");

        // 快照仍看到旧世界：失活前 e 有 Pos，失活后快照里 e 仍存活
        assert_eq!(snap.get::<Pos>(e), Some(&Pos(1)));
        assert!(snap.is_alive(e));

        // 世界间 ptr_eq 比较：同源 clone 互为 true，不同根为 false
        let snap2 = snap.clone();
        assert!(snap.ptr_eq(&snap2));
        assert!(!w.ptr_eq(&snap2));
    }

    #[test]
    fn test_entity_zero_never_valid() {
        let mut w = World::new();
        assert!(!w.is_alive(Entity(0)));
        assert_eq!(w.get::<Pos>(Entity(0)), None);
        assert_eq!(w.get_mut::<Pos>(Entity(0)), None);
        assert!(!w.contains::<Pos>(Entity(0)));
        assert_eq!(w.remove::<Pos>(Entity(0)), None);
        assert!(!w.despawn(Entity(0)));
    }
}

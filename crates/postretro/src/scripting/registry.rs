// Entity/component registry: the scripting surface that scripts address.
// See: context/lib/scripting.md

use std::collections::HashMap;
use std::fmt;

use glam::{Quat, Vec3};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::components::billboard_emitter::BillboardEmitterComponent;
use super::components::light::LightComponent;
use super::components::particle::ParticleState;
use super::components::sprite_visual::SpriteVisual;

/// Packed entity identifier: `index: 16 | generation: 16`.
///
/// 16/16 gives 65,536 live slots and 65,536 generations per slot — comfortably
/// above the design ceiling for a single level. When a slot's generation is
/// bumped past `u16::MAX` on despawn, the slot is **permanently retired**
/// (removed from the free list and never re-allocated); see [`EntityRegistry::despawn`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct EntityId(u32);

impl EntityId {
    const INDEX_BITS: u32 = 16;
    const INDEX_MASK: u32 = (1 << Self::INDEX_BITS) - 1;

    fn new(index: u16, generation: u16) -> Self {
        Self(((generation as u32) << Self::INDEX_BITS) | (index as u32))
    }

    fn index(self) -> u16 {
        (self.0 & Self::INDEX_MASK) as u16
    }

    fn generation(self) -> u16 {
        (self.0 >> Self::INDEX_BITS) as u16
    }

    /// Raw packed `u32` representation. The scripting FFI layer crosses the
    /// language boundary as a JS number / Lua integer — both of which can
    /// losslessly carry a 32-bit integer.
    pub(crate) fn to_raw(self) -> u32 {
        self.0
    }

    /// Inverse of [`EntityId::to_raw`]. The binding layer reconstructs an
    /// `EntityId` from a script-supplied number; validation happens when the
    /// registry dereferences it.
    pub(crate) fn from_raw(raw: u32) -> Self {
        Self(raw)
    }
}

impl fmt::Debug for EntityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "EntityId {{ index: {}, generation: {} }}",
            self.index(),
            self.generation()
        )
    }
}

impl fmt::Display for EntityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}@{}", self.index(), self.generation())
    }
}

/// Enumeration of every component kind the scripting surface knows about.
///
/// `#[repr(u16)]` makes the discriminant a zero-cost index into the
/// component-storage vector array. Not `#[non_exhaustive]`: the enum is
/// `pub(crate)`, and `non_exhaustive` is a no-op on non-`pub` items.
#[repr(u16)]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub(crate) enum ComponentKind {
    Transform = 0,
    Light = 1,
    BillboardEmitter = 2,
    ParticleState = 3,
    SpriteVisual = 4,
}

impl ComponentKind {
    /// Count of variants, derived from an exhaustive const array.
    /// `std::mem::variant_count` is not yet const-stable on this toolchain,
    /// so we list every variant once; the compiler enforces exhaustiveness in
    /// match arms that touch `ComponentKind` elsewhere.
    pub(crate) const COUNT: usize = {
        const VARIANTS: &[ComponentKind] = &[
            ComponentKind::Transform,
            ComponentKind::Light,
            ComponentKind::BillboardEmitter,
            ComponentKind::ParticleState,
            ComponentKind::SpriteVisual,
        ];
        VARIANTS.len()
    };
}

/// Position / rotation / scale in world space.
///
/// `rotation` is stored as a quaternion. Scripts receive Euler degrees
/// (`pitch`, `yaw`, `roll`) converted at the FFI boundary; never a raw
/// quaternion.
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub(crate) struct Transform {
    pub(crate) position: Vec3,
    pub(crate) rotation: Quat,
    pub(crate) scale: Vec3,
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            position: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
        }
    }
}

/// Serde-serializable container for every concrete component struct.
///
/// The `kind` discriminant matches [`ComponentKind`] one-to-one; downstream
/// FFI plans serialize this directly to/from JS/Luau tables.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum ComponentValue {
    Transform(Transform),
    Light(LightComponent),
    BillboardEmitter(BillboardEmitterComponent),
    ParticleState(ParticleState),
    SpriteVisual(SpriteVisual),
}

/// Trait implemented by concrete component structs so they can be stored
/// and looked up in the registry without a string key.
///
/// `pub(crate)` — sealed in practice by virtue of the crate-private scope.
pub(crate) trait Component: Sized {
    const KIND: ComponentKind;
    fn from_value(value: &ComponentValue) -> Option<&Self>;
    fn into_value(self) -> ComponentValue;
}

impl Component for Transform {
    const KIND: ComponentKind = ComponentKind::Transform;

    fn from_value(value: &ComponentValue) -> Option<&Self> {
        match value {
            ComponentValue::Transform(t) => Some(t),
            _ => None,
        }
    }

    fn into_value(self) -> ComponentValue {
        ComponentValue::Transform(self)
    }
}

impl Component for LightComponent {
    const KIND: ComponentKind = ComponentKind::Light;

    fn from_value(value: &ComponentValue) -> Option<&Self> {
        match value {
            ComponentValue::Light(l) => Some(l),
            _ => None,
        }
    }

    fn into_value(self) -> ComponentValue {
        ComponentValue::Light(self)
    }
}

impl Component for BillboardEmitterComponent {
    const KIND: ComponentKind = ComponentKind::BillboardEmitter;

    fn from_value(value: &ComponentValue) -> Option<&Self> {
        match value {
            ComponentValue::BillboardEmitter(e) => Some(e),
            _ => None,
        }
    }

    fn into_value(self) -> ComponentValue {
        ComponentValue::BillboardEmitter(self)
    }
}

impl Component for ParticleState {
    const KIND: ComponentKind = ComponentKind::ParticleState;

    fn from_value(value: &ComponentValue) -> Option<&Self> {
        match value {
            ComponentValue::ParticleState(p) => Some(p),
            _ => None,
        }
    }

    fn into_value(self) -> ComponentValue {
        ComponentValue::ParticleState(self)
    }
}

impl Component for SpriteVisual {
    const KIND: ComponentKind = ComponentKind::SpriteVisual;

    fn from_value(value: &ComponentValue) -> Option<&Self> {
        match value {
            ComponentValue::SpriteVisual(s) => Some(s),
            _ => None,
        }
    }

    fn into_value(self) -> ComponentValue {
        ComponentValue::SpriteVisual(self)
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub(crate) enum RegistryError {
    #[error("entity {0} does not exist")]
    EntityNotFound(EntityId),
    #[error("entity {id} has no component of kind {kind:?}")]
    ComponentNotFound { id: EntityId, kind: ComponentKind },
    #[error("entity id {0} is stale (generation mismatch)")]
    GenerationMismatch(EntityId),
}

/// Internal per-slot metadata. `generation` matches the generation a live
/// `EntityId` must present; `retired` flags a slot permanently removed from
/// circulation after a generation wrap (see [`EntityRegistry::despawn`]).
#[derive(Debug)]
struct Slot {
    generation: u16,
    live: bool,
    retired: bool,
}

pub(crate) struct EntityRegistry {
    slots: Vec<Slot>,
    free_list: Vec<u16>,
    /// One component column per `ComponentKind`, indexed by slot index.
    components: [Vec<Option<ComponentValue>>; ComponentKind::COUNT],
    /// Parallel column of per-entity tag lists. Space-delimited in the PRL
    /// wire format; stored here as pre-split `Vec<String>` per slot. An entity
    /// matches `world.query({ tag: "t" })` when any of its tags equals `"t"`.
    /// Empty vec means untagged. Column is resized in lockstep with `components`.
    tags: Vec<Vec<String>>,
    /// Per-entity key/value bag carried over from the FGD `.map` entity that
    /// spawned the entity. Populated by built-in classname handlers (and any
    /// future spawn paths that originate from a map entity); read by the
    /// `getEntityProperty` primitive. Sparsely populated — entities spawned
    /// outside the map-load path have no entry here.
    kvp_table: HashMap<EntityId, HashMap<String, String>>,
}

impl EntityRegistry {
    pub(crate) fn new() -> Self {
        Self {
            slots: Vec::new(),
            free_list: Vec::new(),
            components: [Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            tags: Vec::new(),
            kvp_table: HashMap::new(),
        }
    }

    /// Attach the per-placement KVP bag (authored on the source `.map` entity)
    /// to a spawned entity. Called by built-in classname handlers immediately
    /// after spawn so `getEntityProperty` works uniformly regardless of which
    /// handler ran. Empty bags are stored as an empty map (still creates an
    /// entry); pass through unchanged for readability.
    pub(crate) fn set_map_kvps(
        &mut self,
        id: EntityId,
        kvps: HashMap<String, String>,
    ) -> Result<(), RegistryError> {
        let _ = self.validate(id)?;
        self.kvp_table.insert(id, kvps);
        Ok(())
    }

    /// Read a single key from an entity's per-placement KVP bag. Returns
    /// `Ok(None)` for both "entity has no KVP bag" and "key not present" —
    /// scripts cannot distinguish, and the script-side contract is "absent
    /// keys read as null". Stale entity ids surface as `GenerationMismatch` so
    /// the FFI layer can map to a typed script error.
    pub(crate) fn get_map_kvp(
        &self,
        id: EntityId,
        key: &str,
    ) -> Result<Option<String>, RegistryError> {
        let _ = self.validate(id)?;
        Ok(self.kvp_table.get(&id).and_then(|m| m.get(key).cloned()))
    }

    /// Attach (or overwrite) the tag list on an entity. An empty vec clears
    /// all tags. `world.query` checks membership: an entity matches filter
    /// tag `"t"` when any of its tags equals `"t"`.
    pub(crate) fn set_tags(
        &mut self,
        id: EntityId,
        tags: Vec<String>,
    ) -> Result<(), RegistryError> {
        let index = self.validate(id)?;
        self.tags[index] = tags;
        Ok(())
    }

    pub(crate) fn get_tags(&self, id: EntityId) -> Result<&[String], RegistryError> {
        let index = self.validate(id)?;
        Ok(&self.tags[index])
    }

    /// Iterate every live entity whose component column of `kind` is populated
    /// and whose tag list (if `tag_filter` is `Some`) contains the filter tag.
    /// When `tag_filter` is `None`, every entity with the component matches.
    ///
    /// Yields `(EntityId, &ComponentValue)` pairs in slot-index order. Used by
    /// the `world.query` primitive.
    pub(crate) fn query_by_component_and_tag<'a>(
        &'a self,
        kind: ComponentKind,
        tag_filter: Option<&'a str>,
    ) -> impl Iterator<Item = (EntityId, &'a ComponentValue)> + 'a {
        let column = &self.components[kind as usize];
        self.slots
            .iter()
            .enumerate()
            .filter_map(move |(idx, slot)| {
                if !slot.live || slot.retired {
                    return None;
                }
                if let Some(want) = tag_filter {
                    let entity_tags = self.tags.get(idx).map(|v| v.as_slice()).unwrap_or(&[]);
                    if !entity_tags.iter().any(|t| t == want) {
                        return None;
                    }
                }
                let cell = column.get(idx).and_then(|c| c.as_ref())?;
                let id = EntityId::new(idx as u16, slot.generation);
                Some((id, cell))
            })
    }

    /// Iterate every live entity that carries a component of the given kind.
    /// Yields `(EntityId, &ComponentValue)` pairs in slot-index order.
    ///
    /// Used by scripted bridges (e.g. the light bridge) to walk their
    /// component set each frame without threading a separate index through
    /// every subsystem.
    pub(crate) fn iter_with_kind(
        &self,
        kind: ComponentKind,
    ) -> impl Iterator<Item = (EntityId, &ComponentValue)> + '_ {
        let column = &self.components[kind as usize];
        self.slots
            .iter()
            .enumerate()
            .filter_map(move |(idx, slot)| {
                if !slot.live || slot.retired {
                    return None;
                }
                let cell = column.get(idx).and_then(|c| c.as_ref())?;
                // SAFETY: index fits in u16 because `slots.len() <= u16::MAX + 1`
                // and we never allocate past that in `spawn`.
                let id = EntityId::new(idx as u16, slot.generation);
                Some((id, cell))
            })
    }

    /// Returns `None` when all 65,536 entity slots are exhausted (free list
    /// empty and slot vector at `u16::MAX`). Callers that must not panic
    /// (e.g. script primitives crossing the FFI boundary) should prefer this
    /// over [`EntityRegistry::spawn`]. Tags are attached at slot-mark time;
    /// pass `&[]` to spawn untagged.
    pub(crate) fn try_spawn(&mut self, transform: Transform, tags: &[String]) -> Option<EntityId> {
        if self.free_list.is_empty() && self.slots.len() >= u16::MAX as usize {
            return None;
        }
        let id = self.spawn(transform);
        if !tags.is_empty() {
            // `set_tags` only fails on a stale id — the id was just returned.
            let _ = self.set_tags(id, tags.to_vec());
        }
        Some(id)
    }

    pub(crate) fn spawn(&mut self, transform: Transform) -> EntityId {
        let index = if let Some(i) = self.free_list.pop() {
            i
        } else {
            let i = u16::try_from(self.slots.len())
                .expect("entity index exceeds u16::MAX; raise index bit width");
            self.slots.push(Slot {
                generation: 0,
                live: false,
                retired: false,
            });
            for column in &mut self.components {
                column.push(None);
            }
            self.tags.push(vec![]);
            i
        };

        let slot = &mut self.slots[index as usize];
        debug_assert!(!slot.live, "spawn allocated a live slot");
        debug_assert!(!slot.retired, "spawn allocated a retired slot");
        slot.live = true;

        let id = EntityId::new(index, slot.generation);
        self.components[ComponentKind::Transform as usize][index as usize] =
            Some(ComponentValue::Transform(transform));
        id
    }

    pub(crate) fn despawn(&mut self, id: EntityId) -> Result<(), RegistryError> {
        let index = id.index() as usize;
        let slot = self
            .slots
            .get_mut(index)
            .ok_or(RegistryError::EntityNotFound(id))?;

        if slot.retired || !slot.live || slot.generation != id.generation() {
            return Err(RegistryError::GenerationMismatch(id));
        }

        for column in &mut self.components {
            column[index] = None;
        }
        self.tags[index].clear();
        self.kvp_table.remove(&id);
        slot.live = false;

        // Generation-wrap retirement: reusing the slot after wrap would let a
        // stale `EntityId` compare equal to a freshly allocated one. Retiring
        // the slot is a tiny long-tail memory cost for a sound uniqueness
        // invariant.
        if slot.generation == u16::MAX {
            slot.retired = true;
            // NOT pushed back onto the free list — permanent retirement.
        } else {
            slot.generation += 1;
            self.free_list.push(id.index());
        }
        Ok(())
    }

    pub(crate) fn exists(&self, id: EntityId) -> bool {
        match self.slots.get(id.index() as usize) {
            Some(slot) => slot.live && !slot.retired && slot.generation == id.generation(),
            None => false,
        }
    }

    fn validate(&self, id: EntityId) -> Result<usize, RegistryError> {
        let index = id.index() as usize;
        let slot = self
            .slots
            .get(index)
            .ok_or(RegistryError::EntityNotFound(id))?;
        if !slot.live || slot.retired || slot.generation != id.generation() {
            return Err(RegistryError::GenerationMismatch(id));
        }
        Ok(index)
    }

    pub(crate) fn get_component<T: Component>(&self, id: EntityId) -> Result<&T, RegistryError> {
        let index = self.validate(id)?;
        let column = &self.components[T::KIND as usize];
        let value = column
            .get(index)
            .and_then(|cell| cell.as_ref())
            .ok_or(RegistryError::ComponentNotFound { id, kind: T::KIND })?;
        T::from_value(value).ok_or(RegistryError::ComponentNotFound { id, kind: T::KIND })
    }

    pub(crate) fn set_component<T: Component>(
        &mut self,
        id: EntityId,
        value: T,
    ) -> Result<(), RegistryError> {
        let index = self.validate(id)?;
        self.components[T::KIND as usize][index] = Some(value.into_value());
        Ok(())
    }

    pub(crate) fn remove_component<T: Component>(
        &mut self,
        id: EntityId,
    ) -> Result<(), RegistryError> {
        let index = self.validate(id)?;
        let cell = &mut self.components[T::KIND as usize][index];
        if cell.is_none() {
            return Err(RegistryError::ComponentNotFound { id, kind: T::KIND });
        }
        *cell = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::components::billboard_emitter::{
        BillboardEmitterComponent, SpinAnimation,
    };
    use crate::scripting::components::particle::ParticleState;
    use crate::scripting::components::sprite_visual::SpriteVisual;

    fn sample_transform() -> Transform {
        Transform {
            position: Vec3::new(1.0, 2.0, 3.0),
            rotation: Quat::from_rotation_y(0.5),
            scale: Vec3::splat(2.0),
        }
    }

    #[test]
    fn entity_id_display_shows_index_and_generation() {
        let id = EntityId::new(42, 7);
        assert_eq!(format!("{}", id), "#42@7");
        assert_eq!(id.index(), 42);
        assert_eq!(id.generation(), 7);
    }

    #[test]
    fn spawn_and_exists_round_trip() {
        let mut reg = EntityRegistry::new();
        let id = reg.spawn(sample_transform());
        assert!(reg.exists(id));
    }

    #[test]
    fn despawn_clears_exists_and_reuses_slot_with_bumped_generation() {
        let mut reg = EntityRegistry::new();
        let a = reg.spawn(Transform::default());
        reg.despawn(a).unwrap();
        assert!(!reg.exists(a));

        let b = reg.spawn(Transform::default());
        assert_eq!(b.index(), a.index(), "freed slot should be reused");
        assert_eq!(
            b.generation(),
            a.generation() + 1,
            "reused slot should carry a bumped generation"
        );
        assert!(reg.exists(b));
    }

    #[test]
    fn use_after_despawn_returns_generation_mismatch_without_panicking() {
        let mut reg = EntityRegistry::new();
        let id = reg.spawn(Transform::default());
        reg.despawn(id).unwrap();

        assert_eq!(
            reg.get_component::<Transform>(id),
            Err(RegistryError::GenerationMismatch(id))
        );
        assert_eq!(
            reg.set_component(id, Transform::default()),
            Err(RegistryError::GenerationMismatch(id))
        );
        assert_eq!(
            reg.remove_component::<Transform>(id),
            Err(RegistryError::GenerationMismatch(id))
        );
        assert_eq!(reg.despawn(id), Err(RegistryError::GenerationMismatch(id)));
    }

    #[test]
    fn out_of_bounds_entity_id_returns_entity_not_found() {
        let reg = EntityRegistry::new();
        let bogus = EntityId::new(999, 0);
        assert_eq!(
            reg.get_component::<Transform>(bogus),
            Err(RegistryError::EntityNotFound(bogus))
        );
    }

    #[test]
    fn component_get_set_remove_round_trip() {
        let mut reg = EntityRegistry::new();
        let id = reg.spawn(sample_transform());

        let t = reg.get_component::<Transform>(id).unwrap();
        assert_eq!(t.position, Vec3::new(1.0, 2.0, 3.0));

        let replacement = Transform {
            position: Vec3::new(9.0, 9.0, 9.0),
            ..Transform::default()
        };
        reg.set_component(id, replacement).unwrap();
        assert_eq!(
            reg.get_component::<Transform>(id).unwrap().position,
            Vec3::new(9.0, 9.0, 9.0)
        );

        reg.remove_component::<Transform>(id).unwrap();
        assert_eq!(
            reg.get_component::<Transform>(id),
            Err(RegistryError::ComponentNotFound {
                id,
                kind: ComponentKind::Transform,
            })
        );
        // Double-remove is also ComponentNotFound, not a panic.
        assert_eq!(
            reg.remove_component::<Transform>(id),
            Err(RegistryError::ComponentNotFound {
                id,
                kind: ComponentKind::Transform,
            })
        );
    }

    #[test]
    fn generation_wrap_retires_slot_and_rejects_stale_ids() {
        let mut reg = EntityRegistry::new();
        let id = reg.spawn(Transform::default());
        let index = id.index();

        // Force the slot's generation to u16::MAX so the next despawn
        // would wrap. This is the condition the retirement path exists for.
        reg.slots[index as usize].generation = u16::MAX;
        let live_id = EntityId::new(index, u16::MAX);
        assert!(reg.exists(live_id));

        reg.despawn(live_id).unwrap();

        // Slot must NOT be on the free list.
        assert!(
            !reg.free_list.contains(&index),
            "retired slot must not be returned to the free list"
        );
        assert!(
            reg.slots[index as usize].retired,
            "slot must be marked retired"
        );

        // Stale EntityId targeting the retired slot returns GenerationMismatch,
        // never a false positive.
        assert!(!reg.exists(live_id));
        assert_eq!(
            reg.get_component::<Transform>(live_id),
            Err(RegistryError::GenerationMismatch(live_id))
        );

        // Any fresh spawn must land on a brand-new slot, not the retired one.
        let fresh = reg.spawn(Transform::default());
        assert_ne!(fresh.index(), index, "retired index must not be reused");
    }

    #[test]
    fn query_by_component_and_tag_matches_first_tag_of_multi_tag_entity() {
        // Regression: tag migration from `Option<String>` to `Vec<String>` —
        // an entity with multiple tags must independently match a query for
        // any one of them.
        let mut reg = EntityRegistry::new();
        let id = reg.spawn(Transform::default());
        reg.set_tags(id, vec!["wave1".into(), "reactorMonster".into()])
            .unwrap();

        let matches: Vec<EntityId> = reg
            .query_by_component_and_tag(ComponentKind::Transform, Some("wave1"))
            .map(|(eid, _)| eid)
            .collect();
        assert_eq!(matches, vec![id]);
    }

    #[test]
    fn query_by_component_and_tag_matches_last_tag_of_multi_tag_entity() {
        // Regression: tag migration from `Option<String>` to `Vec<String>` —
        // membership match must work for any position in the tag list, not
        // only the first.
        let mut reg = EntityRegistry::new();
        let id = reg.spawn(Transform::default());
        reg.set_tags(id, vec!["wave1".into(), "reactorMonster".into()])
            .unwrap();

        let matches: Vec<EntityId> = reg
            .query_by_component_and_tag(ComponentKind::Transform, Some("reactorMonster"))
            .map(|(eid, _)| eid)
            .collect();
        assert_eq!(matches, vec![id]);
    }

    #[test]
    fn query_by_component_and_tag_excludes_entity_when_no_tag_matches() {
        // Regression: tag migration from `Option<String>` to `Vec<String>` —
        // a multi-tag entity must NOT match a tag it doesn't carry.
        let mut reg = EntityRegistry::new();
        let id = reg.spawn(Transform::default());
        reg.set_tags(id, vec!["wave1".into(), "reactorMonster".into()])
            .unwrap();

        let matches: Vec<EntityId> = reg
            .query_by_component_and_tag(ComponentKind::Transform, Some("unrelated"))
            .map(|(eid, _)| eid)
            .collect();
        assert!(
            matches.is_empty(),
            "entity {id} matched tag 'unrelated' it does not carry"
        );
    }

    #[test]
    fn billboard_emitter_set_get_round_trip() {
        let mut reg = EntityRegistry::new();
        let id = reg.spawn(Transform::default());
        let value = BillboardEmitterComponent {
            rate: 6.0,
            burst: Some(3),
            spread: 0.4,
            lifetime: 3.0,
            velocity: [0.0, 1.5, 0.0],
            buoyancy: 0.2,
            drag: 0.5,
            size_over_lifetime: vec![0.3, 1.0, 0.5],
            opacity_over_lifetime: vec![0.0, 0.8, 0.0],
            color: [1.0, 0.6, 0.2],
            sprite: "smoke".into(),
            spin_rate: 1.2,
            spin_animation: Some(SpinAnimation {
                duration: 2.0,
                rate_curve: vec![0.0, 3.5, 0.0],
            }),
        };
        reg.set_component(id, value.clone()).unwrap();
        let back = reg.get_component::<BillboardEmitterComponent>(id).unwrap();
        assert_eq!(*back, value);
    }

    #[test]
    fn particle_state_set_get_round_trip() {
        let mut reg = EntityRegistry::new();
        let parent = reg.spawn(Transform::default());
        let id = reg.spawn(Transform::default());
        let value = ParticleState {
            velocity: [0.5, 1.5, -0.25],
            age: 0.4,
            lifetime: 2.5,
            buoyancy: -1.0,
            drag: 0.3,
            size_curve: vec![0.2, 1.0, 0.5],
            opacity_curve: vec![0.0, 1.0, 0.0],
            emitter: Some(parent),
        };
        reg.set_component(id, value.clone()).unwrap();
        let back = reg.get_component::<ParticleState>(id).unwrap();
        assert_eq!(*back, value);
    }

    #[test]
    fn sprite_visual_set_get_round_trip() {
        let mut reg = EntityRegistry::new();
        let id = reg.spawn(Transform::default());
        let value = SpriteVisual {
            sprite: "smoke".into(),
            size: 1.25,
            opacity: 0.5,
            rotation: 0.75,
            tint: [1.0, 0.6, 0.2],
        };
        reg.set_component(id, value.clone()).unwrap();
        let back = reg.get_component::<SpriteVisual>(id).unwrap();
        assert_eq!(*back, value);
    }

    #[test]
    fn spawn_despawn_10k_cycles_under_10ms_release_sanity() {
        // Sanity check — not a strict perf target. In debug this runs
        // slower; we assert only on release builds so CI debug runs don't
        // flake on slow hardware.
        let mut reg = EntityRegistry::new();
        let start = std::time::Instant::now();
        for _ in 0..10_000 {
            let id = reg.spawn(Transform::default());
            reg.despawn(id).unwrap();
        }
        let elapsed = start.elapsed();

        if !cfg!(debug_assertions) {
            assert!(
                elapsed.as_millis() < 10,
                "10k spawn/despawn cycles took {:?}, expected <10ms on release",
                elapsed
            );
        }
    }
}

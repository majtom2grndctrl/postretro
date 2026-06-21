// Entity/component registry: the scripting surface that scripts address.
// See: context/lib/scripting.md

use std::collections::HashMap;
use std::fmt;

use glam::{Quat, Vec3};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::components::agent::AgentComponent;
use super::components::billboard_emitter::BillboardEmitterComponent;
use super::components::brain::BrainComponent;
use super::components::fog_volume::FogAnimation;
use super::components::health::HealthComponent;
use super::components::light::LightComponent;
use super::components::mesh::MeshComponent;
use super::components::particle::ParticleState;
use super::components::player_movement::PlayerMovementComponent;
use super::components::sprite_visual::SpriteVisual;
use super::components::weapon::WeaponComponent;
use super::provenance::DescriptorProvenance;

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

/// All component kinds the engine tracks internally.
///
/// Not all variants are queryable via the script surface (`worldQuery`):
/// `PlayerMovement` and `Weapon` are engine-owned runtime components, so
/// scripts address them through higher-level descriptors and systems.
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
    FogVolume = 5,
    PlayerMovement = 6,
    Weapon = 7,
    DescriptorProvenance = 8,
    Mesh = 9,
    Health = 10,
    /// Movable navigation agent (engine-internal, like `PlayerMovement` — never
    /// reachable through `worldQuery`). See `components::agent`.
    Agent = 11,
    /// Engine-owned AI brain: enemy FSM logical state, timers, and resolved
    /// `components.ai` tuning (engine-internal, like `PlayerMovement`/`Agent` —
    /// never reachable through `worldQuery`). See `components::brain`.
    Brain = 12,
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
            ComponentKind::FogVolume,
            ComponentKind::PlayerMovement,
            ComponentKind::Weapon,
            ComponentKind::DescriptorProvenance,
            ComponentKind::Mesh,
            ComponentKind::Health,
            ComponentKind::Agent,
            ComponentKind::Brain,
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
/// The `kind` discriminant matches [`ComponentKind`] one-to-one.
// Not Copy: FogVolumeComponent carries a heap-backed Vec<f32> (density curve).
// Do not add Copy here without first removing that field.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum ComponentValue {
    Transform(Transform),
    Light(LightComponent),
    BillboardEmitter(BillboardEmitterComponent),
    ParticleState(ParticleState),
    SpriteVisual(SpriteVisual),
    FogVolume(FogVolumeComponent),
    // Boxed: `PlayerMovementComponent` is by far the largest component
    // (~400+ bytes, and the dash `DashPrograms` bound trees grow it further),
    // so an unboxed variant would inflate every `ComponentValue` to its size
    // (clippy::large_enum_variant). Boxing keeps the enum compact; the player
    // pawn is a singleton, so the extra indirection is paid once.
    PlayerMovement(Box<PlayerMovementComponent>),
    Weapon(WeaponComponent),
    DescriptorProvenance(DescriptorProvenance),
    Mesh(MeshComponent),
    Health(HealthComponent),
    Agent(AgentComponent),
    Brain(BrainComponent),
}

impl ComponentValue {
    pub(crate) fn kind(&self) -> ComponentKind {
        match self {
            ComponentValue::Transform(_) => ComponentKind::Transform,
            ComponentValue::Light(_) => ComponentKind::Light,
            ComponentValue::BillboardEmitter(_) => ComponentKind::BillboardEmitter,
            ComponentValue::ParticleState(_) => ComponentKind::ParticleState,
            ComponentValue::SpriteVisual(_) => ComponentKind::SpriteVisual,
            ComponentValue::FogVolume(_) => ComponentKind::FogVolume,
            ComponentValue::PlayerMovement(_) => ComponentKind::PlayerMovement,
            ComponentValue::Weapon(_) => ComponentKind::Weapon,
            ComponentValue::DescriptorProvenance(_) => ComponentKind::DescriptorProvenance,
            ComponentValue::Mesh(_) => ComponentKind::Mesh,
            ComponentValue::Health(_) => ComponentKind::Health,
            ComponentValue::Agent(_) => ComponentKind::Agent,
            ComponentValue::Brain(_) => ComponentKind::Brain,
        }
    }
}

fn default_fog_tint() -> [f32; 3] {
    [1.0, 1.0, 1.0]
}
fn default_fog_saturation() -> f32 {
    1.0
}
fn default_fog_min_brightness() -> f32 {
    0.0
}
fn default_fog_light_range() -> f32 {
    1.0
}

/// Script-facing fog volume component. Carries the runtime-tweakable fog
/// parameters; the AABB lives in the `FogVolumeBridge` side-table (baked at
/// level load) and is not exposed through `ComponentValue` because it is not
/// runtime-settable.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub(crate) struct FogVolumeComponent {
    pub(crate) density: f32,
    pub(crate) glow: f32,
    pub(crate) edge_softness: f32,
    pub(crate) falloff: f32,
    /// Scatter tint multiplier. `[1, 1, 1]` = no tint. Applied after saturation.
    #[serde(default = "default_fog_tint")]
    pub(crate) tint: [f32; 3],
    /// Scatter saturation: 0 = greyscale, 1 = natural, >1 = boosted.
    #[serde(default = "default_fog_saturation")]
    pub(crate) saturation: f32,
    #[serde(default = "default_fog_min_brightness")]
    pub(crate) min_brightness: f32,
    #[serde(default = "default_fog_light_range")]
    pub(crate) light_range: f32,
    /// Optional animation carrying any combination of density, saturation,
    /// min_brightness, and light_range curves. `None`
    /// holds static values. Installed by the `setFogAnimation` reaction
    /// primitive; the fog bridge evaluates per frame and writes back static
    /// values once a finite `play_count` completes.
    #[serde(default)]
    pub(crate) animation: Option<FogAnimation>,
}

impl FogVolumeComponent {
    /// Script-facing field list, paired with the camelCase keys the FFI
    /// boundary uses. Centralized so adding a runtime-tweakable field updates
    /// every read/write site (`into_js`, `into_lua`, `world.query` JSON shape)
    /// in one place. The wire-shared struct keeps snake_case Rust idents; the
    /// camelCase mapping lives only here.
    pub(crate) fn camel_fields(&self) -> [(&'static str, f32); 7] {
        [
            ("density", self.density),
            ("glow", self.glow),
            ("edgeSoftness", self.edge_softness),
            ("falloff", self.falloff),
            ("saturation", self.saturation),
            ("minBrightness", self.min_brightness),
            ("lightRange", self.light_range),
        ]
    }
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

impl Component for FogVolumeComponent {
    const KIND: ComponentKind = ComponentKind::FogVolume;

    fn from_value(value: &ComponentValue) -> Option<&Self> {
        match value {
            ComponentValue::FogVolume(f) => Some(f),
            _ => None,
        }
    }

    fn into_value(self) -> ComponentValue {
        ComponentValue::FogVolume(self)
    }
}

impl Component for PlayerMovementComponent {
    const KIND: ComponentKind = ComponentKind::PlayerMovement;

    fn from_value(value: &ComponentValue) -> Option<&Self> {
        match value {
            ComponentValue::PlayerMovement(p) => Some(p.as_ref()),
            _ => None,
        }
    }

    fn into_value(self) -> ComponentValue {
        ComponentValue::PlayerMovement(Box::new(self))
    }
}

impl Component for WeaponComponent {
    const KIND: ComponentKind = ComponentKind::Weapon;

    fn from_value(value: &ComponentValue) -> Option<&Self> {
        match value {
            ComponentValue::Weapon(w) => Some(w),
            _ => None,
        }
    }

    fn into_value(self) -> ComponentValue {
        ComponentValue::Weapon(self)
    }
}

impl Component for DescriptorProvenance {
    const KIND: ComponentKind = ComponentKind::DescriptorProvenance;

    fn from_value(value: &ComponentValue) -> Option<&Self> {
        match value {
            ComponentValue::DescriptorProvenance(p) => Some(p),
            _ => None,
        }
    }

    fn into_value(self) -> ComponentValue {
        ComponentValue::DescriptorProvenance(self)
    }
}

impl Component for MeshComponent {
    const KIND: ComponentKind = ComponentKind::Mesh;

    fn from_value(value: &ComponentValue) -> Option<&Self> {
        match value {
            ComponentValue::Mesh(m) => Some(m),
            _ => None,
        }
    }

    fn into_value(self) -> ComponentValue {
        ComponentValue::Mesh(self)
    }
}

impl Component for HealthComponent {
    const KIND: ComponentKind = ComponentKind::Health;

    fn from_value(value: &ComponentValue) -> Option<&Self> {
        match value {
            ComponentValue::Health(h) => Some(h),
            _ => None,
        }
    }

    fn into_value(self) -> ComponentValue {
        ComponentValue::Health(self)
    }
}

impl Component for AgentComponent {
    const KIND: ComponentKind = ComponentKind::Agent;

    fn from_value(value: &ComponentValue) -> Option<&Self> {
        match value {
            ComponentValue::Agent(a) => Some(a),
            _ => None,
        }
    }

    fn into_value(self) -> ComponentValue {
        ComponentValue::Agent(self)
    }
}

impl Component for BrainComponent {
    const KIND: ComponentKind = ComponentKind::Brain;

    fn from_value(value: &ComponentValue) -> Option<&Self> {
        match value {
            ComponentValue::Brain(b) => Some(b),
            _ => None,
        }
    }

    fn into_value(self) -> ComponentValue {
        ComponentValue::Brain(self)
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
    /// Per-entity previous-tick transform, indexed by slot index. Holds the
    /// `Transform` as of the *start* of the current fixed tick — the snapshot
    /// the renderer interpolates from toward the live `Transform` column.
    /// Kept parallel to `components` (not folded into a component) so the
    /// snapshot pass and the render accessor read it without disturbing the
    /// scripting component surface. `None` mirrors a dead/uninitialized slot.
    /// See: context/lib/entity_model.md §5.
    previous_transforms: Vec<Option<Transform>>,
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
    /// Engine-internal selection for the one local player pawn driven by the
    /// Phase 0 sim command. Not script-visible and not a tag/KVP, so it cannot
    /// affect world queries or authored entity properties.
    local_player_pawn: Option<EntityId>,
}

impl EntityRegistry {
    pub(crate) fn new() -> Self {
        Self {
            slots: Vec::new(),
            free_list: Vec::new(),
            components: std::array::from_fn(|_| Vec::new()),
            previous_transforms: Vec::new(),
            tags: Vec::new(),
            kvp_table: HashMap::new(),
            local_player_pawn: None,
        }
    }

    /// Mark the pawn driven by the single local Phase 0 sim command. Systems
    /// that need "the player" prefer this marker, then fall back to the legacy
    /// first-`PlayerMovement` lookup for older fixtures and maps.
    pub(crate) fn mark_local_player_pawn(&mut self, id: EntityId) -> Result<(), RegistryError> {
        let _ = self.validate(id)?;
        self.local_player_pawn = Some(id);
        Ok(())
    }

    /// Return the marked local player pawn when it is still live.
    pub(crate) fn local_player_pawn(&self) -> Option<EntityId> {
        self.local_player_pawn
            .filter(|id| self.validate(*id).is_ok())
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
                // idx as u16 is valid: slots.len() is bounded to u16::MAX by spawn.
                let id = EntityId::new(idx as u16, slot.generation);
                Some((id, cell))
            })
    }

    /// Returns `None` when all 65,536 entity slots are exhausted (free list
    /// empty and slot vector at `u16::MAX`). Callers that must not panic
    /// (e.g. script primitives crossing the FFI boundary) should prefer this
    /// over [`EntityRegistry::spawn`]. Tags are attached at slot-mark time;
    /// pass `&[]` to spawn untagged.
    ///
    /// KVPs from a source `MapEntity` must be written separately via
    /// `set_map_kvps` after spawn — `try_spawn` does not accept them.
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
            self.previous_transforms.push(None);
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
        // Seed previous == current at construction so an entity spawned
        // mid-tick (after the tick's snapshot pass already ran) never pops:
        // its first interpolated transform blends from its own spawn pose.
        self.previous_transforms[index as usize] = Some(transform);
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
        self.previous_transforms[index] = None;
        self.tags[index].clear();
        self.kvp_table.remove(&id);
        if self.local_player_pawn == Some(id) {
            self.local_player_pawn = None;
        }
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

    /// Despawn every live entity while preserving slot-generation semantics.
    /// Level unload uses this instead of replacing the registry wholesale so
    /// stale `EntityId`s from the old level cannot become valid in a later load.
    pub(crate) fn clear_for_level_unload(&mut self) {
        let live_ids: Vec<EntityId> = self
            .slots
            .iter()
            .enumerate()
            .filter_map(|(idx, slot)| {
                if slot.live && !slot.retired {
                    Some(EntityId::new(idx as u16, slot.generation))
                } else {
                    None
                }
            })
            .collect();
        for id in live_ids {
            let _ = self.despawn(id);
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

    pub(crate) fn set_component_value(
        &mut self,
        id: EntityId,
        value: ComponentValue,
    ) -> Result<(), RegistryError> {
        let index = self.validate(id)?;
        let kind = value.kind();
        self.components[kind as usize][index] = Some(value);
        Ok(())
    }

    pub(crate) fn has_component_kind(
        &self,
        id: EntityId,
        kind: ComponentKind,
    ) -> Result<bool, RegistryError> {
        let index = self.validate(id)?;
        Ok(self.components[kind as usize][index].is_some())
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

    pub(crate) fn remove_component_kind(
        &mut self,
        id: EntityId,
        kind: ComponentKind,
    ) -> Result<(), RegistryError> {
        let index = self.validate(id)?;
        let cell = &mut self.components[kind as usize][index];
        if cell.is_none() {
            return Err(RegistryError::ComponentNotFound { id, kind });
        }
        *cell = None;
        Ok(())
    }

    /// Order-0 fixed-tick step: copy every live entity's current `Transform`
    /// into its previous-tick slot. Runs once at the *start* of each tick,
    /// before any movement/behavior system mutates transforms, so the renderer
    /// can interpolate between the start-of-tick pose and the post-tick pose.
    ///
    /// Entities spawned later in the same tick are unaffected: `spawn` already
    /// seeds previous == current, so they interpolate against themselves until
    /// the next tick's snapshot runs (no pop on spawn). See:
    /// context/lib/entity_model.md §5.
    pub(crate) fn snapshot_transforms(&mut self) {
        let transform_column = &self.components[ComponentKind::Transform as usize];
        for (index, slot) in self.slots.iter().enumerate() {
            if !slot.live || slot.retired {
                continue;
            }
            // A live entity always carries a Transform (seeded at spawn), but
            // guard rather than unwrap: a future code path could remove it, and
            // a stale previous transform is less surprising than a panic.
            if let Some(ComponentValue::Transform(current)) =
                transform_column.get(index).and_then(|c| c.as_ref())
            {
                self.previous_transforms[index] = Some(*current);
            }
        }
    }

    /// Remote-presentation write (M15 Phase 2 interpolation): set the entity's
    /// visible transform to a network-interpolated pose and stamp its previous-tick
    /// slot to the **same** pose, so the render-stage
    /// [`interpolated_transform`](Self::interpolated_transform) blend is a no-op at
    /// any alpha and the buffer's already-resolved pose is shown verbatim.
    ///
    /// Time-base reasoning (why previous == current / alpha-agnostic): remote
    /// entities are not moved by a sim system. The interpolation buffer
    /// (`RemoteInterpolationBuffer`) already resolves the final pose `P(N)` for this
    /// render frame at the correct *server-time* target
    /// (`estimated_server_tick - interpolation_delay`). That resolution is
    /// independent of — and must not be re-blended by — the render accessor's
    /// `alpha`, which is the unrelated *sim sub-tick* fraction
    /// (`accumulator / tick_duration`, see `crate::frame_timing`). Blending `P(N)`
    /// toward the last-presented `P(N-1)` by that sub-tick alpha would re-sample the
    /// buffer's smooth trajectory at a frame-varying offset, injecting sub-frame
    /// jitter. Setting `previous == current` makes
    /// `lerp(previous, current, alpha) == current` for every alpha, so the remote
    /// pose is *alpha-invariant*: rendered exactly as the buffer produced it.
    ///
    /// No pop: continuity comes from the buffer itself, which produces a smooth
    /// trajectory across frames. Sampling it once per render frame (previous ==
    /// current each frame) reproduces that motion faithfully; it does not rely on
    /// the render-stage blend carrying the prior frame's pose. No other consumer
    /// reads a remote entity's previous transform (no motion blur / trails), so
    /// collapsing it to `current` is safe.
    ///
    /// See: context/lib/entity_model.md §5 · context/lib/networking.md
    pub(crate) fn set_remote_presentation_transform(
        &mut self,
        id: EntityId,
        pose: Transform,
    ) -> Result<(), RegistryError> {
        let index = self.validate(id)?;
        self.components[ComponentKind::Transform as usize][index] =
            Some(ComponentValue::Transform(pose));
        // previous == current: the buffer already resolved the final frame pose, so
        // the render blend must reproduce it at any alpha (see doc comment above).
        self.previous_transforms[index] = Some(pose);
        Ok(())
    }

    /// Render-stage accessor: the entity's visual transform blended between its
    /// previous-tick and current transforms by `alpha` (0 = previous, 1 =
    /// current). Position and scale are component-lerped; rotation is
    /// shortest-path slerped (glam's `Quat::slerp` negates one endpoint when
    /// the dot is negative, so it never takes the long arc).
    ///
    /// `alpha` is supplied by the caller — the render-frame collector passes
    /// the same frame alpha the player camera reads from `crate::frame_timing`
    /// (`FrameTickResult::alpha`). Returns `GenerationMismatch`/`EntityNotFound` for
    /// stale or unknown ids, and `ComponentNotFound` if the entity carries no
    /// `Transform`. See: context/lib/entity_model.md §5.
    pub(crate) fn interpolated_transform(
        &self,
        id: EntityId,
        alpha: f32,
    ) -> Result<Transform, RegistryError> {
        let index = self.validate(id)?;
        let current = self.get_component::<Transform>(id)?;
        // Previous is seeded at spawn and refreshed by the snapshot pass, so a
        // live entity with a Transform always has one. Fall back to current if
        // it is somehow absent rather than failing the render read.
        let previous = self.previous_transforms[index].as_ref().unwrap_or(current);
        Ok(Transform {
            position: previous.position.lerp(current.position, alpha),
            rotation: previous.rotation.slerp(current.rotation, alpha),
            scale: previous.scale.lerp(current.scale, alpha),
        })
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
            size_over_lifetime: [0.3, 1.0, 0.5].into(),
            opacity_over_lifetime: [0.0, 0.8, 0.0].into(),
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
            size_curve: [0.2, 1.0, 0.5].into(),
            opacity_curve: [0.0, 1.0, 0.0].into(),
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

    // -- Per-entity transform interpolation (entity_model.md §5) --

    // Approximate-equality epsilon for interpolated float comparisons. Slerp
    // and lerp accumulate small rounding error; 1e-4 is loose enough to absorb
    // it while still catching a wrong-half-path or off-by-alpha result.
    const INTERP_EPSILON: f32 = 1e-4;

    fn vec3_approx_eq(a: Vec3, b: Vec3) -> bool {
        (a - b).length() < INTERP_EPSILON
    }

    #[test]
    fn spawn_seeds_previous_transform_equal_to_current() {
        // No-pop-on-spawn invariant: before any snapshot pass runs, a fresh
        // entity must interpolate against itself, so any alpha returns its
        // spawn pose unchanged.
        let mut reg = EntityRegistry::new();
        let spawn = sample_transform();
        let id = reg.spawn(spawn);

        for alpha in [0.0, 0.5, 1.0] {
            let interp = reg.interpolated_transform(id, alpha).unwrap();
            assert!(
                vec3_approx_eq(interp.position, spawn.position),
                "alpha={alpha}: position should equal spawn pose, got {:?}",
                interp.position
            );
            assert!(
                vec3_approx_eq(interp.scale, spawn.scale),
                "alpha={alpha}: scale should equal spawn pose"
            );
            // angle_between is 0 when the rotations match (slerp of equal quats).
            assert!(
                interp.rotation.angle_between(spawn.rotation) < INTERP_EPSILON,
                "alpha={alpha}: rotation should equal spawn pose"
            );
        }
    }

    #[test]
    fn interpolated_transform_returns_midpoint_at_alpha_half() {
        // Snapshot captures the start pose, then the live transform is moved.
        // At alpha 0.5 the accessor must return the component-wise midpoint of
        // position and scale.
        let mut reg = EntityRegistry::new();
        let start = Transform {
            position: Vec3::new(0.0, 0.0, 0.0),
            rotation: Quat::IDENTITY,
            scale: Vec3::splat(1.0),
        };
        let id = reg.spawn(start);

        // Order-0 snapshot freezes `start` as previous, then a movement system
        // would write the new current transform.
        reg.snapshot_transforms();
        let end = Transform {
            position: Vec3::new(10.0, 20.0, 30.0),
            rotation: Quat::IDENTITY,
            scale: Vec3::splat(3.0),
        };
        reg.set_component(id, end).unwrap();

        let interp = reg.interpolated_transform(id, 0.5).unwrap();
        assert!(
            vec3_approx_eq(interp.position, Vec3::new(5.0, 10.0, 15.0)),
            "position midpoint, got {:?}",
            interp.position
        );
        assert!(
            vec3_approx_eq(interp.scale, Vec3::splat(2.0)),
            "scale midpoint, got {:?}",
            interp.scale
        );
    }

    #[test]
    fn interpolated_transform_returns_endpoints_at_alpha_zero_and_one() {
        let mut reg = EntityRegistry::new();
        let start = Transform {
            position: Vec3::new(1.0, 1.0, 1.0),
            rotation: Quat::from_rotation_y(0.2),
            scale: Vec3::splat(1.0),
        };
        let id = reg.spawn(start);
        reg.snapshot_transforms();
        let end = Transform {
            position: Vec3::new(5.0, 5.0, 5.0),
            rotation: Quat::from_rotation_y(1.2),
            scale: Vec3::splat(4.0),
        };
        reg.set_component(id, end).unwrap();

        let at_zero = reg.interpolated_transform(id, 0.0).unwrap();
        assert!(
            vec3_approx_eq(at_zero.position, start.position),
            "alpha=0 yields previous-tick (start) position"
        );

        let at_one = reg.interpolated_transform(id, 1.0).unwrap();
        assert!(
            vec3_approx_eq(at_one.position, end.position),
            "alpha=1 yields current (end) position"
        );
    }

    #[test]
    fn interpolated_transform_rotation_takes_shortest_path() {
        // Shortest-path slerp: previous at -170° and current at +170° about Y
        // are 340° apart the long way but only 20° apart the short way. The
        // halfway blend must land near ±180° (the short arc's midpoint), not
        // near 0° (the long arc's midpoint).
        let mut reg = EntityRegistry::new();
        let prev_angle = (-170.0f32).to_radians();
        let curr_angle = 170.0f32.to_radians();

        let start = Transform {
            position: Vec3::ZERO,
            rotation: Quat::from_rotation_y(prev_angle),
            scale: Vec3::ONE,
        };
        let id = reg.spawn(start);
        reg.snapshot_transforms();
        reg.set_component(
            id,
            Transform {
                position: Vec3::ZERO,
                rotation: Quat::from_rotation_y(curr_angle),
                scale: Vec3::ONE,
            },
        )
        .unwrap();

        let interp = reg.interpolated_transform(id, 0.5).unwrap();
        // The short-arc midpoint is rotation by 180° about Y. Compare against
        // that target; the long-arc midpoint (identity) would be ~180° away and
        // fail this assertion.
        let short_midpoint = Quat::from_rotation_y(180.0f32.to_radians());
        assert!(
            interp.rotation.angle_between(short_midpoint) < 1e-3,
            "slerp should follow the 20-degree short arc, not the 340-degree long arc"
        );
    }

    #[test]
    fn snapshot_only_freezes_entities_live_at_snapshot_time() {
        // An entity spawned after the snapshot pass must still read previous ==
        // current (seeded at spawn), so it renders at its spawn pose with no
        // pop, independent of when the snapshot ran.
        let mut reg = EntityRegistry::new();
        let early = reg.spawn(Transform {
            position: Vec3::new(0.0, 0.0, 0.0),
            ..Transform::default()
        });

        // Snapshot freezes `early`'s start pose, then `early` moves.
        reg.snapshot_transforms();
        reg.set_component(
            early,
            Transform {
                position: Vec3::new(8.0, 0.0, 0.0),
                ..Transform::default()
            },
        )
        .unwrap();

        // A mid-tick spawn lands AFTER the snapshot already ran.
        let late_pose = Transform {
            position: Vec3::new(100.0, 0.0, 0.0),
            ..Transform::default()
        };
        let late = reg.spawn(late_pose);

        // `early` interpolates across its moved range.
        let early_interp = reg.interpolated_transform(early, 0.5).unwrap();
        assert!(
            vec3_approx_eq(early_interp.position, Vec3::new(4.0, 0.0, 0.0)),
            "pre-snapshot entity interpolates across its tick movement"
        );

        // `late` does not pop: any alpha returns its spawn pose.
        let late_interp = reg.interpolated_transform(late, 0.5).unwrap();
        assert!(
            vec3_approx_eq(late_interp.position, late_pose.position),
            "post-snapshot spawn renders at spawn pose (no pop)"
        );
    }

    #[test]
    fn interpolated_transform_rejects_stale_id() {
        let mut reg = EntityRegistry::new();
        let id = reg.spawn(Transform::default());
        reg.despawn(id).unwrap();
        assert_eq!(
            reg.interpolated_transform(id, 0.5),
            Err(RegistryError::GenerationMismatch(id))
        );
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

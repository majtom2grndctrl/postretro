// Entity/component registry: the scripting surface that scripts address.
// See: context/lib/scripting.md

use std::collections::HashMap;
use std::fmt;

use glam::{Quat, Vec3};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::components::agent::AgentComponent;
use crate::components::ammo_reserve::AmmoReserve;
use crate::components::billboard_emitter::BillboardEmitterComponent;
use crate::components::brain::BrainComponent;
use crate::components::deferred_effect::DeferredEffectComponent;
use crate::components::entity_state::EntityStateComponent;
use crate::components::fog_volume::FogAnimation;
use crate::components::health::{HealthComponent, ImpactDispatch};
use crate::components::inventory::Inventory;
use crate::components::kinematic_mover::KinematicMoverComponent;
use crate::components::light::LightComponent;
use crate::components::mesh::MeshComponent;
use crate::components::particle::ParticleState;
use crate::components::player_movement::PlayerMovementComponent;
use crate::components::projectile::{ProjectileComponent, ProjectilePresentationAge};
use crate::components::spawner::SpawnerComponent;
use crate::components::sprite_visual::SpriteVisual;
use crate::components::touchable::TouchableComponent;
use crate::components::trigger_volume::TriggerVolumeComponent;
use crate::components::weapon::WeaponComponent;
use crate::provenance::DescriptorProvenance;
use postretro_foundation::{MAX_PENDING_PRESENTATION_SPAWNS, PresentationSpawn, Seat};

/// Packed entity identifier: `index: 16 | generation: 16`.
///
/// 16/16 gives 65,536 live slots and 65,536 generations per slot — comfortably
/// above the design ceiling for a single level. When a slot's generation is
/// bumped past `u16::MAX` on despawn, the slot is **permanently retired**
/// (removed from the free list and never re-allocated); see [`EntityRegistry::despawn`].
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EntityId(u32);

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
    pub fn to_raw(self) -> u32 {
        self.0
    }

    /// Inverse of [`EntityId::to_raw`]. The binding layer reconstructs an
    /// `EntityId` from a script-supplied number; validation happens when the
    /// registry dereferences it.
    pub fn from_raw(raw: u32) -> Self {
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
/// `PlayerMovement`, `Weapon`, and `AmmoReserve` are engine-owned runtime
/// components, so scripts address them through higher-level descriptors and
/// systems.
///
/// `#[repr(u16)]` makes the discriminant a zero-cost index into the
/// component-storage vector array. Not `#[non_exhaustive]`: the enum is
/// `pub`, and `non_exhaustive` is a no-op on non-`pub` items.
#[repr(u16)]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub enum ComponentKind {
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
    /// Engine-owned AI brain: the retained behavior graph, its current state
    /// index, and per-instance timers (engine-internal, like
    /// `PlayerMovement`/`Agent` — never reachable through `worldQuery`). See
    /// `components::brain`.
    Brain = 12,
    /// Deterministic linear mover. Scripts query mover handles through
    /// `world.query`; raw phase remains engine-owned and non-attachable.
    KinematicMover = 13,
    /// Engine-owned trigger configuration and mutable arming state.
    TriggerVolume = 14,
    /// Pawn-owned ammunition balances pooled by authored ammo type.
    AmmoReserve = 15,
    /// Map-authored, fixed-tick enemy-spawn configuration. The resolved
    /// descriptor remains in the session spawn context, not this serde value.
    Spawner = 16,
    /// Per-instance modder-owned numeric fields. Every entity receives an
    /// empty component at spawn; fields emerge on first write.
    EntityState = 17,
    /// Per-entity deferred impact-effect queue and terminal inert flag.
    /// Every entity receives an empty component at spawn so effects do not
    /// depend on an AI brain being present.
    DeferredEffect = 18,
    /// Pawn-owned ordered wieldable instances and in-flight switch target.
    Inventory = 19,
    /// Host-local interaction tuning for a world touchable entity.
    Touchable = 20,
    /// Engine-owned direct-impact projectile flight state.
    Projectile = 21,
}

impl ComponentKind {
    /// Count of variants, derived from an exhaustive const array.
    /// `std::mem::variant_count` is not yet const-stable on this toolchain,
    /// so we list every variant once; the compiler enforces exhaustiveness in
    /// match arms that touch `ComponentKind` elsewhere.
    pub const COUNT: usize = {
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
            ComponentKind::KinematicMover,
            ComponentKind::TriggerVolume,
            ComponentKind::AmmoReserve,
            ComponentKind::Spawner,
            ComponentKind::EntityState,
            ComponentKind::DeferredEffect,
            ComponentKind::Inventory,
            ComponentKind::Touchable,
            ComponentKind::Projectile,
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
pub struct Transform {
    pub position: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
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
// ComponentValue is the registry's owned, closed vocabulary. Boxing the largest
// variants would add indirection to every dynamic component exchange solely to
// satisfy a size heuristic, without reducing the stored component columns.
#[allow(clippy::large_enum_variant)]
pub enum ComponentValue {
    Transform(Transform),
    Light(LightComponent),
    BillboardEmitter(BillboardEmitterComponent),
    ParticleState(ParticleState),
    SpriteVisual(SpriteVisual),
    FogVolume(FogVolumeComponent),
    // Boxed: `PlayerMovementComponent` is large (~400+ bytes — the dash
    // `DashPrograms` bound trees grow it further), large enough that an unboxed
    // variant would inflate every `ComponentValue` to its size
    // (clippy::large_enum_variant). Boxing keeps the enum compact; the player
    // pawn is a singleton, so the extra indirection is paid once.
    PlayerMovement(Box<PlayerMovementComponent>),
    Weapon(WeaponComponent),
    DescriptorProvenance(DescriptorProvenance),
    // Boxed: `MeshComponent` carries the per-instance `PoseInputs` render
    // payload, whose fixed `[FootProbe; MAX_FEET]` foot-probe array makes it
    // large enough that an unboxed variant would inflate every `ComponentValue`
    // to its size (clippy::large_enum_variant). Boxing keeps the transport enum
    // compact.
    Mesh(Box<MeshComponent>),
    Health(HealthComponent),
    Agent(AgentComponent),
    Brain(BrainComponent),
    KinematicMover(KinematicMoverComponent),
    TriggerVolume(TriggerVolumeComponent),
    AmmoReserve(AmmoReserve),
    Spawner(SpawnerComponent),
    EntityState(EntityStateComponent),
    DeferredEffect(DeferredEffectComponent),
    Inventory(Inventory),
    Touchable(TouchableComponent),
    Projectile(ProjectileComponent),
}

impl ComponentValue {
    pub fn kind(&self) -> ComponentKind {
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
            ComponentValue::KinematicMover(_) => ComponentKind::KinematicMover,
            ComponentValue::TriggerVolume(_) => ComponentKind::TriggerVolume,
            ComponentValue::AmmoReserve(_) => ComponentKind::AmmoReserve,
            ComponentValue::Spawner(_) => ComponentKind::Spawner,
            ComponentValue::EntityState(_) => ComponentKind::EntityState,
            ComponentValue::DeferredEffect(_) => ComponentKind::DeferredEffect,
            ComponentValue::Inventory(_) => ComponentKind::Inventory,
            ComponentValue::Touchable(_) => ComponentKind::Touchable,
            ComponentValue::Projectile(_) => ComponentKind::Projectile,
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
pub struct FogVolumeComponent {
    pub density: f32,
    pub glow: f32,
    pub edge_softness: f32,
    pub falloff: f32,
    /// Scatter tint multiplier. `[1, 1, 1]` = no tint. Applied after saturation.
    #[serde(default = "default_fog_tint")]
    pub tint: [f32; 3],
    /// Scatter saturation: 0 = greyscale, 1 = natural, >1 = boosted.
    #[serde(default = "default_fog_saturation")]
    pub saturation: f32,
    #[serde(default = "default_fog_min_brightness")]
    pub min_brightness: f32,
    #[serde(default = "default_fog_light_range")]
    pub light_range: f32,
    /// Optional animation carrying any combination of density, saturation,
    /// min_brightness, and light_range curves. `None`
    /// holds static values. Installed by the `setFogAnimation` reaction
    /// primitive; the fog bridge evaluates per frame and writes back static
    /// values once a finite `play_count` completes.
    #[serde(default)]
    pub animation: Option<FogAnimation>,
}

impl FogVolumeComponent {
    /// Script-facing field list, paired with the camelCase keys the FFI
    /// boundary uses. Centralized so adding a runtime-tweakable field updates
    /// every read/write site (`into_js`, `into_lua`, `world.query` JSON shape)
    /// in one place. The wire-shared struct keeps snake_case Rust idents; the
    /// camelCase mapping lives only here.
    pub fn camel_fields(&self) -> [(&'static str, f32); 7] {
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
/// `pub` — sealed in practice by virtue of the crate-private scope.
pub trait Component: Sized {
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
            ComponentValue::Mesh(m) => Some(m.as_ref()),
            _ => None,
        }
    }

    fn into_value(self) -> ComponentValue {
        ComponentValue::Mesh(Box::new(self))
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

impl Component for KinematicMoverComponent {
    const KIND: ComponentKind = ComponentKind::KinematicMover;

    fn from_value(value: &ComponentValue) -> Option<&Self> {
        match value {
            ComponentValue::KinematicMover(m) => Some(m),
            _ => None,
        }
    }

    fn into_value(self) -> ComponentValue {
        ComponentValue::KinematicMover(self)
    }
}

impl Component for TriggerVolumeComponent {
    const KIND: ComponentKind = ComponentKind::TriggerVolume;
    fn from_value(value: &ComponentValue) -> Option<&Self> {
        match value {
            ComponentValue::TriggerVolume(trigger) => Some(trigger),
            _ => None,
        }
    }
    fn into_value(self) -> ComponentValue {
        ComponentValue::TriggerVolume(self)
    }
}

impl Component for AmmoReserve {
    const KIND: ComponentKind = ComponentKind::AmmoReserve;
    fn from_value(value: &ComponentValue) -> Option<&Self> {
        match value {
            ComponentValue::AmmoReserve(reserve) => Some(reserve),
            _ => None,
        }
    }
    fn into_value(self) -> ComponentValue {
        ComponentValue::AmmoReserve(self)
    }
}

impl Component for SpawnerComponent {
    const KIND: ComponentKind = ComponentKind::Spawner;

    fn from_value(value: &ComponentValue) -> Option<&Self> {
        match value {
            ComponentValue::Spawner(spawner) => Some(spawner),
            _ => None,
        }
    }

    fn into_value(self) -> ComponentValue {
        ComponentValue::Spawner(self)
    }
}

impl Component for EntityStateComponent {
    const KIND: ComponentKind = ComponentKind::EntityState;

    fn from_value(value: &ComponentValue) -> Option<&Self> {
        match value {
            ComponentValue::EntityState(state) => Some(state),
            _ => None,
        }
    }

    fn into_value(self) -> ComponentValue {
        ComponentValue::EntityState(self)
    }
}

impl Component for DeferredEffectComponent {
    const KIND: ComponentKind = ComponentKind::DeferredEffect;

    fn from_value(value: &ComponentValue) -> Option<&Self> {
        match value {
            ComponentValue::DeferredEffect(effects) => Some(effects),
            _ => None,
        }
    }

    fn into_value(self) -> ComponentValue {
        ComponentValue::DeferredEffect(self)
    }
}

impl Component for Inventory {
    const KIND: ComponentKind = ComponentKind::Inventory;

    fn from_value(value: &ComponentValue) -> Option<&Self> {
        match value {
            ComponentValue::Inventory(inventory) => Some(inventory),
            _ => None,
        }
    }

    fn into_value(self) -> ComponentValue {
        ComponentValue::Inventory(self)
    }
}

impl Component for TouchableComponent {
    const KIND: ComponentKind = ComponentKind::Touchable;

    fn from_value(value: &ComponentValue) -> Option<&Self> {
        match value {
            ComponentValue::Touchable(touchable) => Some(touchable),
            _ => None,
        }
    }

    fn into_value(self) -> ComponentValue {
        ComponentValue::Touchable(self)
    }
}

impl Component for ProjectileComponent {
    const KIND: ComponentKind = ComponentKind::Projectile;

    fn from_value(value: &ComponentValue) -> Option<&Self> {
        match value {
            ComponentValue::Projectile(projectile) => Some(projectile),
            _ => None,
        }
    }

    fn into_value(self) -> ComponentValue {
        ComponentValue::Projectile(self)
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RegistryError {
    #[error("entity {0} does not exist")]
    EntityNotFound(EntityId),
    #[error("entity {id} has no component of kind {kind:?}")]
    ComponentNotFound { id: EntityId, kind: ComponentKind },
    #[error("entity id {0} is stale (generation mismatch)")]
    GenerationMismatch(EntityId),
    #[error("entity {id} has no projectile presentation age")]
    ProjectilePresentationAgeNotFound { id: EntityId },
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

pub struct EntityRegistry {
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
    /// Presentation-only projectile timing, kept outside `ComponentValue` so it
    /// never grows the replicated component vocabulary. The remote materializer
    /// stamps it from the local shared-content clock and the billboard collector
    /// reads it while packing the visual-only projectile body.
    projectile_presentation_ages: Vec<Option<ProjectilePresentationAge>>,
    /// Changes only when Light-component membership changes, not when an existing
    /// light's fields mutate. Render-side enrollment uses it to avoid scanning the
    /// full light column on frames that cannot contain a new runtime light.
    light_membership_generation: u64,
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
    /// Reverse index of the session seat that owns each player pawn. Sparse —
    /// only pawns currently bound to a seat appear, so it is a map rather than
    /// a per-slot column, matching `kvp_table`. Kept parallel to `components`
    /// (not folded into a component) so owner-addressed layers — impact effects,
    /// impact policy, reaction dispatch — read it without disturbing the
    /// scripting component surface, and without adding a `ComponentKind`
    /// discriminant that the replication wire format would have to carry.
    ///
    /// The seat table above this registry owns the seat→pawn direction and is
    /// the sole writer here. An entry exists exactly while a seat is bound to a
    /// live pawn: `despawn` removes it, so a recycled slot never inherits one.
    /// See: context/lib/networking.md §Session-state ledger.
    pawn_seats: HashMap<EntityId, Seat>,
    /// Fires from the health chokepoint after each accepted damage call. The
    /// event payload belongs to the health component module, while the registry
    /// owns the single-threaded handoff to impact-policy consumers.
    impact_dispatches: Vec<ImpactDispatch>,
    /// App-side presentation producers enqueue transient spawn facts here. The
    /// registry is the sole bridge between fixed-tick producers and the
    /// frame-time presentation pool; it owns no presentation lifetime or GPU
    /// state.
    presentation_spawns: Vec<PresentationSpawn>,
    /// Sparse worklist for entities with non-empty deferred-effect queues.
    /// Ownership returns here after every tick so the allocation is reused.
    active_deferred_effects: Vec<EntityId>,
    /// Entity ids marked by terminal impact effects. The app drains this once
    /// per frame after presentation/reaction work; game logic never removes an
    /// entity inline through this channel.
    end_of_frame_removals: Vec<EntityId>,
    /// A test-only artificial slot ceiling. Production still has exactly the
    /// `u16::MAX` registry capacity promised by `try_spawn`.
    #[cfg(any(test, feature = "test-support"))]
    test_capacity_limit: Option<usize>,
}

impl EntityRegistry {
    pub fn new() -> Self {
        Self {
            slots: Vec::new(),
            free_list: Vec::new(),
            components: std::array::from_fn(|_| Vec::new()),
            previous_transforms: Vec::new(),
            projectile_presentation_ages: Vec::new(),
            light_membership_generation: 0,
            tags: Vec::new(),
            kvp_table: HashMap::new(),
            local_player_pawn: None,
            pawn_seats: HashMap::new(),
            impact_dispatches: Vec::new(),
            presentation_spawns: Vec::with_capacity(MAX_PENDING_PRESENTATION_SPAWNS),
            active_deferred_effects: Vec::new(),
            end_of_frame_removals: Vec::new(),
            #[cfg(any(test, feature = "test-support"))]
            test_capacity_limit: None,
        }
    }

    /// Artificially cap fresh slots for a focused exhaustion test. Reused
    /// free-list slots remain valid capacity, matching the real registry.
    #[cfg(any(test, feature = "test-support"))]
    pub fn set_test_capacity_limit(&mut self, limit: usize) {
        self.test_capacity_limit = Some(limit);
    }

    /// Mark the pawn driven by the single local Phase 0 sim command. Systems
    /// that need "the player" prefer this marker, then fall back to the legacy
    /// first-`PlayerMovement` lookup for older fixtures and maps.
    pub fn mark_local_player_pawn(&mut self, id: EntityId) -> Result<(), RegistryError> {
        let _ = self.validate(id)?;
        self.local_player_pawn = Some(id);
        Ok(())
    }

    /// Record `seat` as the owner of `pawn`, evicting whatever pawn that seat
    /// owned before. The relationship is one seat per pawn *and* one pawn per
    /// seat: a rebind that left the outgoing pawn behind would make two pawns
    /// resolve to the same owner. Callers reach this only through the seat
    /// table's single binding path, which keeps its own seat→pawn map in step.
    pub fn bind_pawn_seat(&mut self, pawn: EntityId, seat: Seat) {
        self.pawn_seats.retain(|_, owner| *owner != seat);
        self.pawn_seats.insert(pawn, seat);
    }

    /// Drop `pawn`'s seat ownership. A pawn with no entry is not an error: the
    /// unbinding paths run for seats that may never have carried a pawn.
    pub fn clear_pawn_seat(&mut self, pawn: EntityId) {
        self.pawn_seats.remove(&pawn);
    }

    /// Resolve the session seat that owns `pawn`, if any.
    ///
    /// This is the pawn-to-owner lookup for layers that hold a registry and an
    /// entity but not the host's seat table. Stale ids resolve to `None`: the
    /// generation is part of the key, and `despawn` removes the entry.
    #[must_use]
    pub fn seat_for_pawn(&self, pawn: EntityId) -> Option<Seat> {
        self.pawn_seats.get(&pawn).copied()
    }

    /// Return the marked local player pawn when it is still live.
    pub fn local_player_pawn(&self) -> Option<EntityId> {
        self.local_player_pawn
            .filter(|id| self.validate(*id).is_ok())
    }

    /// Resolve the pawn driven by local movement input. A live marked pawn wins when
    /// it carries `PlayerMovement`; otherwise older maps and fixtures fall back to the
    /// first movement pawn in registry order. This selects identity only. Callers apply
    /// their own Health, Transform, camera, or presentation requirements afterward.
    pub fn local_player_movement_pawn(&self) -> Option<EntityId> {
        if let Some(id) = self.local_player_pawn()
            && self.has_component_kind(id, ComponentKind::PlayerMovement) == Ok(true)
        {
            return Some(id);
        }

        self.iter_with_kind(ComponentKind::PlayerMovement)
            .next()
            .map(|(id, _)| id)
    }

    /// Attach the per-placement KVP bag (authored on the source `.map` entity)
    /// to a spawned entity. Called by built-in classname handlers immediately
    /// after spawn so `getEntityProperty` works uniformly regardless of which
    /// handler ran. Empty bags are stored as an empty map (still creates an
    /// entry); pass through unchanged for readability.
    pub fn set_map_kvps(
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
    pub fn get_map_kvp(&self, id: EntityId, key: &str) -> Result<Option<String>, RegistryError> {
        let _ = self.validate(id)?;
        Ok(self.kvp_table.get(&id).and_then(|m| m.get(key).cloned()))
    }

    /// Attach (or overwrite) the tag list on an entity. An empty vec clears
    /// all tags. `world.query` checks membership: an entity matches filter
    /// tag `"t"` when any of its tags equals `"t"`.
    pub fn set_tags(&mut self, id: EntityId, tags: Vec<String>) -> Result<(), RegistryError> {
        let index = self.validate(id)?;
        self.tags[index] = tags;
        Ok(())
    }

    pub fn get_tags(&self, id: EntityId) -> Result<&[String], RegistryError> {
        let index = self.validate(id)?;
        Ok(&self.tags[index])
    }

    /// Iterate every live entity whose component column of `kind` is populated
    /// and whose tag list (if `tag_filter` is `Some`) contains the filter tag.
    /// When `tag_filter` is `None`, every entity with the component matches.
    ///
    /// Yields `(EntityId, &ComponentValue)` pairs in slot-index order. Used by
    /// the `world.query` primitive.
    pub fn query_by_component_and_tag<'a>(
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
    pub fn iter_with_kind(
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
    pub fn try_spawn(&mut self, transform: Transform, tags: &[String]) -> Option<EntityId> {
        #[cfg(any(test, feature = "test-support"))]
        if self.free_list.is_empty()
            && self
                .test_capacity_limit
                .is_some_and(|limit| self.slots.len() >= limit)
        {
            return None;
        }
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

    pub fn spawn(&mut self, transform: Transform) -> EntityId {
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
            self.projectile_presentation_ages.push(None);
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
        self.components[ComponentKind::EntityState as usize][index as usize] =
            Some(ComponentValue::EntityState(EntityStateComponent::default()));
        self.components[ComponentKind::DeferredEffect as usize][index as usize] = Some(
            ComponentValue::DeferredEffect(DeferredEffectComponent::default()),
        );
        // Seed previous == current at construction so an entity spawned
        // mid-tick (after the tick's snapshot pass already ran) never pops:
        // its first interpolated transform blends from its own spawn pose.
        self.previous_transforms[index as usize] = Some(transform);
        id
    }

    pub fn despawn(&mut self, id: EntityId) -> Result<(), RegistryError> {
        let index = id.index() as usize;
        let had_light = self.components[ComponentKind::Light as usize]
            .get(index)
            .is_some_and(Option::is_some);
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
        if had_light {
            self.light_membership_generation = self.light_membership_generation.wrapping_add(1);
        }
        self.previous_transforms[index] = None;
        self.projectile_presentation_ages[index] = None;
        self.tags[index].clear();
        self.kvp_table.remove(&id);
        // Seat ownership is a property of a live pawn. Clearing it here is also
        // what keeps `clear_for_level_unload` — which despawns every live
        // entity — from leaving an entry behind for a recycled slot index.
        self.pawn_seats.remove(&id);
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

    pub fn exists(&self, id: EntityId) -> bool {
        match self.slots.get(id.index() as usize) {
            Some(slot) => slot.live && !slot.retired && slot.generation == id.generation(),
            None => false,
        }
    }

    /// Store fixed-tick presentation timing for a descriptor-materialized projectile.
    /// This is intentionally not a serializable registry component: the baseline's
    /// authoritative server tick supplies the shared epoch without a wire field.
    pub fn set_projectile_presentation_age(
        &mut self,
        id: EntityId,
        age: ProjectilePresentationAge,
    ) -> Result<(), RegistryError> {
        let index = self.validate(id)?;
        self.projectile_presentation_ages[index] = Some(age);
        Ok(())
    }

    /// Read the fixed-tick timing state for a visual-only projectile.
    pub fn projectile_presentation_age(
        &self,
        id: EntityId,
    ) -> Result<&ProjectilePresentationAge, RegistryError> {
        let index = self.validate(id)?;
        self.projectile_presentation_ages[index]
            .as_ref()
            .ok_or(RegistryError::ProjectilePresentationAgeNotFound { id })
    }

    /// Publish one damage-chokepoint dispatch for the engine's impact-policy
    /// consumer. This is intentionally separate from component columns: it is
    /// ephemeral per-fire data, not persistent entity state.
    pub(crate) fn push_impact_dispatch(&mut self, dispatch: ImpactDispatch) {
        self.impact_dispatches.push(dispatch);
    }

    /// Drain every impact dispatch published since the previous consumer pass.
    pub fn take_impact_dispatches(&mut self) -> Vec<ImpactDispatch> {
        std::mem::take(&mut self.impact_dispatches)
    }

    /// Publish one transient presentation spawn for the app-side frame pool.
    /// Producers deliberately provide facts and an anchor, but no spawn time:
    /// the pool stamps its own frame-time clock when it drains this queue.
    pub fn push_presentation_spawn(&mut self, spawn: PresentationSpawn) {
        if self.presentation_spawns.len() < MAX_PENDING_PRESENTATION_SPAWNS {
            self.presentation_spawns.push(spawn);
        }
    }

    /// Drain every presentation spawn published since the previous render pass.
    pub fn take_presentation_spawns(&mut self) -> Vec<PresentationSpawn> {
        let mut spawns = Vec::with_capacity(self.presentation_spawns.len());
        spawns.append(&mut self.presentation_spawns);
        spawns
    }

    /// Move queued presentation spawns into caller-owned reusable storage.
    /// Both vectors retain their allocations after the caller drains `out`, so
    /// the fixed-tick/frame bridge is allocation-free once its burst high-water
    /// mark has been reached.
    pub fn drain_presentation_spawns_into(&mut self, out: &mut Vec<PresentationSpawn>) {
        out.clear();
        out.append(&mut self.presentation_spawns);
    }

    /// Retain selected queued presentation spawns in place. Host routing uses
    /// this to remove addressed remote cosmetics while leaving host-local work
    /// for the ordinary frame-pool drain without allocating a partition Vec.
    pub fn retain_presentation_spawns(&mut self, mut keep: impl FnMut(&PresentationSpawn) -> bool) {
        self.presentation_spawns.retain(|spawn| keep(spawn));
    }

    /// Discard presentation work whose world anchors belong to the retiring
    /// level. Lifecycle teardown calls this before another world can drain the
    /// fixed-tick intake.
    pub fn clear_presentation_spawns(&mut self) {
        self.presentation_spawns.clear();
    }

    /// Mutable access to the engine-managed deferred-effect storage.
    pub fn deferred_effect_mut(
        &mut self,
        id: EntityId,
    ) -> Result<&mut DeferredEffectComponent, RegistryError> {
        let index = self.validate(id)?;
        match self.components[ComponentKind::DeferredEffect as usize][index].as_mut() {
            Some(ComponentValue::DeferredEffect(effects)) => Ok(effects),
            _ => Err(RegistryError::ComponentNotFound {
                id,
                kind: ComponentKind::DeferredEffect,
            }),
        }
    }

    /// Mutable access to the per-instance modder state column.
    pub fn entity_state_mut(
        &mut self,
        id: EntityId,
    ) -> Result<&mut EntityStateComponent, RegistryError> {
        let index = self.validate(id)?;
        match self.components[ComponentKind::EntityState as usize][index].as_mut() {
            Some(ComponentValue::EntityState(state)) => Ok(state),
            _ => Err(RegistryError::ComponentNotFound {
                id,
                kind: ComponentKind::EntityState,
            }),
        }
    }

    /// Enroll an entity in deferred-effect ticking once. The worklist is
    /// sparse: entities whose engine-managed queue is empty never appear here.
    pub fn activate_deferred_effects(&mut self, id: EntityId) -> Result<(), RegistryError> {
        let _ = self.validate(id)?;
        if !self.active_deferred_effects.contains(&id) {
            self.active_deferred_effects.push(id);
        }
        Ok(())
    }

    /// Temporarily transfer the sparse worklist to the fixed-tick executor.
    pub fn take_active_deferred_effects(&mut self) -> Vec<EntityId> {
        std::mem::take(&mut self.active_deferred_effects)
    }

    /// Return the compacted worklist after a tick, retaining its allocation.
    pub fn replace_active_deferred_effects(&mut self, active: Vec<EntityId>) {
        debug_assert!(self.active_deferred_effects.is_empty());
        self.active_deferred_effects = active;
    }

    /// Stage a live entity for the dedicated frame-end removal pass. Repeated
    /// marks collapse to one removal attempt while the id remains live.
    pub fn mark_for_end_of_frame_removal(&mut self, id: EntityId) -> Result<(), RegistryError> {
        let _ = self.validate(id)?;
        if !self.end_of_frame_removals.contains(&id) {
            self.end_of_frame_removals.push(id);
        }
        Ok(())
    }

    /// Whether `id` is queued for the app-owned frame-end removal pass.
    ///
    /// Fixed-tick systems use this to avoid mutating an entity whose terminal
    /// deferred effect already won earlier in the tick.
    pub fn is_marked_for_end_of_frame_removal(&self, id: EntityId) -> Result<bool, RegistryError> {
        let _ = self.validate(id)?;
        Ok(self.end_of_frame_removals.contains(&id))
    }

    /// Drain ids staged for the app-owned frame-end removal pass.
    pub fn take_end_of_frame_removals(&mut self) -> Vec<EntityId> {
        std::mem::take(&mut self.end_of_frame_removals)
    }

    /// Despawn every live entity while preserving slot-generation semantics.
    /// Level unload uses this instead of replacing the registry wholesale so
    /// stale `EntityId`s from the old level cannot become valid in a later load.
    pub fn clear_for_level_unload(&mut self) {
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
        self.impact_dispatches.clear();
        self.presentation_spawns.clear();
        self.active_deferred_effects.clear();
        self.end_of_frame_removals.clear();
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

    pub fn get_component<T: Component>(&self, id: EntityId) -> Result<&T, RegistryError> {
        let index = self.validate(id)?;
        let column = &self.components[T::KIND as usize];
        let value = column
            .get(index)
            .and_then(|cell| cell.as_ref())
            .ok_or(RegistryError::ComponentNotFound { id, kind: T::KIND })?;
        T::from_value(value).ok_or(RegistryError::ComponentNotFound { id, kind: T::KIND })
    }

    /// Mutably access one erased component cell without cloning its payload.
    /// Fixed-tick systems use this when a component owns persistent vectors or
    /// strings whose immutable configuration should not be copied each tick.
    pub fn get_component_value_mut(
        &mut self,
        id: EntityId,
        kind: ComponentKind,
    ) -> Result<&mut ComponentValue, RegistryError> {
        let index = self.validate(id)?;
        self.components[kind as usize][index]
            .as_mut()
            .ok_or(RegistryError::ComponentNotFound { id, kind })
    }

    pub fn set_component<T: Component>(
        &mut self,
        id: EntityId,
        value: T,
    ) -> Result<(), RegistryError> {
        let index = self.validate(id)?;
        let adds_light =
            T::KIND == ComponentKind::Light && self.components[T::KIND as usize][index].is_none();
        self.components[T::KIND as usize][index] = Some(value.into_value());
        if adds_light {
            self.light_membership_generation = self.light_membership_generation.wrapping_add(1);
        }
        Ok(())
    }

    pub fn set_component_value(
        &mut self,
        id: EntityId,
        value: ComponentValue,
    ) -> Result<(), RegistryError> {
        let index = self.validate(id)?;
        let kind = value.kind();
        let adds_light =
            kind == ComponentKind::Light && self.components[kind as usize][index].is_none();
        self.components[kind as usize][index] = Some(value);
        if adds_light {
            self.light_membership_generation = self.light_membership_generation.wrapping_add(1);
        }
        Ok(())
    }

    /// Monotonic membership stamp for the Light component column.
    pub fn light_membership_generation(&self) -> u64 {
        self.light_membership_generation
    }

    pub fn has_component_kind(
        &self,
        id: EntityId,
        kind: ComponentKind,
    ) -> Result<bool, RegistryError> {
        let index = self.validate(id)?;
        Ok(self.components[kind as usize][index].is_some())
    }

    pub fn remove_component<T: Component>(&mut self, id: EntityId) -> Result<(), RegistryError> {
        let index = self.validate(id)?;
        let cell = &mut self.components[T::KIND as usize][index];
        if cell.is_none() {
            return Err(RegistryError::ComponentNotFound { id, kind: T::KIND });
        }
        *cell = None;
        if T::KIND == ComponentKind::Light {
            self.light_membership_generation = self.light_membership_generation.wrapping_add(1);
        }
        Ok(())
    }

    pub fn remove_component_kind(
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
        if kind == ComponentKind::Light {
            self.light_membership_generation = self.light_membership_generation.wrapping_add(1);
        }
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
    pub fn snapshot_transforms(&mut self) {
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

    /// Single-entity counterpart of [`snapshot_transforms`](Self::snapshot_transforms):
    /// copy one entity's current `Transform` into its previous-tick slot. Used by the
    /// connected-client prediction path (M15 Phase 3), which advances ONLY the local
    /// pawn each fixed tick and so cannot afford the registry-wide snapshot
    /// `simulate_tick` runs at stage 0 (that path also reruns AI/weapons/death — the
    /// connected client skips it entirely). Calling this per predicted tick, before
    /// writing the new predicted pose, keeps the local pawn's previous→current pair
    /// coherent so the render-stage [`interpolated_transform`](Self::interpolated_transform)
    /// blend (and any prev/current-derived velocity) advances smoothly rather than
    /// lerping live-current against an ever-staler frozen-previous. A no-op for a stale
    /// id or an entity without a `Transform`.
    pub fn snapshot_transform(&mut self, id: EntityId) {
        let Ok(index) = self.validate(id) else {
            return;
        };
        if let Some(ComponentValue::Transform(current)) = self.components
            [ComponentKind::Transform as usize]
            .get(index)
            .and_then(|c| c.as_ref())
        {
            self.previous_transforms[index] = Some(*current);
        }
    }

    /// Presentation-pose write: set the entity's visible transform to `pose` and
    /// stamp its previous-tick slot to the **same** pose, so the render-stage
    /// [`interpolated_transform`](Self::interpolated_transform) blend is a no-op at
    /// any alpha and `pose` is shown verbatim. Callers share one shape:
    ///
    /// - **Remote interpolation (M15 Phase 2):** the interpolation buffer already
    ///   resolved the final pose for this render frame at the correct server-time
    ///   target, so the render `alpha` (an unrelated sim sub-tick fraction) must not
    ///   re-blend it. Setting `previous == current` makes
    ///   `lerp(previous, current, alpha) == current` for every alpha — the remote
    ///   pose is alpha-invariant, rendered exactly as the buffer produced it.
    /// - **Local-pawn reconcile teleport:** a teleport-class
    ///   correction snaps the predicted pawn to the authoritative pose and must
    ///   leave no prev→current arc for the render blend to interpolate across — that
    ///   arc would smear the teleport into a visible slide. Stamping
    ///   `previous == current` collapses it, so the snapped pose renders cleanly the
    ///   frame the teleport lands.
    /// - **Discrete visibility transitions:** when gameplay restores presentation
    ///   after relocating an entity, its first visible frame must start at the new
    ///   pose rather than blending from hidden, stale transform history.
    ///
    /// Time-base reasoning (why previous == current / alpha-agnostic): the render
    /// accessor's `alpha` is the *sim sub-tick* fraction (`accumulator /
    /// tick_duration`, see `crate::frame_timing`), unrelated to either the remote
    /// buffer's server-time target or a one-shot discrete teleport. Blending toward the
    /// prior frame's pose by that sub-tick alpha would re-sample at a frame-varying
    /// offset (injecting jitter) or smear a teleport. No pop: no consumer reads
    /// these entities' previous transform for motion blur / trails, so collapsing it
    /// to `current` is safe.
    ///
    /// See: context/lib/entity_model.md §5 · context/lib/networking.md
    pub fn set_presentation_transform(
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
    pub fn interpolated_transform(
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

impl Default for EntityRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use postretro_foundation::{PresentationFade, PresentationMotion};

    use super::*;
    use crate::components::billboard_emitter::{BillboardEmitterComponent, SpinAnimation};
    use crate::components::particle::ParticleState;
    use crate::components::sprite_visual::SpriteVisual;
    use crate::data_descriptors::{
        AirParams, CapsuleParams, FallParams, GroundParams, PlayerMovementDescriptor, SpeedParams,
    };

    fn sample_transform() -> Transform {
        Transform {
            position: Vec3::new(1.0, 2.0, 3.0),
            rotation: Quat::from_rotation_y(0.5),
            scale: Vec3::splat(2.0),
        }
    }

    fn test_movement_component() -> PlayerMovementComponent {
        PlayerMovementComponent::from_descriptor(&PlayerMovementDescriptor {
            capsule: CapsuleParams {
                radius: 0.35,
                half_height: 0.9,
                eye_height: 1.1,
            },
            ground: GroundParams {
                speed: SpeedParams {
                    walk: 7.0,
                    run: 11.0,
                    crouch: 3.0,
                },
                accel: 12.0,
                step_height: 0.35,
                max_slope: 45.0,
            },
            air: AirParams {
                forward_steer: 0.3,
                accel: 2.0,
                max_control_speed: 4.0,
                bunny_hop: true,
                jumps: 1,
                jump_velocity: 5.0,
                jump_ceiling: 2.0,
            },
            fall: FallParams {
                terminal_velocity: 50.0,
            },
            stuck_stop_enabled: true,
            stuck_stop_threshold: 0.001,
            dash: None,
            forgiveness: None,
            crouch: None,
            slide: None,
            view_feel: None,
        })
    }

    fn spawn_test_movement_pawn(registry: &mut EntityRegistry) -> EntityId {
        let pawn = registry.spawn(Transform::default());
        registry
            .set_component(pawn, test_movement_component())
            .unwrap();
        pawn
    }

    fn assert_number_approx_eq(actual: f32, expected: f32) {
        const EPSILON: f32 = 1.0e-6;
        assert!(
            (actual - expected).abs() <= EPSILON,
            "expected {expected} ± {EPSILON}, got {actual}"
        );
    }

    #[test]
    fn presentation_spawn_intake_drains_once_in_fifo_order() {
        let mut registry = EntityRegistry::new();
        let spawn = |template: &str| PresentationSpawn {
            world_anchor: Vec3::ZERO,
            template: template.into(),
            facts: BTreeMap::new(),
            presenter: None,
            lifetime_seconds: 1.0,
            motion: PresentationMotion::default(),
            fade: PresentationFade::default(),
            scatter_radius: 0.0,
        };

        registry.push_presentation_spawn(spawn("first"));
        registry.push_presentation_spawn(spawn("second"));

        let drained = registry.take_presentation_spawns();
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0].template.0, "first");
        assert_eq!(drained[1].template.0, "second");
        assert!(registry.take_presentation_spawns().is_empty());
    }

    // Regression: moving the queue with `mem::take` discarded its fixed
    // reserved allocation after every render-frame drain.
    #[test]
    fn presentation_spawn_drain_reuses_registry_and_consumer_capacity() {
        let mut registry = EntityRegistry::new();
        registry.push_presentation_spawn(PresentationSpawn {
            world_anchor: Vec3::ZERO,
            template: "first".into(),
            facts: BTreeMap::new(),
            presenter: None,
            lifetime_seconds: 1.0,
            motion: PresentationMotion::default(),
            fade: PresentationFade::default(),
            scatter_radius: 0.0,
        });
        let registry_capacity = registry.presentation_spawns.capacity();
        let mut drained = Vec::new();

        registry.drain_presentation_spawns_into(&mut drained);
        let drained_capacity = drained.capacity();
        drained.clear();
        registry.push_presentation_spawn(PresentationSpawn {
            world_anchor: Vec3::ZERO,
            template: "second".into(),
            facts: BTreeMap::new(),
            presenter: None,
            lifetime_seconds: 1.0,
            motion: PresentationMotion::default(),
            fade: PresentationFade::default(),
            scatter_radius: 0.0,
        });
        registry.drain_presentation_spawns_into(&mut drained);

        assert_eq!(registry.presentation_spawns.capacity(), registry_capacity);
        assert_eq!(drained.capacity(), drained_capacity);
        assert_eq!(drained[0].template.0, "second");
    }

    #[test]
    fn presentation_spawn_intake_has_fixed_capacity() {
        let mut registry = EntityRegistry::new();
        for index in 0..=MAX_PENDING_PRESENTATION_SPAWNS {
            registry.push_presentation_spawn(PresentationSpawn {
                world_anchor: Vec3::ZERO,
                template: format!("template-{index}").into(),
                facts: BTreeMap::new(),
                presenter: None,
                lifetime_seconds: 1.0,
                motion: PresentationMotion::default(),
                fade: PresentationFade::default(),
                scatter_radius: 0.0,
            });
        }

        let drained = registry.take_presentation_spawns();
        assert_eq!(drained.len(), MAX_PENDING_PRESENTATION_SPAWNS);
        assert_eq!(drained.first().unwrap().template.0, "template-0");
        assert_eq!(
            drained.last().unwrap().template.0,
            format!("template-{}", MAX_PENDING_PRESENTATION_SPAWNS - 1)
        );
    }

    // Regression: level unload left a fixed-tick presentation spawn queued for
    // the next map's frame-time pool.
    #[test]
    fn level_unload_discards_pending_presentation_spawns() {
        let mut registry = EntityRegistry::new();
        registry.push_presentation_spawn(PresentationSpawn {
            world_anchor: Vec3::ZERO,
            template: "old-level".into(),
            facts: BTreeMap::new(),
            presenter: None,
            lifetime_seconds: 1.0,
            motion: PresentationMotion::default(),
            fade: PresentationFade::default(),
            scatter_radius: 0.0,
        });

        registry.clear_for_level_unload();

        assert!(registry.take_presentation_spawns().is_empty());
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
    fn spawn_creates_empty_per_entity_state() {
        let mut registry = EntityRegistry::new();
        let first = registry.spawn(Transform::default());
        let second = registry.spawn(Transform::default());

        assert_number_approx_eq(
            registry
                .get_component::<EntityStateComponent>(first)
                .expect("every spawn receives state")
                .get("hits"),
            0.0,
        );
        assert_number_approx_eq(
            registry
                .get_component::<EntityStateComponent>(second)
                .expect("every spawn receives state")
                .get("hits"),
            0.0,
        );
    }

    #[test]
    fn local_player_movement_pawn_prefers_marked_pawn_over_registry_order() {
        let mut registry = EntityRegistry::new();
        let first = spawn_test_movement_pawn(&mut registry);
        let marked = spawn_test_movement_pawn(&mut registry);
        registry.mark_local_player_pawn(marked).unwrap();

        assert_eq!(registry.local_player_movement_pawn(), Some(marked));
        assert_ne!(marked, first);
    }

    #[test]
    fn local_player_movement_pawn_falls_back_to_first_movement_pawn() {
        let mut registry = EntityRegistry::new();
        let first = spawn_test_movement_pawn(&mut registry);
        let _second = spawn_test_movement_pawn(&mut registry);

        assert_eq!(registry.local_player_movement_pawn(), Some(first));

        let marked_without_movement = registry.spawn(Transform::default());
        registry
            .mark_local_player_pawn(marked_without_movement)
            .unwrap();
        assert_eq!(
            registry.local_player_movement_pawn(),
            Some(first),
            "a marker without movement does not replace the legacy movement fallback"
        );
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

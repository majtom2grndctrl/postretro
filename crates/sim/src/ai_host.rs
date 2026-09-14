//! Synchronous engine effects exposed to the enemy-AI policy layer.
//!
//! AI keeps concrete read access to registry, navigation, and collision data.
//! This host owns only effects whose ordering must remain inside one AI tick.

use postretro_entities::components::health::{DamageContext, apply_damage_with_context};
use std::borrow::Cow;
use std::cell::RefCell;

use postretro_entities::{
    EntityId, EntityRegistry, EntityTypeDescriptor, FactionRegistry, FactionSentimentState,
};
use postretro_foundation::DamagePayload;
use postretro_physics::collision::CollisionWorld;

use crate::nav::NavGraph;
use crate::sim::{EnemyProjectilePresentationSpawn, spawn_projectile};
use crate::weapon::ProjectileLaunch;

/// Effects AI may apply synchronously while publishing one tick's outcomes.
pub trait AiHost {
    /// Materialize one projectile completely before returning its id.
    ///
    /// `Some(id)` means the entity and all gameplay components described by
    /// `launch` are live in `registry`. `None` means materialization failed and
    /// the registry is unchanged: no partial entity, presentation spawn, attack
    /// event, or caller-side shot/cooldown commit may be inferred from failure.
    fn spawn_projectile(
        &mut self,
        registry: &mut EntityRegistry,
        owner_pawn: EntityId,
        owner_weapon: EntityId,
        launch: ProjectileLaunch,
    ) -> Option<EntityId>;

    /// Apply damage synchronously through the contextual health chokepoint.
    /// Every later host call in this AI batch must observe the resulting health
    /// and lifecycle state.
    fn apply_damage(
        &mut self,
        registry: &mut EntityRegistry,
        target: EntityId,
        payload: &DamagePayload,
        context: DamageContext,
    );

    /// Observe whether `entity` must reject further same-batch AI outcomes.
    ///
    /// Quiescent means depleted health or a committed terminal lifecycle effect;
    /// it is observable immediately after [`Self::apply_damage`] and after an
    /// inline impact callback mutates lifecycle state.
    fn is_quiescent(&self, registry: &EntityRegistry, entity: EntityId) -> bool;

    /// Complete impact dispatch inline before returning. Any health, despawn,
    /// recovery, or sentiment writes are visible to the next AI outcome.
    fn on_impact(&mut self, registry: &mut EntityRegistry);
}

/// Borrowed world data read by one host-side AI tick.
///
/// The descriptor slice and its generation are one snapshot: callers must
/// obtain both from the same data-registry borrow. Keeping the world data
/// borrowed makes this a stack-only view with no per-tick allocation or
/// ownership transfer.
pub struct AiTickInputs<'a> {
    pub nav_graph: Option<&'a NavGraph>,
    pub collision_world: Option<&'a CollisionWorld>,
    pub descriptors: &'a [EntityTypeDescriptor],
    pub descriptor_generation: u64,
    /// Immutable manifest baseline from the same snapshot as descriptors.
    pub factions: &'a FactionRegistry,
    /// Engine-owned live sentiment state, borrowed only inside AI compute.
    pub faction_sentiment: &'a RefCell<FactionSentimentState>,
}

/// Finalized host-only output from exactly one AI stage invocation.
///
/// `events` contains only named events committed by successful outcomes during
/// this invocation. `projectile_spawns` contains exactly the successfully
/// materialized enemy projectiles that require presentation, in commit order;
/// failed spawn attempts appear in neither collection. Once returned, the host
/// has completed all synchronous damage and impact work for those outcomes.
pub struct AiTickResult {
    pub events: Vec<Cow<'static, str>>,
    pub projectile_spawns: Vec<EnemyProjectilePresentationSpawn>,
}

/// Bridge only for the duplicate sim crate identity compiled by sim's own
/// dev-dependency cycle through `postretro-ai`. Production runners return
/// `AiTickResult` directly and never allocate or rebuild projectile output.
#[cfg(any(test, feature = "test-support"))]
impl From<(Vec<Cow<'static, str>>, Vec<(EntityId, String)>)> for AiTickResult {
    fn from(
        (events, projectile_spawns): (Vec<Cow<'static, str>>, Vec<(EntityId, String)>),
    ) -> Self {
        Self {
            events,
            projectile_spawns: projectile_spawns
                .into_iter()
                .map(
                    |(projectile, descriptor_class)| EnemyProjectilePresentationSpawn {
                        projectile,
                        descriptor_class,
                    },
                )
                .collect(),
        }
    }
}

/// Production adapter used by the fixed-tick simulation seam.
pub struct SimAiHost<'a> {
    on_impact: &'a mut dyn FnMut(&mut EntityRegistry),
}

impl<'a> SimAiHost<'a> {
    fn from_callback(on_impact: &'a mut impl FnMut(&mut EntityRegistry)) -> Self {
        Self { on_impact }
    }

    #[cfg(feature = "test-support")]
    pub fn new(on_impact: &'a mut impl FnMut(&mut EntityRegistry)) -> Self {
        Self::from_callback(on_impact)
    }

    #[cfg(not(feature = "test-support"))]
    pub(crate) fn new(on_impact: &'a mut impl FnMut(&mut EntityRegistry)) -> Self {
        Self::from_callback(on_impact)
    }

    fn invoke_on_impact(&mut self, registry: &mut EntityRegistry) {
        (self.on_impact)(registry);
    }

    /// Forward impact work across the test-only duplicate-crate bridge.
    #[cfg(feature = "test-support")]
    pub fn on_impact(&mut self, registry: &mut EntityRegistry) {
        self.invoke_on_impact(registry);
    }
}

impl AiHost for SimAiHost<'_> {
    fn spawn_projectile(
        &mut self,
        registry: &mut EntityRegistry,
        owner_pawn: EntityId,
        owner_weapon: EntityId,
        launch: ProjectileLaunch,
    ) -> Option<EntityId> {
        spawn_projectile(registry, owner_pawn, owner_weapon, launch, None)
    }

    fn apply_damage(
        &mut self,
        registry: &mut EntityRegistry,
        target: EntityId,
        payload: &DamagePayload,
        context: DamageContext,
    ) {
        apply_damage_with_context(registry, target, payload, context);
    }

    fn is_quiescent(&self, registry: &EntityRegistry, entity: EntityId) -> bool {
        crate::scripting_systems::health::is_quiescent(registry, entity)
    }

    fn on_impact(&mut self, registry: &mut EntityRegistry) {
        self.invoke_on_impact(registry);
    }
}

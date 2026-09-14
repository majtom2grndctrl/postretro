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
    fn spawn_projectile(
        &mut self,
        registry: &mut EntityRegistry,
        owner_pawn: EntityId,
        owner_weapon: EntityId,
        launch: ProjectileLaunch,
    ) -> Option<EntityId>;

    fn apply_damage(
        &mut self,
        registry: &mut EntityRegistry,
        target: EntityId,
        payload: &DamagePayload,
        context: DamageContext,
    );

    fn is_quiescent(&self, registry: &EntityRegistry, entity: EntityId) -> bool;

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

/// Host-only output from the AI stage.
pub struct AiTickResult {
    pub events: Vec<Cow<'static, str>>,
    pub projectile_spawns: Vec<EnemyProjectilePresentationSpawn>,
}

/// Production adapter used by the fixed-tick simulation seam.
pub struct SimAiHost<'a, F> {
    on_impact: &'a mut F,
}

impl<'a, F> SimAiHost<'a, F>
where
    F: FnMut(&mut EntityRegistry),
{
    pub fn new(on_impact: &'a mut F) -> Self {
        Self { on_impact }
    }

    /// Reach the production adapter from the extracted AI crate's tests.
    #[cfg(any(test, feature = "test-support"))]
    pub fn for_test(on_impact: &'a mut F) -> Self {
        Self::new(on_impact)
    }
}

impl<F> AiHost for SimAiHost<'_, F>
where
    F: FnMut(&mut EntityRegistry),
{
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
        (self.on_impact)(registry);
    }
}

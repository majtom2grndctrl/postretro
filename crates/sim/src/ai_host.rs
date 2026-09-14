//! Synchronous engine effects exposed to the enemy-AI policy layer.
//!
//! AI keeps concrete read access to registry, navigation, and collision data.
//! This host owns only effects whose ordering must remain inside one AI tick.

use postretro_entities::components::health::{DamageContext, apply_damage_with_context};
use postretro_entities::{EntityId, EntityRegistry};
use postretro_foundation::DamagePayload;

use crate::sim::spawn_projectile;
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

/// Production adapter used by the fixed-tick simulation seam.
pub struct SimAiHost<'a, F> {
    on_impact: &'a mut F,
}

impl<'a, F> SimAiHost<'a, F>
where
    F: FnMut(&mut EntityRegistry),
{
    pub(crate) fn new(on_impact: &'a mut F) -> Self {
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

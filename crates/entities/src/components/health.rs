// Health component: descriptor-authored hit points plus the damage chokepoint.
// Spawn and hot reload set `max` and the optional hitbox; `current` is live HP
// that damage mutates and a later death sweep resolves.
//
// See: context/lib/entity_model.md §2 (Health component), §7 (Collision / hitscan targeting)

use std::collections::HashMap;

use glam::Vec3;
use serde::{Deserialize, Serialize};

use crate::data_descriptors::HealthDescriptor;
use crate::registry::{ComponentKind, EntityId, EntityRegistry};
use postretro_foundation::DamagePayload;

/// Maximum number of exact contributor source ids retained per target before
/// later distinct sources collapse into the overflow bucket.
pub const CONTRIBUTOR_LEDGER_CAPACITY: usize = 8;

/// One world-aligned AABB hitbox, fixed per archetype. Health-bearing entities
/// are hitscan-targetable when they carry this hitbox or use a zone-bearing
/// skinned model. `offset` shifts the box center from the entity's
/// `Transform.position`; entity rotation is ignored.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Hitbox {
    pub half_extents: Vec3,
    pub offset: Vec3,
}

/// Facts supplied to [`HealthComponent::record_contributor_damage`].
///
/// This is intentionally narrower than the future damage chokepoint context:
/// it only describes the already-mitigated hit that should be recorded in the
/// target-owned contributor ledger.
#[derive(Debug, Clone, PartialEq)]
pub struct ContributorLedgerRecord {
    pub source_id: String,
    pub damage: f32,
    pub zone: Option<String>,
    pub attacker: Option<EntityId>,
    pub weapon: Option<EntityId>,
}

impl ContributorLedgerRecord {
    pub fn new(source_id: impl Into<String>, damage: f32) -> Self {
        Self {
            source_id: source_id.into(),
            damage,
            zone: None,
            attacker: None,
            weapon: None,
        }
    }
}

/// Attribution facts that travel beside a [`DamagePayload`] at the health
/// chokepoint. The payload remains foundation-owned and amount-focused; entity
/// identity stays here in `postretro-entities`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DamageContext {
    pub source_id: String,
    pub attacker: Option<EntityId>,
    pub weapon: Option<EntityId>,
    pub zone: Option<String>,
}

impl DamageContext {
    pub fn unattributed() -> Self {
        Self {
            source_id: String::new(),
            attacker: None,
            weapon: None,
            zone: None,
        }
    }
}

/// Exact per-source contributor facts retained by a health component.
#[derive(Debug, Clone, PartialEq)]
pub struct ContributorLedgerEntry {
    pub source_id: String,
    pub accumulated_damage: f32,
    pub hit_count: u32,
    pub last_hit_damage: f32,
    pub last_hit_zone: Option<String>,
    pub last_attacker: Option<EntityId>,
    pub last_weapon: Option<EntityId>,
}

impl ContributorLedgerEntry {
    fn from_record(record: ContributorLedgerRecord) -> Self {
        Self {
            source_id: record.source_id,
            accumulated_damage: record.damage,
            hit_count: 1,
            last_hit_damage: record.damage,
            last_hit_zone: record.zone,
            last_attacker: record.attacker,
            last_weapon: record.weapon,
        }
    }

    fn record_hit(&mut self, record: ContributorLedgerRecord) {
        self.accumulated_damage += record.damage;
        self.hit_count += 1;
        self.last_hit_damage = record.damage;
        self.last_hit_zone = record.zone;
        self.last_attacker = record.attacker;
        self.last_weapon = record.weapon;
    }
}

/// Reduced facts for distinct source ids that arrived after the exact ledger
/// filled. It deliberately has no source id, so exact-source queries never
/// mistake overflow damage for a retained contributor.
#[derive(Debug, Clone, PartialEq)]
pub struct ContributorLedgerOverflow {
    pub accumulated_damage: f32,
    pub hit_count: u32,
    pub last_hit_damage: f32,
    pub last_hit_zone: Option<String>,
    pub last_attacker: Option<EntityId>,
    pub last_weapon: Option<EntityId>,
}

impl ContributorLedgerOverflow {
    fn from_record(record: ContributorLedgerRecord) -> Self {
        Self {
            accumulated_damage: record.damage,
            hit_count: 1,
            last_hit_damage: record.damage,
            last_hit_zone: record.zone,
            last_attacker: record.attacker,
            last_weapon: record.weapon,
        }
    }

    fn record_hit(&mut self, record: ContributorLedgerRecord) {
        self.accumulated_damage += record.damage;
        self.hit_count += 1;
        self.last_hit_damage = record.damage;
        self.last_hit_zone = record.zone;
        self.last_attacker = record.attacker;
        self.last_weapon = record.weapon;
    }
}

/// Bounded target-owned contributor ledger. The first
/// [`CONTRIBUTOR_LEDGER_CAPACITY`] distinct source ids remain exact; any later
/// distinct source rolls into `overflow`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ContributorLedger {
    entries: Vec<ContributorLedgerEntry>,
    overflow: Option<ContributorLedgerOverflow>,
}

impl ContributorLedger {
    pub fn record(&mut self, record: ContributorLedgerRecord) {
        if !record.damage.is_finite() || record.damage <= 0.0 {
            return;
        }

        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.source_id == record.source_id)
        {
            entry.record_hit(record);
            return;
        }

        if self.entries.len() < CONTRIBUTOR_LEDGER_CAPACITY {
            self.entries
                .push(ContributorLedgerEntry::from_record(record));
            return;
        }

        match &mut self.overflow {
            Some(overflow) => overflow.record_hit(record),
            None => self.overflow = Some(ContributorLedgerOverflow::from_record(record)),
        }
    }

    pub fn entries(&self) -> &[ContributorLedgerEntry] {
        &self.entries
    }

    pub fn overflow(&self) -> Option<&ContributorLedgerOverflow> {
        self.overflow.as_ref()
    }

    pub fn recorded_damage_by_source(&self, source_id: &str) -> Option<f32> {
        self.entries
            .iter()
            .find(|entry| entry.source_id == source_id)
            .map(|entry| entry.accumulated_damage)
    }

    pub fn total_recorded_damage(&self) -> f32 {
        let exact = self
            .entries
            .iter()
            .map(|entry| entry.accumulated_damage)
            .sum::<f32>();
        exact
            + self
                .overflow
                .as_ref()
                .map(|overflow| overflow.accumulated_damage)
                .unwrap_or(0.0)
    }

    pub fn total_recorded_hits(&self) -> u32 {
        let exact = self
            .entries
            .iter()
            .map(|entry| entry.hit_count)
            .sum::<u32>();
        exact
            + self
                .overflow
                .as_ref()
                .map(|overflow| overflow.hit_count)
                .unwrap_or(0)
    }
}

// Not `Copy`: `zone_multipliers` carries a heap-backed `HashMap`, so
// `HealthComponent` clones rather than copies. (`max`/`current`/`hitbox` stay
// scalar; the map is the sole heap field.)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HealthComponent {
    pub max: f32,
    pub current: f32,
    pub hitbox: Option<Hitbox>,
    /// One-shot latch: set when a persisting zero-HP player's death is reported
    /// so the `playerDied` event fires exactly once. The death sweep
    /// (`systems/health.rs`) is this field's only writer; nothing here mutates it.
    #[serde(default)]
    pub death_handled: bool,
    /// Per-skeletal-zone damage multipliers, tag → factor, materialized from the
    /// descriptor. The damage site scales the payload by `zone_multipliers[tag]`
    /// for the struck zone (absent zone OR absent entry ⇒ `1.0`). Reseeded on
    /// hot reload so multiplier edits land on live entities without respawn.
    #[serde(default)]
    pub zone_multipliers: HashMap<String, f32>,
    /// Transient per-target combat attribution. Skipped for serde so component
    /// serialization and future replication snapshots do not accidentally
    /// persist combat history; despawn/level reload clear it by dropping this
    /// component.
    #[serde(skip)]
    pub contributor_ledger: ContributorLedger,
}

impl HealthComponent {
    /// Materialize from a descriptor at spawn. `current` initializes to `max`.
    pub fn from_descriptor(desc: &HealthDescriptor) -> Self {
        Self {
            max: desc.max,
            current: desc.max,
            hitbox: desc.hitbox.as_ref().map(|h| Hitbox {
                half_extents: Vec3::from_array(h.half_extents),
                offset: Vec3::from_array(h.offset.unwrap_or([0.0, 0.0, 0.0])),
            }),
            death_handled: false,
            zone_multipliers: desc.zone_multipliers.clone(),
            contributor_ledger: ContributorLedger::default(),
        }
    }

    /// Hot-reload refresh: `max`, `hitbox`, and `zone_multipliers` reseed from
    /// the new descriptor; `current` clamps to the new max so an authored max
    /// reduction cannot leave HP above the cap. `death_handled` is live state
    /// and is preserved. The reseeded multiplier map carries the edit onto live
    /// entities without a respawn.
    pub fn refresh_from_descriptor(&mut self, desc: &HealthDescriptor) {
        self.max = desc.max;
        self.current = self.current.min(desc.max);
        self.hitbox = desc.hitbox.as_ref().map(|h| Hitbox {
            half_extents: Vec3::from_array(h.half_extents),
            offset: Vec3::from_array(h.offset.unwrap_or([0.0, 0.0, 0.0])),
        });
        self.zone_multipliers = desc.zone_multipliers.clone();
    }

    pub fn record_contributor_damage(&mut self, record: ContributorLedgerRecord) {
        self.contributor_ledger.record(record);
    }
}

/// Locate the local player pawn and return it paired with its live `Health`
/// component, when both are present. The registry marker wins; older
/// fixtures/maps with no marker fall back to the first entity carrying
/// `PlayerMovement`.
/// `None` when there is no resolved pawn or it carries no `Health` component.
/// Note: when the registry marker resolves a pawn that carries `PlayerMovement`
/// but lacks `Health`, this function returns `None` immediately — it does NOT
/// fall through to the legacy `PlayerMovement` iteration. The early return is
/// intentional: the marker is authoritative, so a different pawn is not
/// substituted when the marked pawn simply has no health component.
///
/// Shared by the `player.health` slot-range producers: the level-install path
/// (attaching `[0, max]` at materialization) and the hot-reload range-follow
/// hook both resolve the pawn the same way.
pub fn pawn_with_health(registry: &EntityRegistry) -> Option<(EntityId, HealthComponent)> {
    if let Some(pawn) = registry.local_player_pawn() {
        if matches!(
            registry.has_component_kind(pawn, ComponentKind::PlayerMovement),
            Ok(true)
        ) {
            return registry
                .get_component::<HealthComponent>(pawn)
                .ok()
                .map(|health| (pawn, health.clone()));
        }
    }

    let (pawn, _) = registry
        .iter_with_kind(ComponentKind::PlayerMovement)
        .next()?;
    registry
        .get_component::<HealthComponent>(pawn)
        .ok()
        .map(|health| (pawn, health.clone()))
}

/// Compatibility shim for producers not yet migrated to contextual damage.
pub fn apply_damage(registry: &mut EntityRegistry, id: EntityId, payload: &DamagePayload) {
    apply_damage_with_context(registry, id, payload, DamageContext::unattributed());
}

/// The damage chokepoint every producer routes through. Subtracts the payload's
/// `amount` from the entity's current HP, flooring at zero. No-ops (returns
/// without error) when the entity carries no `Health` component or no longer
/// exists — damage to a non-health entity is simply ignored, never an error.
///
/// Damage arrives only as a [`DamagePayload`] (never a bare scalar); spatial
/// info rides beside the payload, never inside it.
pub fn apply_damage_with_context(
    registry: &mut EntityRegistry,
    id: EntityId,
    payload: &DamagePayload,
    context: DamageContext,
) {
    let Ok(health) = registry.get_component::<HealthComponent>(id) else {
        return;
    };
    let mut updated = health.clone();
    updated.current = (updated.current - payload.amount).max(0.0);
    if !updated.death_handled && payload.amount.is_finite() && payload.amount > 0.0 {
        let mut record = ContributorLedgerRecord::new(context.source_id, payload.amount);
        record.zone = context.zone;
        record.attacker = context.attacker;
        record.weapon = context.weapon;
        updated.record_contributor_damage(record);
    }
    // `set_component` only fails on a stale id, which `get_component` already
    // ruled out above.
    let _ = registry.set_component(id, updated);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_descriptors::HitboxDescriptor;
    use crate::registry::Transform;

    fn descriptor(max: f32) -> HealthDescriptor {
        HealthDescriptor {
            max,
            hitbox: None,
            zone_multipliers: HashMap::new(),
        }
    }

    #[test]
    fn from_descriptor_initializes_current_to_max() {
        let component = HealthComponent::from_descriptor(&descriptor(80.0));
        assert_eq!(component.current, 80.0);
        assert_eq!(component.max, 80.0);
        assert!(component.hitbox.is_none());
        assert!(!component.death_handled);
        assert!(component.contributor_ledger.entries().is_empty());
        assert!(component.contributor_ledger.overflow().is_none());
    }

    #[test]
    fn from_descriptor_carries_hitbox_with_default_offset() {
        let desc = HealthDescriptor {
            max: 50.0,
            hitbox: Some(HitboxDescriptor {
                half_extents: [0.5, 1.0, 0.5],
                offset: None,
            }),
            zone_multipliers: HashMap::new(),
        };
        let component = HealthComponent::from_descriptor(&desc);
        let hitbox = component.hitbox.expect("hitbox materialized");
        assert_eq!(hitbox.half_extents, Vec3::new(0.5, 1.0, 0.5));
        assert_eq!(hitbox.offset, Vec3::ZERO, "absent offset defaults to zero");
    }

    #[test]
    fn refresh_clamps_current_to_new_lower_max() {
        let mut component = HealthComponent::from_descriptor(&descriptor(100.0));
        component.current = 90.0;
        component.refresh_from_descriptor(&descriptor(40.0));
        assert_eq!(component.max, 40.0);
        assert_eq!(component.current, 40.0, "current clamps to the new max");
    }

    #[test]
    fn refresh_preserves_current_below_new_max() {
        let mut component = HealthComponent::from_descriptor(&descriptor(100.0));
        component.current = 30.0;
        component.refresh_from_descriptor(&descriptor(200.0));
        assert_eq!(component.max, 200.0);
        assert_eq!(
            component.current, 30.0,
            "current under the cap is untouched"
        );
    }

    #[test]
    fn apply_damage_subtracts_amount() {
        let mut reg = EntityRegistry::new();
        let id = reg.spawn(Transform::default());
        reg.set_component(id, HealthComponent::from_descriptor(&descriptor(100.0)))
            .unwrap();

        apply_damage(&mut reg, id, &DamagePayload { amount: 25.0 });

        assert_eq!(
            reg.get_component::<HealthComponent>(id).unwrap().current,
            75.0
        );
    }

    #[test]
    fn apply_damage_floors_current_at_zero() {
        let mut reg = EntityRegistry::new();
        let id = reg.spawn(Transform::default());
        reg.set_component(id, HealthComponent::from_descriptor(&descriptor(10.0)))
            .unwrap();

        apply_damage(&mut reg, id, &DamagePayload { amount: 999.0 });

        assert_eq!(
            reg.get_component::<HealthComponent>(id).unwrap().current,
            0.0,
            "HP never goes negative"
        );
    }

    #[test]
    fn apply_damage_with_context_records_unclamped_post_mitigation_damage() {
        let mut reg = EntityRegistry::new();
        let target = reg.spawn(Transform::default());
        let attacker = EntityId::from_raw(11);
        let weapon = EntityId::from_raw(12);
        reg.set_component(target, HealthComponent::from_descriptor(&descriptor(100.0)))
            .unwrap();

        apply_damage_with_context(
            &mut reg,
            target,
            &DamagePayload { amount: 25.0 },
            DamageContext {
                source_id: "weapon.plasma".to_string(),
                attacker: Some(attacker),
                weapon: Some(weapon),
                zone: Some("torso".to_string()),
            },
        );
        apply_damage_with_context(
            &mut reg,
            target,
            &DamagePayload { amount: 90.0 },
            DamageContext {
                source_id: "weapon.plasma".to_string(),
                attacker: Some(attacker),
                weapon: Some(weapon),
                zone: Some("head".to_string()),
            },
        );

        let health = reg.get_component::<HealthComponent>(target).unwrap();
        assert_eq!(health.current, 0.0);
        let entries = health.contributor_ledger.entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].source_id, "weapon.plasma");
        assert_eq!(
            entries[0].accumulated_damage, 115.0,
            "ledger records the full payload amount that reached the chokepoint"
        );
        assert_eq!(entries[0].hit_count, 2);
        assert_eq!(entries[0].last_hit_damage, 90.0);
        assert_eq!(entries[0].last_hit_zone.as_deref(), Some("head"));
        assert_eq!(entries[0].last_attacker, Some(attacker));
        assert_eq!(entries[0].last_weapon, Some(weapon));
    }

    #[test]
    fn apply_damage_with_context_does_not_record_after_death_latch() {
        let mut reg = EntityRegistry::new();
        let target = reg.spawn(Transform::default());
        let mut component = HealthComponent::from_descriptor(&descriptor(100.0));
        component.death_handled = true;
        reg.set_component(target, component).unwrap();

        apply_damage_with_context(
            &mut reg,
            target,
            &DamagePayload { amount: 25.0 },
            DamageContext {
                source_id: "weapon.plasma".to_string(),
                attacker: None,
                weapon: None,
                zone: None,
            },
        );

        let health = reg.get_component::<HealthComponent>(target).unwrap();
        assert_eq!(
            health.current, 75.0,
            "HP mutation still follows the chokepoint"
        );
        assert!(
            health.contributor_ledger.entries().is_empty(),
            "latched deaths must not accumulate later contributors"
        );
        assert!(health.contributor_ledger.overflow().is_none());
    }

    #[test]
    fn from_descriptor_carries_zone_multipliers() {
        let mut desc = descriptor(100.0);
        desc.zone_multipliers.insert("head".to_string(), 1.5);
        let component = HealthComponent::from_descriptor(&desc);
        assert_eq!(component.zone_multipliers.get("head"), Some(&1.5));
    }

    #[test]
    fn refresh_reseeds_zone_multipliers() {
        // Hot reload: a multiplier edit lands on the live component without a
        // respawn, mirroring how `max`/`hitbox` reseed.
        let mut start = descriptor(100.0);
        start.zone_multipliers.insert("head".to_string(), 1.5);
        let mut component = HealthComponent::from_descriptor(&start);
        component.current = 40.0;

        let mut reloaded = descriptor(100.0);
        reloaded.zone_multipliers.insert("head".to_string(), 2.0);
        reloaded.zone_multipliers.insert("leg".to_string(), 0.5);
        component.refresh_from_descriptor(&reloaded);

        assert_eq!(component.zone_multipliers.get("head"), Some(&2.0));
        assert_eq!(component.zone_multipliers.get("leg"), Some(&0.5));
        assert_eq!(component.current, 40.0, "live HP preserved across reload");
    }

    #[test]
    fn zone_multiplier_scales_payload_amount() {
        // Mirrors the damage-site computation: a listed tag scales the payload,
        // an unlisted tag and an absent zone both apply 1.0. `apply_damage`
        // itself stays amount-only (the scaling happens at the fire site).
        let mut desc = descriptor(100.0);
        desc.zone_multipliers.insert("head".to_string(), 1.5);
        let component = HealthComponent::from_descriptor(&desc);

        let base = 20.0_f32;
        let mult = |zone: Option<&str>| {
            zone.and_then(|tag| component.zone_multipliers.get(tag).copied())
                .unwrap_or(1.0)
        };
        assert_eq!(base * mult(Some("head")), 30.0, "head: 1.5x");
        assert_eq!(base * mult(Some("torso")), 20.0, "unlisted tag: 1.0x");
        assert_eq!(base * mult(None), 20.0, "absent zone: 1.0x");
    }

    #[test]
    fn apply_damage_is_noop_without_health_component() {
        let mut reg = EntityRegistry::new();
        let id = reg.spawn(Transform::default());

        // No Health component attached: must not error or panic.
        apply_damage(&mut reg, id, &DamagePayload { amount: 25.0 });

        assert!(reg.get_component::<HealthComponent>(id).is_err());
    }

    #[test]
    fn contributor_ledger_aggregates_repeated_source_and_last_hit_metadata() {
        let attacker = EntityId::from_raw(11);
        let weapon = EntityId::from_raw(12);
        let mut component = HealthComponent::from_descriptor(&descriptor(100.0));

        let mut first = ContributorLedgerRecord::new("weapon.plasma", 10.0);
        first.zone = Some("torso".to_string());
        first.attacker = Some(attacker);
        component.record_contributor_damage(first);

        let mut second = ContributorLedgerRecord::new("weapon.plasma", 15.0);
        second.zone = Some("head".to_string());
        second.weapon = Some(weapon);
        component.record_contributor_damage(second);

        let entries = component.contributor_ledger.entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].source_id, "weapon.plasma");
        assert_eq!(entries[0].accumulated_damage, 25.0);
        assert_eq!(entries[0].hit_count, 2);
        assert_eq!(entries[0].last_hit_damage, 15.0);
        assert_eq!(entries[0].last_hit_zone.as_deref(), Some("head"));
        assert_eq!(entries[0].last_attacker, None);
        assert_eq!(entries[0].last_weapon, Some(weapon));
        assert_eq!(
            component
                .contributor_ledger
                .recorded_damage_by_source("weapon.plasma"),
            Some(25.0)
        );
    }

    #[test]
    fn contributor_ledger_ignores_non_positive_and_non_finite_damage() {
        let mut component = HealthComponent::from_descriptor(&descriptor(100.0));

        for damage in [0.0, -1.0, f32::INFINITY, f32::NAN] {
            component.record_contributor_damage(ContributorLedgerRecord::new(
                format!("source.{damage}"),
                damage,
            ));
        }

        assert!(component.contributor_ledger.entries().is_empty());
        assert!(component.contributor_ledger.overflow().is_none());
        assert_eq!(component.contributor_ledger.total_recorded_damage(), 0.0);
        assert_eq!(component.contributor_ledger.total_recorded_hits(), 0);
    }

    #[test]
    fn contributor_ledger_overflow_is_bounded_deterministic_and_keeps_exact_sources() {
        let mut component = HealthComponent::from_descriptor(&descriptor(100.0));

        for index in 0..CONTRIBUTOR_LEDGER_CAPACITY {
            component.record_contributor_damage(ContributorLedgerRecord::new(
                format!("source.{index}"),
                1.0,
            ));
        }

        component.record_contributor_damage(ContributorLedgerRecord::new("source.overflow-a", 4.0));
        component.record_contributor_damage(ContributorLedgerRecord::new("source.overflow-b", 5.0));
        component.record_contributor_damage(ContributorLedgerRecord::new("source.0", 7.0));

        let ledger = &component.contributor_ledger;
        assert_eq!(ledger.entries().len(), CONTRIBUTOR_LEDGER_CAPACITY);
        for (index, entry) in ledger.entries().iter().enumerate() {
            assert_eq!(
                entry.source_id,
                format!("source.{index}"),
                "retained source ids stay in first-seen order"
            );
        }

        let overflow = ledger.overflow().expect("overflow bucket should exist");
        assert_eq!(overflow.accumulated_damage, 9.0);
        assert_eq!(overflow.hit_count, 2);
        assert_eq!(overflow.last_hit_damage, 5.0);
        assert_eq!(ledger.recorded_damage_by_source("source.0"), Some(8.0));
        assert_eq!(ledger.recorded_damage_by_source("source.overflow-a"), None);
        assert_eq!(ledger.total_recorded_damage(), 24.0);
        assert_eq!(ledger.total_recorded_hits(), 11);
    }

    #[test]
    fn pawn_with_health_does_not_fallback_when_marked_movement_pawn_lacks_health() {
        use crate::components::player_movement::PlayerMovementComponent;
        use crate::data_descriptors::{
            AirParams, CapsuleParams, FallParams, GroundParams, PlayerMovementDescriptor,
            SpeedParams,
        };

        fn movement_descriptor() -> PlayerMovementDescriptor {
            PlayerMovementDescriptor {
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
                view_feel: None,
            }
        }

        let mut reg = EntityRegistry::new();
        let local = reg.spawn(Transform::default());
        reg.set_component(
            local,
            PlayerMovementComponent::from_descriptor(&movement_descriptor()),
        )
        .unwrap();
        reg.mark_local_player_pawn(local).unwrap();

        let remote = reg.spawn(Transform::default());
        reg.set_component(
            remote,
            PlayerMovementComponent::from_descriptor(&movement_descriptor()),
        )
        .unwrap();
        reg.set_component(remote, HealthComponent::from_descriptor(&descriptor(100.0)))
            .unwrap();

        assert_eq!(
            pawn_with_health(&reg),
            None,
            "a marked local movement pawn without Health must not fall back to a remote pawn"
        );
    }
}

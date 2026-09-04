// Per-tick death sweep: latches zero-HP entities after damage settles.
// Zero HP alone never removes an entity; the player pawn
// latches at zero and reports `playerDied` exactly once. Component-only state
// (HP, `death_handled`) lives in `components/health.rs`; this is the system
// half of that split, mirroring the components/systems separation of
// `particle_sim`.
//
// Brain-bearing enemies are a THIRD case, layered between player and plain
// non-player: at zero HP they latch (reusing `HealthComponent.death_handled`)
// and freeze kill credit, but are NOT reported or despawned here. An authored
// deferred `despawn` owns eventual removal and kill reporting.
//
// See: context/lib/entity_model.md §3 (Destruction)

use postretro_entities::components::health::{
    ContributorLedger, ContributorLedgerEntry, ContributorLedgerOverflow, HealthComponent,
    PendingKillCredit,
};
use postretro_entities::registry::{ComponentKind, ComponentValue, EntityId, EntityRegistry};

/// Event name fired once when the player pawn's HP reaches zero. Latched by
/// `HealthComponent::death_handled` so a persisting zero-HP pawn never re-fires.
pub(crate) const PLAYER_DIED_EVENT: &str = "playerDied";

/// What one death sweep observed, returned to the caller because the sweep
/// cannot reach the event-dispatch path itself. Non-player kill credit stays on
/// `HealthComponent` until a deferred removal actually succeeds; only the
/// player branch still produces a zero-HP event here.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct DeathReport {
    /// Set once on the tick the player pawn first reaches zero HP. The
    /// `death_handled` latch guarantees later sweeps leave this `false`.
    pub(crate) player_died: bool,
    /// Contributor ledger captured when the player death latch flips. Present
    /// exactly when `player_died` is true.
    pub(crate) player_contributor_ledger: Option<ContributorLedgerSnapshot>,
}

/// Owned contributor facts handed to report consumers. Player credit is cloned
/// by this sweep; non-player credit is frozen at first-zero HP and converted
/// from the pending component state when deferred removal succeeds.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct ContributorLedgerSnapshot {
    pub(crate) entries: Vec<ContributorLedgerEntry>,
    pub(crate) overflow: Option<ContributorLedgerOverflow>,
}

impl ContributorLedgerSnapshot {
    fn from_health(health: &HealthComponent) -> Self {
        Self::from_contributor_ledger(&health.contributor_ledger)
    }

    pub(crate) fn from_contributor_ledger(contributor_ledger: &ContributorLedger) -> Self {
        Self {
            entries: contributor_ledger.entries().to_vec(),
            overflow: contributor_ledger.overflow().cloned(),
        }
    }
}

/// Resolve every entity at zero HP. Two-pass like `particle_sim`: collect the
/// dead ids under an immutable borrow, then mutate (latch) so the
/// registry is never written mid-walk.
///
/// - **Player** (carries `PlayerMovement`): never despawn. If `death_handled`
///   is already set, skip entirely (the one-shot latch). Otherwise set the
///   latch and report `player_died`.
/// - **Brain enemy** (carries `Brain`, not `PlayerMovement`): if
///   `death_handled` is already set, skip. Otherwise set the latch and freeze
///   its tags plus contributor ledger on the health component.
/// - **Plain non-player** (neither `PlayerMovement` nor `Brain`): follows the
///   same latch-and-credit behavior. Neither branch removes the entity; an
///   authored deferred `despawn` owns removal.
///
/// Frame ordering: runs in the game-logic stage after the weapon fire tick, so
/// damage applied this frame is resolved before render reads entity state.
pub(crate) fn sweep_deaths(registry: &mut EntityRegistry) -> DeathReport {
    // Pass 1: collect ids at zero HP under the immutable iterator borrow, which
    // must be dropped before the latch writes below.
    let mut dead: Vec<EntityId> = Vec::new();
    for (id, value) in registry.iter_with_kind(ComponentKind::Health) {
        let ComponentValue::Health(health) = value else {
            continue;
        };
        // `<= 0.0`, not `== 0.0`: the contextual damage chokepoint floors HP at
        // exactly `0.0`, so today HP never goes negative or non-finite.
        // The guard defends against a future direct write that could: a negative
        // OR a NaN `current` (`NaN <= 0.0` is false, which would otherwise leave a
        // corrupt entity immortal).
        if health.current <= 0.0 || !health.current.is_finite() {
            dead.push(id);
        }
    }

    let mut report = DeathReport::default();

    // Pass 2: mutate. Player vs. non-player is decided by the PlayerMovement
    // component per entity_model.md ("a player by virtue of carrying
    // PlayerMovement").
    for id in dead {
        let is_player = registry
            .has_component_kind(id, ComponentKind::PlayerMovement)
            .unwrap_or(false);

        if is_player {
            // Read the latch; skip if death was already reported on an earlier
            // tick so `playerDied` fires exactly once.
            let Ok(health) = registry.get_component::<HealthComponent>(id) else {
                continue;
            };
            if health.death_handled {
                continue;
            }
            let mut updated = health.clone();
            updated.death_handled = true;
            let ledger = ContributorLedgerSnapshot::from_health(&updated);
            let _ = registry.set_component(id, updated);
            report.player_died = true;
            report.player_contributor_ledger = Some(ledger);
            continue;
        }

        // A brain-bearing enemy latches credit once here. It persists until an
        // authored deferred despawn removes and reports it.
        let is_brain = registry
            .has_component_kind(id, ComponentKind::Brain)
            .unwrap_or(false);
        if is_brain {
            let Ok(health) = registry.get_component::<HealthComponent>(id) else {
                continue;
            };
            if health.death_handled {
                // Latch holds: do not re-count an entity that persists at zero HP.
                continue;
            }
            let mut updated = health.clone();
            updated.death_handled = true;
            let tags = registry
                .get_tags(id)
                .map(|tags| tags.to_vec())
                .unwrap_or_default();
            updated.pending_kill_credit = Some(PendingKillCredit {
                tags,
                contributor_ledger: updated.contributor_ledger.clone(),
            });
            let _ = registry.set_component(id, updated);
            continue;
        }

        // Plain non-player: just like a brain, latch before freezing credit.
        // It persists at zero HP, so the latch suppresses later changes.
        let Ok(health) = registry.get_component::<HealthComponent>(id) else {
            continue;
        };
        if health.death_handled {
            continue;
        }
        let mut updated = health.clone();
        updated.death_handled = true;
        let tags = registry
            .get_tags(id)
            .map(|tags| tags.to_vec())
            .unwrap_or_default();
        updated.pending_kill_credit = Some(PendingKillCredit {
            tags,
            contributor_ledger: updated.contributor_ledger.clone(),
        });
        let _ = registry.set_component(id, updated);
    }

    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_entities::components::brain::attach_brain_graph;
    use postretro_entities::components::health::{
        ContributorLedgerRecord, DamageContext, DamageProducer, apply_damage_with_context,
        set_health_absolute,
    };
    use postretro_entities::components::player_movement::PlayerMovementComponent;
    use postretro_entities::registry::Transform;
    use postretro_foundation::DamagePayload;
    use postretro_scripting_core::data_descriptors::{
        AirParams, BehaviorActivityDescriptor, BehaviorGraphDescriptor, BehaviorGraphEnvelope,
        CapsuleParams, FallParams, GroundParams, HealthDescriptor, MotionVerb,
        PlayerMovementDescriptor, SpeedParams,
    };

    /// Attach a Brain component, marking the entity as brain-bearing for the
    /// sweep. Mirrors `make_player`: the sweep branches on component *presence*.
    fn make_brain(registry: &mut EntityRegistry, id: EntityId) {
        let graph = BehaviorGraphDescriptor {
            envelope: BehaviorGraphEnvelope {
                initial: "idle".to_string(),
                activities: std::collections::BTreeMap::from([(
                    "idle".to_string(),
                    BehaviorActivityDescriptor {
                        animation: Some("idle".to_string()),
                        motion: Some(MotionVerb::Hold),
                        action: None,
                        on_enter: None,
                        layers: std::collections::BTreeMap::new(),
                    },
                )]),
                transitions: std::collections::BTreeMap::new(),
            },
            candidate_filter: None,
            patrol: None,
            attacks: Default::default(),
            engagement_radius: None,
            move_speed: 3.5,
        };
        attach_brain_graph(registry, id, &graph).unwrap();
    }

    fn health(max: f32) -> HealthDescriptor {
        HealthDescriptor {
            max,
            hitbox: None,
            zone_multipliers: std::collections::HashMap::new(),
        }
    }

    /// Spawn an entity carrying a Health component at the given current HP,
    /// optionally tagged. `current` is set after `from_descriptor` (which seeds
    /// current == max) so tests can place it directly at zero.
    fn spawn_health_entity(
        registry: &mut EntityRegistry,
        max: f32,
        current: f32,
        tags: &[&str],
    ) -> EntityId {
        let id = registry.spawn(Transform::default());
        let mut component = HealthComponent::from_descriptor(&health(max));
        component.current = current;
        registry.set_component(id, component).unwrap();
        if !tags.is_empty() {
            let owned: Vec<String> = tags.iter().map(|t| t.to_string()).collect();
            registry.set_tags(id, owned).unwrap();
        }
        id
    }

    fn record_contributor(
        registry: &mut EntityRegistry,
        id: EntityId,
        source_id: &str,
        damage: f32,
    ) {
        let mut health = registry
            .get_component::<HealthComponent>(id)
            .expect("health component should exist")
            .clone();
        health.record_contributor_damage(ContributorLedgerRecord::new(source_id, damage));
        registry.set_component(id, health).unwrap();
    }

    /// Attach a PlayerMovement component, marking the entity as the player pawn
    /// for the sweep's purposes. The sweep branches only on the component's
    /// *presence* (`entity_model.md`: "a player by virtue of carrying
    /// `PlayerMovement`"), so a minimal materialized descriptor suffices — the
    /// tuning values are never read here.
    fn make_player(registry: &mut EntityRegistry, id: EntityId) {
        let descriptor = PlayerMovementDescriptor {
            capsule: CapsuleParams {
                radius: 0.5,
                half_height: 0.9,
                eye_height: 0.7,
            },
            ground: GroundParams {
                speed: SpeedParams {
                    walk: 7.0,
                    run: 10.0,
                    crouch: 3.0,
                },
                accel: 60.0,
                step_height: 0.3,
                max_slope: 45.0,
            },
            air: AirParams {
                forward_steer: 1.0,
                accel: 10.0,
                max_control_speed: 10.0,
                bunny_hop: false,
                jumps: 1,
                jump_velocity: 6.0,
                jump_ceiling: 0.0,
            },
            fall: FallParams {
                terminal_velocity: 50.0,
            },
            stuck_stop_enabled: PlayerMovementDescriptor::DEFAULT_STUCK_STOP_ENABLED,
            stuck_stop_threshold: PlayerMovementDescriptor::DEFAULT_STUCK_STOP_THRESHOLD,
            dash: None,
            forgiveness: None,
            crouch: None,
            slide: None,
            view_feel: None,
        };
        registry
            .set_component(id, PlayerMovementComponent::from_descriptor(&descriptor))
            .unwrap();
    }

    #[test]
    fn nonplayer_at_zero_persists_latches_and_freezes_credit_without_a_report() {
        let mut reg = EntityRegistry::new();
        let id = spawn_health_entity(&mut reg, 50.0, 0.0, &["reactorMonster", "wave1"]);
        record_contributor(&mut reg, id, "weapon.test", 50.0);

        let report = sweep_deaths(&mut reg);

        assert!(
            reg.exists(id),
            "zero HP alone must not despawn a non-player"
        );
        assert!(
            reg.get_component::<HealthComponent>(id)
                .unwrap()
                .death_handled,
            "a persisting non-player must latch after its first zero-HP sweep"
        );
        assert_eq!(
            report,
            DeathReport::default(),
            "zero HP emits no kill report"
        );
        let credit = reg
            .get_component::<HealthComponent>(id)
            .unwrap()
            .pending_kill_credit
            .as_ref()
            .expect("first zero-HP latch freezes pending credit");
        assert_eq!(
            credit.tags,
            vec!["reactorMonster".to_string(), "wave1".to_string()]
        );
        assert_eq!(credit.contributor_ledger.entries().len(), 1);
        assert_eq!(
            credit.contributor_ledger.entries()[0].source_id,
            "weapon.test"
        );
        assert_eq!(
            credit.contributor_ledger.entries()[0].accumulated_damage,
            50.0
        );

        assert_eq!(
            sweep_deaths(&mut reg),
            DeathReport::default(),
            "the latch must suppress repeated reports while the entity persists",
        );
    }

    #[test]
    fn entity_above_zero_is_untouched() {
        let mut reg = EntityRegistry::new();
        let id = spawn_health_entity(&mut reg, 100.0, 1.0, &["mob"]);

        let report = sweep_deaths(&mut reg);

        assert!(reg.exists(id), "living entity must not be despawned");
        assert_eq!(report, DeathReport::default());
    }

    #[test]
    fn player_at_zero_is_not_despawned_and_reports_player_died_once() {
        let mut reg = EntityRegistry::new();
        let id = spawn_health_entity(&mut reg, 100.0, 0.0, &[]);
        record_contributor(&mut reg, id, "enemy.attack", 100.0);
        make_player(&mut reg, id);

        let first = sweep_deaths(&mut reg);
        assert!(reg.exists(id), "player pawn must never despawn from damage");
        assert!(first.player_died, "first zero-HP sweep reports playerDied");
        assert!(
            first.player_contributor_ledger.is_some(),
            "player death report captures the player's contributor ledger"
        );
        let player_ledger = first
            .player_contributor_ledger
            .as_ref()
            .expect("player contributor ledger");
        assert_eq!(player_ledger.entries.len(), 1);
        assert_eq!(player_ledger.entries[0].source_id, "enemy.attack");
        assert_eq!(player_ledger.entries[0].accumulated_damage, 100.0);
        assert!(
            reg.get_component::<HealthComponent>(id)
                .unwrap()
                .death_handled,
            "death_handled latch must be set after reporting"
        );

        // Second sweep with HP still at zero must report nothing (latch holds).
        let second = sweep_deaths(&mut reg);
        assert_eq!(
            second,
            DeathReport::default(),
            "latched player death must not re-report on a later sweep"
        );
        assert!(reg.exists(id));
    }

    #[test]
    fn zero_health_write_preserves_player_died_one_shot_latch() {
        let mut reg = EntityRegistry::new();
        let id = spawn_health_entity(&mut reg, 100.0, 0.0, &[]);
        make_player(&mut reg, id);

        assert!(sweep_deaths(&mut reg).player_died);
        set_health_absolute(&mut reg, id, 0.0);

        assert_eq!(
            sweep_deaths(&mut reg),
            DeathReport::default(),
            "setHealth(0) must not re-arm playerDied",
        );
        assert!(
            reg.get_component::<HealthComponent>(id)
                .unwrap()
                .death_handled
        );
    }

    #[test]
    fn multiple_dead_nonplayers_all_latch_without_a_report() {
        let mut reg = EntityRegistry::new();
        let a = spawn_health_entity(&mut reg, 10.0, 0.0, &["a"]);
        let b = spawn_health_entity(&mut reg, 10.0, 0.0, &["b"]);
        record_contributor(&mut reg, a, "source.a", 10.0);
        record_contributor(&mut reg, b, "source.b", 10.0);
        // A survivor to prove the sweep is selective.
        let alive = spawn_health_entity(&mut reg, 10.0, 5.0, &["c"]);

        let report = sweep_deaths(&mut reg);

        assert!(reg.exists(a));
        assert!(reg.exists(b));
        assert!(reg.exists(alive));
        assert!(!report.player_died);
        assert_eq!(report, DeathReport::default());
        for (id, tag, source) in [(a, "a", "source.a"), (b, "b", "source.b")] {
            let credit = reg
                .get_component::<HealthComponent>(id)
                .unwrap()
                .pending_kill_credit
                .as_ref()
                .expect("zero-HP non-player freezes credit");
            assert_eq!(credit.tags, vec![tag.to_string()]);
            assert_eq!(credit.contributor_ledger.entries()[0].source_id, source);
        }
    }

    #[test]
    fn untagged_dead_nonplayer_freezes_empty_tag_credit_without_a_report() {
        let mut reg = EntityRegistry::new();
        let id = spawn_health_entity(&mut reg, 10.0, 0.0, &[]);

        let report = sweep_deaths(&mut reg);

        assert!(reg.exists(id));
        assert!(
            reg.get_component::<HealthComponent>(id)
                .unwrap()
                .death_handled
        );
        assert_eq!(report, DeathReport::default());
        let credit = reg
            .get_component::<HealthComponent>(id)
            .unwrap()
            .pending_kill_credit
            .as_ref()
            .expect("zero-HP non-player freezes credit");
        assert!(credit.tags.is_empty());
        assert!(credit.contributor_ledger.entries().is_empty());
        assert!(credit.contributor_ledger.overflow().is_none());
    }

    #[test]
    fn brain_at_zero_latches_freezes_credit_and_stops_late_ledger_recording() {
        // A brain-bearing enemy at zero HP latches its credit but reports only
        // after an authored deferred despawn actually removes it.
        let mut reg = EntityRegistry::new();
        let id = spawn_health_entity(&mut reg, 30.0, 0.0, &["grunt", "wave1"]);
        record_contributor(&mut reg, id, "weapon.before-latch", 30.0);
        make_brain(&mut reg, id);

        let first = sweep_deaths(&mut reg);
        assert!(
            reg.exists(id),
            "a brain enemy is not despawned by the sweep"
        );
        assert_eq!(first, DeathReport::default());
        let credit = reg
            .get_component::<HealthComponent>(id)
            .unwrap()
            .pending_kill_credit
            .as_ref()
            .expect("zero-HP brain freezes credit");
        assert_eq!(credit.tags, vec!["grunt".to_string(), "wave1".to_string()]);
        assert_eq!(
            credit.contributor_ledger.entries()[0].source_id,
            "weapon.before-latch"
        );
        assert!(
            reg.get_component::<HealthComponent>(id)
                .unwrap()
                .death_handled,
            "the death_handled latch must be set when the sweep freezes credit"
        );

        apply_damage_with_context(
            &mut reg,
            id,
            &DamagePayload { amount: 5.0 },
            DamageContext::new("script.after-latch", DamageProducer::InTick),
        );
        let latched_health = reg.get_component::<HealthComponent>(id).unwrap();
        assert_eq!(
            latched_health.contributor_ledger.entries().len(),
            1,
            "death-latched brain must not accumulate later damage"
        );
        assert!(
            latched_health
                .contributor_ledger
                .recorded_damage_by_source("script.after-latch")
                .is_none()
        );

        // The enemy persists at zero HP. A later sweep must NOT re-latch or
        // re-freeze credit (latch holds).
        let second = sweep_deaths(&mut reg);
        assert_eq!(
            second,
            DeathReport::default(),
            "a latched brain must not re-latch or re-freeze credit on a later sweep"
        );
        assert!(reg.exists(id), "still awaiting an authored despawn");
    }

    // Regression: a second impact before the later sweep could overwrite the
    // first-zero-HP contributor set.
    #[test]
    fn two_impacts_before_sweep_latch_only_the_lethal_hit_credit() {
        let mut reg = EntityRegistry::new();
        let id = spawn_health_entity(&mut reg, 10.0, 10.0, &["grunt"]);

        apply_damage_with_context(
            &mut reg,
            id,
            &DamagePayload { amount: 10.0 },
            DamageContext::new("weapon.lethal", DamageProducer::InTick),
        );
        apply_damage_with_context(
            &mut reg,
            id,
            &DamagePayload { amount: 25.0 },
            DamageContext::new("weapon.corpse-hit", DamageProducer::InTick),
        );

        let report = sweep_deaths(&mut reg);

        assert_eq!(report, DeathReport::default());
        let health = reg.get_component::<HealthComponent>(id).unwrap();
        assert!(health.death_handled);
        let credit = health
            .pending_kill_credit
            .as_ref()
            .expect("the sweep latches pending credit once");
        assert_eq!(credit.tags, vec!["grunt".to_string()]);
        assert_eq!(credit.contributor_ledger.entries().len(), 1);
        assert_eq!(
            credit.contributor_ledger.entries()[0].source_id,
            "weapon.lethal"
        );
        assert_eq!(credit.contributor_ledger.entries()[0].hit_count, 1);
        assert!(
            credit
                .contributor_ledger
                .recorded_damage_by_source("weapon.corpse-hit")
                .is_none()
        );

        assert_eq!(sweep_deaths(&mut reg), DeathReport::default());
        let credit_after_second_sweep = reg
            .get_component::<HealthComponent>(id)
            .unwrap()
            .pending_kill_credit
            .as_ref()
            .unwrap();
        assert_eq!(
            credit_after_second_sweep.contributor_ledger.entries().len(),
            1
        );
    }

    #[test]
    fn two_brain_kills_latch_each_credit_once_without_reports() {
        let mut reg = EntityRegistry::new();
        let a = spawn_health_entity(&mut reg, 30.0, 0.0, &["grunt"]);
        make_brain(&mut reg, a);
        let b = spawn_health_entity(&mut reg, 30.0, 0.0, &["grunt"]);
        make_brain(&mut reg, b);

        let report = sweep_deaths(&mut reg);
        assert_eq!(report, DeathReport::default());
        for id in [a, b] {
            assert_eq!(
                reg.get_component::<HealthComponent>(id)
                    .unwrap()
                    .pending_kill_credit
                    .as_ref()
                    .expect("zero-HP brain freezes credit")
                    .tags,
                vec!["grunt".to_string()]
            );
        }

        let second = sweep_deaths(&mut reg);
        assert_eq!(
            second,
            DeathReport::default(),
            "latched brain kills must not re-report on a later sweep",
        );
    }

    #[test]
    fn nonbrain_nonplayer_persists_and_latches() {
        // An entity with neither PlayerMovement nor Brain has the same one-shot
        // latch behavior as a brain and awaits an authored despawn.
        let mut reg = EntityRegistry::new();
        let id = spawn_health_entity(&mut reg, 10.0, 0.0, &["barrel"]);

        let report = sweep_deaths(&mut reg);

        assert!(reg.exists(id), "plain non-player persists at zero HP");
        assert!(
            reg.get_component::<HealthComponent>(id)
                .unwrap()
                .death_handled,
            "plain non-player latches its first zero-HP sweep",
        );
        assert_eq!(report, DeathReport::default());
        assert_eq!(
            reg.get_component::<HealthComponent>(id)
                .unwrap()
                .pending_kill_credit
                .as_ref()
                .expect("zero-HP non-player freezes credit")
                .tags,
            vec!["barrel".to_string()]
        );
    }

    #[test]
    fn non_finite_hp_is_treated_as_dead_by_the_sweep() {
        // Defensive: a corrupt NaN `current` (`NaN <= 0.0` is false on its own)
        // must not leave an entity immortal — the sweep's finiteness guard
        // collects it as dead.
        let mut reg = EntityRegistry::new();
        let id = spawn_health_entity(&mut reg, 10.0, f32::NAN, &["barrel"]);

        let report = sweep_deaths(&mut reg);

        assert!(reg.exists(id), "a NaN-HP entity is latched but not removed");
        assert!(
            reg.get_component::<HealthComponent>(id)
                .unwrap()
                .death_handled
        );
        assert_eq!(report, DeathReport::default());
    }
}

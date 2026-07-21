// Per-tick death sweep: latches zero-HP entities after damage settles.
// Zero HP alone never removes an entity; the player pawn
// latches at zero and reports `playerDied` exactly once. Component-only state
// (HP, `death_handled`) lives in `components/health.rs`; this is the system
// half of that split, mirroring the components/systems separation of
// `particle_sim`.
//
// Brain-bearing enemies are a THIRD case, layered between player and plain
// non-player: at zero HP they latch (reusing `HealthComponent.death_handled`)
// and report the kill exactly ONCE — at the false→true latch transition — but
// are NOT despawned here. An authored deferred `despawn` owns eventual removal.
//
// See: context/lib/entity_model.md §3 (Destruction)

use postretro_entities::components::health::{
    ContributorLedgerEntry, ContributorLedgerOverflow, HealthComponent,
};
use postretro_entities::registry::{ComponentKind, ComponentValue, EntityId, EntityRegistry};

/// Event name fired once when the player pawn's HP reaches zero. Latched by
/// `HealthComponent::death_handled` so a persisting zero-HP pawn never re-fires.
pub(crate) const PLAYER_DIED_EVENT: &str = "playerDied";

/// What one death sweep observed, returned to the caller because the sweep
/// cannot reach the progress tracker or the event-dispatch path itself. The
/// caller feeds `killed_tags` through `ProgressTracker::on_entity_killed` and
/// fires the resulting events (plus `PLAYER_DIED_EVENT` when `player_died`) via
/// the death-event drain. Owned data is captured at the latch, so no `EntityId`
/// crosses the boundary.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct DeathReport {
    /// Tags of every entity KILLED this sweep, one entry per killed entity (tag
    /// lists are not deduplicated across entities). Non-player tags are
    /// captured at their kill latch while the entity persists, so the kill is
    /// counted by the progress tracker exactly once. Empty when no non-player
    /// died.
    pub(crate) killed_tags: Vec<Vec<String>>,
    /// Contributor ledgers captured for the same killed entities as
    /// `killed_tags`. Index-aligned: `killed_contributor_ledgers[i]` describes
    /// the entity whose tags are in `killed_tags[i]`.
    pub(crate) killed_contributor_ledgers: Vec<ContributorLedgerSnapshot>,
    /// Set once on the tick the player pawn first reaches zero HP. The
    /// `death_handled` latch guarantees later sweeps leave this `false`.
    pub(crate) player_died: bool,
    /// Contributor ledger captured when the player death latch flips. Present
    /// exactly when `player_died` is true.
    pub(crate) player_contributor_ledger: Option<ContributorLedgerSnapshot>,
}

/// Owned clone of a target's contributor ledger at the instant death is
/// reported. This is fact data only; later combat-event/reward policy consumes
/// it outside this sweep.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct ContributorLedgerSnapshot {
    pub(crate) entries: Vec<ContributorLedgerEntry>,
    pub(crate) overflow: Option<ContributorLedgerOverflow>,
}

impl ContributorLedgerSnapshot {
    fn from_health(health: &HealthComponent) -> Self {
        Self {
            entries: health.contributor_ledger.entries().to_vec(),
            overflow: health.contributor_ledger.overflow().cloned(),
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
///   `death_handled` is already set, skip. Otherwise set the latch and capture
///   its tags into `killed_tags`.
/// - **Plain non-player** (neither `PlayerMovement` nor `Brain`): follows the
///   same latch-and-capture behavior. Neither branch removes the entity; an
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

        // A brain-bearing enemy latches and reports the kill once here. It
        // persists until an authored deferred despawn removes it.
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
            let ledger = ContributorLedgerSnapshot::from_health(&updated);
            let _ = registry.set_component(id, updated);
            // Capture tags for the progress tracker; the entity lives on, so the
            // tags are still readable next sweep but the latch suppresses a
            // re-report.
            let tags = registry
                .get_tags(id)
                .map(|t| t.to_vec())
                .unwrap_or_default();
            report.killed_tags.push(tags);
            report.killed_contributor_ledgers.push(ledger);
            continue;
        }

        // Plain non-player: just like a brain, latch before capturing. Unlike
        // the former immediate-despawn path, it persists at zero HP, so the
        // latch suppresses repeated reports on later sweeps.
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
        let tags = registry
            .get_tags(id)
            .map(|t| t.to_vec())
            .unwrap_or_default();
        report.killed_tags.push(tags);
        report.killed_contributor_ledgers.push(ledger);
    }

    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_entities::components::brain::attach_brain;
    use postretro_entities::components::health::{
        ContributorLedgerRecord, DamageContext, DamageProducer, apply_damage_with_context,
    };
    use postretro_entities::components::player_movement::PlayerMovementComponent;
    use postretro_entities::registry::Transform;
    use postretro_foundation::DamagePayload;
    use postretro_scripting_core::data_descriptors::{
        AiDescriptor, AiStateNames, AirParams, CapsuleParams, FallParams, GroundParams,
        HealthDescriptor, PlayerMovementDescriptor, SpeedParams,
    };

    /// A minimal valid AI descriptor so a brain can be attached to mark an
    /// entity as brain-bearing for the sweep's branch. The tuning values are not
    /// read by the sweep — only the Brain component's *presence* matters.
    fn ai_descriptor() -> AiDescriptor {
        AiDescriptor {
            detection_range: 18.0,
            attack_range: 2.0,
            leash_range: 26.0,
            attack_damage: 8.0,
            attack_cooldown_ms: 1000.0,
            move_speed: 3.5,
            death_despawn_ms: 1500.0,
            states: AiStateNames {
                idle: "idle".into(),
                alert: "walk".into(),
                attack: "attack".into(),
                death: "die".into(),
            },
        }
    }

    /// Attach a Brain component, marking the entity as brain-bearing for the
    /// sweep. Mirrors `make_player`: the sweep branches on component *presence*.
    fn make_brain(registry: &mut EntityRegistry, id: EntityId) {
        attach_brain(registry, id, &ai_descriptor()).unwrap();
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
            view_feel: None,
        };
        registry
            .set_component(id, PlayerMovementComponent::from_descriptor(&descriptor))
            .unwrap();
    }

    #[test]
    fn nonplayer_at_zero_persists_latches_and_tags_reported() {
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
            report.killed_tags,
            vec![vec!["reactorMonster".to_string(), "wave1".to_string()]]
        );
        assert_eq!(
            report.killed_contributor_ledgers.len(),
            report.killed_tags.len(),
            "killed ledger snapshots must be index-aligned with killed tags"
        );
        assert_eq!(report.killed_contributor_ledgers[0].entries.len(), 1);
        assert_eq!(
            report.killed_contributor_ledgers[0].entries[0].source_id,
            "weapon.test"
        );
        assert_eq!(
            report.killed_contributor_ledgers[0].entries[0].accumulated_damage,
            50.0
        );
        assert!(!report.player_died);
        assert!(report.player_contributor_ledger.is_none());

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
            first.killed_tags.is_empty(),
            "the player is not a kill — no tags reported"
        );
        assert!(
            first.killed_contributor_ledgers.is_empty(),
            "the player is not a kill — no killed ledger reported"
        );
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
    fn multiple_dead_nonplayers_all_reported() {
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
        assert_eq!(report.killed_tags.len(), 2, "both dead entities reported");
        assert_eq!(
            report.killed_contributor_ledgers.len(),
            report.killed_tags.len(),
            "each killed tag set has an index-aligned ledger snapshot"
        );
        assert!(report.killed_tags.contains(&vec!["a".to_string()]));
        assert!(report.killed_tags.contains(&vec!["b".to_string()]));
        for (tags, ledger) in report
            .killed_tags
            .iter()
            .zip(report.killed_contributor_ledgers.iter())
        {
            let entry = ledger.entries.first().expect("ledger entry");
            match tags.as_slice() {
                [tag] if tag == "a" => assert_eq!(entry.source_id, "source.a"),
                [tag] if tag == "b" => assert_eq!(entry.source_id, "source.b"),
                _ => panic!("unexpected killed tags: {tags:?}"),
            }
        }
    }

    #[test]
    fn untagged_dead_nonplayer_reports_empty_tag_list() {
        let mut reg = EntityRegistry::new();
        let id = spawn_health_entity(&mut reg, 10.0, 0.0, &[]);

        let report = sweep_deaths(&mut reg);

        assert!(reg.exists(id));
        assert!(
            reg.get_component::<HealthComponent>(id)
                .unwrap()
                .death_handled
        );
        assert_eq!(report.killed_tags, vec![Vec::<String>::new()]);
        assert_eq!(report.killed_contributor_ledgers.len(), 1);
        assert!(report.killed_contributor_ledgers[0].entries.is_empty());
        assert!(report.killed_contributor_ledgers[0].overflow.is_none());
    }

    #[test]
    fn brain_at_zero_latches_reports_kill_once_and_stops_late_ledger_recording() {
        // A brain-bearing enemy at zero HP is latched and its tags reported ONCE
        // (so progress counts it), but it persists until an authored despawn.
        let mut reg = EntityRegistry::new();
        let id = spawn_health_entity(&mut reg, 30.0, 0.0, &["grunt", "wave1"]);
        record_contributor(&mut reg, id, "weapon.before-latch", 30.0);
        make_brain(&mut reg, id);

        let first = sweep_deaths(&mut reg);
        assert!(
            reg.exists(id),
            "a brain enemy is not despawned by the sweep"
        );
        assert_eq!(
            first.killed_tags,
            vec![vec!["grunt".to_string(), "wave1".to_string()]],
            "the kill's tags flow to the progress tracker exactly once"
        );
        assert_eq!(first.killed_contributor_ledgers.len(), 1);
        assert_eq!(
            first.killed_contributor_ledgers[0].entries[0].source_id,
            "weapon.before-latch"
        );
        assert!(!first.player_died);
        assert!(
            reg.get_component::<HealthComponent>(id)
                .unwrap()
                .death_handled,
            "the death_handled latch must be set after reporting"
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

        // The enemy persists at zero HP. A later sweep must NOT re-count or
        // re-report the kill (latch holds).
        let second = sweep_deaths(&mut reg);
        assert_eq!(
            second,
            DeathReport::default(),
            "a latched brain kill must not re-report on a later sweep"
        );
        assert!(reg.exists(id), "still awaiting an authored despawn");
    }

    #[test]
    fn two_brain_kills_report_each_tag_set_once() {
        // Two brain enemies latch this sweep → both tag sets flow to the progress
        // tracker once. A second sweep reports NONE — the latch guarantees the
        // single report per kill.
        let mut reg = EntityRegistry::new();
        let a = spawn_health_entity(&mut reg, 30.0, 0.0, &["grunt"]);
        make_brain(&mut reg, a);
        let b = spawn_health_entity(&mut reg, 30.0, 0.0, &["grunt"]);
        make_brain(&mut reg, b);

        let report = sweep_deaths(&mut reg);
        assert_eq!(
            report.killed_tags,
            vec![vec!["grunt".to_string()], vec!["grunt".to_string()]],
            "each brain kill's tags flow to the progress tracker exactly once",
        );

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
        assert_eq!(report.killed_tags, vec![vec!["barrel".to_string()]]);
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
        assert_eq!(report.killed_tags, vec![vec!["barrel".to_string()]]);
    }
}

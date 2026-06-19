// Per-tick death sweep: resolves zero-HP entities after damage settles.
// Non-players at zero HP despawn (and report their tags); the player pawn
// latches at zero and reports `playerDied` exactly once. Component-only state
// (HP, `death_handled`) lives in `components/health.rs`; this is the system
// half of that split, mirroring the components/systems separation of
// `particle_sim`.
//
// Brain-bearing enemies are a THIRD case, layered between player and plain
// non-player: at zero HP they latch (reusing `HealthComponent.death_handled`)
// and report the kill exactly ONCE — at the false→true latch transition — but
// are NOT despawned here. The AI tick (`systems/ai.rs`) owns the brain death:
// it plays the death animation on entering Death and despawns the entity after
// `death_despawn_ms`. This is the single authoritative kill latch for a brain;
// the FSM never re-reports the kill, so a brain death is counted exactly once.
//
// See: context/lib/entity_model.md §3 (Destruction)

use crate::scripting::components::health::HealthComponent;
use crate::scripting::registry::{ComponentKind, ComponentValue, EntityId, EntityRegistry};

/// Event name fired once when the player pawn's HP reaches zero. Latched by
/// `HealthComponent::death_handled` so a persisting zero-HP pawn never re-fires.
pub(crate) const PLAYER_DIED_EVENT: &str = "playerDied";

/// What one death sweep observed, returned to the caller because the sweep
/// cannot reach the progress tracker or the event-dispatch path itself. The
/// caller feeds `killed_tags` through `ProgressTracker::on_entity_killed` and
/// fires the resulting events (plus `PLAYER_DIED_EVENT` when `player_died`) via
/// the death-event drain. Owned data: every reported entity is despawned (or
/// latched) inside the sweep, so no `EntityId` crosses the boundary.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct DeathReport {
    /// Tags of every entity KILLED this sweep, one entry per killed entity (tag
    /// lists are not deduplicated across entities). A non-brain non-player is
    /// despawned in this sweep and its tags captured first; a brain-bearing
    /// enemy's tags are captured at its kill latch (it is NOT despawned here —
    /// the AI tick despawns it later) so the kill is counted by the progress
    /// tracker exactly once. Empty when no non-player died.
    pub(crate) killed_tags: Vec<Vec<String>>,
    /// Set once on the tick the player pawn first reaches zero HP. The
    /// `death_handled` latch guarantees later sweeps leave this `false`.
    pub(crate) player_died: bool,
}

/// Resolve every entity at zero HP. Two-pass like `particle_sim`: collect the
/// dead ids under an immutable borrow, then mutate (despawn / latch) so the
/// registry is never written mid-walk.
///
/// - **Player** (carries `PlayerMovement`): never despawn. If `death_handled`
///   is already set, skip entirely (the one-shot latch). Otherwise set the
///   latch and report `player_died`.
/// - **Brain enemy** (carries `Brain`, not `PlayerMovement`): never despawn
///   here. If `death_handled` is already set, skip (the one-shot latch holds
///   while the AI tick counts down to despawn). Otherwise set the latch and
///   capture its tags into `killed_tags` — the single authoritative kill report.
///   The AI tick owns the eventual despawn.
/// - **Plain non-player** (neither `PlayerMovement` nor `Brain`): capture its
///   tags, despawn it immediately, and record the tags in `killed_tags`.
///
/// Frame ordering: runs in the game-logic stage after the weapon fire tick, so
/// damage applied this frame is resolved before render reads entity state.
pub(crate) fn sweep_deaths(registry: &mut EntityRegistry) -> DeathReport {
    // Pass 1: collect ids at zero HP under the immutable iterator borrow, which
    // must be dropped before the despawn/latch writes below.
    let mut dead: Vec<EntityId> = Vec::new();
    for (id, value) in registry.iter_with_kind(ComponentKind::Health) {
        let ComponentValue::Health(health) = value else {
            continue;
        };
        // `<= 0.0`, not `== 0.0`: `apply_damage` floors HP at exactly `0.0` (the
        // sole damage chokepoint), so today HP never goes negative. The `<=`
        // defends against any future direct write that could; the AI tick's death
        // check (`ai.rs`) also keys on `<= 0.0`, so the kill-latch and despawn
        // agree on "dead".
        if health.current <= 0.0 {
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
            let _ = registry.set_component(id, updated);
            report.player_died = true;
            continue;
        }

        // A brain-bearing enemy is the single authoritative kill latch: it
        // latches and reports the kill once here, but is NOT despawned by the
        // sweep — the AI tick despawns it after `death_despawn_ms`. The FSM
        // never re-reports the kill, so a brain death is counted exactly once.
        let is_brain = registry
            .has_component_kind(id, ComponentKind::Brain)
            .unwrap_or(false);
        if is_brain {
            let Ok(health) = registry.get_component::<HealthComponent>(id) else {
                continue;
            };
            if health.death_handled {
                // Latch holds: the enemy persists at zero HP while the AI tick
                // counts down to despawn. Do not re-count or re-despawn.
                continue;
            }
            let mut updated = health.clone();
            updated.death_handled = true;
            let _ = registry.set_component(id, updated);
            // Capture tags for the progress tracker; the entity lives on, so the
            // tags are still readable next sweep but the latch suppresses a
            // re-report.
            let tags = registry
                .get_tags(id)
                .map(|t| t.to_vec())
                .unwrap_or_default();
            report.killed_tags.push(tags);
            continue;
        }

        // Plain non-player: capture tags before despawn clears them. Stale-id
        // reads default to no tags rather than aborting the sweep.
        let tags = registry
            .get_tags(id)
            .map(|t| t.to_vec())
            .unwrap_or_default();
        let _ = registry.despawn(id);
        report.killed_tags.push(tags);
    }

    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::components::brain::attach_brain;
    use crate::scripting::components::player_movement::PlayerMovementComponent;
    use crate::scripting::data_descriptors::{
        AiDescriptor, AiStateNames, AirParams, CapsuleParams, FallParams, GroundParams,
        HealthDescriptor, PlayerMovementDescriptor, SpeedParams,
    };
    use crate::scripting::registry::Transform;

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
    fn nonplayer_at_zero_is_despawned_and_tags_reported() {
        let mut reg = EntityRegistry::new();
        let id = spawn_health_entity(&mut reg, 50.0, 0.0, &["reactorMonster", "wave1"]);

        let report = sweep_deaths(&mut reg);

        assert!(!reg.exists(id), "dead non-player must be despawned");
        assert_eq!(
            report.killed_tags,
            vec![vec!["reactorMonster".to_string(), "wave1".to_string()]]
        );
        assert!(!report.player_died);
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
        make_player(&mut reg, id);

        let first = sweep_deaths(&mut reg);
        assert!(reg.exists(id), "player pawn must never despawn from damage");
        assert!(first.player_died, "first zero-HP sweep reports playerDied");
        assert!(
            first.killed_tags.is_empty(),
            "the player is not a kill — no tags reported"
        );
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
        // A survivor to prove the sweep is selective.
        let alive = spawn_health_entity(&mut reg, 10.0, 5.0, &["c"]);

        let report = sweep_deaths(&mut reg);

        assert!(!reg.exists(a));
        assert!(!reg.exists(b));
        assert!(reg.exists(alive));
        assert!(!report.player_died);
        assert_eq!(report.killed_tags.len(), 2, "both dead entities reported");
        assert!(report.killed_tags.contains(&vec!["a".to_string()]));
        assert!(report.killed_tags.contains(&vec!["b".to_string()]));
    }

    #[test]
    fn untagged_dead_nonplayer_reports_empty_tag_list() {
        let mut reg = EntityRegistry::new();
        let id = spawn_health_entity(&mut reg, 10.0, 0.0, &[]);

        let report = sweep_deaths(&mut reg);

        assert!(!reg.exists(id));
        assert_eq!(report.killed_tags, vec![Vec::<String>::new()]);
    }

    #[test]
    fn brain_at_zero_latches_reports_kill_once_and_is_not_despawned() {
        // A brain-bearing enemy at zero HP is the single authoritative kill
        // latch: latched and tags reported ONCE (so progress counts it), but
        // NOT despawned — the AI tick owns the despawn.
        let mut reg = EntityRegistry::new();
        let id = spawn_health_entity(&mut reg, 30.0, 0.0, &["grunt", "wave1"]);
        make_brain(&mut reg, id);

        let first = sweep_deaths(&mut reg);
        assert!(
            reg.exists(id),
            "a brain enemy is not despawned by the sweep; the AI tick owns that"
        );
        assert_eq!(
            first.killed_tags,
            vec![vec!["grunt".to_string(), "wave1".to_string()]],
            "the kill's tags flow to the progress tracker exactly once"
        );
        assert!(!first.player_died);
        assert!(
            reg.get_component::<HealthComponent>(id)
                .unwrap()
                .death_handled,
            "the death_handled latch must be set after reporting"
        );

        // The enemy persists at zero HP (awaiting the AI tick's despawn). A
        // later sweep must NOT re-count or re-report the kill (latch holds).
        let second = sweep_deaths(&mut reg);
        assert_eq!(
            second,
            DeathReport::default(),
            "a latched brain kill must not re-report on a later sweep"
        );
        assert!(reg.exists(id), "still awaiting the AI tick's despawn");
    }

    #[test]
    fn two_brain_kills_report_each_tag_set_once() {
        // Two brain enemies die this sweep → both tag sets flow to the progress
        // tracker once. A second sweep (both still awaiting AI-tick despawn)
        // reports NONE — the latch guarantees the single report per kill.
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
    fn nonbrain_nonplayer_still_despawns_immediately() {
        // Regression guard: an entity with neither PlayerMovement nor Brain at
        // zero HP keeps the original immediate-despawn behavior.
        let mut reg = EntityRegistry::new();
        let id = spawn_health_entity(&mut reg, 10.0, 0.0, &["barrel"]);

        let report = sweep_deaths(&mut reg);

        assert!(!reg.exists(id), "plain non-player despawns in the sweep");
        assert_eq!(report.killed_tags, vec![vec!["barrel".to_string()]]);
    }
}

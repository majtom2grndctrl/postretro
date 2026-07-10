//! Host-authoritative fixed-tick evaluation for declarative trigger volumes.
//!
//! The system deliberately consumes only explicit player snapshots and per-player
//! Use edges. Input ownership and remote-input plumbing remain outside this module.

use std::collections::HashMap;

use glam::Vec3;
use postretro_entities::{
    ComponentKind, EntityId, EntityRegistry, Transform, TriggerActivation, TriggerFireMode,
    TriggerVolumeComponent,
};
use postretro_foundation::PlayerMovementComponent;

use crate::kinematic_mover::apply_mover_command_to_targets;
use crate::scripting_systems::trigger_volume_bridge::TriggerVolumeBridge;

/// Identity passed to trigger activation without assigning trigger-ownership
/// policy. E18 may use this distinction when it adds co-op ownership rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum PlayerId {
    Local(EntityId),
    Remote(u64),
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct AuthoritativePlayer {
    pub(crate) id: PlayerId,
    pub(crate) pawn: EntityId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TriggerActivationDecision {
    Fire,
    Suppress,
}

/// Per-level trigger evaluator state. The overlap table is keyed by both
/// trigger and player so one player's entry never consumes another's edge.
#[derive(Debug, Default)]
pub(crate) struct TriggerSystem {
    prior_overlap: HashMap<(EntityId, PlayerId), bool>,
}

impl TriggerSystem {
    pub(crate) fn clear(&mut self) {
        self.prior_overlap.clear();
    }

    /// Run after player movement and before AI. This function is called only by
    /// the host/single-player simulation path; clients receive mover phase over
    /// replication and never evaluate or apply trigger commands locally.
    pub(crate) fn run_authoritative_tick(
        &mut self,
        registry: &mut EntityRegistry,
        bridge: &TriggerVolumeBridge,
        players: &[AuthoritativePlayer],
        use_pressed: &HashMap<PlayerId, bool>,
        tick_dt: f32,
    ) {
        let player_capsules: Vec<(PlayerId, Vec3, f32, f32)> = players
            .iter()
            .filter_map(|player| {
                let transform = registry.get_component::<Transform>(player.pawn).ok()?;
                let movement = registry
                    .get_component::<PlayerMovementComponent>(player.pawn)
                    .ok()?;
                Some((
                    player.id,
                    transform.position,
                    movement.capsule.radius,
                    movement.capsule.half_height,
                ))
            })
            .collect();

        let trigger_ids: Vec<EntityId> = registry
            .iter_with_kind(ComponentKind::TriggerVolume)
            .map(|(id, _)| id)
            .collect();

        for trigger_id in trigger_ids {
            let Some((aabb_min, aabb_max)) = bridge.aabb(trigger_id) else {
                continue;
            };
            let Ok(mut trigger) = registry
                .get_component::<TriggerVolumeComponent>(trigger_id)
                .cloned()
            else {
                continue;
            };

            decrement_rearm(&mut trigger, tick_dt);
            for &(player_id, center, radius, half_height) in &player_capsules {
                let overlapping =
                    capsule_overlaps_aabb(center, radius, half_height, aabb_min, aabb_max);
                let was_overlapping = self
                    .prior_overlap
                    .insert((trigger_id, player_id), overlapping)
                    .unwrap_or(false);
                let activated = match trigger.activation {
                    TriggerActivation::Touch => overlapping && !was_overlapping,
                    TriggerActivation::Use => {
                        overlapping && use_pressed.get(&player_id).copied().unwrap_or(false)
                    }
                };
                if !activated {
                    continue;
                }

                if evaluate_trigger_activation(&trigger, player_id)
                    != TriggerActivationDecision::Fire
                {
                    continue;
                }

                let targets: Vec<EntityId> = registry
                    .query_by_component_and_tag(
                        ComponentKind::KinematicMover,
                        Some(&trigger.target_tag),
                    )
                    .map(|(id, _)| id)
                    .collect();
                apply_mover_command_to_targets(registry, &targets, &trigger.command);
                update_after_fire(&mut trigger);
            }

            // Store countdown progress even when no activation occurred. The
            // explicit write also makes disabled triggers persist as inert state.
            let _ = registry.set_component(trigger_id, trigger);
        }
    }
}

/// The sole activation decision point for touch and use routes. It intentionally
/// knows nothing about ownership policy; E18 extends this seam rather than adding
/// a second firing path.
fn evaluate_trigger_activation(
    state: &TriggerVolumeComponent,
    #[allow(unused_variables)] activator: PlayerId,
) -> TriggerActivationDecision {
    #[cfg(feature = "dev-tools")]
    log::debug!("[Trigger] activation candidate from {activator:?}");

    let fire = state.armed
        && !matches!(state.fire_mode, TriggerFireMode::Once if state.latched)
        && state.rearm_remaining_ms <= 0.0;
    if fire {
        #[cfg(test)]
        record_gate_fire(activator);
        TriggerActivationDecision::Fire
    } else {
        TriggerActivationDecision::Suppress
    }
}

fn decrement_rearm(trigger: &mut TriggerVolumeComponent, tick_dt: f32) {
    if trigger.rearm_remaining_ms > 0.0 && tick_dt.is_finite() && tick_dt > 0.0 {
        trigger.rearm_remaining_ms = (trigger.rearm_remaining_ms - tick_dt * 1000.0).max(0.0);
    }
}

fn update_after_fire(trigger: &mut TriggerVolumeComponent) {
    match trigger.fire_mode {
        TriggerFireMode::Once => trigger.latched = true,
        TriggerFireMode::Multiple => trigger.rearm_remaining_ms = trigger.rearm_ms.max(0.0),
    }
}

/// Exact overlap for an upright capsule and an AABB. The capsule centerline is
/// vertical, so the closest-point calculation decomposes into X/Z point-to-range
/// distance and Y segment-to-range distance.
fn capsule_overlaps_aabb(
    center: Vec3,
    radius: f32,
    half_height: f32,
    min: Vec3,
    max: Vec3,
) -> bool {
    if !center.is_finite() || !radius.is_finite() || !half_height.is_finite() || radius < 0.0 {
        return false;
    }
    let axis_min_y = center.y - half_height.max(0.0);
    let axis_max_y = center.y + half_height.max(0.0);
    let dx = range_distance(center.x, min.x, max.x);
    let dz = range_distance(center.z, min.z, max.z);
    let dy = segment_range_distance(axis_min_y, axis_max_y, min.y, max.y);
    dx * dx + dy * dy + dz * dz <= radius * radius
}

fn range_distance(value: f32, min: f32, max: f32) -> f32 {
    if value < min {
        min - value
    } else if value > max {
        value - max
    } else {
        0.0
    }
}

fn segment_range_distance(
    segment_min: f32,
    segment_max: f32,
    range_min: f32,
    range_max: f32,
) -> f32 {
    if segment_max < range_min {
        range_min - segment_max
    } else if segment_min > range_max {
        segment_min - range_max
    } else {
        0.0
    }
}

#[cfg(test)]
use std::sync::{Mutex, OnceLock};

#[cfg(test)]
fn gate_fires() -> &'static Mutex<Vec<PlayerId>> {
    static FIRES: OnceLock<Mutex<Vec<PlayerId>>> = OnceLock::new();
    FIRES.get_or_init(|| Mutex::new(Vec::new()))
}

#[cfg(test)]
fn gate_test_guard() -> &'static Mutex<()> {
    static GUARD: OnceLock<Mutex<()>> = OnceLock::new();
    GUARD.get_or_init(|| Mutex::new(()))
}

#[cfg(test)]
fn record_gate_fire(activator: PlayerId) {
    gate_fires()
        .lock()
        .expect("gate fire recorder poisoned")
        .push(activator);
}

#[cfg(test)]
fn reset_gate_fires() {
    gate_fires()
        .lock()
        .expect("gate fire recorder poisoned")
        .clear();
}

#[cfg(test)]
fn recorded_gate_fires() -> Vec<PlayerId> {
    gate_fires()
        .lock()
        .expect("gate fire recorder poisoned")
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Quat;
    use postretro_entities::{KinematicMoverComponent, KinematicMoverMode, MoverCommand};
    use postretro_foundation::{
        AirParams, CapsuleParams, FallParams, GroundParams, PlayerMovementComponent,
        PlayerMovementDescriptor, SpeedParams,
    };

    const DT: f32 = 0.05;

    fn movement() -> PlayerMovementComponent {
        PlayerMovementComponent::from_descriptor(&PlayerMovementDescriptor {
            capsule: CapsuleParams {
                radius: 0.4,
                half_height: 0.8,
                eye_height: 0.5,
            },
            ground: GroundParams {
                speed: SpeedParams {
                    walk: 7.0,
                    run: 11.0,
                    crouch: 3.0,
                },
                accel: 10.0,
                step_height: 0.3,
                max_slope: 45.0,
            },
            air: AirParams {
                forward_steer: 0.0,
                accel: 0.7,
                max_control_speed: 0.5,
                bunny_hop: false,
                jumps: 0,
                jump_velocity: 5.5,
                jump_ceiling: 0.0,
            },
            fall: FallParams {
                terminal_velocity: 40.0,
            },
            stuck_stop_enabled: PlayerMovementDescriptor::DEFAULT_STUCK_STOP_ENABLED,
            stuck_stop_threshold: PlayerMovementDescriptor::DEFAULT_STUCK_STOP_THRESHOLD,
            dash: None,
            forgiveness: None,
            crouch: None,
            view_feel: None,
        })
    }

    fn spawn_player(registry: &mut EntityRegistry, position: Vec3) -> EntityId {
        let id = registry.spawn(Transform {
            position,
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
        });
        registry.set_component(id, movement()).unwrap();
        id
    }

    fn spawn_mover(registry: &mut EntityRegistry) -> EntityId {
        let id = registry.spawn(Transform::default());
        registry.set_tags(id, vec!["lift".into()]).unwrap();
        registry
            .set_component(
                id,
                KinematicMoverComponent::new(
                    1,
                    vec![Vec3::ZERO, Vec3::new(2.0, 0.0, 0.0)],
                    vec!["start".into(), "end".into()],
                    1.0,
                    0.0,
                    KinematicMoverMode::Once,
                    false,
                ),
            )
            .unwrap();
        id
    }

    fn spawn_trigger(
        registry: &mut EntityRegistry,
        bridge: &mut TriggerVolumeBridge,
        activation: TriggerActivation,
        fire_mode: TriggerFireMode,
        rearm_ms: f32,
        enabled: bool,
    ) -> EntityId {
        let id = registry.spawn(Transform::default());
        registry
            .set_component(
                id,
                TriggerVolumeComponent::new(
                    activation,
                    "lift".into(),
                    MoverCommand::Start,
                    fire_mode,
                    rearm_ms,
                    enabled,
                ),
            )
            .unwrap();
        bridge.insert_for_test(id, Vec3::new(-1.0, 0.0, -1.0), Vec3::new(1.0, 2.0, 1.0));
        id
    }

    fn tick(
        system: &mut TriggerSystem,
        registry: &mut EntityRegistry,
        bridge: &TriggerVolumeBridge,
        players: &[AuthoritativePlayer],
        uses: &[(PlayerId, bool)],
    ) {
        let uses = uses.iter().copied().collect();
        system.run_authoritative_tick(registry, bridge, players, &uses, DT);
    }

    #[test]
    fn touch_tracks_each_players_rising_entry_and_once_latches() {
        let _guard = gate_test_guard().lock().expect("gate test guard poisoned");
        reset_gate_fires();
        let mut registry = EntityRegistry::new();
        let mut bridge = TriggerVolumeBridge::new();
        let trigger = spawn_trigger(
            &mut registry,
            &mut bridge,
            TriggerActivation::Touch,
            TriggerFireMode::Once,
            0.0,
            true,
        );
        let first = spawn_player(&mut registry, Vec3::new(4.0, 1.0, 0.0));
        let second = spawn_player(&mut registry, Vec3::new(4.0, 1.0, 0.0));
        let players = [
            AuthoritativePlayer {
                id: PlayerId::Local(first),
                pawn: first,
            },
            AuthoritativePlayer {
                id: PlayerId::Remote(7),
                pawn: second,
            },
        ];
        let mut system = TriggerSystem::default();

        tick(&mut system, &mut registry, &bridge, &players, &[]);
        registry.get_component::<Transform>(first).unwrap();
        let mut entered = *registry.get_component::<Transform>(first).unwrap();
        entered.position = Vec3::new(0.0, 1.0, 0.0);
        registry.set_component(first, entered).unwrap();
        tick(&mut system, &mut registry, &bridge, &players, &[]);
        let mut second_entered = *registry.get_component::<Transform>(second).unwrap();
        second_entered.position = Vec3::new(0.0, 1.0, 0.0);
        registry.set_component(second, second_entered).unwrap();
        tick(&mut system, &mut registry, &bridge, &players, &[]);

        assert!(
            registry
                .get_component::<TriggerVolumeComponent>(trigger)
                .unwrap()
                .latched
        );
        assert_eq!(recorded_gate_fires(), vec![PlayerId::Local(first)]);
    }

    #[test]
    fn multiple_rearms_disabled_is_inert_and_use_needs_same_tick_edge() {
        let _guard = gate_test_guard().lock().expect("gate test guard poisoned");
        reset_gate_fires();
        let mut registry = EntityRegistry::new();
        let mut bridge = TriggerVolumeBridge::new();
        let multiple = spawn_trigger(
            &mut registry,
            &mut bridge,
            TriggerActivation::Touch,
            TriggerFireMode::Multiple,
            100.0,
            true,
        );
        let disabled = spawn_trigger(
            &mut registry,
            &mut bridge,
            TriggerActivation::Touch,
            TriggerFireMode::Once,
            0.0,
            false,
        );
        let use_trigger = spawn_trigger(
            &mut registry,
            &mut bridge,
            TriggerActivation::Use,
            TriggerFireMode::Multiple,
            0.0,
            true,
        );
        let player = spawn_player(&mut registry, Vec3::new(0.0, 1.0, 0.0));
        let id = PlayerId::Local(player);
        let players = [AuthoritativePlayer { id, pawn: player }];
        let mut system = TriggerSystem::default();

        tick(&mut system, &mut registry, &bridge, &players, &[]);
        tick(&mut system, &mut registry, &bridge, &players, &[(id, true)]);
        tick(&mut system, &mut registry, &bridge, &players, &[]);
        let mut out = *registry.get_component::<Transform>(player).unwrap();
        out.position.x = 4.0;
        registry.set_component(player, out).unwrap();
        tick(&mut system, &mut registry, &bridge, &players, &[]);
        let mut back = *registry.get_component::<Transform>(player).unwrap();
        back.position.x = 0.0;
        registry.set_component(player, back).unwrap();
        tick(&mut system, &mut registry, &bridge, &players, &[]);

        assert!(
            registry
                .get_component::<TriggerVolumeComponent>(multiple)
                .unwrap()
                .rearm_remaining_ms
                > 0.0
        );
        assert!(
            !registry
                .get_component::<TriggerVolumeComponent>(disabled)
                .unwrap()
                .latched
        );
        // Touch fires on entry, use fires only on its explicit edge, then touch
        // fires again only after the 100 ms rearm interval has elapsed.
        assert_eq!(recorded_gate_fires(), vec![id, id, id]);
        assert!(
            !registry
                .get_component::<TriggerVolumeComponent>(use_trigger)
                .unwrap()
                .latched
        );
    }

    #[test]
    fn trigger_command_starts_targeted_mover_and_gate_is_sole_fire_path() {
        let _guard = gate_test_guard().lock().expect("gate test guard poisoned");
        reset_gate_fires();
        let mut registry = EntityRegistry::new();
        let mut bridge = TriggerVolumeBridge::new();
        spawn_trigger(
            &mut registry,
            &mut bridge,
            TriggerActivation::Use,
            TriggerFireMode::Once,
            0.0,
            true,
        );
        let mover = spawn_mover(&mut registry);
        let player = spawn_player(&mut registry, Vec3::new(0.0, 1.0, 0.0));
        let id = PlayerId::Local(player);
        let players = [AuthoritativePlayer { id, pawn: player }];
        let mut system = TriggerSystem::default();

        tick(&mut system, &mut registry, &bridge, &players, &[(id, true)]);
        let mover_state = registry
            .get_component::<KinematicMoverComponent>(mover)
            .unwrap();
        assert!(
            mover_state.started,
            "trigger command must mutate the mover phase"
        );
        assert_eq!(
            recorded_gate_fires(),
            vec![id],
            "only the gate records fires and receives its activator"
        );
        let mut mover_ticks = crate::kinematic_mover::MoverTickStateTable::default();
        crate::kinematic_mover::run_kinematic_mover_tick(&mut registry, &mut mover_ticks, DT);
        assert!(
            registry
                .get_component::<Transform>(mover)
                .unwrap()
                .position
                .x
                > 0.0,
            "the trigger command must produce observed mover motion on the next fixed tick"
        );
    }
}

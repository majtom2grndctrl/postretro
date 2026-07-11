//! Host-authoritative fixed-tick evaluation for declarative trigger volumes.
//!
//! The system deliberately consumes only explicit player snapshots and per-player
//! Use edges. Input ownership and remote-input plumbing remain outside this module.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

use glam::Vec3;
use postretro_entities::{
    ComponentKind, EntityId, EntityRegistry, Transform, TriggerActivation, TriggerFireMode,
    TriggerVolumeComponent,
};
use postretro_foundation::PlayerMovementComponent;
use postretro_scripting_core::reaction_registry::ReactionPrimitiveRegistry;

use crate::kinematic_mover::apply_mover_command_to_targets;
use crate::scripting_systems::trigger_volume_bridge::TriggerVolumeBridge;

/// Identity passed to trigger activation without assigning trigger-ownership
/// policy. E18 may use this distinction when it adds co-op ownership rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
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

/// A named trigger event that fired during an authoritative tick. Empty event
/// names are omitted: they intentionally mean that the trigger has no event on
/// that edge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TriggerEventFire {
    pub(crate) trigger: EntityId,
    pub(crate) player: PlayerId,
    pub(crate) event_name: String,
}

/// Named trigger events produced by one authoritative tick. The split lets the
/// binding/dispatch layer preserve enter and paired-exit semantics without
/// re-evaluating trigger gates.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct TriggerFireReport {
    pub(crate) enters: Vec<TriggerEventFire>,
    pub(crate) exits: Vec<TriggerEventFire>,
}

/// Per-level trigger evaluator state. Sorted keys make edge emission stable
/// across otherwise equivalent authoritative input orderings.
#[derive(Debug, Default)]
pub(crate) struct TriggerSystem {
    occupants: BTreeMap<EntityId, BTreeSet<PlayerId>>,
    paired_enters: BTreeSet<(EntityId, PlayerId)>,
}

impl TriggerSystem {
    pub(crate) fn clear(&mut self) {
        self.occupants.clear();
        self.paired_enters.clear();
    }

    /// Number of players currently overlapping `trigger`, independently of
    /// whether the trigger was armed or its activation gate fired.
    #[allow(dead_code)] // Consumed by the E18-B policy and E18 diagnostics tasks.
    pub(crate) fn occupancy(&self, trigger: EntityId) -> usize {
        self.occupants.get(&trigger).map_or(0, BTreeSet::len)
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
    ) -> TriggerFireReport {
        let mut player_capsules: Vec<(PlayerId, Vec3, f32, f32)> = players
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
        player_capsules.sort_unstable_by_key(|(player, _, _, _)| *player);

        let mut trigger_ids: Vec<EntityId> = registry
            .iter_with_kind(ComponentKind::TriggerVolume)
            .map(|(id, _)| id)
            .collect();
        trigger_ids.sort_unstable();

        let mut report = TriggerFireReport::default();

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
            let current_occupants: BTreeSet<PlayerId> = player_capsules
                .iter()
                .filter_map(|&(player_id, center, radius, half_height)| {
                    capsule_overlaps_aabb(center, radius, half_height, aabb_min, aabb_max)
                        .then_some(player_id)
                })
                .collect();
            let previous_occupants = self
                .occupants
                .insert(trigger_id, current_occupants.clone())
                .unwrap_or_default();

            let edge_players: BTreeSet<PlayerId> = previous_occupants
                .union(&current_occupants)
                .copied()
                .collect();
            for player_id in edge_players {
                let overlapping = current_occupants.contains(&player_id);
                let was_overlapping = previous_occupants.contains(&player_id);
                let entered = overlapping && !was_overlapping;
                let left = was_overlapping && !overlapping;

                if left {
                    if self.paired_enters.remove(&(trigger_id, player_id)) {
                        record_paired_exit(player_id);
                        if !trigger.on_exit.is_empty() {
                            report.exits.push(TriggerEventFire {
                                trigger: trigger_id,
                                player: player_id,
                                event_name: trigger.on_exit.clone(),
                            });
                        }
                    }
                    continue;
                }

                let activated = match trigger.activation {
                    TriggerActivation::Touch => entered,
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
                self.paired_enters.insert((trigger_id, player_id));
                if !trigger.on_fire.is_empty() {
                    report.enters.push(TriggerEventFire {
                        trigger: trigger_id,
                        player: player_id,
                        event_name: trigger.on_fire.clone(),
                    });
                }
            }

            // Store countdown progress even when no activation occurred. The
            // explicit write also makes disabled triggers persist as inert state.
            let _ = registry.set_component(trigger_id, trigger);
        }

        report
    }
}

/// Fully re-arm a trigger. A fresh arm intentionally clears one-shot latching
/// and any running rearm timer so map logic can re-use a trigger immediately.
pub(crate) fn arm_trigger(trigger: &mut TriggerVolumeComponent) {
    trigger.armed = true;
    trigger.latched = false;
    trigger.rearm_remaining_ms = 0.0;
}

/// Disarm future enter activation without cancelling a previously paired exit.
pub(crate) fn disarm_trigger(trigger: &mut TriggerVolumeComponent) {
    trigger.armed = false;
}

/// Apply arm to an already resolved target set. Mixed tags are valid: targets
/// without trigger state are skipped rather than acquiring a component.
pub(crate) fn arm_trigger_targets(registry: &mut EntityRegistry, targets: &[EntityId]) {
    apply_trigger_mutation_to_targets(registry, targets, arm_trigger);
}

/// Apply disarm to an already resolved target set. See [`arm_trigger_targets`]
/// for the mixed-tag contract.
pub(crate) fn disarm_trigger_targets(registry: &mut EntityRegistry, targets: &[EntityId]) {
    apply_trigger_mutation_to_targets(registry, targets, disarm_trigger);
}

fn apply_trigger_mutation_to_targets(
    registry: &mut EntityRegistry,
    targets: &[EntityId],
    mutate: impl Fn(&mut TriggerVolumeComponent),
) {
    for &entity in targets {
        let Ok(mut trigger) = registry
            .get_component::<TriggerVolumeComponent>(entity)
            .cloned()
        else {
            warn_non_trigger_target_once(entity);
            continue;
        };
        mutate(&mut trigger);
        let _ = registry.set_component(entity, trigger);
    }
}

/// Register tag-targeted trigger arm controls for named reactions. The
/// descriptor dispatcher resolves tags before invoking these handlers.
pub(crate) fn register_trigger_reaction_primitives(registry: &mut ReactionPrimitiveRegistry) {
    registry.register("armTrigger", |registry, targets, _args| {
        arm_trigger_targets(registry, targets);
        Ok(())
    });
    registry.register("disarmTrigger", |registry, targets, _args| {
        disarm_trigger_targets(registry, targets);
        Ok(())
    });
}

fn warn_non_trigger_target_once(entity: EntityId) {
    static WARNED_TARGETS: OnceLock<Mutex<HashSet<EntityId>>> = OnceLock::new();
    let warned = WARNED_TARGETS.get_or_init(|| Mutex::new(HashSet::new()));
    let Ok(mut warned) = warned.lock() else {
        return;
    };
    if warned.insert(entity) {
        log::warn!("[Trigger] arm/disarm target {entity} has no TriggerVolumeComponent; skipping");
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
fn gate_fires() -> &'static Mutex<Vec<PlayerId>> {
    static FIRES: OnceLock<Mutex<Vec<PlayerId>>> = OnceLock::new();
    FIRES.get_or_init(|| Mutex::new(Vec::new()))
}

#[cfg(test)]
fn paired_exits() -> &'static Mutex<Vec<PlayerId>> {
    static EXITS: OnceLock<Mutex<Vec<PlayerId>>> = OnceLock::new();
    EXITS.get_or_init(|| Mutex::new(Vec::new()))
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
fn record_paired_exit(activator: PlayerId) {
    paired_exits()
        .lock()
        .expect("paired exit recorder poisoned")
        .push(activator);
}

#[cfg(not(test))]
fn record_paired_exit(_activator: PlayerId) {}

#[cfg(test)]
fn reset_gate_fires() {
    gate_fires()
        .lock()
        .expect("gate fire recorder poisoned")
        .clear();
    paired_exits()
        .lock()
        .expect("paired exit recorder poisoned")
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
fn recorded_paired_exits() -> Vec<PlayerId> {
    paired_exits()
        .lock()
        .expect("paired exit recorder poisoned")
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
                    String::new(),
                    String::new(),
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
    ) -> TriggerFireReport {
        let uses = uses.iter().copied().collect();
        system.run_authoritative_tick(registry, bridge, players, &uses, DT)
    }

    fn set_player_position(registry: &mut EntityRegistry, player: EntityId, position: Vec3) {
        let mut transform = *registry
            .get_component::<Transform>(player)
            .expect("player transform attached");
        transform.position = position;
        registry
            .set_component(player, transform)
            .expect("update player position");
    }

    fn set_event_names(
        registry: &mut EntityRegistry,
        trigger: EntityId,
        on_fire: &str,
        on_exit: &str,
    ) {
        let mut component = registry
            .get_component::<TriggerVolumeComponent>(trigger)
            .expect("trigger component attached")
            .clone();
        component.on_fire = on_fire.into();
        component.on_exit = on_exit.into();
        registry
            .set_component(trigger, component)
            .expect("update trigger event names");
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

    #[test]
    fn occupancy_tracks_each_overlapping_player_independently_of_activation_gate() {
        let mut registry = EntityRegistry::new();
        let mut bridge = TriggerVolumeBridge::new();
        let trigger = spawn_trigger(
            &mut registry,
            &mut bridge,
            TriggerActivation::Touch,
            TriggerFireMode::Once,
            0.0,
            false,
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

        let _ = tick(&mut system, &mut registry, &bridge, &players, &[]);
        assert_eq!(system.occupancy(trigger), 0);
        set_player_position(&mut registry, first, Vec3::new(0.0, 1.0, 0.0));
        let _ = tick(&mut system, &mut registry, &bridge, &players, &[]);
        assert_eq!(system.occupancy(trigger), 1);
        set_player_position(&mut registry, second, Vec3::new(0.0, 1.0, 0.0));
        let _ = tick(&mut system, &mut registry, &bridge, &players, &[]);
        assert_eq!(system.occupancy(trigger), 2);
        set_player_position(&mut registry, first, Vec3::new(4.0, 1.0, 0.0));
        let _ = tick(&mut system, &mut registry, &bridge, &players, &[]);
        assert_eq!(system.occupancy(trigger), 1);
        set_player_position(&mut registry, second, Vec3::new(4.0, 1.0, 0.0));
        let _ = tick(&mut system, &mut registry, &bridge, &players, &[]);
        assert_eq!(system.occupancy(trigger), 0);
    }

    #[test]
    fn fire_report_orders_enter_and_paired_exit_edges_by_trigger_then_player() {
        let _guard = gate_test_guard().lock().expect("gate test guard poisoned");
        reset_gate_fires();
        let mut registry = EntityRegistry::new();
        let mut bridge = TriggerVolumeBridge::new();
        let first_trigger = spawn_trigger(
            &mut registry,
            &mut bridge,
            TriggerActivation::Touch,
            TriggerFireMode::Multiple,
            0.0,
            true,
        );
        let second_trigger = spawn_trigger(
            &mut registry,
            &mut bridge,
            TriggerActivation::Touch,
            TriggerFireMode::Multiple,
            0.0,
            true,
        );
        set_event_names(&mut registry, first_trigger, "first_enter", "first_exit");
        set_event_names(&mut registry, second_trigger, "second_enter", "second_exit");
        let local = spawn_player(&mut registry, Vec3::new(4.0, 1.0, 0.0));
        let remote = spawn_player(&mut registry, Vec3::new(4.0, 1.0, 0.0));
        let local_id = PlayerId::Local(local);
        let remote_id = PlayerId::Remote(7);
        let players = [
            AuthoritativePlayer {
                id: remote_id,
                pawn: remote,
            },
            AuthoritativePlayer {
                id: local_id,
                pawn: local,
            },
        ];
        let mut system = TriggerSystem::default();

        let _ = tick(&mut system, &mut registry, &bridge, &players, &[]);
        set_player_position(&mut registry, local, Vec3::new(0.0, 1.0, 0.0));
        set_player_position(&mut registry, remote, Vec3::new(0.0, 1.0, 0.0));
        let enters = tick(&mut system, &mut registry, &bridge, &players, &[]);
        assert_eq!(
            enters.enters,
            vec![
                TriggerEventFire {
                    trigger: first_trigger,
                    player: local_id,
                    event_name: "first_enter".into(),
                },
                TriggerEventFire {
                    trigger: first_trigger,
                    player: remote_id,
                    event_name: "first_enter".into(),
                },
                TriggerEventFire {
                    trigger: second_trigger,
                    player: local_id,
                    event_name: "second_enter".into(),
                },
                TriggerEventFire {
                    trigger: second_trigger,
                    player: remote_id,
                    event_name: "second_enter".into(),
                },
            ]
        );

        set_player_position(&mut registry, local, Vec3::new(4.0, 1.0, 0.0));
        set_player_position(&mut registry, remote, Vec3::new(4.0, 1.0, 0.0));
        let exits = tick(&mut system, &mut registry, &bridge, &players, &[]);
        assert_eq!(
            exits.exits,
            vec![
                TriggerEventFire {
                    trigger: first_trigger,
                    player: local_id,
                    event_name: "first_exit".into(),
                },
                TriggerEventFire {
                    trigger: first_trigger,
                    player: remote_id,
                    event_name: "first_exit".into(),
                },
                TriggerEventFire {
                    trigger: second_trigger,
                    player: local_id,
                    event_name: "second_exit".into(),
                },
                TriggerEventFire {
                    trigger: second_trigger,
                    player: remote_id,
                    event_name: "second_exit".into(),
                },
            ]
        );
    }

    #[test]
    fn suppressed_enter_does_not_produce_a_paired_exit() {
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
        set_event_names(&mut registry, trigger, "entered", "left");
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

        let _ = tick(&mut system, &mut registry, &bridge, &players, &[]);
        set_player_position(&mut registry, first, Vec3::new(0.0, 1.0, 0.0));
        let fired = tick(&mut system, &mut registry, &bridge, &players, &[]);
        assert_eq!(fired.enters.len(), 1);
        set_player_position(&mut registry, second, Vec3::new(0.0, 1.0, 0.0));
        let suppressed = tick(&mut system, &mut registry, &bridge, &players, &[]);
        assert!(suppressed.enters.is_empty());
        set_player_position(&mut registry, second, Vec3::new(4.0, 1.0, 0.0));
        let exited = tick(&mut system, &mut registry, &bridge, &players, &[]);

        assert!(exited.exits.is_empty());
        assert!(recorded_paired_exits().is_empty());
    }

    #[test]
    fn paired_exit_survives_once_rearm_and_mid_stand_disarm() {
        let _guard = gate_test_guard().lock().expect("gate test guard poisoned");
        reset_gate_fires();
        let mut registry = EntityRegistry::new();
        let mut bridge = TriggerVolumeBridge::new();
        let once = spawn_trigger(
            &mut registry,
            &mut bridge,
            TriggerActivation::Touch,
            TriggerFireMode::Once,
            0.0,
            true,
        );
        let rearming = spawn_trigger(
            &mut registry,
            &mut bridge,
            TriggerActivation::Touch,
            TriggerFireMode::Multiple,
            1_000.0,
            true,
        );
        let disarmed = spawn_trigger(
            &mut registry,
            &mut bridge,
            TriggerActivation::Touch,
            TriggerFireMode::Multiple,
            0.0,
            true,
        );
        set_event_names(&mut registry, once, "once_enter", "once_exit");
        set_event_names(&mut registry, rearming, "rearm_enter", "rearm_exit");
        set_event_names(&mut registry, disarmed, "disarm_enter", "disarm_exit");
        let player = spawn_player(&mut registry, Vec3::new(4.0, 1.0, 0.0));
        let player_id = PlayerId::Local(player);
        let players = [AuthoritativePlayer {
            id: player_id,
            pawn: player,
        }];
        let mut system = TriggerSystem::default();

        let _ = tick(&mut system, &mut registry, &bridge, &players, &[]);
        set_player_position(&mut registry, player, Vec3::new(0.0, 1.0, 0.0));
        let entered = tick(&mut system, &mut registry, &bridge, &players, &[]);
        assert_eq!(entered.enters.len(), 3);
        assert!(
            registry
                .get_component::<TriggerVolumeComponent>(once)
                .expect("once trigger exists")
                .latched
        );
        assert!(
            registry
                .get_component::<TriggerVolumeComponent>(rearming)
                .expect("rearming trigger exists")
                .rearm_remaining_ms
                > 0.0
        );
        disarm_trigger_targets(&mut registry, &[disarmed]);

        set_player_position(&mut registry, player, Vec3::new(4.0, 1.0, 0.0));
        let exited = tick(&mut system, &mut registry, &bridge, &players, &[]);
        assert_eq!(
            exited.exits,
            vec![
                TriggerEventFire {
                    trigger: once,
                    player: player_id,
                    event_name: "once_exit".into(),
                },
                TriggerEventFire {
                    trigger: rearming,
                    player: player_id,
                    event_name: "rearm_exit".into(),
                },
                TriggerEventFire {
                    trigger: disarmed,
                    player: player_id,
                    event_name: "disarm_exit".into(),
                },
            ]
        );
        assert_eq!(recorded_paired_exits(), vec![player_id; 3]);
    }

    #[test]
    fn arm_and_disarm_primitives_control_enter_firing_and_reset_gate_state() {
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
            false,
        );
        set_event_names(&mut registry, trigger, "armed_enter", "armed_exit");
        let player = spawn_player(&mut registry, Vec3::new(4.0, 1.0, 0.0));
        let player_id = PlayerId::Local(player);
        let players = [AuthoritativePlayer {
            id: player_id,
            pawn: player,
        }];
        let mut system = TriggerSystem::default();
        let mut reactions = ReactionPrimitiveRegistry::new();
        register_trigger_reaction_primitives(&mut reactions);

        let _ = tick(&mut system, &mut registry, &bridge, &players, &[]);
        set_player_position(&mut registry, player, Vec3::new(0.0, 1.0, 0.0));
        assert!(
            tick(&mut system, &mut registry, &bridge, &players, &[])
                .enters
                .is_empty(),
            "a disabled-on-spawn trigger must not fire"
        );
        assert!(
            reactions
                .dispatch(
                    "armTrigger",
                    &mut registry,
                    &[trigger],
                    &serde_json::Value::Null
                )
                .expect("arm dispatch succeeds")
        );
        assert!(
            registry
                .get_component::<TriggerVolumeComponent>(trigger)
                .expect("trigger exists")
                .armed
        );
        let _ = tick(&mut system, &mut registry, &bridge, &players, &[]);
        set_player_position(&mut registry, player, Vec3::new(4.0, 1.0, 0.0));
        let _ = tick(&mut system, &mut registry, &bridge, &players, &[]);
        set_player_position(&mut registry, player, Vec3::new(0.0, 1.0, 0.0));
        assert_eq!(
            tick(&mut system, &mut registry, &bridge, &players, &[]).enters,
            vec![TriggerEventFire {
                trigger,
                player: player_id,
                event_name: "armed_enter".into(),
            }]
        );

        let mut component = registry
            .get_component::<TriggerVolumeComponent>(trigger)
            .expect("trigger exists")
            .clone();
        component.armed = false;
        component.rearm_remaining_ms = 250.0;
        registry
            .set_component(trigger, component)
            .expect("seed arm reset state");
        assert!(
            reactions
                .dispatch(
                    "armTrigger",
                    &mut registry,
                    &[trigger],
                    &serde_json::Value::Null
                )
                .expect("second arm dispatch succeeds")
        );
        let component = registry
            .get_component::<TriggerVolumeComponent>(trigger)
            .expect("trigger exists");
        assert!(component.armed);
        assert!(!component.latched);
        assert_eq!(component.rearm_remaining_ms, 0.0);

        set_player_position(&mut registry, player, Vec3::new(4.0, 1.0, 0.0));
        let _ = tick(&mut system, &mut registry, &bridge, &players, &[]);
        set_player_position(&mut registry, player, Vec3::new(0.0, 1.0, 0.0));
        assert_eq!(
            tick(&mut system, &mut registry, &bridge, &players, &[])
                .enters
                .len(),
            1,
            "arming clears a once latch and enables a new enter"
        );

        assert!(
            reactions
                .dispatch(
                    "disarmTrigger",
                    &mut registry,
                    &[trigger],
                    &serde_json::Value::Null
                )
                .expect("disarm dispatch succeeds")
        );
        set_player_position(&mut registry, player, Vec3::new(4.0, 1.0, 0.0));
        let _ = tick(&mut system, &mut registry, &bridge, &players, &[]);
        set_player_position(&mut registry, player, Vec3::new(0.0, 1.0, 0.0));
        assert!(
            tick(&mut system, &mut registry, &bridge, &players, &[])
                .enters
                .is_empty(),
            "disarming prevents later enter fires"
        );
    }

    #[test]
    fn trigger_primitives_skip_non_trigger_targets() {
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
        let non_trigger = registry.spawn(Transform::default());
        let mut reactions = ReactionPrimitiveRegistry::new();
        register_trigger_reaction_primitives(&mut reactions);

        assert!(
            reactions
                .dispatch(
                    "disarmTrigger",
                    &mut registry,
                    &[trigger, non_trigger],
                    &serde_json::Value::Null
                )
                .expect("disarm dispatch succeeds")
        );
        assert!(
            !registry
                .get_component::<TriggerVolumeComponent>(trigger)
                .expect("trigger retained its component")
                .armed
        );
        assert!(
            registry
                .get_component::<TriggerVolumeComponent>(non_trigger)
                .is_err(),
            "a mixed tag target must not gain trigger state"
        );
    }
}

//! Host-authoritative fixed-tick trigger evaluation and command dispatch.
//! See: context/lib/entity_model.md §5 · context/lib/scripting.md §10

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use glam::Vec3;
use postretro_entities::{
    ComponentKind, EntityId, EntityRegistry, Transform, TriggerActivation, TriggerFireMode,
    TriggerVolumeComponent,
};
use postretro_foundation::PlayerMovementComponent;
use postretro_scripting_core::reaction_registry::ReactionPrimitiveRegistry;
use postretro_scripting_core::sequence::SequencedPrimitiveRegistry;

use crate::kinematic_mover::MoverAutoCloseTimers;
use crate::mover_commands::MoverCommandDiagnostics;
use crate::scripting_systems::trigger_volume_bridge::TriggerVolumeBridge;

/// Stable player identity for per-player trigger state and event ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PlayerId {
    Local(EntityId),
    Remote(u64),
}

#[derive(Debug, Clone, Copy)]
pub struct AuthoritativePlayer {
    pub id: PlayerId,
    pub pawn: EntityId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TriggerActivationDecision {
    Fire,
    Suppress,
}

/// A trigger event that fired during an authoritative tick. An empty event
/// name is valid when a script binding owns the fired edge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriggerEventFire {
    pub trigger: EntityId,
    pub player: PlayerId,
    pub event_name: String,
}

/// Which edge produced a trigger event. The trigger stage owns this
/// ordering so commands from one edge can affect a later enter gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TriggerEventEdge {
    Enter,
    Exit,
}

/// A trigger fire together with its source edge. This is the canonical
/// fixed-tick event stream; it is ordered by `(trigger, player)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriggerEvent {
    pub fire: TriggerEventFire,
    pub edge: TriggerEventEdge,
}

/// Trigger events produced by one authoritative tick. Consumers must preserve
/// `fires` order; a script-only edge may intentionally carry an empty event
/// name. Paired exits are already authorized and never need a second gate
/// evaluation.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TriggerFireReport {
    pub fires: Vec<TriggerEvent>,
}

/// Read-only trigger binding state needed while dispatching one fixed tick.
pub struct TriggerDispatchInputs<'a> {
    pub alive_players: &'a HashSet<PlayerId>,
    pub bound_edges: &'a HashSet<(EntityId, TriggerEventEdge)>,
}

/// Authoritative player state sampled for one fixed trigger tick.
pub struct TriggerTickInputs<'a> {
    pub players: &'a [AuthoritativePlayer],
    pub use_pressed: &'a HashMap<PlayerId, bool>,
    pub tick_dt: f32,
}

impl TriggerFireReport {
    #[cfg(test)]
    fn enters(&self) -> Vec<TriggerEventFire> {
        self.fires
            .iter()
            .filter(|event| event.edge == TriggerEventEdge::Enter)
            .map(|event| event.fire.clone())
            .collect()
    }

    #[cfg(test)]
    fn exits(&self) -> Vec<TriggerEventFire> {
        self.fires
            .iter()
            .filter(|event| event.edge == TriggerEventEdge::Exit)
            .map(|event| event.fire.clone())
            .collect()
    }
}

/// Per-level trigger evaluator state. Sorted keys make edge emission stable
/// across otherwise equivalent authoritative input orderings.
#[derive(Debug, Default)]
pub struct TriggerSystem {
    occupants: crate::trigger_occupants::TriggerOccupants,
    /// Counted occupants per volume after the last trigger stage, for the
    /// dev-tools Triggers tab; filled only in dev-tools builds.
    #[cfg(feature = "dev-tools")]
    effective_counts: BTreeMap<EntityId, usize>,
    paired_enters: BTreeSet<(EntityId, PlayerId)>,
    warned_duplicate_players: HashSet<PlayerId>,
    direct_commands: crate::trigger_direct_commands::DirectCommandHandles,
    // Per-system test probes prevent parallel trigger tests from sharing traces.
    // They survive `clear` so level-replacement tests can observe both levels.
    #[cfg(test)]
    recorded_gate_fires: Vec<PlayerId>,
    #[cfg(test)]
    recorded_paired_exits: Vec<PlayerId>,
}

impl TriggerSystem {
    pub fn with_mover_auto_close_timers(auto_close_timers: MoverAutoCloseTimers) -> Self {
        Self {
            direct_commands: crate::trigger_direct_commands::DirectCommandHandles {
                auto_close_timers: Some(auto_close_timers),
                spawn_context: None,
            },
            ..Self::default()
        }
    }

    /// The spawn context a direct `fire` command spawns a spawner's batch through.
    pub fn with_spawn_context(mut self, spawn_context: crate::spawner::SpawnContext) -> Self {
        self.direct_commands.spawn_context = Some(spawn_context);
        self
    }

    pub fn clear(&mut self) {
        self.occupants.clear();
        #[cfg(feature = "dev-tools")]
        self.effective_counts.clear();
        self.paired_enters.clear();
        self.warned_duplicate_players.clear();
    }

    /// Number of players currently overlapping `trigger`, independently of
    /// whether the trigger was armed or its activation gate fired.
    #[cfg_attr(not(feature = "dev-tools"), allow(dead_code))]
    pub fn occupancy(&self, trigger: EntityId) -> usize {
        self.occupants.count(trigger)
    }

    /// After this tick's trigger stage: each volume the capture's counted
    /// players occupy, with their pawns.
    pub fn counted_volumes(
        &mut self,
        capabilities: &crate::sim::player_life::TriggerStageCapabilities,
    ) -> Vec<(EntityId, Vec<EntityId>)> {
        let volumes = self
            .occupants
            .counted_volumes(&capabilities.occupants, &capabilities.pawns);
        #[cfg(feature = "dev-tools")]
        {
            self.effective_counts.clear();
            self.effective_counts.extend(
                volumes
                    .iter()
                    .map(|(volume, inside)| (*volume, inside.len())),
            );
        }
        volumes
    }

    /// Counted occupants of `trigger` after the last trigger stage: raw
    /// occupancy filtered by the effective rule, as `occupancy.tag` counts them.
    #[cfg(feature = "dev-tools")]
    pub fn counted_occupancy(&self, trigger: EntityId) -> usize {
        self.effective_counts.get(&trigger).copied().unwrap_or(0)
    }

    /// The set of `(trigger, player)` pairs whose Enter gate has fired and whose
    /// paired Exit obligation is still standing. E18 Task 5's enrollment check
    /// borrows this from the frame-end drain: an interruptible wait may park only
    /// while its origin's paired enter is live, otherwise a player who entered and
    /// left within one frame would park an uncancellable instance (O52, O60).
    pub fn paired_enters(&self) -> &BTreeSet<(EntityId, PlayerId)> {
        &self.paired_enters
    }

    /// Run after player movement and before AI. This function is called only by
    /// the host/single-player simulation path; clients receive mover phase over
    /// replication and never evaluate or apply trigger commands locally.
    #[cfg(any(test, feature = "test-support"))]
    pub fn run_authoritative_tick(
        &mut self,
        registry: &mut EntityRegistry,
        bridge: &TriggerVolumeBridge,
        players: &[AuthoritativePlayer],
        use_pressed: &HashMap<PlayerId, bool>,
        tick_dt: f32,
    ) -> TriggerFireReport {
        let alive_players = players.iter().map(|player| player.id).collect();
        let bound_edges = HashSet::new();
        self.run_authoritative_tick_with_dispatch(
            registry,
            bridge,
            TriggerTickInputs {
                players,
                use_pressed,
                tick_dt,
            },
            TriggerDispatchInputs {
                alive_players: &alive_players,
                bound_edges: &bound_edges,
            },
            |_, _, _, _| {},
        )
    }

    /// Evaluate and dispatch every trigger edge in one stable stream. The
    /// callback runs immediately after the direct command and gate-state
    /// update, before the next `(trigger, player)` edge is evaluated. This lets
    /// a trigger reaction arm or disarm a later trigger in the same fixed tick
    /// without bringing a script VM into the simulation seam. The occupant slice
    /// is the completed tick occupancy filtered through `alive_players`; its
    /// length is the count passed beside it. Residual producers retain a copy
    /// only when that edge leaves frame-end work.
    pub fn run_authoritative_tick_with_dispatch(
        &mut self,
        registry: &mut EntityRegistry,
        bridge: &TriggerVolumeBridge,
        tick_inputs: TriggerTickInputs<'_>,
        dispatch_inputs: TriggerDispatchInputs<'_>,
        dispatch: impl FnMut(&TriggerEvent, usize, &[PlayerId], &mut EntityRegistry),
    ) -> TriggerFireReport {
        let capabilities = crate::sim::player_life::TriggerStageCapabilities::capture(
            registry,
            tick_inputs.players,
        );
        let alive_players = capabilities.effective_occupants(dispatch_inputs.alive_players);
        self.run_authoritative_tick_with_capabilities(
            registry,
            bridge,
            tick_inputs,
            TriggerDispatchInputs {
                alive_players: &alive_players,
                bound_edges: dispatch_inputs.bound_edges,
            },
            &capabilities,
            dispatch,
        )
    }

    pub fn run_authoritative_tick_with_capabilities(
        &mut self,
        registry: &mut EntityRegistry,
        bridge: &TriggerVolumeBridge,
        tick_inputs: TriggerTickInputs<'_>,
        dispatch_inputs: TriggerDispatchInputs<'_>,
        capabilities: &crate::sim::player_life::TriggerStageCapabilities,
        mut dispatch: impl FnMut(&TriggerEvent, usize, &[PlayerId], &mut EntityRegistry),
    ) -> TriggerFireReport {
        let player_capsules = canonical_player_capsules(
            registry,
            tick_inputs.players,
            &mut self.warned_duplicate_players,
        );

        let mut trigger_ids: Vec<EntityId> = registry
            .iter_with_kind(ComponentKind::TriggerVolume)
            .map(|(id, _)| id)
            .collect();
        trigger_ids.sort_unstable();

        // Removing a trigger deliberately drops any unmatched paired exits:
        // without its AABB, no valid edge remains to resolve them on. Revisit
        // this policy if runtime trigger despawn becomes supported.
        let active_triggers: BTreeSet<EntityId> = trigger_ids
            .iter()
            .copied()
            .filter(|trigger| bridge.aabb(*trigger).is_some())
            .collect();
        self.occupants.retain_triggers(&active_triggers);
        self.paired_enters
            .retain(|(trigger, _)| active_triggers.contains(trigger));

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

            decrement_rearm(&mut trigger, tick_inputs.tick_dt);
            let touch_reactivation_pending = trigger.activation == TriggerActivation::Touch
                && trigger.touch_reactivation_pending;
            trigger.touch_reactivation_pending = false;
            let _ = registry.set_component(trigger_id, trigger.clone());

            let leaving_players = self.occupants.take_leaving(trigger_id, |player_id| {
                player_capsules
                    .get(&player_id)
                    .is_some_and(|&(_, center, radius, half_height)| {
                        capsule_overlaps_aabb(center, radius, half_height, aabb_min, aabb_max)
                    })
            });

            let mut edges: Vec<(PlayerId, TriggerEventEdge)> = leaving_players
                .into_iter()
                .map(|player| (player, TriggerEventEdge::Exit))
                .collect();
            for (&player_id, &(_, center, radius, half_height)) in &player_capsules {
                let overlapping =
                    capsule_overlaps_aabb(center, radius, half_height, aabb_min, aabb_max);
                let entered = overlapping && self.occupants.enter(trigger_id, player_id);
                let activated = match trigger.activation {
                    TriggerActivation::Touch => {
                        capabilities.permits_touch(player_id)
                            && (entered || (touch_reactivation_pending && overlapping))
                    }
                    TriggerActivation::Use => {
                        capabilities.permits_use(player_id)
                            && overlapping
                            && tick_inputs
                                .use_pressed
                                .get(&player_id)
                                .copied()
                                .unwrap_or(false)
                    }
                };
                if activated {
                    edges.push((player_id, TriggerEventEdge::Enter));
                }
            }
            edges.sort_unstable_by_key(|(player, _)| *player);

            for (player_id, edge) in edges {
                match edge {
                    TriggerEventEdge::Exit => {
                        if !self.paired_enters.remove(&(trigger_id, player_id)) {
                            continue;
                        }
                        #[cfg(test)]
                        self.recorded_paired_exits.push(player_id);
                        let Ok(trigger) = registry
                            .get_component::<TriggerVolumeComponent>(trigger_id)
                            .cloned()
                        else {
                            continue;
                        };
                        if trigger.on_exit.is_empty()
                            && !dispatch_inputs
                                .bound_edges
                                .contains(&(trigger_id, TriggerEventEdge::Exit))
                        {
                            continue;
                        }
                        let event = TriggerEvent {
                            fire: TriggerEventFire {
                                trigger: trigger_id,
                                player: player_id,
                                event_name: trigger.on_exit,
                            },
                            edge,
                        };
                        let occupants =
                            self.effective_occupants(trigger_id, dispatch_inputs.alive_players);
                        dispatch(&event, occupants.len(), &occupants, registry);
                        report.fires.push(event);
                    }
                    TriggerEventEdge::Enter => {
                        // Re-read after every earlier event. A bound arm/disarm
                        // command may have changed this trigger since its edge
                        // was discovered, and only the gate decides whether the
                        // enter is still eligible to fire.
                        let Ok(mut trigger) = registry
                            .get_component::<TriggerVolumeComponent>(trigger_id)
                            .cloned()
                        else {
                            continue;
                        };
                        if evaluate_trigger_activation(&trigger, player_id)
                            != TriggerActivationDecision::Fire
                        {
                            continue;
                        }
                        #[cfg(test)]
                        self.recorded_gate_fires.push(player_id);

                        crate::trigger_direct_commands::apply_direct_trigger_command(
                            registry,
                            &trigger.target_tag,
                            &trigger.command,
                            &self.direct_commands,
                        );
                        update_after_fire(&mut trigger);
                        let event_name = trigger.on_fire.clone();
                        let _ = registry.set_component(trigger_id, trigger);
                        self.paired_enters.insert((trigger_id, player_id));
                        if event_name.is_empty()
                            && !dispatch_inputs
                                .bound_edges
                                .contains(&(trigger_id, TriggerEventEdge::Enter))
                        {
                            continue;
                        }
                        let event = TriggerEvent {
                            fire: TriggerEventFire {
                                trigger: trigger_id,
                                player: player_id,
                                event_name,
                            },
                            edge,
                        };
                        let occupants =
                            self.effective_occupants(trigger_id, dispatch_inputs.alive_players);
                        dispatch(&event, occupants.len(), &occupants, registry);
                        report.fires.push(event);
                    }
                }
            }
        }

        report
    }

    fn effective_occupants(
        &self,
        trigger: EntityId,
        alive_players: &HashSet<PlayerId>,
    ) -> Vec<PlayerId> {
        self.occupants.effective(trigger, alive_players)
    }

    #[cfg(test)]
    fn recorded_gate_fires(&self) -> &[PlayerId] {
        &self.recorded_gate_fires
    }

    #[cfg(test)]
    fn recorded_paired_exits(&self) -> &[PlayerId] {
        &self.recorded_paired_exits
    }
}

pub(crate) fn canonical_player_capsules(
    registry: &EntityRegistry,
    players: &[AuthoritativePlayer],
    warned_duplicate_players: &mut HashSet<PlayerId>,
) -> BTreeMap<PlayerId, (EntityId, Vec3, f32, f32)> {
    let canonical_pawns = canonical_player_pawns(registry, players);
    let mut seen_players = HashSet::new();
    for player in players {
        if registry.get_component::<Transform>(player.pawn).is_err()
            || registry
                .get_component::<PlayerMovementComponent>(player.pawn)
                .is_err()
        {
            continue;
        }
        if !seen_players.insert(player.id) {
            warn_duplicate_player_once(warned_duplicate_players, player.id);
        }
    }
    canonical_pawns
        .into_iter()
        .filter_map(|(player_id, pawn)| {
            let transform = registry.get_component::<Transform>(pawn).ok()?;
            let movement = registry
                .get_component::<PlayerMovementComponent>(pawn)
                .ok()?;
            Some((
                player_id,
                (
                    pawn,
                    transform.position,
                    movement.capsule.radius,
                    movement.capsule.half_height,
                ),
            ))
        })
        .collect()
}

/// Select the one pawn that represents each player identity this tick. Trigger
/// collision and trigger-fire context resolution share this rule.
pub(crate) fn canonical_player_pawns(
    registry: &EntityRegistry,
    players: &[AuthoritativePlayer],
) -> BTreeMap<PlayerId, EntityId> {
    let mut pawns: BTreeMap<PlayerId, EntityId> = BTreeMap::new();
    for player in players {
        if registry.get_component::<Transform>(player.pawn).is_err()
            || registry
                .get_component::<PlayerMovementComponent>(player.pawn)
                .is_err()
        {
            continue;
        }
        pawns
            .entry(player.id)
            .and_modify(|canonical| *canonical = (*canonical).min(player.pawn))
            .or_insert(player.pawn);
    }
    pawns
}

/// Fully re-arm a trigger. A fresh arm intentionally clears one-shot latching
/// and any running rearm timer so map logic can re-use a trigger immediately.
/// Touch triggers also admit their currently standing occupants on the next tick.
pub(crate) fn arm_trigger(trigger: &mut TriggerVolumeComponent) {
    trigger.armed = true;
    trigger.latched = false;
    trigger.rearm_remaining_ms = 0.0;
    if trigger.activation == TriggerActivation::Touch {
        trigger.touch_reactivation_pending = true;
    }
}

/// Disarm future enter activation without cancelling a previously paired exit.
pub(crate) fn disarm_trigger(trigger: &mut TriggerVolumeComponent) {
    trigger.armed = false;
}

/// Apply arm to an already resolved target set. Mixed tags are valid: targets
/// without trigger state are skipped rather than acquiring a component.
pub(crate) fn arm_trigger_targets(
    registry: &mut EntityRegistry,
    targets: &[EntityId],
    diagnostics: &MoverCommandDiagnostics,
) {
    apply_trigger_mutation_to_targets(registry, targets, arm_trigger, diagnostics);
}

/// Apply disarm to an already resolved target set. See [`arm_trigger_targets`]
/// for the mixed-tag contract.
pub(crate) fn disarm_trigger_targets(
    registry: &mut EntityRegistry,
    targets: &[EntityId],
    diagnostics: &MoverCommandDiagnostics,
) {
    apply_trigger_mutation_to_targets(registry, targets, disarm_trigger, diagnostics);
}

fn apply_trigger_mutation_to_targets(
    registry: &mut EntityRegistry,
    targets: &[EntityId],
    mutate: impl Fn(&mut TriggerVolumeComponent),
    diagnostics: &MoverCommandDiagnostics,
) {
    for &entity in targets {
        let Ok(mut trigger) = registry
            .get_component::<TriggerVolumeComponent>(entity)
            .cloned()
        else {
            diagnostics.warn_non_trigger_target_once(entity);
            continue;
        };
        mutate(&mut trigger);
        let _ = registry.set_component(entity, trigger);
    }
}

/// Register tag-targeted trigger arm controls for named reactions. The
/// descriptor dispatcher resolves tags before invoking these handlers.
pub(crate) fn register_trigger_reaction_primitives(
    registry: &mut ReactionPrimitiveRegistry,
    diagnostics: MoverCommandDiagnostics,
) {
    let arm_diagnostics = diagnostics.clone();
    registry.register("armTrigger", move |registry, targets, _args| {
        arm_trigger_targets(registry, targets, &arm_diagnostics);
        Ok(())
    });
    registry.register("disarmTrigger", move |registry, targets, _args| {
        disarm_trigger_targets(registry, targets, &diagnostics);
        Ok(())
    });
}

/// Register per-entity trigger controls for sequenced reactions. SDK trigger
/// handles target one resolved entity id, while named primitive reactions use
/// the tag-targeted registrar above.
pub fn register_sequenced_trigger_primitives(
    registry: &mut SequencedPrimitiveRegistry,
    ctx: postretro_entities::ScriptCtx,
    diagnostics: MoverCommandDiagnostics,
) {
    register_sequenced_trigger_command(
        registry,
        ctx.clone(),
        diagnostics.clone(),
        "armTrigger",
        arm_trigger,
    );
    register_sequenced_trigger_command(registry, ctx, diagnostics, "disarmTrigger", disarm_trigger);
}

fn register_sequenced_trigger_command(
    registry: &mut SequencedPrimitiveRegistry,
    ctx: postretro_entities::ScriptCtx,
    diagnostics: MoverCommandDiagnostics,
    name: &'static str,
    command: fn(&mut TriggerVolumeComponent),
) {
    registry.register(name, move |id, _args| {
        let mut entities = ctx.registry.borrow_mut();
        apply_trigger_mutation_to_targets(&mut entities, &[id], command, &diagnostics);
        Ok(())
    });
}

fn warn_duplicate_player_once(warned_players: &mut HashSet<PlayerId>, player: PlayerId) {
    if warned_players.insert(player) {
        log::warn!(
            "[Trigger] duplicate authoritative snapshot for {player:?}; using the lowest entity ID"
        );
    }
}

/// The sole activation decision point for touch and use routes.
fn evaluate_trigger_activation(
    state: &TriggerVolumeComponent,
    activator: PlayerId,
) -> TriggerActivationDecision {
    #[cfg(not(feature = "dev-tools"))]
    let _ = activator;
    #[cfg(feature = "dev-tools")]
    log::debug!("[Trigger] activation candidate from {activator:?}");

    let fire = state.armed
        && !matches!(state.fire_mode, TriggerFireMode::Once if state.latched)
        && state.rearm_remaining_ms <= 0.0;
    if fire {
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

pub(crate) fn range_distance(value: f32, min: f32, max: f32) -> f32 {
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
mod tests;

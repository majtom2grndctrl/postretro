// Headless fixed-tick game-state advance seam.
// See: context/lib/entity_model.md §5 · context/lib/networking.md

#[cfg(test)]
mod activity_speed_tests;
pub mod assist;
pub mod assist_pairs;
mod assist_pass;
mod assist_scratch;
pub mod assist_snapshot;
pub mod cpu_stages;
#[cfg(test)]
mod damage_in_activity_tests;
#[cfg(test)]
mod damage_type_tests;
pub mod defense_layers;
#[cfg(test)]
mod defense_layers_tests;
#[cfg(test)]
pub(crate) mod defense_parameter_tests;
pub mod defense_parameters;
#[cfg(test)]
mod grant_layer_tests;
#[cfg(test)]
mod layer_lifecycle_tests;
#[cfg(test)]
mod layer_stress_tests;
#[cfg(test)]
mod script_impact_tick_tests;
#[cfg(test)]
mod status_dev_fixture_tests;
#[cfg(test)]
mod status_fluid_tests;
#[cfg(test)]
mod status_policy_tests;
#[cfg(test)]
mod status_tick_tests;
// Callers hand the CPU timing gate to the tick as a value.
pub use postretro_stage_timing::TimingGate;
mod descriptor_health;
pub mod frame_timing;
mod mesh_bindings;
pub mod player_life;
mod respawn;
mod tick_start;
pub use tick_start::{
    PendingTickStart, RespawnTurn, StartedTick, begin_tick, finish_tick_start,
    simulate_tick_with_presentation_aim,
};
#[cfg(test)]
mod player_life_tests;
mod player_locomotion;
pub mod presentation_pool;
pub mod presentation_projection;
mod projectile_stage;
#[cfg(test)]
mod remote_presence_tests;
pub mod splash;
pub mod touch;
#[cfg(any(test, feature = "test-support"))]
pub use crate::scripting::builtins::{acquire_wieldable_at_for_test, release_wieldable_for_test};

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use glam::{EulerRot, Mat4, Vec3};

use crate::agent_steering;
use crate::ai_host::{AiTickInputs, AiTickResult, SimAiHost};
use crate::collision::moving::{
    CombinedCollisionWorld, MoverCollider, MoverPoseSource, cast_ray_combined,
};
use crate::collision::{COS_WALKABLE, CollisionWorld, cast_ray};
use crate::kinematic_mover::{self, MoverTickStateTable};
use crate::movement::MovementInput;
use crate::nav::NavGraph;
use crate::scripting_systems;
use crate::scripting_systems::hit_zones::{
    HitZoneStore, model_matrix, sample_world_pose_for_probe,
};
use crate::scripting_systems::trigger_volume_bridge::TriggerVolumeBridge;
use crate::sim::touch::TouchSystem;
use crate::trigger_bindings::{TriggerBindingTable, TriggerResidualHandle};
use crate::trigger_commands::TriggerFireContext;
#[cfg(test)]
use crate::trigger_system::TriggerEvent;
use crate::trigger_system::{AuthoritativePlayer, PlayerId, TriggerSystem};
#[cfg(test)]
use crate::weapon;
use crate::weapon::FireButtonState;
pub use descriptor_health::install_descriptor_player_health_range;
pub use mesh_bindings::{resolve_mesh_entity_bindings, resolve_mesh_entity_bindings_for_entities};
pub use player_locomotion::{
    CrouchTier, FALL_DELAY_TICKS, JUMP_LOOP_STATE, JumpStates, LocomotionFooting, LocomotionInput,
    LocomotionPosture, LocomotionStance, LocomotionTier, LocomotionTierKind,
    PlayerLocomotionStates, classify_locomotion_direction, player_travel_heading_yaw,
    store_locomotion_direction, store_locomotion_memory,
};
use postretro_collision::EnvironmentView;
use postretro_combat_model::{OpenAuthorizedShot, ShotId};
use postretro_entities::PoseInputs;
use postretro_entities::components::agent::AgentComponent;
use postretro_entities::components::brain::{BrainComponent, TravelVariant};
#[cfg(test)]
use postretro_entities::components::health::HealthComponent;
use postretro_entities::components::mesh::{
    MeshAnimation, MeshComponent, SwitchResult, player_capsule_feet_origin_offset,
    switch_animation_state,
};
use postretro_entities::components::player_movement::PlayerMovementComponent;
#[cfg(test)]
use postretro_entities::components::weapon::WeaponComponent;
use postretro_entities::{
    ComponentKind, ComponentValue, EntityId, EntityRegistry, EntityTypeDescriptor, FactionRegistry,
    FactionSentimentState, ScriptCtx, SlotTable,
};
use postretro_foundation::{
    WeaponPlacementDescriptor,
    pose::{FootProbe, MAX_FEET},
};
use postretro_net::wire::NetworkId;
use postretro_scripting_core::reaction_dispatch::ProgressTracker;
#[cfg(any(test, feature = "test-support"))]
pub use projectile_stage::advance;
pub use projectile_stage::{
    PredictedProjectileResolution, ProjectileContactEvent, advance_predicted,
    correct_predicted_projectile, projectile_splash_occlusion_origin,
    projectile_static_contact_distance, resolve_projectile_impact,
    set_predicted_projectile_visible,
};
pub use reload::{wieldable_equip_ticks, wieldable_reload_due};
pub use weapon_stage::{
    ProjectileSource, begin_wieldable_lower, finish_wieldable_lower,
    projectile_model_body_rotation, spawn_projectile,
};

#[derive(Debug, Clone)]
pub struct SimCommand {
    /// This pawn/controller's logical input tick, independent of wieldable instance age.
    /// Remote execution advances on its separate authoritative host fire tick.
    pub input_tick: u32,
    pub movement: MovementInput,
    pub fire_button: FireButtonState,
    pub reload: bool,
    pub secondary_button: FireButtonState,
    pub activation: postretro_foundation::ActivationInput,
    /// Slot the local client declares as the source of fire. The host resolves it
    /// from pawn inventory by possession rather than from its active pointer.
    pub firing_slot: u8,
    /// Direct number-row selection is a discrete simulation command. Cursor and
    /// dwell remain input-only state.
    pub select_slot: Option<usize>,
    /// Use rising edge routed to the host-authoritative trigger stage. Kept on
    /// the full command alongside fire/reload; `MovementInput` mirrors it for the
    /// client-prediction input boundary.
    pub use_pressed: bool,
    /// Drop rising edge routed to the host-authoritative touch stage. Kept on
    /// the full command alongside `use_pressed`; `MovementInput` mirrors it for
    /// the client-prediction input boundary.
    pub drop_pressed: bool,
    /// The resolved `assist` level, true on every command built while the
    /// assist binding is down. Read by both runners from here, never from
    /// `MovementInput`; a gap-held command carries it as a level.
    pub assist_held: bool,
}

pub struct PostMovementCommand {
    pub aim_origin: Vec3,
    pub aim_direction: Vec3,
}

/// Roll back a host-refused locally-running inventory switch without entering a
/// second equip transition. The network Control drain owns when this is invoked;
/// the weapon stage owns the machine-state cleanup.
pub fn refuse_local_wieldable_switch(
    registry: &mut EntityRegistry,
    pawn: EntityId,
    refused_slot: usize,
    rollback_slot: usize,
) -> bool {
    weapon_stage::refuse_local_switch(registry, pawn, refused_slot, rollback_slot)
}

/// Clear despawned wieldable slots and surface pawns whose active instance changed.
/// The caller feeds those pawns into the existing attachment-dirty path.
pub fn normalize_wieldable_inventories(registry: &mut EntityRegistry) -> Vec<EntityId> {
    weapon_stage::normalize_all_inventory_liveness(registry)
}

/// Normalize one pawn before a host declaration and report whether its active
/// instance changed so presentation dirtiness follows the same liveness result.
pub fn normalize_wieldable_inventory(
    registry: &mut EntityRegistry,
    pawn: EntityId,
) -> Option<(postretro_entities::components::inventory::Inventory, bool)> {
    weapon_stage::normalize_inventory_liveness(registry, pawn)
}

#[cfg(any(test, feature = "test-support"))]
pub fn run_local_weapon_fire_for_test(
    registry: &Rc<RefCell<EntityRegistry>>,
    pawn: EntityId,
    command: &crate::weapon::WeaponFireCommand,
    collision_world: &CollisionWorld,
    hit_zone_store: &HitZoneStore,
    tick_dt: f32,
) {
    let mut no_impact = |_: &mut EntityRegistry| {};
    let _ = weapon_stage::run_local_weapon_command(
        registry,
        Some(pawn),
        false,
        None,
        command,
        false,
        collision_world,
        hit_zone_store,
        0.0,
        tick_dt,
        &mut no_impact,
    );
}

/// Advance only the connected client's local wieldable machine. Movement stays on
/// the prediction path and authoritative combat stays on the host; this pass owns
/// the immediate local lower/raise/repoint and suppresses fire while doing so.
#[allow(clippy::too_many_arguments)]
pub fn simulate_client_wieldable_tick(
    registry: Rc<RefCell<EntityRegistry>>,
    collision_world: &CollisionWorld,
    hit_zone_store: &HitZoneStore,
    pawn: Option<EntityId>,
    mod_block_during_reload: bool,
    select_slot: Option<usize>,
    fire_button: crate::weapon::FireButtonState,
    reload_held: bool,
    anim_time: f64,
    tick_dt: f32,
) -> (bool, Option<EntityId>) {
    let requested_new_slot = pawn.is_some_and(|pawn| {
        player_life::permits(
            &registry.borrow(),
            pawn,
            postretro_foundation::Capability::SwitchWeapon,
        ) && registry
            .borrow()
            .get_component::<postretro_entities::components::inventory::Inventory>(pawn)
            .is_ok_and(|inventory| {
                select_slot.is_some_and(|slot| {
                    slot != inventory.active_slot
                        && inventory.switch_target != Some(slot)
                        && inventory.wieldables.get(slot).copied().flatten().is_some()
                })
            })
    });
    let command = crate::weapon::WeaponFireCommand {
        button: fire_button,
        aim_origin: Vec3::ZERO,
        aim_direction: Vec3::Z,
        can_fire: false,
    };
    let _ = reload_held; // Activation prediction owns reload edge/cancellation.
    let result = weapon_stage::run_client_weapon_equip(
        &registry,
        pawn,
        mod_block_during_reload,
        select_slot,
        &command,
        collision_world,
        hit_zone_store,
        anim_time,
        tick_dt,
    );
    let accepted = requested_new_slot
        && match (pawn, select_slot) {
            (Some(pawn), Some(slot)) => registry
                .borrow()
                .get_component::<postretro_entities::components::inventory::Inventory>(pawn)
                .is_ok_and(|inventory| {
                    inventory.switch_target == Some(slot) || inventory.active_slot == slot
                }),
            _ => false,
        };
    (accepted, result.repointed_pawn)
}

#[cfg(test)]
fn player_is_alive_for_trigger_occupancy(
    registry: &EntityRegistry,
    player: &AuthoritativePlayer,
) -> bool {
    registry.exists(player.pawn)
        && registry
            .get_component::<HealthComponent>(player.pawn)
            .map_or(true, |health| {
                !scripting_systems::health::is_depleted(health)
            })
}

/// Host decisions in client-tick order, consumed by the remote weapon pass.
#[derive(Debug, Clone, Copy, Default)]
pub struct RemoteEquipGate {
    pub reload_refused: bool,
    pub begin_lower_after_machine: bool,
}

#[derive(Debug, Clone)]
pub struct RemotePawnCommand {
    /// Client tick of the recovered reload press, independent of playout trim.
    pub reload_press_tick: Option<u32>,
    pub equip: RemoteEquipGate,
    pub real_command: bool,
    pub rejected_activation: Option<postretro_foundation::ActivationToken>,
    pub pawn: EntityId,
    pub owner_client_id: u64,
    pub weapon: Option<EntityId>,
    pub shot_id: Option<ShotId>,
    pub fire_tick: u32,
    #[allow(dead_code)]
    pub client_tick: u32,
    pub aim_pitch: f32,
    pub command: SimCommand,
    /// Aim of the command that carried this tick's delivered activation start,
    /// captured at host intake. Only that shot's FIRE reconstruction reads it;
    /// movement and facing keep `command.movement`. `None` uses the delivering
    /// command's aim.
    pub start_aim: Option<RemoteStartAim>,
}

/// A client's declared aim, in radians, as one input command carried it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RemoteStartAim {
    pub pitch: f32,
    pub yaw: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RemoteActivationProgress {
    pub pawn: EntityId,
    pub owner_client_id: u64,
    pub weapon: EntityId,
    pub tick: u32,
    pub recovery_ms: f32,
    pub advance: crate::weapon::execution::WeaponActivationAdvance,
}

/// A host-only presentation launch for an accepted connected-client projectile
/// fire. The authoritative hit remains client-declared; this is only the data the
/// host needs to show that flight to observers through the existing snapshot path.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteProjectilePresentationLaunch {
    pub action: Option<std::sync::Arc<postretro_foundation::WeaponActivationDescriptor>>,
    pub model_scale: f32,
    pub owner_client_id: u64,
    pub shot_id: ShotId,
    pub origin: Vec3,
    pub direction: Vec3,
    pub range: f32,
    pub descriptor_class: String,
    pub projectile: postretro_foundation::ProjectileDescriptor,
}

/// Host-resolved projectile FIRE refusal. It reuses the existing owner-private
/// ShotVerdict wire fact so the client can stop its matching predicted flight
/// without waiting for a later impact or expiry declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteProjectileFireRejection {
    pub owner_client_id: u64,
    pub shot_id: ShotId,
}

/// Host-only inputs for the trigger stage. The system itself consumes the
/// per-player map, never an action snapshot; local and remote use edges are
/// keyed by `PlayerId` at this boundary.
pub struct TriggerTickContext<'a> {
    pub teleporters: Option<&'a crate::teleporter_system::TeleporterBindingTable>,
    pub system: &'a mut TriggerSystem,
    pub bridge: &'a TriggerVolumeBridge,
    pub bindings: &'a TriggerBindingTable,
    pub slot_table: Rc<RefCell<SlotTable>>,
    /// Present for production level installs and IR-aware harnesses. Literal
    /// fixtures may omit it and keep their direct table execution path.
    pub script_ctx: Option<ScriptCtx>,
    /// Host-only auto-close timer side table. Omitted by lightweight fixtures
    /// and never constructed for connected-client prediction.
    pub auto_close_timers: Option<kinematic_mover::MoverAutoCloseTimers>,
    pub use_edges: &'a HashMap<PlayerId, bool>,
}

#[derive(Debug, Default, PartialEq)]
pub struct TickEvents {
    pub remote_activation_progress: Vec<RemoteActivationProgress>,
    /// CPU stage values for this tick; empty when timing is off.
    pub cpu: postretro_stage_timing::StageFrame<cpu_stages::SimStage>,
    /// Local-pawn movement events, each on the pawn that raised it.
    pub movement: Vec<crate::emission::MovementEmission>,
    /// Ordered local-pawn state edges for render-rate view-feel presentation.
    /// This is frame-local, never serialized, and deliberately separate from
    /// the script-address drain above.
    pub movement_edges: Vec<crate::movement::MovementStateEdge>,
    /// Authoritative arrivals retained separately from local-only movement emissions.
    pub teleports: Vec<(EntityId, crate::movement::TeleportEvent)>,
    pub teleport_residuals: Vec<crate::teleporter_system::TeleportResidual>,
    /// AI events raised this tick, each on its enemy: attacks fired and
    /// activities entered (an entry with no authored `on_enter` fires nothing
    /// but still plays its sound).
    pub ai: Vec<crate::emission::AiEmission>,
    /// Weapon events this tick: fire, dry fire, spawn, and one impact per
    /// activation carrying its contacts (hitscan and projectile alike).
    pub weapon: Vec<crate::emission::WeaponEmission>,
    /// The local shooter's hitscan fluid splashes this tick, in shot and
    /// pellet order. Presentation facts beside the `impact` emission; a
    /// remote client's shot raises none here (peer splash is a follow-up).
    pub weapon_fluid_splashes: Vec<crate::weapon::FluidSplashFact>,
    /// Per-pellet cast points for determinism tests. Capture precedes impact
    /// policy, so tests compare the cast set rather than the applied subset.
    #[cfg(test)]
    pub(crate) weapon_impact_points: Vec<Vec3>,
    /// Host-local kinematic mover transition edges. A connected client never
    /// runs the host simulation seam, so its bucket remains empty.
    pub mover: Vec<(kinematic_mover::MoverEventKind, u32)>,
    pub death: Vec<String>,
    pub authorized_shots: Vec<OpenAuthorizedShot>,
    /// Host-only remote-fire visual launches. No component here is gameplay
    /// authoritative and nothing in this event crosses the wire directly.
    pub remote_projectile_presentation_launches: Vec<RemoteProjectilePresentationLaunch>,
    /// Prompt owner-private corrections for projectile FIRE attempts rejected by
    /// the host weapon gate. Accepted hitscan and pellet verdict timing is unchanged.
    pub rejected_remote_projectile_fires: Vec<RemoteProjectileFireRejection>,
    /// Locally simulated projectiles that a listen host mirrors for remote observers.
    /// The host's renderer suppresses the mirror and continues to draw this source.
    pub local_projectile_spawns: Vec<EntityId>,
    /// Host-authoritative enemy projectiles that need a presentation-only mirror for
    /// connected clients. The canonical weapon name is transient host event data:
    /// the existing snapshot carries it as the presentation entity's `entity_class`.
    pub enemy_projectile_spawns: Vec<EnemyProjectilePresentationSpawn>,
    /// Host-authoritative projectile-trap shots, mirrored for connected clients
    /// exactly as enemy projectiles are, classed by the trap's weapon.
    pub trap_projectile_spawns: Vec<EnemyProjectilePresentationSpawn>,
    /// Locally simulated projectile contacts that retire listen-host mirror flights.
    pub local_projectile_contacts: Vec<ProjectileContactEvent>,
    pub reload_deliveries: Vec<ReloadDelivery>,
    /// `reload_deliveries` stamped for presentation when they fired, in the
    /// same order: each anchored at its reloading pawn and naming its weapon's
    /// descriptor, as `weapon` emissions are. The frame drain reads these, so a
    /// pawn or weapon gone by then still sounds its reload.
    pub reload: Vec<crate::emission::WeaponEmission>,
    /// Pawns whose active inventory slot repointed this tick. Presentation drains
    /// this after simulation so the hand socket follows committed ownership, never
    /// a pending selection.
    pub repointed_pawns: Vec<EntityId>,
    /// The host drop path restores meshes outside spawn-context resolution, so it
    /// reports them here for clip binding in the same fixed tick, before rendering.
    pub dropped_item_meshes: Vec<EntityId>,
    /// Bound trigger residuals drained app-side after every fixed tick this frame,
    /// each carried with the `(trigger, player)` that fired it. The origin rides
    /// alongside the opaque handle so the frame-end drain can key an enrolled
    /// timed-reaction instance to its activator — two players entering one plate
    /// push the same handle twice and must not collapse into one instance (O59).
    pub trigger_residuals: Vec<(TriggerResidualHandle, EntityId, PlayerId, Vec<PlayerId>)>,
    /// This tick's paired-trigger Exit fires, as `(trigger, player)`. Production
    /// reads them in the `RedrawRequested` arm to cancel matching interruptible
    /// timed-reaction instances before their countdown advances (O4). Derived from
    /// `TriggerEvent.fire` filtered by `edge == Exit`; a non-`cfg(test)` field
    /// because the scheduler consumes it in release, unlike `trigger_fires`.
    pub trigger_exit_fires: Vec<(EntityId, PlayerId)>,
    /// This tick's assist completions, in `players()` order of the helper, at
    /// most one per target. Each carries the target's life state and elapsed
    /// at record for the player-event pass's re-check.
    pub assist_completions: Vec<assist::AssistCompletion>,
    /// This tick's Triggers-stage player pawns, captured before any trigger
    /// reaction or damage. The host derives session player status from it.
    pub trigger_stage_pawns: player_life::TriggerStagePawns,
    /// Test-only fixed-tick trace. Production consumes residual handles only;
    /// keeping the detailed sequence out of non-test builds avoids a hot-path
    /// diagnostic allocation.
    #[cfg(test)]
    pub(crate) trigger_fires: Vec<TriggerEvent>,
    #[cfg(test)]
    pub(crate) trigger_command_fires: Vec<TriggerCommandFire>,
}

/// A host-owned enemy projectile and the resolved weapon descriptor that supplies
/// its presentation class. This is deliberately an in-process tick event, never a
/// replicated gameplay component or a new wire record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnemyProjectilePresentationSpawn {
    pub projectile: EntityId,
    pub descriptor_class: String,
}

/// Advance transient presentation work on a connected client. Connected clients
/// do not run [`simulate_tick`], so their predicted and observer-materialized
/// impact lights need an explicit per-frame deferred-effect queue boundary.
pub fn advance_client_presentation_effects(registry: &mut EntityRegistry, frame_dt: f32) {
    crate::impact_effects::tick_deferred_effects(registry, frame_dt);
}

/// Advance the authoritative live sentiment overlay before any impact producer
/// or AI consumer runs this tick. The caller deliberately scopes the `RefMut`
/// to this helper so AI can take its transient immutable live view afterward.
fn decay_faction_sentiment(
    factions: &FactionRegistry,
    faction_sentiment: &RefCell<FactionSentimentState>,
    tick_dt: f32,
) {
    faction_sentiment.borrow_mut().decay_step(
        tick_dt,
        |from, to| factions.sentiment_decay(from as f32, to as f32),
        |from, to| factions.sentiment(from as f32, to as f32),
    );
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriggerCommandFire {
    pub(crate) event: TriggerEvent,
    pub(crate) commands: Vec<crate::trigger_bindings::BoundTriggerCommandKind>,
}

#[allow(clippy::too_many_arguments, dead_code)]
pub fn simulate_tick<AiResult>(
    registry: Rc<RefCell<EntityRegistry>>,
    collision_world: &CollisionWorld,
    hit_zone_store: &HitZoneStore,
    nav_graph: Option<&NavGraph>,
    environment: EnvironmentView<'_>,
    _legacy_active_wieldable: Option<EntityId>,
    anim_time: f64,
    _progress_tracker: &mut ProgressTracker,
    run_ai: impl FnMut(&mut EntityRegistry, f32, AiTickInputs<'_>, &mut SimAiHost<'_>) -> AiResult,
    mover_colliders: &[MoverCollider],
    mover_tick_states: &mut MoverTickStateTable,
    remote_pawn_commands: &[RemotePawnCommand],
    command: &SimCommand,
    post_movement: impl FnMut(&Rc<RefCell<EntityRegistry>>) -> PostMovementCommand,
    tick_dt: f32,
    trigger_context: Option<TriggerTickContext<'_>>,
    on_impact: impl FnMut(&mut EntityRegistry),
) -> TickEvents
where
    AiResult: Into<AiTickResult>,
{
    let mut touch_system = TouchSystem::default();
    let mut assist_state = assist::AssistState::default();
    let touch_edges = HashMap::new();
    let factions = FactionRegistry::default();
    let faction_sentiment = RefCell::new(FactionSentimentState::default());
    simulate_tick_with_presentation_aim(
        registry,
        collision_world,
        hit_zone_store,
        nav_graph,
        environment,
        false,
        anim_time,
        (0.0, 0.0),
        _progress_tracker,
        run_ai,
        mover_colliders,
        mover_tick_states,
        remote_pawn_commands,
        command,
        post_movement,
        tick_dt,
        &mut touch_system,
        &mut assist_state,
        &[],
        0,
        &factions,
        &faction_sentiment,
        None,
        &touch_edges,
        &touch_edges,
        trigger_context,
        |_, _| {},
        on_impact,
        crate::sim::TimingGate::OFF,
    )
}

mod pose_inputs;
mod status_stage;
mod tick;
pub use pose_inputs::*;
pub use tick::*;

mod host_movement;
mod movement_overlap;
#[cfg(test)]
mod movement_overlap_tests;
mod reload;
pub(crate) mod weapon_stage;

pub use reload::{ReloadDelivery, ReloadOutcome};
pub use reload::{
    clear_feedback_for_weapon as clear_reload_feedback_for_weapon,
    clear_owner_feedback_for_weapons as clear_owner_reload_feedback_for_weapons,
};
pub use weapon_stage::apply_authorized_weapon_impact_damage;

pub use host_movement::drop_vault_if_barred;
#[cfg(any(test, feature = "test-support"))]
pub use host_movement::run_host_movement_tick;
pub use movement_overlap::report_static_overlap;

// Hosts `SimHarness` (`frame`/`new`/`tick`/`record`); the tick-driven
// timed-reaction ordering tests live in its child module.
#[cfg(test)]
mod attack_pool_tick_tests;
#[cfg(test)]
pub(crate) mod determinism_tests;
#[cfg(test)]
mod divergence_spike_tests;
#[cfg(test)]
mod member_tick_test_fixtures;
#[cfg(test)]
mod mover_pin_tick_tests;
#[cfg(test)]
pub(crate) mod predict_reconcile;
#[cfg(test)]
pub(crate) mod teleport_tick_tests;
#[cfg(test)]
mod trap_tick_tests;

struct LocalMovementTickEvents {
    stepped_pawn: Option<EntityId>,
    teleport: Option<(EntityId, crate::movement::TeleportEvent)>,
    emissions: Vec<crate::emission::MovementEmission>,
    state_edges: Vec<crate::movement::MovementStateEdge>,
}

/// Local-pawn movement stage. Resolves the local movement pawn from the registry
/// marker and advances it through `host_movement::run_host_movement_tick` as a
/// one-pawn list. Single-player and the listen host both move their own pawn here;
/// the host moves remote pawns in a separate, earlier call.
fn run_movement_tick(
    registry: &Rc<RefCell<EntityRegistry>>,
    collision: &impl crate::movement::MovementCollisionSource,
    environment: EnvironmentView<'_>,
    input: &MovementInput,
    tick_dt: f32,
) -> LocalMovementTickEvents {
    let local = {
        let registry = registry.borrow();
        registry.local_player_movement_pawn()
    };
    let Some(id) = local else {
        return LocalMovementTickEvents {
            stepped_pawn: None,
            teleport: None,
            emissions: Vec::new(),
            state_edges: Vec::new(),
        };
    };

    let pawn_inputs = [(id, input.clone())];
    let mut registry = registry.borrow_mut();
    let events = host_movement::run_host_movement_tick(
        &mut registry,
        collision,
        environment,
        &pawn_inputs,
        tick_dt,
    );
    let stepped_pawn = events.iter().any(|(pawn, _)| *pawn == id).then_some(id);
    let emitter = crate::emission::entity_emitter(&registry, id);
    let mut emissions = Vec::new();
    let mut state_edges = Vec::new();
    let mut teleport = None;
    for (pawn, movement_events) in events {
        if pawn == id {
            teleport = movement_events.teleport.map(|event| (pawn, event));
            crate::emission::append_movement_emissions(&movement_events, &emitter, &mut emissions);
            state_edges = movement_events.state_edges;
        }
    }
    LocalMovementTickEvents {
        stepped_pawn,
        teleport,
        emissions,
        state_edges,
    }
}
pub fn run_death_sweep(registry: &Rc<RefCell<EntityRegistry>>) -> Vec<String> {
    let report = {
        let mut registry = registry.borrow_mut();
        scripting_systems::health::sweep_deaths(&mut registry)
    };

    let mut events = Vec::new();
    if report.player_died {
        events.push(scripting_systems::health::PLAYER_DIED_EVENT.to_string());
    }
    events
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collision::CollisionWorld;
    use crate::kinematic_mover::MoverTickStateTable;
    use crate::scripting::reactions::registry::ReactionPrimitiveRegistry;
    use crate::scripting::reactions::system_commands::{
        SystemReactionRegistry, register_system_reaction_primitives,
    };
    use crate::scripting_systems::hit_zones::HitZoneStore;
    use crate::trigger_bindings::TriggerBindingTable;
    use crate::trigger_system::TriggerEventEdge;
    use crate::weapon::FireButtonState;
    use glam::Vec2;
    use postretro_entities::components::agent::AgentComponent;
    use postretro_entities::components::inventory::Inventory;
    use postretro_entities::components::mesh::{
        AnimationState, DEFAULT_CROSSFADE_MS, InterruptPolicy, LocomotionDirection, MeshAnimation,
        MeshComponent, resolve_pending_animation_stamps,
    };
    use postretro_entities::components::touchable::TouchableComponent;
    use postretro_entities::data_descriptors::{
        BehaviorActivityDescriptor, BehaviorGraphDescriptor, BehaviorGraphEnvelope, MotionVerb,
    };
    use postretro_entities::{
        DescriptorComponentKind, DescriptorMapOverride, DescriptorProvenance, DescriptorSpawnPath,
    };

    /// A direct-graph brain staged directly into its `alert` state.
    fn alert_brain(move_speed: f32) -> BrainComponent {
        let graph = BehaviorGraphDescriptor {
            knockback: Default::default(),
            envelope: BehaviorGraphEnvelope {
                initial: "idle".to_string(),
                activities: BTreeMap::from([
                    (
                        "idle".to_string(),
                        BehaviorActivityDescriptor {
                            pose: None,
                            stagger_to: None,
                            passes: None,
                            sound: None,
                            animation: Some("idle".to_string()),
                            motion: Some(MotionVerb::Hold),
                            action: None,
                            on_enter: None,
                            facing: None,
                            reposition: None,
                            attack_turn: None,
                            speed: None,
                            layers: BTreeMap::new(),
                        },
                    ),
                    (
                        "alert".to_string(),
                        BehaviorActivityDescriptor {
                            pose: None,
                            stagger_to: None,
                            passes: None,
                            sound: None,
                            animation: Some("walk".to_string()),
                            motion: Some(MotionVerb::ChaseTarget),
                            action: None,
                            on_enter: None,
                            facing: None,
                            reposition: None,
                            attack_turn: None,
                            speed: None,
                            layers: BTreeMap::new(),
                        },
                    ),
                ]),
                transitions: BTreeMap::new(),
            },
            candidate_filter: None,
            retaliation: None,
            patrol: None,
            attacks: Default::default(),
            engagement_radius: None,
            move_speed,
        };
        let mut brain = BrainComponent::from_graph(&graph);
        assert!(
            brain.enter_activity_at(
                0,
                brain
                    .graph
                    .envelope
                    .activities
                    .keys()
                    .position(|name| name == "alert")
                    .expect("the graph declares `alert`"),
            )
        );
        brain
    }
    use postretro_entities::{
        AmmoReserve, DataRegistry, KinematicMoverComponent, KinematicMoverMode, NamedReaction,
        NumericRange, PrimitiveDescriptor, ReactionDescriptor, SlotOwnership, SlotRecord,
        SlotSchema, SlotTable, SlotType, SlotValue, Transform, TriggerActivation, TriggerCommand,
        TriggerFireMode, TriggerVolumeComponent,
    };
    use postretro_foundation::{
        AirParams, AmmoResource, CapsuleParams, FallParams, GroundParams, PlayerMovementComponent,
        PlayerMovementDescriptor, ReloadStyle, ResolutionMode, SpeedParams, WeaponDescriptor,
        WeaponResource,
    };
    use postretro_net::wire::NetworkId;
    use postretro_scripting_core::reaction_dispatch::{
        ResidualOrigin, fire_prepartitioned_reactions_with_sequences,
    };
    use postretro_scripting_core::sequence::SequencedPrimitiveRegistry;
    use std::collections::{BTreeMap, HashMap};

    #[test]
    fn trigger_effective_occupancy_uses_live_health_and_allows_healthless_pawns() {
        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform::default());
        let player = AuthoritativePlayer {
            id: PlayerId::Local(pawn),
            pawn,
        };
        assert!(player_is_alive_for_trigger_occupancy(&registry, &player));
        registry
            .set_component(
                pawn,
                HealthComponent {
                    max: 100.0,
                    current: 0.0,
                    hitbox: None,
                    death_handled: true,
                    pending_kill_credit: None,
                    zone_multipliers: Default::default(),
                    contributor_ledger: Default::default(),
                },
            )
            .unwrap();
        assert!(registry.exists(pawn), "death does not remove the pawn");
        assert!(!player_is_alive_for_trigger_occupancy(&registry, &player));

        let mut health = registry
            .get_component::<HealthComponent>(pawn)
            .unwrap()
            .clone();
        health.death_handled = false;
        registry.set_component(pawn, health).unwrap();
        assert!(!player_is_alive_for_trigger_occupancy(&registry, &player));

        postretro_entities::components::health::set_health_absolute(&mut registry, pawn, 25.0);
        assert!(player_is_alive_for_trigger_occupancy(&registry, &player));

        let mut health = registry
            .get_component::<HealthComponent>(pawn)
            .unwrap()
            .clone();
        health.current = f32::NAN;
        registry.set_component(pawn, health).unwrap();
        assert!(!player_is_alive_for_trigger_occupancy(&registry, &player));

        registry.despawn(pawn).unwrap();

        assert!(!player_is_alive_for_trigger_occupancy(&registry, &player));
    }

    #[test]
    fn disconnected_remote_fire_context_keeps_trigger_and_occupancy_but_has_no_activator() {
        let trigger = EntityId::from_raw(0x0001_0000);
        let event = TriggerEvent {
            fire: crate::trigger_system::TriggerEventFire {
                trigger,
                player: PlayerId::Remote(77),
                event_name: "left".into(),
            },
            edge: TriggerEventEdge::Exit,
        };

        let context = trigger_fire_context(&event, 3, &BTreeMap::new());
        assert!(context.activator.is_none());
        assert_eq!(context.fired_trigger, Some(trigger));
        assert_eq!(context.occupancy, 3);
    }

    // Regression: the fire context scanned every matching remote command and
    // broadcast one trigger event to duplicate remote pawns.
    #[test]
    fn remote_fire_context_uses_the_collision_canonical_pawn() {
        let trigger = EntityId::from_raw(0x0001_0000);
        let canonical_pawn = EntityId::from_raw(0x0001_0001);
        let duplicate_pawn = EntityId::from_raw(0x0001_0002);
        let event = TriggerEvent {
            fire: crate::trigger_system::TriggerEventFire {
                trigger,
                player: PlayerId::Remote(77),
                event_name: "pressed".into(),
            },
            edge: TriggerEventEdge::Enter,
        };
        let canonical_players = BTreeMap::from([(PlayerId::Remote(77), canonical_pawn)]);

        let context = trigger_fire_context(&event, 1, &canonical_players);
        assert_eq!(context.activator, Some(canonical_pawn));
        assert_ne!(context.activator, Some(duplicate_pawn));
    }

    #[test]
    fn local_fire_context_resolves_the_local_entity_without_player_list_entry() {
        let local_pawn = EntityId::from_raw(0x0001_0001);
        let event = TriggerEvent {
            fire: crate::trigger_system::TriggerEventFire {
                trigger: EntityId::from_raw(0x0001_0000),
                player: PlayerId::Local(local_pawn),
                event_name: "pressed".into(),
            },
            edge: TriggerEventEdge::Enter,
        };

        assert_eq!(
            trigger_fire_context(&event, 1, &BTreeMap::new()).activator,
            Some(local_pawn),
        );
    }

    pub(super) fn weapon_component(credit_source: &str) -> WeaponComponent {
        WeaponComponent::from_descriptor(&WeaponDescriptor {
            sounds: None,
            knockback: None,
            damage: 10.0,
            pellet_count: 1,
            spread_degrees: 0.0,
            bloom_per_shot_degrees: 0.0,
            bloom_max_degrees: 0.0,
            bloom_decay_degrees_per_second: 0.0,
            bloom_decay_delay_ms: 0.0,
            movement_spread_degrees: 0.0,
            spread_vertical_bias: 0.0,
            range: 100.0,
            primary: postretro_foundation::WeaponActivationDescriptor::single(
                postretro_foundation::ActivationTrigger::Press,
                100.0,
            ),
            secondary: None,
            resolution: ResolutionMode::Hitscan,
            projectile: None,
            splash: None,
            credit_source: Some(credit_source.to_string()),
            third_person_model: None,
            viewmodel: None,
            placement: None,
            muzzle_offset: None,
            resource: None,
            lower_ms: 0,
            raise_ms: 0,
            block_during_reload: None,
            damage_type: None,
        })
    }

    pub(super) fn ammo_weapon_component(
        credit_source: &str,
        capacity: u32,
        reserve: u32,
        reload_ms: u32,
    ) -> (WeaponComponent, AmmoReserve) {
        let descriptor = WeaponDescriptor {
            sounds: None,
            knockback: None,
            damage: 10.0,
            pellet_count: 1,
            spread_degrees: 0.0,
            bloom_per_shot_degrees: 0.0,
            bloom_max_degrees: 0.0,
            bloom_decay_degrees_per_second: 0.0,
            bloom_decay_delay_ms: 0.0,
            movement_spread_degrees: 0.0,
            spread_vertical_bias: 0.0,
            range: 100.0,
            primary: postretro_foundation::WeaponActivationDescriptor::single(
                postretro_foundation::ActivationTrigger::Press,
                100.0,
            ),
            secondary: None,
            resolution: ResolutionMode::Hitscan,
            projectile: None,
            splash: None,
            credit_source: Some(credit_source.to_string()),
            third_person_model: None,
            viewmodel: None,
            placement: None,
            muzzle_offset: None,
            resource: Some(WeaponResource::Ammo(AmmoResource {
                ammo_type: "bullets.light".to_string(),
                magazine: capacity,
                cost_per_shot: 1,
                reserve,
                reload_ms,
                reload_style: ReloadStyle::Magazine,
            })),
            lower_ms: 0,
            raise_ms: 0,
            block_during_reload: None,
            damage_type: None,
        };
        let descriptor = descriptor.validate().unwrap();
        let mut ammo_reserve = AmmoReserve::new();
        ammo_reserve.credit("bullets.light", reserve);
        (WeaponComponent::from_descriptor(&descriptor), ammo_reserve)
    }

    pub(super) fn zero_movement() -> MovementInput {
        MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
            facing_pitch: 0.0,
            use_pressed: false,
            drop_pressed: false,
        }
    }

    pub(super) fn sim_command(fire: bool, reload: bool) -> SimCommand {
        SimCommand {
            assist_held: false,
            input_tick: 0,
            secondary_button: crate::weapon::FireButtonState {
                pressed: false,
                active: false,
            },
            activation: postretro_foundation::ActivationInput::default(),
            movement: zero_movement(),
            fire_button: FireButtonState {
                pressed: fire,
                active: fire,
            },
            reload,
            firing_slot: 0,
            select_slot: None,
            use_pressed: false,
            drop_pressed: false,
        }
    }

    pub(super) fn sim_command_at(input_tick: u32, fire: bool, reload: bool) -> SimCommand {
        SimCommand {
            input_tick,
            ..sim_command(fire, reload)
        }
    }

    pub(super) fn spawn_reload_pair(
        registry: &mut EntityRegistry,
        capacity: u32,
        reserve: u32,
        reload_ms: u32,
        magazine: u32,
    ) -> (EntityId, EntityId) {
        let pawn = registry.spawn(Transform::default());
        let weapon = registry.spawn(Transform::default());
        let (mut component, reserve_component) =
            ammo_weapon_component("weapon.test.reload", capacity, reserve, reload_ms);
        component.magazine = magazine;
        registry.set_component(weapon, component).unwrap();
        registry.set_component(pawn, reserve_component).unwrap();
        let mut inventory = Inventory::default();
        inventory.wieldables[0] = Some(weapon);
        registry.set_component(pawn, inventory).unwrap();
        (pawn, weapon)
    }

    pub(super) fn trigger_movement() -> PlayerMovementComponent {
        PlayerMovementComponent::from_descriptor(&PlayerMovementDescriptor {
            sounds: None,
            knockback: Default::default(),
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
                friction: GroundParams::DEFAULT_FRICTION,
                stop_speed: GroundParams::DEFAULT_STOP_SPEED,
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
            slide: None,
            swim: None,
            vault: None,
            view_feel: None,
        })
    }

    fn animated_mesh() -> MeshComponent {
        let mut states = HashMap::new();
        for (name, clip_index) in [("idle", 0), ("attack", 1)] {
            states.insert(
                name.to_string(),
                AnimationState {
                    clip: format!("{name}_clip"),
                    looping: name == "idle",
                    crossfade_ms: DEFAULT_CROSSFADE_MS,
                    interrupt: InterruptPolicy::Smooth,
                    travel_speed: None,
                    max_playback_rate: None,
                    scale_with_speed: true,
                    playback_rate: None,
                    clip_index: Some(clip_index),
                },
            );
        }
        MeshComponent::animated(
            "test_model".to_string(),
            MeshAnimation::new(states, "idle".into()),
        )
    }

    // Regression: held fire on a fresh wieldable reused the outgoing instance's shot identity.
    #[test]
    fn local_activation_input_ticks_survive_weapon_switch_and_preserve_authored_waits() {
        use postretro_foundation::{ActivationLane, ActivationToken};

        for first_tick in [0, u32::MAX - 1] {
            let descriptor = serde_json::from_value::<WeaponDescriptor>(serde_json::json!({
                "damage": 10, "range": 100, "resolution": "hitscan",
                "primary": { "trigger": "hold", "recoveryMs": 0, "steps": [
                    { "kind": "shot" }, { "kind": "wait", "durationMs": 50 }, { "kind": "shot" }
                ] },
                "resource": { "kind": "ammo", "type": "rounds", "magazine": 10, "reserve": 0 },
                "lowerMs": 0, "raiseMs": 0,
            }))
            .unwrap()
            .validate()
            .unwrap();
            let registry = Rc::new(RefCell::new(EntityRegistry::new()));
            let (pawn, outgoing, incoming) = {
                let mut world = registry.borrow_mut();
                let pawn = world.spawn(Transform::default());
                world.set_component(pawn, trigger_movement()).unwrap();
                world.mark_local_player_pawn(pawn).unwrap();
                let outgoing = world.spawn(Transform::default());
                let incoming = world.spawn(Transform::default());
                for weapon in [outgoing, incoming] {
                    world
                        .set_component(weapon, WeaponComponent::from_descriptor(&descriptor))
                        .unwrap();
                }
                let mut inventory = Inventory::default();
                inventory.wieldables[0] = Some(outgoing);
                inventory.wieldables[1] = Some(incoming);
                world.set_component(pawn, inventory).unwrap();
                (pawn, outgoing, incoming)
            };
            let world = CollisionWorld::new();
            let hit_zones = HitZoneStore::new();
            let mut progress = ProgressTracker::new();
            let mut ai = postretro_ai::AiRuntime::new();
            let mut movers = MoverTickStateTable::default();
            let mut run = |offset: u32, select_slot: Option<usize>| {
                let input_tick = first_tick.wrapping_add(offset);
                let mut command = sim_command_at(input_tick, false, false);
                command.select_slot = select_slot;
                command.fire_button = FireButtonState {
                    pressed: offset == 0,
                    active: true,
                };
                if offset == 0 {
                    command.activation.initiation = Some(ActivationToken {
                        start_tick: input_tick,
                        lane: ActivationLane::Primary,
                    });
                }
                let events = simulate_tick(
                    registry.clone(),
                    &world,
                    &hit_zones,
                    None,
                    postretro_collision::EnvironmentView::level_only(0.0),
                    None,
                    0.0,
                    &mut progress,
                    postretro_ai::test_tick_runner!(&mut ai),
                    &[],
                    &mut movers,
                    &[],
                    &command,
                    |_| PostMovementCommand {
                        aim_origin: Vec3::ZERO,
                        aim_direction: Vec3::NEG_Z,
                    },
                    1.0 / 60.0,
                    None,
                    |_| {},
                );
                events
                    .weapon
                    .into_iter()
                    .filter(|event| event.address == "activate")
                    .map(|event| {
                        event
                            .shot_id
                            .expect("local activation emission retains the shot identity")
                    })
                    .collect::<Vec<_>>()
            };
            let outgoing_shot = run(0, None);
            assert_eq!(
                outgoing_shot,
                [ShotId::from_parts(
                    0,
                    first_tick,
                    ActivationLane::Primary,
                    0
                )]
            );
            assert!(
                run(1, Some(1)).is_empty(),
                "switch cancels the outgoing wait before another shot"
            );
            assert_eq!(
                registry
                    .borrow()
                    .get_component::<Inventory>(pawn)
                    .unwrap()
                    .active_wieldable(),
                Some(incoming)
            );
            let incoming_start = first_tick.wrapping_add(2);
            let incoming_shot = run(2, None);
            assert_eq!(
                incoming_shot,
                [ShotId::from_parts(
                    0,
                    incoming_start,
                    ActivationLane::Primary,
                    0
                )]
            );
            assert_ne!(
                outgoing_shot, incoming_shot,
                "a held restart names its actual input tick across instances"
            );
            assert!(run(3, None).is_empty());
            assert!(run(4, None).is_empty());
            assert_eq!(
                run(5, None),
                [ShotId::from_parts(
                    0,
                    incoming_start,
                    ActivationLane::Primary,
                    1
                )],
                "the 50 ms wait remains three fixed ticks across wrap"
            );
            assert_eq!(
                run(6, None),
                [ShotId::from_parts(
                    0,
                    first_tick.wrapping_add(6),
                    ActivationLane::Primary,
                    0
                )],
                "every held restart receives another logical input identity"
            );
            let registry = registry.borrow();
            assert_eq!(
                registry
                    .get_component::<WeaponComponent>(outgoing)
                    .unwrap()
                    .magazine,
                9
            );
            assert_eq!(
                registry
                    .get_component::<WeaponComponent>(incoming)
                    .unwrap()
                    .magazine,
                7
            );
        }
    }

    #[test]
    fn player_travel_heading_uses_negative_z_as_yaw_zero() {
        let epsilon = 1.0e-5;
        let forward = LocomotionDirection::Forward;
        assert!(player_travel_heading_yaw(Vec3::NEG_Z, forward, 9.0).abs() < epsilon);
        assert!(
            (player_travel_heading_yaw(Vec3::X, forward, 9.0) + std::f32::consts::FRAC_PI_2).abs()
                < epsilon
        );
        assert_eq!(player_travel_heading_yaw(Vec3::ZERO, forward, 0.75), 0.75);
    }

    // Regression: host locomotion hard-coded an `idle` state while connected clients
    // used the descriptor's valid `defaultState`, causing perpetual correction for
    // descriptors whose rest state had another author-defined name.
    #[test]
    fn player_locomotion_uses_descriptor_default_state_as_idle_contract() {
        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform::default());
        registry.set_component(pawn, trigger_movement()).unwrap();

        let state = |clip: &str, clip_index| AnimationState {
            clip: clip.to_string(),
            looping: true,
            crossfade_ms: DEFAULT_CROSSFADE_MS,
            interrupt: InterruptPolicy::Smooth,
            travel_speed: None,
            max_playback_rate: None,
            scale_with_speed: true,
            playback_rate: None,
            clip_index: Some(clip_index),
        };
        let states = HashMap::from([
            ("stand".to_string(), state("stand", 0)),
            ("walk_forward".to_string(), state("walk", 1)),
        ]);
        let mut animation = MeshAnimation::new(states, "stand".to_string());
        animation.current_state = "walk_forward".to_string();
        registry
            .set_component(
                pawn,
                MeshComponent::animated("player".to_string(), animation),
            )
            .unwrap();

        assert_eq!(
            registry
                .get_component::<PlayerMovementComponent>(pawn)
                .unwrap()
                .velocity,
            Vec3::ZERO,
            "the descriptor default state applies only when the player is stationary"
        );

        update_player_animation_locomotion(&mut registry, &HitZoneStore::new(), 0.0);

        assert_eq!(
            registry
                .get_component::<MeshComponent>(pawn)
                .unwrap()
                .animation
                .as_ref()
                .unwrap()
                .current_state,
            "stand"
        );
    }

    #[test]
    fn player_locomotion_backpedal_selects_backward_states_per_tier() {
        let mut registry = EntityRegistry::new();
        // Identity transform: aim yaw zero faces -Z, so +Z travel is a backpedal.
        let pawn = registry.spawn(Transform::default());
        let state = |clip: &str, clip_index: Option<usize>, travel_speed| AnimationState {
            clip: clip.to_string(),
            looping: true,
            crossfade_ms: DEFAULT_CROSSFADE_MS,
            interrupt: InterruptPolicy::Smooth,
            travel_speed,
            max_playback_rate: None,
            scale_with_speed: true,
            playback_rate: None,
            clip_index,
        };
        let set_states = |registry: &mut EntityRegistry, states: Vec<(&str, AnimationState)>| {
            let states = states
                .into_iter()
                .map(|(name, state)| (name.to_string(), state))
                .collect();
            registry
                .set_component(
                    pawn,
                    MeshComponent::animated(
                        "player".to_string(),
                        MeshAnimation::new(states, "idle".to_string()),
                    ),
                )
                .unwrap();
        };
        let step = |registry: &mut EntityRegistry, velocity: Vec3| {
            let mut movement = trigger_movement();
            movement.velocity = velocity;
            registry.set_component(pawn, movement).unwrap();
            update_player_animation_locomotion(registry, &HitZoneStore::new(), 0.0);
            let animation = registry
                .get_component::<MeshComponent>(pawn)
                .unwrap()
                .animation
                .clone()
                .unwrap();
            (
                animation.current_state.clone(),
                animation.locomotion_direction,
                animation.rate,
            )
        };
        let backward = LocomotionDirection::Backward;
        let forward = LocomotionDirection::Forward;

        set_states(
            &mut registry,
            vec![
                ("idle", state("idle", Some(0), None)),
                ("walk_forward", state("walk_forward", Some(1), Some(4.0))),
                ("walk_backward", state("walk_backward", Some(2), Some(5.0))),
                ("run_forward", state("run_forward", Some(3), None)),
                ("run_backward", state("run_backward", Some(4), None)),
            ],
        );
        let (state_name, direction, rate) = step(&mut registry, Vec3::Z * 5.0);
        assert_eq!(
            (state_name.as_str(), direction),
            ("walk_backward", backward)
        );
        assert!(
            (rate - 1.0).abs() < 1.0e-5,
            "rate-scaled by walk_backward's own travelSpeed"
        );
        // 10 m/s clears the run tier's entry (walk/run midpoint 9 plus the band, 9.6).
        let (state_name, direction, _) = step(&mut registry, Vec3::Z * 10.0);
        assert_eq!((state_name.as_str(), direction), ("run_backward", backward));
        // Pure strafe sits in the dead zone: the backpedal holds.
        let (state_name, direction, _) = step(&mut registry, Vec3::X * 5.0);
        assert_eq!(
            (state_name.as_str(), direction),
            ("walk_backward", backward)
        );
        // Stopping keeps the memory, so a delayed presentation still drawing the
        // last of the strafe keeps its fold.
        let (state_name, direction, _) = step(&mut registry, Vec3::ZERO);
        assert_eq!((state_name.as_str(), direction), ("idle", backward));
        let (state_name, direction, _) = step(&mut registry, Vec3::NEG_Z * 10.0);
        assert_eq!((state_name.as_str(), direction), ("run_forward", forward));
        let (state_name, direction, rate) = step(&mut registry, Vec3::NEG_Z * 5.0);
        assert_eq!((state_name.as_str(), direction), ("walk_forward", forward));
        assert!(
            (rate - 1.25).abs() < 1.0e-5,
            "rate-scaled by walk_forward's own travelSpeed"
        );
        let (state_name, direction, _) = step(&mut registry, Vec3::ZERO);
        assert_eq!((state_name.as_str(), direction), ("idle", forward));

        // A backward state whose clip did not resolve plays the forward clip, and
        // the legs still backpedal.
        set_states(
            &mut registry,
            vec![
                ("idle", state("idle", Some(0), None)),
                ("walk_forward", state("walk_forward", Some(1), None)),
                ("walk_backward", state("walk_backward", None, None)),
            ],
        );
        let (state_name, direction, _) = step(&mut registry, Vec3::Z * 5.0);
        assert_eq!((state_name.as_str(), direction), ("walk_forward", backward));
    }

    #[test]
    fn player_locomotion_crouch_and_slide_select_crouch_states_by_movement_state() {
        use postretro_foundation::MovementState;
        let mut registry = EntityRegistry::new();
        // Identity transform: aim yaw zero faces -Z, so +Z travel is a backpedal.
        let pawn = registry.spawn(Transform::default());
        let state = |clip: &str, clip_index, travel_speed| AnimationState {
            clip: clip.to_string(),
            looping: true,
            crossfade_ms: DEFAULT_CROSSFADE_MS,
            interrupt: InterruptPolicy::Smooth,
            travel_speed,
            max_playback_rate: None,
            scale_with_speed: true,
            playback_rate: None,
            clip_index: Some(clip_index),
        };
        let states = HashMap::from([
            ("idle".to_string(), state("idle", 0, None)),
            ("walk_forward".to_string(), state("walk_forward", 1, None)),
            ("walk_backward".to_string(), state("walk_backward", 2, None)),
            ("crouch_idle".to_string(), state("crouch_idle", 3, None)),
            (
                "crouch_forward".to_string(),
                state("crouch_forward", 4, None),
            ),
            (
                "crouch_backward".to_string(),
                state("crouch_backward", 5, Some(2.0)),
            ),
        ]);
        registry
            .set_component(
                pawn,
                MeshComponent::animated(
                    "player".to_string(),
                    MeshAnimation::new(states, "idle".to_string()),
                ),
            )
            .unwrap();
        let step = |registry: &mut EntityRegistry, movement_state, velocity: Vec3| {
            let mut movement = trigger_movement();
            movement.movement_state = movement_state;
            movement.velocity = velocity;
            registry.set_component(pawn, movement).unwrap();
            update_player_animation_locomotion(registry, &HitZoneStore::new(), 0.0);
            let animation = registry
                .get_component::<MeshComponent>(pawn)
                .unwrap()
                .animation
                .clone()
                .unwrap();
            (
                animation.current_state.clone(),
                animation.locomotion_direction,
                animation.rate,
            )
        };
        let crouching = MovementState::Crouching { eye_current: 0.3 };
        let sliding = MovementState::Sliding {
            elapsed_ms: 40.0,
            boost: Vec3::NEG_Z * 4.0,
            eye_current: 0.3,
        };

        let (state_name, _, rate) = step(&mut registry, crouching, Vec3::ZERO);
        assert_eq!(state_name, "crouch_idle");
        assert!((rate - 1.0).abs() < 1.0e-5);
        // `ground.speed.crouch` is 3: 2.4 m/s plays at 0.8, where walk speed's
        // 7 would clamp the rate to its 0.5 floor.
        let (state_name, direction, rate) = step(&mut registry, crouching, Vec3::NEG_Z * 2.4);
        assert_eq!(
            (state_name.as_str(), direction),
            ("crouch_forward", LocomotionDirection::Forward)
        );
        assert!(
            (rate - 0.8).abs() < 1.0e-5,
            "rate-scaled against crouch speed, got {rate}"
        );
        let (state_name, direction, rate) = step(&mut registry, crouching, Vec3::Z * 2.4);
        assert_eq!(
            (state_name.as_str(), direction),
            ("crouch_backward", LocomotionDirection::Backward)
        );
        assert!(
            (rate - 1.2).abs() < 1.0e-5,
            "rate-scaled by crouch_backward's own travelSpeed, got {rate}"
        );
        // A slide holds `crouch_idle` at the authored rate whatever its speed.
        let (state_name, _, rate) = step(&mut registry, sliding, Vec3::NEG_Z * 12.0);
        assert_eq!(state_name, "crouch_idle");
        assert!(
            (rate - 1.0).abs() < 1.0e-5,
            "slide plays unscaled, got {rate}"
        );
        // Standing up returns to the walk tier.
        let (state_name, direction, _) =
            step(&mut registry, MovementState::Normal, Vec3::NEG_Z * 5.0);
        assert_eq!(
            (state_name.as_str(), direction),
            ("walk_forward", LocomotionDirection::Forward)
        );
    }

    #[test]
    fn player_locomotion_jump_sequence_plays_jump_states_at_the_authored_rate() {
        use crate::scripting_systems::hit_zones::ModelHitZones;
        use postretro_foundation::GroundRef;
        use postretro_model::ModelHandle;
        use postretro_model::skeleton::{AnimationClip, Skeleton};

        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform::default());
        let state = |clip: &str, clip_index, looping, travel_speed| AnimationState {
            clip: clip.to_string(),
            looping,
            crossfade_ms: DEFAULT_CROSSFADE_MS,
            interrupt: InterruptPolicy::Smooth,
            travel_speed,
            max_playback_rate: None,
            scale_with_speed: true,
            playback_rate: None,
            clip_index: Some(clip_index),
        };
        let states = HashMap::from([
            ("idle".to_string(), state("idle", 0, true, None)),
            (
                "walk_forward".to_string(),
                state("walk_forward", 1, true, Some(4.0)),
            ),
            (
                "run_forward".to_string(),
                state("run_forward", 2, true, Some(8.0)),
            ),
            ("jump_up".to_string(), state("jump_up", 3, false, None)),
            ("jump_loop".to_string(), state("jump_loop", 4, true, None)),
            ("jump_down".to_string(), state("jump_down", 5, false, None)),
        ]);
        registry
            .set_component(
                pawn,
                MeshComponent::animated(
                    "player".to_string(),
                    MeshAnimation::new(states, "idle".to_string()),
                ),
            )
            .unwrap();
        // The one-shots' lengths come from the model's clips: `jump_up` 0.4 s,
        // `jump_down` 0.3 s.
        let clip = |name: &str, duration| AnimationClip {
            name: name.to_string(),
            duration,
            joints: Vec::new(),
            travel_speed: None,
            events: Vec::new(),
        };
        let mut hit_zones = HitZoneStore::new();
        hit_zones.insert_for_test(
            ModelHandle::from("player"),
            ModelHitZones {
                skeleton: std::sync::Arc::new(Skeleton { joints: Vec::new() }),
                clips: std::sync::Arc::new(vec![
                    clip("idle", 1.0),
                    clip("walk_forward", 1.0),
                    clip("run_forward", 1.0),
                    clip("jump_up", 0.4),
                    clip("jump_loop", 1.0),
                    clip("jump_down", 0.3),
                ]),
                joint_zones: Vec::new(),
                sockets: HashMap::new(),
                derived_bound: None,
                legs: Vec::new(),
                pose_stack: Default::default(),
            },
        );
        // `air_ticks` 0 is grounded; otherwise airborne that many ticks, with the
        // jump spent when `jumped`.
        let step = |registry: &mut EntityRegistry,
                    anim_time: f64,
                    air_ticks: u32,
                    jumped: bool,
                    velocity: Vec3| {
            let mut movement = trigger_movement();
            movement.ground = if air_ticks == 0 {
                GroundRef::World
            } else {
                GroundRef::Airborne
            };
            movement.air_ticks = air_ticks;
            movement.jump_spent = jumped;
            movement.velocity = velocity;
            registry.set_component(pawn, movement).unwrap();
            update_player_animation_locomotion(registry, &hit_zones, anim_time);
            let animation = registry
                .get_component::<MeshComponent>(pawn)
                .unwrap()
                .animation
                .clone()
                .unwrap();
            (animation.current_state.clone(), animation.rate)
        };
        let walking = Vec3::NEG_Z * 5.0;
        let authored = |(state, rate): (String, f32)| {
            assert!((rate - 1.0).abs() < 1.0e-5, "{state} rate {rate}");
            state
        };

        let (state, rate) = step(&mut registry, 0.0, 0, false, walking);
        assert_eq!(state, "walk_forward");
        assert!((rate - 1.25).abs() < 1.0e-5, "walk rate-scales, got {rate}");
        // Takeoff, rising, apex.
        let rising = Vec3::new(0.0, 6.0, -5.0);
        assert_eq!(
            authored(step(&mut registry, 0.02, 1, true, rising)),
            "jump_up"
        );
        let near_apex = Vec3::new(0.0, 0.5, -5.0);
        assert_eq!(
            authored(step(&mut registry, 0.2, 12, true, near_apex)),
            "jump_up"
        );
        let falling = Vec3::new(0.0, -0.5, -3.0);
        assert_eq!(
            authored(step(&mut registry, 0.22, 13, true, falling)),
            "jump_loop"
        );
        // A walking-pace landing plays `jump_down` until its clip ends.
        let landed = Vec3::NEG_Z * 3.0;
        assert_eq!(
            authored(step(&mut registry, 0.5, 0, false, landed)),
            "jump_down"
        );
        assert_eq!(
            authored(step(&mut registry, 0.79, 0, false, landed)),
            "jump_down"
        );
        let (state, rate) = step(&mut registry, 0.81, 0, false, landed);
        assert_eq!(state, "walk_forward");
        assert!((rate - 0.75).abs() < 1.0e-5, "walk rate-scales, got {rate}");

        // A running jump lands back in the stride.
        let running = Vec3::NEG_Z * 10.0;
        assert_eq!(
            authored(step(&mut registry, 1.0, 1, true, running + Vec3::Y * 6.0)),
            "jump_up"
        );
        assert_eq!(
            authored(step(&mut registry, 1.3, 18, true, running - Vec3::Y)),
            "jump_loop"
        );
        assert_eq!(step(&mut registry, 1.5, 0, false, running).0, "run_forward");
    }

    fn trigger_component(on_fire: &str, armed: bool) -> TriggerVolumeComponent {
        TriggerVolumeComponent::new(
            TriggerActivation::Touch,
            String::new(),
            on_fire.to_string(),
            String::new(),
            TriggerCommand::Start,
            TriggerFireMode::Multiple,
            0.0,
            armed,
        )
    }

    fn trigger_slots() -> SlotTable {
        let mut slots = SlotTable::new();
        slots
            .insert_namespace(
                "trigger",
                vec![(
                    "flag".into(),
                    SlotRecord::new(SlotSchema {
                        slot_type: SlotType::Number,
                        default: Some(SlotValue::Number(0.0)),
                        range: Some(NumericRange { min: 0.0, max: 1.0 }),
                        persist: false,
                        readonly: false,
                        ownership: SlotOwnership::Mod,
                        network: Default::default(),
                        per_owner: false,
                        accumulate: None,
                    }),
                )],
            )
            .unwrap();
        slots
    }

    pub(super) fn remote_command(
        pawn: EntityId,
        weapon: Option<EntityId>,
        network_id: u32,
        client_tick: u32,
        fire: bool,
        reload: bool,
    ) -> RemotePawnCommand {
        RemotePawnCommand {
            reload_press_tick: None,
            equip: RemoteEquipGate::default(),
            real_command: true,
            rejected_activation: None,
            pawn,
            owner_client_id: 7,
            weapon,
            shot_id: Some(ShotId::from_parts(
                (NetworkId(network_id)).0,
                client_tick,
                postretro_foundation::ActivationLane::Primary,
                0,
            )),
            fire_tick: client_tick,
            client_tick,
            aim_pitch: 0.0,
            start_aim: None,
            command: {
                let mut command = sim_command(fire, reload);
                if fire {
                    command.activation.initiation = Some(postretro_foundation::ActivationToken {
                        start_tick: client_tick,
                        lane: postretro_foundation::ActivationLane::Primary,
                    });
                }
                command
            },
        }
    }

    pub(super) fn run_remote_only_tick(
        registry: Rc<RefCell<EntityRegistry>>,
        remote: &[RemotePawnCommand],
    ) -> TickEvents {
        let world = CollisionWorld::new();
        let hit_zones = HitZoneStore::new();
        let mut progress = ProgressTracker::new();
        let mut ai_runtime = postretro_ai::AiRuntime::new();
        let mover_colliders = Vec::new();
        let mut mover_states = MoverTickStateTable::default();
        simulate_tick(
            registry,
            &world,
            &hit_zones,
            None,
            postretro_collision::EnvironmentView::level_only(-9.81),
            None,
            0.0,
            &mut progress,
            postretro_ai::test_tick_runner!(&mut ai_runtime),
            &mover_colliders,
            &mut mover_states,
            remote,
            &sim_command(false, false),
            |_| PostMovementCommand {
                aim_origin: Vec3::ZERO,
                aim_direction: Vec3::NEG_Z,
            },
            1.0 / 60.0,
            None,
            |_| {},
        )
    }

    fn timed_tick(timing: crate::sim::TimingGate) -> TickEvents {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        {
            let mut registry = registry.borrow_mut();
            let pawn = registry.spawn(Transform::default());
            registry.set_component(pawn, trigger_movement()).unwrap();
            registry.mark_local_player_pawn(pawn).unwrap();
        }
        let world = CollisionWorld::new();
        let hit_zones = HitZoneStore::new();
        let mut progress = ProgressTracker::new();
        let mut ai_runtime = postretro_ai::AiRuntime::new();
        let mut mover_states = MoverTickStateTable::default();
        let mut touch_system = TouchSystem::default();
        let edges = HashMap::new();
        simulate_tick_with_presentation_aim(
            registry,
            &world,
            &hit_zones,
            None,
            postretro_collision::EnvironmentView::level_only(-9.81),
            false,
            0.0,
            (0.0, 0.0),
            &mut progress,
            postretro_ai::test_tick_runner!(&mut ai_runtime),
            &[],
            &mut mover_states,
            &[],
            &sim_command(false, false),
            |_| PostMovementCommand {
                aim_origin: Vec3::ZERO,
                aim_direction: Vec3::NEG_Z,
            },
            1.0 / 60.0,
            &mut touch_system,
            &mut Default::default(),
            &[],
            0,
            &FactionRegistry::default(),
            &RefCell::new(FactionSentimentState::default()),
            None,
            &edges,
            &edges,
            None,
            |_, _| {},
            |_| {},
            timing,
        )
    }

    #[test]
    fn timed_tick_reports_every_substage_inside_the_tick() {
        use postretro_stage_timing::StageSet;
        let events = timed_tick(crate::sim::TimingGate::ON);
        let tick = events
            .cpu
            .value(cpu_stages::SimStage::Tick)
            .expect("tick timed");
        // `PlayerEvents` runs after the tick returns, in the frame loop.
        // `WeaponFluidCrossings` runs only in a tick that fires a hitscan;
        // `timed_hitscan_tick_reports_fluid_crossings_inside_weapons` covers it.
        // AI's sub-stages cannot cross this test's two-build AI runner; the AI
        // crate's `stage_timing` tests drive the same-build runner instead.
        for &stage in cpu_stages::SimStage::ALL[1..].iter().filter(|stage| {
            stage.parent().is_some()
                && **stage != cpu_stages::SimStage::WeaponFluidCrossings
                && stage.parent() != Some(cpu_stages::SimStage::Ai)
        }) {
            let value = events
                .cpu
                .value(stage)
                .unwrap_or_else(|| panic!("{stage:?} ran"));
            assert!(value <= tick, "{stage:?} {value} ns exceeds tick {tick} ns");
        }
        assert_eq!(
            events.cpu.value(cpu_stages::SimStage::WeaponFluidCrossings),
            None,
            "no hitscan fired, so no crossing stage"
        );
    }

    #[test]
    fn untimed_tick_reports_no_stage() {
        use postretro_stage_timing::StageSet;
        let events = timed_tick(crate::sim::TimingGate::OFF);
        assert!(
            cpu_stages::SimStage::ALL
                .iter()
                .all(|&stage| events.cpu.value(stage).is_none())
        );
    }

    #[test]
    fn touch_runs_without_a_trigger_context_before_ai() {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (pawn, item) = {
            let mut registry = registry.borrow_mut();
            let pawn = registry.spawn(Transform::default());
            registry.set_component(pawn, trigger_movement()).unwrap();
            registry.set_component(pawn, Inventory::default()).unwrap();
            registry.mark_local_player_pawn(pawn).unwrap();

            let item = registry.spawn(Transform::default());
            registry
                .set_component(
                    item,
                    TouchableComponent {
                        mode: postretro_entities::TouchMode::Auto,
                        radius: 1.0,
                    },
                )
                .unwrap();
            registry
                .set_component(item, weapon_component("weapon.touch"))
                .unwrap();
            registry
                .set_component(
                    item,
                    DescriptorProvenance {
                        canonical_name: "weapon.touch".to_string(),
                        owned_components: std::collections::BTreeSet::from([
                            DescriptorComponentKind::Weapon,
                            DescriptorComponentKind::Touchable,
                        ]),
                        map_overrides: std::collections::BTreeSet::<DescriptorMapOverride>::new(),
                        spawn_path: DescriptorSpawnPath::MapPlacement,
                    },
                )
                .unwrap();
            registry
                .set_component(item, MeshComponent::stateless("weapon.glb".to_string()))
                .unwrap();
            (pawn, item)
        };
        let world = CollisionWorld::new();
        let hit_zones = HitZoneStore::new();
        let mut progress = ProgressTracker::new();
        let mut ai_runtime = postretro_ai::AiRuntime::new();
        let mut mover_states = MoverTickStateTable::default();
        let mut touch_system = TouchSystem::default();
        let edges = HashMap::new();

        let events = simulate_tick_with_presentation_aim(
            registry.clone(),
            &world,
            &hit_zones,
            None,
            postretro_collision::EnvironmentView::level_only(-9.81),
            false,
            0.0,
            (0.0, 0.0),
            &mut progress,
            postretro_ai::test_tick_runner!(&mut ai_runtime),
            &[],
            &mut mover_states,
            &[],
            &sim_command(false, false),
            |_| PostMovementCommand {
                aim_origin: Vec3::ZERO,
                aim_direction: Vec3::NEG_Z,
            },
            1.0 / 60.0,
            &mut touch_system,
            &mut Default::default(),
            &[],
            0,
            &FactionRegistry::default(),
            &RefCell::new(FactionSentimentState::default()),
            None,
            &edges,
            &edges,
            None,
            |_, _| {},
            |_| {},
            crate::sim::TimingGate::OFF,
        );

        assert_eq!(
            registry
                .borrow()
                .get_component::<Inventory>(pawn)
                .unwrap()
                .wieldables[0],
            Some(item)
        );
        assert_eq!(events.repointed_pawns, vec![pawn]);
        assert!(
            registry
                .borrow()
                .get_component::<TouchableComponent>(item)
                .is_err()
        );
    }

    #[test]
    fn use_edge_activates_trigger_and_press_item_in_the_same_fixed_tick() {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (pawn, item, trigger) = {
            let mut registry = registry.borrow_mut();
            let pawn = registry.spawn(Transform::default());
            registry.set_component(pawn, trigger_movement()).unwrap();
            registry.set_component(pawn, Inventory::default()).unwrap();
            registry.mark_local_player_pawn(pawn).unwrap();

            let item = registry.spawn(Transform::default());
            registry
                .set_component(
                    item,
                    TouchableComponent {
                        mode: postretro_entities::TouchMode::Press,
                        radius: 1.0,
                    },
                )
                .unwrap();
            registry
                .set_component(item, weapon_component("weapon.press"))
                .unwrap();
            registry
                .set_component(
                    item,
                    DescriptorProvenance {
                        canonical_name: "weapon.press".to_string(),
                        owned_components: std::collections::BTreeSet::from([
                            DescriptorComponentKind::Weapon,
                            DescriptorComponentKind::Touchable,
                        ]),
                        map_overrides: std::collections::BTreeSet::<DescriptorMapOverride>::new(),
                        spawn_path: DescriptorSpawnPath::MapPlacement,
                    },
                )
                .unwrap();
            registry
                .set_component(item, MeshComponent::stateless("weapon.glb".to_string()))
                .unwrap();

            let trigger = registry.spawn(Transform::default());
            registry
                .set_component(
                    trigger,
                    TriggerVolumeComponent::new(
                        TriggerActivation::Use,
                        String::new(),
                        "use_target".to_string(),
                        String::new(),
                        TriggerCommand::Start,
                        TriggerFireMode::Multiple,
                        0.0,
                        true,
                    ),
                )
                .unwrap();
            (pawn, item, trigger)
        };
        let world = CollisionWorld::new();
        let hit_zones = HitZoneStore::new();
        let mut progress = ProgressTracker::new();
        let mut ai_runtime = postretro_ai::AiRuntime::new();
        let mut mover_states = MoverTickStateTable::default();
        let mut touch_system = TouchSystem::default();
        let mut trigger_system = TriggerSystem::default();
        let mut bridge = TriggerVolumeBridge::new();
        bridge.insert_for_test(trigger, Vec3::splat(-1.0), Vec3::splat(1.0));
        let bindings = TriggerBindingTable::default();
        let slot_table = Rc::new(RefCell::new(SlotTable::new()));
        let use_edges = HashMap::from([(PlayerId::Local(pawn), true)]);

        let events = simulate_tick_with_presentation_aim(
            registry.clone(),
            &world,
            &hit_zones,
            None,
            postretro_collision::EnvironmentView::level_only(-9.81),
            false,
            0.0,
            (0.0, 0.0),
            &mut progress,
            postretro_ai::test_tick_runner!(&mut ai_runtime),
            &[],
            &mut mover_states,
            &[],
            &sim_command(false, false),
            |_| PostMovementCommand {
                aim_origin: Vec3::ZERO,
                aim_direction: Vec3::NEG_Z,
            },
            1.0 / 60.0,
            &mut touch_system,
            &mut Default::default(),
            &[],
            0,
            &FactionRegistry::default(),
            &RefCell::new(FactionSentimentState::default()),
            None,
            &use_edges,
            &HashMap::new(),
            Some(TriggerTickContext {
                teleporters: None,
                system: &mut trigger_system,
                bridge: &bridge,
                bindings: &bindings,
                slot_table,
                script_ctx: None,
                auto_close_timers: None,
                use_edges: &use_edges,
            }),
            |_, _| {},
            |_| {},
            crate::sim::TimingGate::OFF,
        );

        assert_eq!(
            events.trigger_fires.len(),
            1,
            "the trigger stage consumes the shared Use edge first"
        );
        assert_eq!(events.trigger_fires[0].fire.trigger, trigger);
        assert_eq!(
            registry
                .borrow()
                .get_component::<Inventory>(pawn)
                .unwrap()
                .wieldables[0],
            Some(item),
            "the later touch stage reads the same Use edge instead of a consumed one"
        );
    }

    #[test]
    fn unowned_world_weapon_keeps_all_live_state_after_many_fixed_ticks() {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let weapon = {
            let mut registry = registry.borrow_mut();
            let pawn = registry.spawn(Transform::default());
            registry.set_component(pawn, trigger_movement()).unwrap();
            registry.set_component(pawn, Inventory::default()).unwrap();
            registry.mark_local_player_pawn(pawn).unwrap();

            let weapon = registry.spawn(Transform {
                position: Vec3::new(10.0, 0.0, 0.0),
                ..Transform::default()
            });
            let mut component = weapon_component("weapon.unowned");
            component.state =
                postretro_entities::components::wieldable_state::WieldableState::Reloading;
            component.state_remaining_ms = 800;
            component.state_total_ms = 1_000;
            component.state_elapsed_sub_ms = 0.5;
            component.reload_credited = 1;
            component.cooldown_remaining_ms = 75.0;
            component.shoot_press_consumed = true;
            component.reload_press_consumed = true;
            registry.set_component(weapon, component).unwrap();
            weapon
        };
        let before = registry
            .borrow()
            .get_component::<WeaponComponent>(weapon)
            .unwrap()
            .clone();

        for _ in 0..5 {
            run_local_only_tick(
                registry.clone(),
                weapon,
                &sim_command(false, false),
                1.0 / 60.0,
            );
        }

        assert_eq!(
            registry
                .borrow()
                .get_component::<WeaponComponent>(weapon)
                .unwrap(),
            &before,
            "an unowned world weapon is never advanced by the weapon stage"
        );
    }

    pub(super) fn run_local_only_tick(
        registry: Rc<RefCell<EntityRegistry>>,
        weapon: EntityId,
        command: &SimCommand,
        tick_dt: f32,
    ) -> TickEvents {
        let world = CollisionWorld::new();
        let hit_zones = HitZoneStore::new();
        let mut progress = ProgressTracker::new();
        let mut ai_runtime = postretro_ai::AiRuntime::new();
        let mut mover_states = MoverTickStateTable::default();
        simulate_tick(
            registry,
            &world,
            &hit_zones,
            None,
            postretro_collision::EnvironmentView::level_only(-9.81),
            Some(weapon),
            0.0,
            &mut progress,
            postretro_ai::test_tick_runner!(&mut ai_runtime),
            &[],
            &mut mover_states,
            &[],
            command,
            |_| PostMovementCommand {
                aim_origin: Vec3::ZERO,
                aim_direction: Vec3::NEG_Z,
            },
            tick_dt,
            None,
            |_| {},
        )
    }

    // Regression: the simulation counted seated zero-HP corpses as alive and
    // froze their seats into on.trigger presentation audiences.
    #[test]
    fn trigger_tick_excludes_seated_corpses_from_count_and_frozen_residual_audience() {
        use crate::residual_drain::{ResidualRegistries, drain_frame_residuals};
        use crate::scripting_systems::reaction_scheduler::ReactionScheduler;
        use postretro_entities::{Recipients, SystemReactionCommand};
        use postretro_foundation::Seat;

        let script_ctx = ScriptCtx::new();
        *script_ctx.slot_table.borrow_mut() = trigger_slots();
        let registry = script_ctx.registry.clone();
        let outside = Vec3::new(10.0, 0.0, 0.0);
        let (live_pawn, dead_pawn, trigger) = {
            let mut registry = registry.borrow_mut();
            let live_pawn = registry.spawn(Transform {
                position: outside,
                ..Transform::default()
            });
            let dead_pawn = registry.spawn(Transform::default());
            for (pawn, seat, current, death_handled) in [
                (live_pawn, Seat(1), 100.0, false),
                (dead_pawn, Seat(2), 0.0, true),
            ] {
                registry.set_component(pawn, trigger_movement()).unwrap();
                registry
                    .set_component(
                        pawn,
                        HealthComponent {
                            max: 100.0,
                            current,
                            hitbox: None,
                            death_handled,
                            pending_kill_credit: None,
                            zone_multipliers: Default::default(),
                            contributor_ledger: Default::default(),
                        },
                    )
                    .unwrap();
                registry.bind_pawn_seat(pawn, seat);
            }
            registry.mark_local_player_pawn(live_pawn).unwrap();
            let trigger = registry.spawn(Transform::default());
            let mut component = trigger_component("occupants", true);
            component.on_exit = "occupants".into();
            registry.set_component(trigger, component).unwrap();
            (live_pawn, dead_pawn, trigger)
        };
        let mut data = DataRegistry::new();
        data.populate_level(
            [
                (
                    "setState",
                    serde_json::json!({
                        "slot": "trigger.flag",
                        "value": { "op": "input", "name": "@occupancy" }
                    }),
                ),
                (
                    "flashScreen",
                    serde_json::json!({
                        "color": [1.0, 0.0, 0.0, 1.0],
                        "durationMs": 30.0,
                        "audience": "@trigger"
                    }),
                ),
            ]
            .into_iter()
            .map(|(primitive, args)| NamedReaction {
                name: "occupants".into(),
                descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                    primitive: primitive.into(),
                    target: None,
                    kind: None,
                    tag: None,
                    on_complete: None,
                    args,
                }),
            })
            .collect(),
            Vec::new(),
            &[],
        );
        let bindings =
            TriggerBindingTable::build_with_script_ctx(&registry.borrow(), &data, &script_ctx);
        let mut bridge = TriggerVolumeBridge::new();
        bridge.insert_for_test(trigger, Vec3::splat(-4.0), Vec3::splat(4.0));
        let mut trigger_system = TriggerSystem::default();
        let mut progress = ProgressTracker::new();
        let mut ai_runtime = postretro_ai::AiRuntime::new();
        let mut mover_states = MoverTickStateTable::default();
        let world = CollisionWorld::new();
        let hit_zones = HitZoneStore::new();
        let use_edges = HashMap::new();
        let remote = [remote_command(dead_pawn, None, 2, 0, false, false)];
        let mut tick = |trigger_system: &mut TriggerSystem| {
            simulate_tick(
                registry.clone(),
                &world,
                &hit_zones,
                None,
                postretro_collision::EnvironmentView::level_only(0.0),
                None,
                0.0,
                &mut progress,
                postretro_ai::test_tick_runner!(&mut ai_runtime),
                &[],
                &mut mover_states,
                &remote,
                &sim_command(false, false),
                |_| PostMovementCommand {
                    aim_origin: Vec3::ZERO,
                    aim_direction: Vec3::NEG_Z,
                },
                1.0 / 60.0,
                Some(TriggerTickContext {
                    teleporters: None,
                    system: trigger_system,
                    bridge: &bridge,
                    bindings: &bindings,
                    slot_table: script_ctx.slot_table.clone(),
                    script_ctx: Some(script_ctx.clone()),
                    auto_close_timers: None,
                    use_edges: &use_edges,
                }),
                |_| {},
            )
        };
        let count = || match script_ctx
            .slot_table
            .borrow()
            .get("trigger.flag")
            .and_then(|record| record.value.as_ref())
        {
            Some(SlotValue::Number(value)) => *value,
            other => panic!("expected effective occupancy, got {other:?}"),
        };
        let move_pawn = |pawn, position| {
            registry
                .borrow_mut()
                .set_component(
                    pawn,
                    Transform {
                        position,
                        ..Transform::default()
                    },
                )
                .unwrap();
        };

        let dead_enter = tick(&mut trigger_system);
        assert_eq!(trigger_system.occupancy(trigger), 1);
        assert_eq!(dead_enter.trigger_residuals.len(), 1);
        assert!(dead_enter.trigger_residuals[0].3.is_empty());
        assert!(count().abs() <= 1.0e-5);

        move_pawn(live_pawn, Vec3::ZERO);
        let live_enter = tick(&mut trigger_system);
        assert_eq!(trigger_system.occupancy(trigger), 2);
        assert_eq!(live_enter.trigger_residuals.len(), 1);
        assert_eq!(
            live_enter.trigger_residuals[0].3,
            vec![PlayerId::Local(live_pawn)]
        );
        assert!((count() - 1.0).abs() <= 1.0e-5);

        move_pawn(live_pawn, outside);
        let live_exit = tick(&mut trigger_system);
        assert_eq!(trigger_system.occupancy(trigger), 1);
        assert_eq!(
            live_exit.trigger_exit_fires,
            vec![(trigger, PlayerId::Local(live_pawn))]
        );
        assert_eq!(live_exit.trigger_residuals.len(), 1);
        assert!(live_exit.trigger_residuals[0].3.is_empty());
        assert!(count().abs() <= 1.0e-5);
        assert!(
            trigger_system
                .paired_enters()
                .contains(&(trigger, PlayerId::Remote(7)))
        );

        let mut system_registry = SystemReactionRegistry::new();
        register_system_reaction_primitives(&mut system_registry);
        let mut follow_ups = Vec::new();
        drain_frame_residuals(
            &live_enter.trigger_residuals,
            &bindings,
            &trigger_system,
            &[],
            &crate::player_events::PlayerEventTable::default(),
            &ReactionScheduler::default(),
            ResidualRegistries {
                sequence: &SequencedPrimitiveRegistry::new(),
                reaction: &ReactionPrimitiveRegistry::new(),
                system: &system_registry,
            },
            &script_ctx,
            &mut follow_ups,
            |player, registry| match player {
                PlayerId::Local(pawn) => registry.seat_for_pawn(pawn),
                PlayerId::Remote(7) => registry.seat_for_pawn(dead_pawn),
                PlayerId::Remote(_) => None,
            },
        );
        assert!(follow_ups.is_empty());
        assert!(matches!(
            script_ctx.system_commands.take_ordered().as_slice(),
            [(Recipients::Seats(seats), SystemReactionCommand::FlashScreen { .. })]
                if seats == &vec![Seat(1)]
        ));
        assert_eq!(registry.borrow().seat_for_pawn(dead_pawn), Some(Seat(2)));

        move_pawn(dead_pawn, outside);
        let dead_exit = tick(&mut trigger_system);
        assert_eq!(
            dead_exit.trigger_exit_fires,
            vec![(trigger, PlayerId::Remote(7))]
        );
        assert_eq!(trigger_system.occupancy(trigger), 0);
        assert!(
            !trigger_system
                .paired_enters()
                .contains(&(trigger, PlayerId::Remote(7)))
        );
    }

    #[test]
    fn trigger_consequences_run_in_tick_once_and_residual_reaches_app_drain() {
        let script_ctx = postretro_entities::ScriptCtx::new();
        *script_ctx.slot_table.borrow_mut() = trigger_slots();
        script_ctx
            .slot_table
            .borrow_mut()
            .insert(
                "trigger.count".to_string(),
                SlotRecord::new(SlotSchema {
                    slot_type: SlotType::Number,
                    default: Some(SlotValue::Number(0.0)),
                    range: Some(NumericRange {
                        min: 0.0,
                        max: 100.0,
                    }),
                    persist: false,
                    readonly: false,
                    ownership: SlotOwnership::Mod,
                    network: Default::default(),
                    per_owner: false,
                    accumulate: None,
                }),
            )
            .expect("trigger count slot should be vacant");
        let registry = script_ctx.registry.clone();
        let (source_trigger, mover, animated_target, arm_target) = {
            let mut registry = registry.borrow_mut();
            let player = registry.spawn(Transform::default());
            registry.set_component(player, trigger_movement()).unwrap();

            let source_trigger = registry.spawn(Transform::default());
            registry
                .set_component(source_trigger, trigger_component("triggered", true))
                .unwrap();

            let mover = registry.spawn(Transform::default());
            registry.set_tags(mover, vec!["door".into()]).unwrap();
            registry
                .set_component(
                    mover,
                    KinematicMoverComponent::new(
                        1,
                        postretro_entities::KinematicMoverConfig {
                            waypoints: vec![Vec3::ZERO, Vec3::new(2.0, 0.0, 0.0)],
                            waypoint_names: vec!["start".into(), "end".into()],
                            speed_mps: 1.0,
                            wait_ms: 0.0,
                            mode: KinematicMoverMode::Once,
                            started: false,
                            spin_axis: Vec3::ZERO,
                            initial_spin_rate_rad_s: 0.0,
                            spin_accel_rad_s2: 0.0,
                            carry_yaw: false,
                        },
                    ),
                )
                .unwrap();

            let animated_target = registry.spawn(Transform::default());
            registry
                .set_tags(animated_target, vec!["enemy".into()])
                .unwrap();
            registry
                .set_component(animated_target, animated_mesh())
                .unwrap();
            registry
                .set_component(
                    animated_target,
                    HealthComponent {
                        max: 100.0,
                        current: 100.0,
                        hitbox: None,
                        death_handled: false,
                        pending_kill_credit: None,
                        zone_multipliers: Default::default(),
                        contributor_ledger: Default::default(),
                    },
                )
                .unwrap();
            resolve_pending_animation_stamps(&mut registry, 0.0);

            let arm_target = registry.spawn(Transform::default());
            registry.set_tags(arm_target, vec!["rearm".into()]).unwrap();
            registry
                .set_component(arm_target, trigger_component("", false))
                .unwrap();

            (source_trigger, mover, animated_target, arm_target)
        };

        let mut data = DataRegistry::new();
        let primitive =
            |primitive: &str, tag: Option<&str>, args: serde_json::Value| NamedReaction {
                name: "triggered".into(),
                descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                    primitive: primitive.into(),
                    target: None,
                    kind: None,
                    tag: tag.map(str::to_string),
                    on_complete: None,
                    args,
                }),
            };
        data.populate_level(
            vec![
                primitive("moverStart", Some("door"), serde_json::json!({})),
                primitive(
                    "applyDamage",
                    Some("enemy"),
                    serde_json::json!({ "amount": 10 }),
                ),
                primitive(
                    "setAnimationState",
                    Some("enemy"),
                    serde_json::json!({ "state": "attack" }),
                ),
                primitive(
                    "setState",
                    None,
                    serde_json::json!({ "slot": "trigger.flag", "value": 1 }),
                ),
                primitive(
                    "setState",
                    None,
                    serde_json::json!({
                        "slot": "trigger.count",
                        "value": {
                            "op": "add",
                            "a": { "op": "input", "name": "trigger.count" },
                            "b": { "op": "const", "value": 1.0 }
                        }
                    }),
                ),
                primitive(
                    "setState",
                    None,
                    serde_json::json!({
                        "slot": "trigger.count",
                        "value": {
                            "op": "add",
                            "a": { "op": "input", "name": "trigger.count" },
                            "b": { "op": "const", "value": 1.0 }
                        }
                    }),
                ),
                primitive("armTrigger", Some("rearm"), serde_json::json!({})),
                primitive(
                    "flashScreen",
                    None,
                    serde_json::json!({ "color": [1.0, 0.0, 0.0, 1.0], "durationMs": 30.0 }),
                ),
            ],
            Vec::new(),
            &[],
        );
        let bindings =
            TriggerBindingTable::build_with_script_ctx(&registry.borrow(), &data, &script_ctx);
        let mut bridge = TriggerVolumeBridge::new();
        bridge.insert_for_test(source_trigger, Vec3::splat(-4.0), Vec3::splat(4.0));
        let mut trigger_system = TriggerSystem::default();
        let mut progress = ProgressTracker::new();
        let mut ai_runtime = postretro_ai::AiRuntime::new();
        let mut mover_states = MoverTickStateTable::default();
        let world = CollisionWorld::new();
        let hit_zones = HitZoneStore::new();
        let use_edges = HashMap::new();

        let events = simulate_tick(
            registry.clone(),
            &world,
            &hit_zones,
            None,
            postretro_collision::EnvironmentView::level_only(-9.81),
            None,
            0.0,
            &mut progress,
            postretro_ai::test_tick_runner!(&mut ai_runtime),
            &[],
            &mut mover_states,
            &[],
            &sim_command(false, false),
            |_| PostMovementCommand {
                aim_origin: Vec3::ZERO,
                aim_direction: Vec3::NEG_Z,
            },
            1.0 / 60.0,
            Some(TriggerTickContext {
                teleporters: None,
                system: &mut trigger_system,
                bridge: &bridge,
                bindings: &bindings,
                slot_table: script_ctx.slot_table.clone(),
                script_ctx: Some(script_ctx.clone()),
                auto_close_timers: None,
                use_edges: &use_edges,
            }),
            |_| {},
        );

        assert_eq!(events.trigger_residuals.len(), 1);
        assert_eq!(events.trigger_command_fires.len(), 1);
        assert_eq!(
            events.trigger_command_fires[0].commands,
            vec![
                crate::trigger_bindings::BoundTriggerCommandKind::Mover,
                crate::trigger_bindings::BoundTriggerCommandKind::Damage,
                crate::trigger_bindings::BoundTriggerCommandKind::AnimationState,
                crate::trigger_bindings::BoundTriggerCommandKind::StoreSlot,
                crate::trigger_bindings::BoundTriggerCommandKind::StoreSlot,
                crate::trigger_bindings::BoundTriggerCommandKind::StoreSlot,
                crate::trigger_bindings::BoundTriggerCommandKind::Arm,
            ],
            "each consequential command must cross the fixed-tick boundary once; final mover, arm, slot, and animation state alone are idempotent"
        );
        let registry_ref = registry.borrow();
        assert!(
            registry_ref
                .get_component::<KinematicMoverComponent>(mover)
                .unwrap()
                .started
        );
        assert_eq!(
            registry_ref
                .get_component::<HealthComponent>(animated_target)
                .unwrap()
                .current,
            90.0
        );
        assert_eq!(
            registry_ref
                .get_component::<MeshComponent>(animated_target)
                .unwrap()
                .animation
                .as_ref()
                .unwrap()
                .current_state,
            "attack"
        );
        assert!(
            registry_ref
                .get_component::<TriggerVolumeComponent>(arm_target)
                .unwrap()
                .armed
        );
        drop(registry_ref);
        let trigger_flag = match script_ctx
            .slot_table
            .borrow()
            .get("trigger.flag")
            .and_then(|record| record.value.as_ref())
        {
            Some(SlotValue::Number(value)) => *value,
            other => panic!("expected numeric trigger flag, got {other:?}"),
        };
        assert!(
            (trigger_flag - 1.0).abs() <= 1.0e-5,
            "trigger literal write must set the flag to 1; got {trigger_flag}"
        );
        let trigger_count = match script_ctx
            .slot_table
            .borrow()
            .get("trigger.count")
            .and_then(|record| record.value.as_ref())
        {
            Some(SlotValue::Number(value)) => *value,
            other => panic!("expected numeric trigger count, got {other:?}"),
        };
        assert!(
            (trigger_count - 2.0).abs() <= 1.0e-5,
            "two IR increments from one on_fire execution must accumulate within the fixed tick; got {trigger_count}"
        );

        let sequence_registry = SequencedPrimitiveRegistry::new();
        let reaction_registry = ReactionPrimitiveRegistry::new();
        let mut system_registry = SystemReactionRegistry::new();
        register_system_reaction_primitives(&mut system_registry);
        let residual = bindings.residual(events.trigger_residuals[0].0).unwrap();
        let _ = fire_prepartitioned_reactions_with_sequences(
            residual.steps(),
            &sequence_registry,
            &reaction_registry,
            &system_registry,
            &script_ctx,
            ResidualOrigin::TriggerBinding,
        );
        assert!(matches!(
            script_ctx.system_commands.take().as_slice(),
            [postretro_entities::SystemReactionCommand::FlashScreen { .. }]
        ));

        let _ = simulate_tick(
            registry.clone(),
            &world,
            &hit_zones,
            None,
            postretro_collision::EnvironmentView::level_only(-9.81),
            None,
            0.0,
            &mut progress,
            postretro_ai::test_tick_runner!(&mut ai_runtime),
            &[],
            &mut mover_states,
            &[],
            &sim_command(false, false),
            |_| PostMovementCommand {
                aim_origin: Vec3::ZERO,
                aim_direction: Vec3::NEG_Z,
            },
            1.0 / 60.0,
            Some(TriggerTickContext {
                teleporters: None,
                system: &mut trigger_system,
                bridge: &bridge,
                bindings: &bindings,
                slot_table: script_ctx.slot_table.clone(),
                script_ctx: Some(script_ctx.clone()),
                auto_close_timers: None,
                use_edges: &use_edges,
            }),
            |_| {},
        );
        let registry_ref = registry.borrow();
        assert!(
            registry_ref
                .get_component::<Transform>(mover)
                .unwrap()
                .position
                .x
                > 0.0,
            "the tick after same-tick mover start advances the mover without an app loop"
        );
        assert_eq!(
            registry_ref
                .get_component::<HealthComponent>(animated_target)
                .unwrap()
                .current,
            90.0,
            "the enter edge does not execute consequential work twice"
        );
    }

    #[test]
    fn trigger_commands_recheck_later_same_tick_gates_in_stable_edge_order() {
        for (command, second_starts_armed, expected_event_names, expected_health) in [
            ("disarmTrigger", true, vec!["first"], 100.0_f32),
            ("armTrigger", false, vec!["first", "second"], 95.0_f32),
        ] {
            let script_ctx = postretro_entities::ScriptCtx::new();
            let registry = script_ctx.registry.clone();
            let (first, second, target) = {
                let mut registry = registry.borrow_mut();
                let player = registry.spawn(Transform::default());
                registry
                    .set_component(player, trigger_movement())
                    .expect("player movement attaches");

                // Spawn order is the authored total order: first's command must
                // settle before the second trigger's gate is evaluated.
                let first = registry.spawn(Transform::default());
                registry
                    .set_component(first, trigger_component("first", true))
                    .expect("first trigger attaches");
                let second = registry.spawn(Transform::default());
                registry
                    .set_tags(second, vec!["second-trigger".into()])
                    .expect("second trigger accepts tag");
                registry
                    .set_component(second, trigger_component("second", second_starts_armed))
                    .expect("second trigger attaches");
                let target = registry.spawn(Transform::default());
                registry
                    .set_tags(target, vec!["damage-target".into()])
                    .expect("damage target accepts tag");
                registry
                    .set_component(
                        target,
                        HealthComponent {
                            max: 100.0,
                            current: 100.0,
                            hitbox: None,
                            death_handled: false,
                            pending_kill_credit: None,
                            zone_multipliers: Default::default(),
                            contributor_ledger: Default::default(),
                        },
                    )
                    .expect("damage target health attaches");
                (first, second, target)
            };
            let mut data = DataRegistry::new();
            data.populate_level(
                vec![
                    NamedReaction {
                        name: "first".into(),
                        descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                            primitive: command.into(),
                            target: None,
                            kind: None,
                            tag: Some("second-trigger".into()),
                            on_complete: None,
                            args: serde_json::json!({}),
                        }),
                    },
                    NamedReaction {
                        name: "second".into(),
                        descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                            primitive: "applyDamage".into(),
                            target: None,
                            kind: None,
                            tag: Some("damage-target".into()),
                            on_complete: None,
                            args: serde_json::json!({ "amount": 5 }),
                        }),
                    },
                ],
                Vec::new(),
                &[],
            );
            let bindings =
                TriggerBindingTable::build_with_script_ctx(&registry.borrow(), &data, &script_ctx);
            let mut bridge = TriggerVolumeBridge::new();
            for trigger in [first, second] {
                bridge.insert_for_test(trigger, Vec3::splat(-4.0), Vec3::splat(4.0));
            }
            let mut trigger_system = TriggerSystem::default();
            let mut progress = ProgressTracker::new();
            let mut ai_runtime = postretro_ai::AiRuntime::new();
            let mut mover_states = MoverTickStateTable::default();
            let use_edges = HashMap::new();

            let events = simulate_tick(
                registry.clone(),
                &CollisionWorld::new(),
                &HitZoneStore::new(),
                None,
                postretro_collision::EnvironmentView::level_only(-9.81),
                None,
                0.0,
                &mut progress,
                postretro_ai::test_tick_runner!(&mut ai_runtime),
                &[],
                &mut mover_states,
                &[],
                &sim_command(false, false),
                |_| PostMovementCommand {
                    aim_origin: Vec3::ZERO,
                    aim_direction: Vec3::NEG_Z,
                },
                1.0 / 60.0,
                Some(TriggerTickContext {
                    teleporters: None,
                    system: &mut trigger_system,
                    bridge: &bridge,
                    bindings: &bindings,
                    slot_table: script_ctx.slot_table.clone(),
                    script_ctx: Some(script_ctx.clone()),
                    auto_close_timers: None,
                    use_edges: &use_edges,
                }),
                |_| {},
            );

            assert_eq!(
                events
                    .trigger_fires
                    .iter()
                    .map(|event| event.fire.event_name.as_str())
                    .collect::<Vec<_>>(),
                expected_event_names,
                "{command} must affect the later trigger gate within this tick"
            );
            let observed_health = registry
                .borrow()
                .get_component::<HealthComponent>(target)
                .expect("damage target remains present")
                .current;
            assert!(
                (observed_health - expected_health).abs() <= 1.0e-6,
                "the second trigger's consequential work must match the rechecked gate; expected {expected_health}, observed {observed_health}"
            );
            assert_eq!(
                registry
                    .borrow()
                    .get_component::<TriggerVolumeComponent>(second)
                    .expect("second trigger remains present")
                    .armed,
                command == "armTrigger"
            );
        }
    }

    #[test]
    fn synchronous_producer_queue_waits_until_next_tick_before_first_decrement() {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let target = {
            let mut registry = registry.borrow_mut();
            let target = registry.spawn(Transform::default());
            registry
                .set_component(
                    target,
                    HealthComponent {
                        max: 100.0,
                        current: 100.0,
                        hitbox: None,
                        death_handled: false,
                        pending_kill_credit: None,
                        zone_multipliers: Default::default(),
                        contributor_ledger: Default::default(),
                    },
                )
                .unwrap();
            target
        };
        let run_tick = |enqueue_effect: bool| {
            let world = CollisionWorld::new();
            let hit_zones = HitZoneStore::new();
            let mut progress = ProgressTracker::new();
            let mut ai_runtime = postretro_ai::AiRuntime::new();
            let mut mover_states = MoverTickStateTable::default();
            simulate_tick(
                registry.clone(),
                &world,
                &hit_zones,
                None,
                postretro_collision::EnvironmentView::level_only(-9.81),
                None,
                0.0,
                &mut progress,
                postretro_ai::test_tick_runner!(&mut ai_runtime),
                &[],
                &mut mover_states,
                &[],
                &sim_command(false, false),
                |registry| {
                    if enqueue_effect {
                        crate::impact_effects::set_health(
                            &mut registry.borrow_mut(),
                            target,
                            25.0,
                            Some(100.0),
                        );
                    }
                    PostMovementCommand {
                        aim_origin: Vec3::ZERO,
                        aim_direction: Vec3::NEG_Z,
                    }
                },
                0.040,
                None,
                |_| {},
            );
        };

        run_tick(true);
        assert_eq!(
            registry
                .borrow()
                .get_component::<postretro_entities::DeferredEffectComponent>(target)
                .unwrap()
                .pending[0]
                .remaining_us,
            100_000,
        );

        run_tick(false);
        assert_eq!(
            registry
                .borrow()
                .get_component::<postretro_entities::DeferredEffectComponent>(target)
                .unwrap()
                .pending[0]
                .remaining_us,
            60_000,
        );
    }

    // Regression: the AI brain skipped a queued despawn, but the later
    // fixed-tick steering stage still followed its retained path. A queued
    // despawn makes the agent a corpse: it keeps falling, and takes no steering.
    #[test]
    fn fixed_tick_queued_despawn_quiesces_brain_agent_steering() {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (enemy, start) = {
            let mut registry = registry.borrow_mut();
            let start = Transform {
                position: Vec3::new(2.0, 1.0, 2.0),
                ..Transform::default()
            };
            let enemy = registry.spawn(start);

            let destination = Vec3::new(12.0, 1.0, 2.0);
            let mut agent = AgentComponent::new(0.35, 1.8, 0.4, 4.0);
            agent.path = vec![destination];
            agent.mandatory_waypoints = vec![false];
            agent.destination = Some(destination);
            agent.planned_destination = Some(destination);
            agent.replan_cooldown_ticks = 10;
            registry.set_component(enemy, agent).unwrap();
            registry.set_component(enemy, alert_brain(4.0)).unwrap();
            crate::impact_effects::despawn(&mut registry, enemy, Some(500.0));
            (enemy, start)
        };

        let world = CollisionWorld::new();
        let hit_zones = HitZoneStore::new();
        let mut progress = ProgressTracker::new();
        let mut ai_runtime = postretro_ai::AiRuntime::new();
        let mut mover_states = MoverTickStateTable::default();
        let events = simulate_tick(
            registry.clone(),
            &world,
            &hit_zones,
            None,
            postretro_collision::EnvironmentView::level_only(0.0),
            None,
            0.0,
            &mut progress,
            postretro_ai::test_tick_runner!(&mut ai_runtime),
            &[],
            &mut mover_states,
            &[],
            &sim_command(false, false),
            |_| PostMovementCommand {
                aim_origin: Vec3::ZERO,
                aim_direction: Vec3::NEG_Z,
            },
            0.1,
            None,
            |_| {},
        );

        assert!(events.ai.is_empty(), "queued despawn must suppress attacks");
        let registry = registry.borrow();
        let position = registry.get_component::<Transform>(enemy).unwrap().position;
        assert_eq!(
            (position.x, position.z),
            (start.position.x, start.position.z),
            "the real fixed-tick steering stage must not steer a queued-despawn brain",
        );
        let agent = registry.get_component::<AgentComponent>(enemy).unwrap();
        assert_eq!(agent.steer_velocity, Vec3::ZERO);
        assert_eq!((agent.velocity.x, agent.velocity.z), (0.0, 0.0));
        assert_eq!(agent.destination, Some(Vec3::new(12.0, 1.0, 2.0)));
        let effects = registry
            .get_component::<postretro_entities::DeferredEffectComponent>(enemy)
            .unwrap();
        assert!(
            !effects.inert,
            "the delayed removal countdown is still active"
        );
        assert!(
            effects.pending[0].remaining_us.abs_diff(400_000) <= 1,
            "one 0.1s fixed tick leaves about 400ms; got {}us",
            effects.pending[0].remaining_us,
        );
    }

    #[test]
    fn authorized_remote_hit_latches_without_reporting_or_removal_in_same_host_tick() {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (weapon_id, attacker, target) = {
            let mut registry = registry.borrow_mut();
            let weapon_id = registry.spawn(Transform::default());
            let attacker = registry.spawn(Transform::default());
            let target = registry.spawn(Transform::default());
            registry
                .set_component(
                    target,
                    HealthComponent {
                        max: 10.0,
                        current: 10.0,
                        hitbox: None,
                        death_handled: false,
                        pending_kill_credit: None,
                        zone_multipliers: Default::default(),
                        contributor_ledger: Default::default(),
                    },
                )
                .unwrap();
            (weapon_id, attacker, target)
        };
        let impact = weapon::WeaponImpact {
            point: Vec3::ZERO,
            normal: Vec3::Y,
            target: Some(target),
            zone: None,
            outcome: weapon::ActivationOutcome::Hit(weapon::DamagePayload {
                amount: 10.0,
                impulse: glam::Vec3::ZERO,
                damage_type: None,
            }),
        };
        {
            let mut registry = registry.borrow_mut();
            apply_authorized_weapon_impact_damage(
                &mut registry,
                weapon_id,
                Some(attacker),
                &impact,
                "weapon.test.remote".to_string(),
                10.0,
            );
            assert!(
                registry.exists(target),
                "damage alone leaves death handling for the sweep hook"
            );
        }

        let death_events = run_death_sweep(&registry);

        assert!(death_events.is_empty());
        assert!(
            registry.borrow().exists(target),
            "the post-HIT sweep leaves a zero-HP target live for authored despawn"
        );
        let health = registry
            .borrow()
            .get_component::<HealthComponent>(target)
            .unwrap()
            .clone();
        assert!(health.death_handled);
        assert!(
            health.pending_kill_credit.is_some(),
            "the sweep freezes credit but emits no progress event",
        );
    }
}

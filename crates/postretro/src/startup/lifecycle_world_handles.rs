//! The CPU world install's boundary: segment A, segment B's handles and
//! products, and the first player's seat binding.
//! See: context/lib/boot_sequence.md §3

use super::*;

/// Segment A of the CPU world install (renderer-free): seed world gravity from
/// the level's authored value and build the runtime navigation graph from the
/// baked navmesh section (`None` when the map has no navmesh bake). Split from
/// segment B ([`install_world_cpu`]) so the windowed path can run its renderer
/// texture/UV/geometry upload and the light-bridge populate BETWEEN the two:
/// light entities must take registry ids before the fog entities segment B
/// creates (both bridges key dirty tracking on `EntityId`). Headless calls A
/// then B back-to-back — no light entities exist, so the fog ids land first,
/// the documented headless entity-id shape.
pub(crate) fn install_world_gravity_and_nav(
    world: &postretro_level_loader::LevelWorld,
    script_ctx: &postretro_entities::ScriptCtx,
) -> Option<crate::nav::NavGraph> {
    script_ctx
        .gravity
        .set(world.environment_volumes.level_gravity);
    world
        .navmesh
        .as_ref()
        .map(crate::nav::NavGraph::from_section)
}

/// CPU world-install products the tick loop consumes. [`install_world_cpu`]
/// fills these from a level payload without touching the renderer, so a headless
/// caller assembles `simulate_tick`'s arguments from the return value plus the
/// nav graph produced by [`install_world_gravity_and_nav`] and the handles it
/// passed in (populated registry, collision world, hit-zone store).
pub(crate) struct WorldInstallProducts {
    /// Static colliders for every loaded kinematic mover.
    pub(crate) mover_colliders: Vec<crate::collision::moving::MoverCollider>,
    /// Spawned mover entity ids aligned with `KinematicGeometry::movers`.
    /// A failed all-or-nothing mover spawn returns an empty vector.
    pub(crate) spawned_mover_entities: Vec<postretro_entities::registry::EntityId>,
    /// Trigger reactions partitioned from the final composed active set.
    pub(crate) trigger_bindings: TriggerBindingTable,
    pub(crate) teleporter_bindings: postretro_sim::teleporter_system::TeleporterBindingTable,
    /// Host-only trigger-pool outcome retained by `App` for diagnostics and
    /// tests. Connected clients receive the default empty report.
    pub(crate) trigger_pool_report: TriggerPoolInstallReport,
    /// A fresh, empty mover tick-state table. Not an install product — it is
    /// caller-owned per-tick state — returned only so the headless batch runner
    /// has one to hand `simulate_tick` without reaching into `App` (the windowed
    /// `App` field is its own `MoverTickStateTable::default()` and ignores this).
    /// The headless driver (`observability::driver`) is the sole reader, so the
    /// field is dead only in a build without that feature.
    #[cfg_attr(not(feature = "observability"), allow(dead_code))]
    pub(crate) mover_tick_states: crate::kinematic_mover::MoverTickStateTable,
    /// Where a pawn stands at the first player's `player_spawn`, plus that
    /// spawn's engine-convention YXZ angles (x=pitch, y=yaw), for the windowed
    /// camera teleport. `None` when the map has no spawn.
    pub(crate) first_spawn: Option<(Vec3, Vec3)>,
    /// The map's `player_spawn` placements, retained by the windowed host for the
    /// runtime net-slot accept path.
    pub(crate) spawn_points: Vec<crate::scripting::map_entity::MapEntity>,
}

/// Bind the installed first player to the host's seat (seat 0) and record the
/// `player_spawn` it took, so joiners are assigned the spawns left free.
pub(crate) fn bind_installed_first_player(
    seats: &mut crate::netcode::SeatTable,
    registry: &mut postretro_entities::EntityRegistry,
    first_player: Option<crate::scripting::builtins::PlayerSpawned>,
) {
    if let Some(spawned) = first_player {
        seats.bind_level_spawn_placement(spawned.pawn, spawned.placement);
    }
    if let Some(pawn) = registry.local_player_pawn() {
        seats.bind_pawn(registry, postretro_foundation::Seat(0), pawn);
    }
}

/// Borrowed handles segment B ([`install_world_cpu`]) reads or mutates. Bundled
/// so the windowed caller and a future headless caller pass one context rather
/// than ~18 positional args; every field is a live borrow held only for the one
/// call. The registry, data registry, and slot table are reached through
/// `script_ctx`'s `RefCell`s at runtime, so borrowing them never conflicts with
/// the session field borrows below.
pub(crate) struct WorldInstallHandles<'a> {
    pub(crate) world: &'a postretro_level_loader::LevelWorld,
    pub(crate) script_ctx: &'a postretro_entities::ScriptCtx,
    pub(crate) command_diagnostics: crate::mover_commands::MoverCommandDiagnostics,
    /// Current mod-wide auto-close default. Static level-install input only.
    pub(crate) mover_auto_close_ms: f32,
    /// Session-owned VM-free resolved descriptor cache for entity spawners.
    pub(crate) spawn_context: crate::spawner::SpawnContext,
    pub(crate) content_root: &'a std::path::Path,
    pub(crate) active_level_tags: &'a [String],
    /// The nav graph produced by segment A; supplies the descriptor-spawn agent
    /// capsule params. `None` on maps without a navmesh bake.
    pub(crate) nav_graph: Option<&'a crate::nav::NavGraph>,
    pub(crate) fog_volume_bridge:
        &'a mut crate::scripting_systems::fog_volume_bridge::FogVolumeBridge,
    pub(crate) trigger_volume_bridge:
        &'a mut crate::scripting_systems::trigger_volume_bridge::TriggerVolumeBridge,
    pub(crate) classname_dispatch: &'a crate::scripting::builtins::ClassnameDispatch,
    pub(crate) script_runtime: &'a postretro_scripting_core::runtime::ScriptRuntime,
    pub(crate) sequence_registry:
        &'a postretro_scripting_core::sequence::SequencedPrimitiveRegistry,
    pub(crate) reaction_registry:
        &'a crate::scripting::reactions::registry::ReactionPrimitiveRegistry,
    pub(crate) system_registry:
        &'a crate::scripting::reactions::system_commands::SystemReactionRegistry,
    pub(crate) modal_stack: &'a mut postretro_ui::modal_stack::ModalStack,
    pub(crate) progress_tracker:
        &'a mut postretro_scripting_core::reaction_dispatch::ProgressTracker,
    pub(crate) crossing_detector:
        &'a mut postretro_scripting_core::state_crossings::CrossingDetector,
    pub(crate) slot_accumulator_bindings:
        &'a mut crate::scripting_systems::slot_accumulators::SlotAccumulatorBindings,
    /// The session's bound `setState` IR, rebuilt before `levelLoad` fires so
    /// its store writes evaluate at dispatch.
    pub(crate) system_reaction_ir_bindings: &'a std::rc::Rc<
        std::cell::RefCell<crate::scripting_systems::system_reactions::SystemReactionIrBindings>,
    >,
    /// Runtime-owned impact policy registry. Level descriptors are installed
    /// after the data script returns its manifest.
    pub(crate) impact_policy_runtime: &'a mut crate::impact_policy::ImpactPolicyRuntime,
    pub(crate) mesh_clip_tables: &'a mut crate::scripting_systems::mesh_anim::MeshClipTables,
    pub(crate) hit_zone_store: &'a mut crate::scripting_systems::hit_zones::HitZoneStore,
    /// Resolved separately for each install. A pinned seed repeats exactly;
    /// arm-all is the deterministic unpinned headless default.
    pub(crate) trigger_pool_policy: TriggerPoolSeedPolicy,
    /// Connected-client setup flag: skip host-authoritative map-placement
    /// materialization and boot-pawn spawn. Both `false` off a connected client
    /// (single-player, listen host, headless).
    pub(crate) suppress_host_replicated: bool,
    pub(crate) suppress_boot_pawn: bool,
    /// Seat-zero carried record, resolved by the caller before descriptor spawn. The
    /// world installer remains seat-table agnostic.
    pub(crate) local_carried_loadout: Option<postretro_combat_model::CarriedState>,
}

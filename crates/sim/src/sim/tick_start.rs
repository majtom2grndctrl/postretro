//! Two-phase authoritative tick start keeps respawn ahead of command admission.
//! Existing headless callers use the compatibility wrapper below.
use super::*;

pub struct PendingTickStart {
    arrivals: Vec<super::respawn::Arrival>,
}

/// Created only by finishing the start; consumed by the simulation body.
pub struct StartedTick {
    _private: (),
}

#[derive(Debug, Clone, Copy)]
pub struct RespawnTurn {
    pub pawn: EntityId,
    pub yaw_delta: f32,
}

/// Snapshot before relocation, then apply queued teleporter changes and respawns.
/// The App resolves this tick's switch/activation lanes after this returns.
pub fn begin_tick(
    registry: &Rc<RefCell<EntityRegistry>>,
    ctx: Option<&ScriptCtx>,
) -> PendingTickStart {
    registry.borrow_mut().snapshot_transforms();
    let arrivals = if let Some(ctx) = ctx {
        crate::teleporter_commands::apply_pending_teleporter_mutations(ctx);
        super::respawn::drain(ctx, &mut registry.borrow_mut())
    } else {
        Vec::new()
    };
    PendingTickStart { arrivals }
}

/// Resolve arrival yaw against the just-resolved facing, then stamp the shared
/// teleport tail before the host admits activation starts in the new frame.
pub fn finish_tick_start(
    pending: PendingTickStart,
    registry: &mut EntityRegistry,
    collision: &CollisionWorld,
    mut facing: impl FnMut(EntityId) -> Option<f32>,
) -> (StartedTick, Vec<RespawnTurn>) {
    let mut turns = Vec::with_capacity(pending.arrivals.len());
    let combined = CombinedCollisionWorld::static_only(collision);
    for arrival in pending.arrivals {
        let Ok(mut movement) = registry
            .get_component::<PlayerMovementComponent>(arrival.pawn)
            .cloned()
        else {
            log::warn!(
                "[Respawn] pawn {:?} vanished before arrival; skipping",
                arrival.pawn
            );
            continue;
        };
        let yaw_delta = arrival.yaw
            - facing(arrival.pawn)
                .filter(|yaw| yaw.is_finite())
                .unwrap_or(arrival.old_yaw);
        crate::movement::teleport::settle_arrival(
            &mut movement,
            &combined,
            arrival.position,
            yaw_delta,
            postretro_foundation::NO_TELEPORTER,
            |index| collision.teleporters.armed(registry, index),
        );
        let _ = registry.set_component(arrival.pawn, movement);
        turns.push(RespawnTurn {
            pawn: arrival.pawn,
            yaw_delta,
        });
    }
    (StartedTick { _private: () }, turns)
}

#[allow(clippy::too_many_arguments)]
pub fn simulate_tick_with_presentation_aim<AiResult>(
    registry: Rc<RefCell<EntityRegistry>>,
    collision_world: &CollisionWorld,
    hit_zone_store: &HitZoneStore,
    nav_graph: Option<&NavGraph>,
    environment: EnvironmentView<'_>,
    mod_block_during_reload: bool,
    anim_time: f64,
    mut presentation_camera_aim: (f32, f32),
    _progress_tracker: &mut ProgressTracker,
    run_ai: impl FnMut(&mut EntityRegistry, f32, AiTickInputs<'_>, &mut SimAiHost<'_>) -> AiResult,
    mover_colliders: &[MoverCollider],
    mover_tick_states: &mut MoverTickStateTable,
    remote_pawn_commands: &[RemotePawnCommand],
    command: &SimCommand,
    post_movement: impl FnMut(&Rc<RefCell<EntityRegistry>>) -> PostMovementCommand,
    tick_dt: f32,
    touch_system: &mut TouchSystem,
    assist_state: &mut super::assist::AssistState,
    descriptors: &[EntityTypeDescriptor],
    descriptor_generation: u64,
    factions: &FactionRegistry,
    faction_sentiment: &RefCell<FactionSentimentState>,
    default_weapon_placement: Option<&WeaponPlacementDescriptor>,
    use_pressed: &HashMap<PlayerId, bool>,
    drop_pressed: &HashMap<PlayerId, bool>,
    trigger_context: Option<TriggerTickContext<'_>>,
    ingest_ready_remote_hits: impl FnMut(&mut EntityRegistry, &mut dyn FnMut(&mut EntityRegistry)),
    on_impact: impl FnMut(&mut EntityRegistry),
    timing: postretro_stage_timing::TimingGate,
) -> TickEvents
where
    AiResult: Into<AiTickResult>,
{
    let ctx = trigger_context
        .as_ref()
        .and_then(|context| context.script_ctx.as_ref());
    let pending = begin_tick(&registry, ctx);
    let local = if pending.arrivals.is_empty() {
        None
    } else {
        let registry = registry.borrow();
        registry.local_player_movement_pawn()
    };
    let (started, turns) = finish_tick_start(
        pending,
        &mut registry.borrow_mut(),
        collision_world,
        |pawn| {
            remote_pawn_commands
                .iter()
                .find(|remote| remote.pawn == pawn)
                .map(|remote| remote.command.movement.facing_yaw)
                .or_else(|| (Some(pawn) == local).then_some(command.movement.facing_yaw))
        },
    );
    let mut command = std::borrow::Cow::Borrowed(command);
    let mut remote_pawn_commands = std::borrow::Cow::Borrowed(remote_pawn_commands);
    for turn in turns {
        if Some(turn.pawn) == local {
            command.to_mut().movement.facing_yaw += turn.yaw_delta;
            presentation_camera_aim.1 += turn.yaw_delta;
        }
        if remote_pawn_commands
            .iter()
            .any(|remote| remote.pawn == turn.pawn)
        {
            for remote in remote_pawn_commands
                .to_mut()
                .iter_mut()
                .filter(|remote| remote.pawn == turn.pawn)
            {
                remote.command.movement.facing_yaw += turn.yaw_delta;
                if let Some(aim) = remote.start_aim.as_mut() {
                    aim.yaw += turn.yaw_delta;
                }
            }
        }
    }
    super::simulate_started_tick_with_presentation_aim(
        started,
        registry,
        collision_world,
        hit_zone_store,
        nav_graph,
        environment,
        mod_block_during_reload,
        anim_time,
        presentation_camera_aim,
        _progress_tracker,
        run_ai,
        mover_colliders,
        mover_tick_states,
        &remote_pawn_commands,
        &command,
        post_movement,
        tick_dt,
        touch_system,
        assist_state,
        descriptors,
        descriptor_generation,
        factions,
        faction_sentiment,
        default_weapon_placement,
        use_pressed,
        drop_pressed,
        trigger_context,
        ingest_ready_remote_hits,
        on_impact,
        timing,
    )
}

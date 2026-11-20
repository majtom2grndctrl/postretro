// The authoritative fixed tick: every stage in `entity_model.md` §5 order.
// See: context/lib/entity_model.md §5 · context/lib/networking.md

use super::*;

/// Fixed-tick simulation with the render-assembly camera aim captured by the App.
/// Headless callers retain the wrapper above and intentionally use the neutral
/// camera; production local-player presentation passes the live camera aim here.
#[allow(clippy::too_many_arguments)]
pub fn simulate_started_tick_with_presentation_aim<AiResult>(
    _started: StartedTick,
    registry: Rc<RefCell<EntityRegistry>>,
    collision_world: &CollisionWorld,
    hit_zone_store: &HitZoneStore,
    nav_graph: Option<&NavGraph>,
    environment: EnvironmentView<'_>,
    mod_block_during_reload: bool,
    anim_time: f64,
    mut presentation_camera_aim: (f32, f32),
    _progress_tracker: &mut ProgressTracker,
    mut run_ai: impl FnMut(&mut EntityRegistry, f32, AiTickInputs<'_>, &mut SimAiHost<'_>) -> AiResult,
    mover_colliders: &[MoverCollider],
    mover_tick_states: &mut MoverTickStateTable,
    remote_pawn_commands: &[RemotePawnCommand],
    remote_present: &[(u64, EntityId)],
    command: &SimCommand,
    mut post_movement: impl FnMut(&Rc<RefCell<EntityRegistry>>) -> PostMovementCommand,
    tick_dt: f32,
    touch_system: &mut TouchSystem,
    assist_state: &mut assist::AssistState,
    descriptors: &[EntityTypeDescriptor],
    descriptor_generation: u64,
    factions: &FactionRegistry,
    faction_sentiment: &RefCell<FactionSentimentState>,
    default_weapon_placement: Option<&WeaponPlacementDescriptor>,
    use_pressed: &HashMap<PlayerId, bool>,
    drop_pressed: &HashMap<PlayerId, bool>,
    trigger_context: Option<TriggerTickContext<'_>>,
    mut ingest_ready_remote_hits: impl FnMut(&mut EntityRegistry, &mut dyn FnMut(&mut EntityRegistry)),
    mut on_impact: impl FnMut(&mut EntityRegistry),
    timing: postretro_stage_timing::TimingGate,
) -> TickEvents
where
    AiResult: Into<AiTickResult>,
{
    use cpu_stages::SimStage;
    let cpu = postretro_stage_timing::StageFrame::new(timing);
    let tick_scope = cpu.scope(SimStage::Tick);
    // A status application inside the tick counts on this tick; outside it,
    // one latches to the next tick start (`scripting.md` §2).
    registry
        .borrow_mut()
        .set_status_applications_immediate(true);
    // Every environment query this tick goes through `environment`, so a timer
    // attached here sees the player and agent resolves and the hitscan
    // crossings wherever they run. Off, the view carries none and reads no clock.
    let environment_timer = postretro_collision::EnvironmentQueryTimer::new();
    let environment = if timing.is_enabled() {
        environment.timed(&environment_timer)
    } else {
        environment
    };
    // Every body's overlap check this tick reads the level's static world, so
    // its timer spans host movement and agent steering alike.
    collision_world.overlap_timer().begin(timing.is_enabled());

    // This is the fixed-tick queue boundary. Producers run later in this tick
    // (ready remote-hit ingest, AI, and local weapon fire), so every newly
    // queued effect keeps its full authored delay until the next fixed tick,
    // including in headless simulation.
    {
        let mut registry = registry.borrow_mut();
        crate::impact_effects::tick_deferred_effects(&mut registry, tick_dt);
    }

    let auto_close_timers = trigger_context
        .as_ref()
        .and_then(|context| context.auto_close_timers.clone());
    // The assist pass seeds pair sides from the store after the trigger stage
    // has consumed the context.
    let assist_slot_table = trigger_context
        .as_ref()
        .map(|context| context.slot_table.clone());
    let stage = cpu.scope(SimStage::Movers);
    // Commands applied since the last blocking pass (the frame-end script
    // drain, player-event fire lists) reach the driver only along a course
    // the pin test has seen (`movement.md` §6, Pinned bodies).
    let mut mover_events = Vec::new();
    {
        let mut registry = registry.borrow_mut();
        let (mover_poses, blocking_state) = mover_tick_states.split_for_blocking();
        kinematic_mover::run_mover_pin_pretest(
            &mut registry,
            collision_world,
            mover_colliders,
            &mover_poses,
            blocking_state,
            tick_dt,
            &mut mover_events,
        );
        kinematic_mover::run_kinematic_mover_tick(&mut registry, mover_tick_states, tick_dt);
    }
    mover_events.extend(mover_tick_states.terminus_events());
    if let Some(auto_close_timers) = auto_close_timers.as_ref() {
        auto_close_timers.arm_opened_termini(&mut registry.borrow_mut(), &mover_events);
    }
    drop(stage);

    // The host player's fire gate reads its movement at tick start, as the
    // client's and the remote path's do, before this tick's movement runs.
    let local_fire_blocked = {
        let registry = registry.borrow();
        registry
            .local_player_movement_pawn()
            .is_some_and(|pawn| crate::weapon::pawn_gate::pawn_fire_blocked(&registry, pawn))
    };
    let stage = cpu.scope(SimStage::Movement);
    let remote_pawn_inputs: Vec<(EntityId, MovementInput)> = remote_pawn_commands
        .iter()
        .map(|remote| (remote.pawn, remote.command.movement.clone()))
        .collect();
    let mut remote_player_aims: HashMap<EntityId, (f32, f32)> = remote_pawn_commands
        .iter()
        .map(|remote| {
            (
                remote.pawn,
                (remote.aim_pitch, remote.command.movement.facing_yaw),
            )
        })
        .collect();

    let combined_collision =
        CombinedCollisionWorld::new(collision_world, mover_colliders, mover_tick_states);
    let remote_movement = {
        let mut registry = registry.borrow_mut();
        host_movement::run_host_movement_tick(
            &mut registry,
            &combined_collision,
            environment,
            &remote_pawn_inputs,
            tick_dt,
        )
    };
    // Remote pawns are authoritatively simulated above, but their local-only
    // presentation/reaction events must never enter the host's script registry.
    let movement = run_movement_tick(
        &registry,
        &combined_collision,
        environment,
        &command.movement,
        tick_dt,
    );
    crate::life_state_commands::advance_unstepped_life_state_time(
        &mut registry.borrow_mut(),
        &remote_movement,
        movement.stepped_pawn,
    );
    let mut remote_pawn_commands = std::borrow::Cow::Borrowed(remote_pawn_commands);
    let mut teleports: Vec<_> = remote_movement
        .into_iter()
        .filter_map(|(pawn, events)| {
            events.teleport.map(|event| {
                if let Some(aim) = remote_player_aims.get_mut(&pawn) {
                    aim.1 += event.yaw_delta;
                }
                if let Some(command) = remote_pawn_commands
                    .to_mut()
                    .iter_mut()
                    .find(|command| command.pawn == pawn)
                {
                    command.command.movement.facing_yaw += event.yaw_delta;
                    if let Some(aim) = &mut command.start_aim {
                        aim.yaw += event.yaw_delta;
                    }
                }
                (pawn, event)
            })
        })
        .collect();
    if let Some((pawn, event)) = movement.teleport {
        presentation_camera_aim.1 += event.yaw_delta;
        teleports.push((pawn, event));
    }
    let mut teleport_residuals = Vec::new();
    if let Some(context) = trigger_context.as_ref()
        && let (Some(bindings), Some(ctx)) = (context.teleporters, context.script_ctx.as_ref())
    {
        // Event order: remote pawns in command order, then the local pawn
        // (the order `teleports` was built in above); residuals keep it.
        for (pawn, event) in &teleports {
            let player = remote_pawn_commands
                .iter()
                .find(|remote| remote.pawn == *pawn)
                .map_or(PlayerId::Local(*pawn), |remote| {
                    PlayerId::Remote(remote.owner_client_id)
                });
            if let Some(residual) = bindings.execute_evaluating_impacts(
                ctx,
                event.teleporter,
                *pawn,
                player,
                &mut on_impact,
            ) {
                teleport_residuals.push(residual);
            }
        }
    }
    // Movement may have resized a capsule (crouch, stand, slide, life posture)
    // with the feet planted; reseat the mesh before this tick's projectile and
    // weapon hit zones read the offset against the moved centre.
    sync_player_mesh_origin_offsets(&mut registry.borrow_mut());
    drop(stage);
    let stage = cpu.scope(SimStage::Triggers);
    // Presence is every owned remote pawn, not only those with a command this
    // tick: a joiner's pawn presses plates and collects pickups from spawn, while
    // movement above stays command-gated and holds its pose.
    let mut players: Vec<AuthoritativePlayer> = remote_present
        .iter()
        .map(|&(owner_client_id, pawn)| AuthoritativePlayer {
            id: PlayerId::Remote(owner_client_id),
            pawn,
        })
        .collect();
    debug_assert!(
        remote_pawn_commands
            .iter()
            .all(|remote| remote_present.contains(&(remote.owner_client_id, remote.pawn))),
        "every commanded remote pawn must be in the present roster"
    );
    let local_player = {
        let registry = registry.borrow();
        registry.local_player_movement_pawn()
    };
    // A defence for owned pawns with no seat, which only fixtures make: the
    // registry cannot see ownership. It keeps presence from listing such a pawn
    // twice; it does not keep host input off it.
    if let Some(pawn) = local_player
        && !players.iter().any(|player| player.pawn == pawn)
    {
        players.push(AuthoritativePlayer {
            id: PlayerId::Local(pawn),
            pawn,
        });
    }

    let mut trigger_residuals = Vec::new();
    let mut trigger_exit_fires: Vec<(EntityId, PlayerId)> = Vec::new();
    #[cfg(test)]
    let mut trigger_fires = Vec::new();
    #[cfg(test)]
    let mut trigger_command_fires = Vec::new();
    let stage_capabilities =
        player_life::TriggerStageCapabilities::capture(&registry.borrow(), &players);
    let mut trigger_stage_pawns = stage_capabilities.stage_pawns();
    if let Some(trigger_context) = trigger_context {
        let canonical_player_pawns = &stage_capabilities.pawns;
        let alive_players = &stage_capabilities.occupants;
        let mut registry = registry.borrow_mut();
        let bound_edges = trigger_context.bindings.bound_edges();
        if let Some(script_ctx) = trigger_context.script_ctx.as_ref() {
            let dispatch_inputs = crate::trigger_system::TriggerDispatchInputs {
                alive_players,
                bound_edges,
            };
            let _report = trigger_context
                .system
                .run_authoritative_tick_with_capabilities(
                    &mut registry,
                    trigger_context.bridge,
                    crate::trigger_system::TriggerTickInputs {
                        players: &players,
                        use_pressed: trigger_context.use_edges,
                        tick_dt,
                    },
                    dispatch_inputs,
                    &stage_capabilities,
                    |event, occupancy, occupants, registry| {
                        let fire_context =
                            trigger_fire_context(event, occupancy, canonical_player_pawns);
                        let execution = trigger_context.bindings.execute_evaluating_impacts(
                            event.fire.trigger,
                            event.edge,
                            registry,
                            script_ctx,
                            &fire_context,
                            &mut on_impact,
                        );
                        #[cfg(test)]
                        {
                            trigger_fires.push(event.clone());
                            trigger_command_fires.push(TriggerCommandFire {
                                event: event.clone(),
                                commands: execution.commands.clone(),
                            });
                        }
                        if let Some(handle) = execution.residual() {
                            // Carry the firing origin beside the handle so the
                            // frame-end drain keys each enrolled instance to its
                            // activator; one handle fired by two players yields two
                            // origins, not one collapsed instance (O59).
                            trigger_residuals.push((
                                handle,
                                event.fire.trigger,
                                event.fire.player,
                                occupants.to_vec(),
                            ));
                        }
                        if event.edge == crate::trigger_system::TriggerEventEdge::Exit {
                            trigger_exit_fires.push((event.fire.trigger, event.fire.player));
                        }
                    },
                );
        } else {
            // Literal fixtures only. This borrow spans command application, so
            // a bound `grantLayer` panics here when the registry's layer
            // parameters read this same slot table (an expression capacity).
            // Narrowing it means threading the `RefCell` into bound-command
            // execution; the script-context path borrows per write.
            let mut slot_table = trigger_context.slot_table.borrow_mut();
            let dispatch_inputs = crate::trigger_system::TriggerDispatchInputs {
                alive_players,
                bound_edges,
            };
            let _report = trigger_context
                .system
                .run_authoritative_tick_with_capabilities(
                    &mut registry,
                    trigger_context.bridge,
                    crate::trigger_system::TriggerTickInputs {
                        players: &players,
                        use_pressed: trigger_context.use_edges,
                        tick_dt,
                    },
                    dispatch_inputs,
                    &stage_capabilities,
                    |event, _occupancy, occupants, registry| {
                        let fire_context = TriggerFireContext::default();
                        let execution = trigger_context.bindings.execute(
                            event.fire.trigger,
                            event.edge,
                            registry,
                            &mut slot_table,
                            &fire_context,
                        );
                        #[cfg(test)]
                        {
                            trigger_fires.push(event.clone());
                            trigger_command_fires.push(TriggerCommandFire {
                                event: event.clone(),
                                commands: execution.commands.clone(),
                            });
                        }
                        if let Some(handle) = execution.residual() {
                            // Carry the firing origin beside the handle so the
                            // frame-end drain keys each enrolled instance to its
                            // activator; one handle fired by two players yields two
                            // origins, not one collapsed instance (O59).
                            trigger_residuals.push((
                                handle,
                                event.fire.trigger,
                                event.fire.player,
                                occupants.to_vec(),
                            ));
                        }
                        if event.edge == crate::trigger_system::TriggerEventEdge::Exit {
                            trigger_exit_fires.push((event.fire.trigger, event.fire.player));
                        }
                    },
                );
        }
        // Occupancy after the trigger stage settles, from the same capture
        // the stage counted with: a player killed later this tick still
        // occupies for it, and leaves on the next.
        trigger_stage_pawns.occupied_volumes =
            trigger_context.system.counted_volumes(&stage_capabilities);
    }
    let touch_events = {
        let mut registry = registry.borrow_mut();
        touch_system.run_authoritative_tick_with_capabilities(
            &mut registry,
            collision_world,
            descriptors,
            &players,
            use_pressed,
            drop_pressed,
            &stage_capabilities,
        )
    };
    drop(stage);
    // Decay is the first writer in the impact phase. Its mutable overlay
    // borrow ends before ready-hit/projectile impacts and the AI's transient
    // immutable read view, so all same-tick writes remain borrow-safe and
    // deterministic: decay first, impacts next, frame-end reactions last.
    decay_faction_sentiment(factions, faction_sentiment, tick_dt);
    // Remote declarations already authorized at this tick's input boundary
    // land beside authoritative projectile impacts, after deferred-effect aging
    // but before AI snapshots combat perception. A declaration waiting on this
    // tick's FIRE authorization remains queued for App's post-sim drain.
    let stage = cpu.scope(SimStage::Projectiles);
    {
        let mut registry = registry.borrow_mut();
        ingest_ready_remote_hits(&mut registry, &mut on_impact);
    }
    // Advance projectiles after this tick's movement settles but before AI
    // snapshots its facts. An impact therefore reaches the Health chokepoint
    // and attacker ledger in time for same-tick retaliation selection.
    let local_projectile_contacts = projectile_stage::advance(
        &registry,
        collision_world,
        hit_zone_store,
        anim_time,
        tick_dt,
        &mut on_impact,
    );
    drop(stage);
    let stage = cpu.scope(SimStage::Ai);
    let ai_result = {
        let mut registry = registry.borrow_mut();
        // Attack-pool requests apply after this tick's triggers fired and
        // before arbitration: a trigger-bound start acts on this tick, and
        // every request issued since the last drain applies first, all in
        // issue order.
        crate::attack_pools::apply_attack_pool_requests(&mut registry);
        let mut host = SimAiHost::new(&mut on_impact);
        run_ai(
            &mut registry,
            tick_dt,
            AiTickInputs {
                nav_graph,
                mover_nav: crate::nav::MoverNav {
                    gate: crate::nav::NavGate::new(mover_tick_states.docks()),
                    movers: mover_colliders,
                    poses: &*mover_tick_states,
                },
                collision_world: Some(collision_world),
                descriptors,
                descriptor_generation,
                factions,
                faction_sentiment,
                clips: Some(hit_zone_store),
                cpu: Some(&cpu),
            },
            &mut host,
        )
        .into()
    };
    let AiTickResult {
        events: ai,
        projectile_spawns: enemy_projectile_spawns,
    } = ai_result;

    drop(stage);
    // Traps run after this tick's trigger and teleport requests were issued,
    // so those act on this tick; work issued later (player events, the
    // frame-end drain) waits on the trap's pending set for the next one.
    let stage = cpu.scope(SimStage::Traps);
    let trap_output = crate::traps::run_trap_stage(&mut registry.borrow_mut(), tick_dt);
    drop(stage);
    let post_movement_command = post_movement(&registry);

    let stage = cpu.scope(SimStage::Steering);

    {
        let mut registry = registry.borrow_mut();
        let substage = cpu.scope(SimStage::SteerAgents);
        // AgentTickResult only carries a diagnostic `replans` counter, not observable sim state, so the return value is intentionally discarded.
        let mut corpse_yields = Vec::new();
        let agent_collision =
            CombinedCollisionWorld::new(collision_world, mover_colliders, &*mover_tick_states);
        let _ = agent_steering::tick_with_mover_pins(
            &mut registry,
            &agent_collision,
            nav_graph,
            agent_steering::MoverPins {
                yields: mover_tick_states.agent_yields(),
                gate: crate::nav::NavGate::new(mover_tick_states.docks()),
            },
            &mut corpse_yields,
            environment,
            tick_dt,
        );
        for (agent, mover_id) in corpse_yields {
            mover_tick_states.yield_agent_as_corpse(agent, mover_id);
        }
        drop(substage);
        let substage = cpu.scope(SimStage::SteerPose);
        update_player_animation_locomotion(&mut registry, hit_zone_store, anim_time);
        update_brain_animation_playback_rates(&mut registry, hit_zone_store, anim_time);
        update_presentation_pose_inputs(
            &mut registry,
            collision_world,
            mover_colliders,
            &*mover_tick_states,
            hit_zone_store,
            anim_time,
            PresentationPoseInputs {
                camera_aim: presentation_camera_aim,
                remote_player_aims: &remote_player_aims,
                remote_aim_pitches: &HashMap::new(),
                remote_heading_yaws: &HashMap::new(),
                remote_network_ids: &HashMap::new(),
                local_pawn_owns_direction: false,
            },
        );
        drop(substage);
        let substage = cpu.scope(SimStage::SteerMovers);
        if let Some(auto_close_timers) = auto_close_timers.as_ref() {
            auto_close_timers.tick(&mut registry, tick_dt);
        }
        let (mover_poses, blocking_state) = mover_tick_states.split_for_blocking();
        kinematic_mover::run_mover_blocking_pass(
            &mut registry,
            collision_world,
            mover_colliders,
            &mover_poses,
            blocking_state,
            tick_dt,
            &mut mover_events,
            &mut on_impact,
        );
        drop(substage);
    }

    drop(stage);
    let stage = cpu.scope(SimStage::Weapons);
    weapon_stage::tick_weapon_resources(&mut registry.borrow_mut(), tick_dt);
    let remote_weapon_result = weapon_stage::run_remote_weapon_commands(
        &registry,
        &remote_pawn_commands,
        descriptors,
        default_weapon_placement,
        collision_world,
        hit_zone_store,
        anim_time,
        tick_dt,
    );
    let own_pawn = {
        let registry = registry.borrow();
        registry.local_player_movement_pawn()
    };
    let weapon_fire = weapon_stage::weapon_fire_command(command.fire_button, post_movement_command);
    let local_result: weapon_stage::LocalWeaponCommandResult =
        weapon_stage::run_local_weapon_command_with_content(
            &registry,
            own_pawn,
            mod_block_during_reload,
            descriptors,
            default_weapon_placement,
            command.select_slot,
            &weapon_fire,
            command.reload,
            collision_world,
            environment,
            hit_zone_store,
            anim_time,
            tick_dt,
            &mut on_impact,
            Some(crate::weapon::execution::ActivationCommand {
                tick: command.input_tick,
                pawn: 0,
                real_command: true,
                input: command.activation,
                controller_starts: true,
                primary: command.fire_button,
                secondary: command.secondary_button,
            }),
            local_fire_blocked,
        );
    let mut reload_deliveries = remote_weapon_result.reload_deliveries;
    reload_deliveries.extend(local_result.reload_deliveries);
    let mut reload = remote_weapon_result.reload_emissions;
    reload.extend(local_result.reload_emissions);
    let mut weapon = local_result.weapon_events;
    let weapon_fluid_splashes = local_result.fluid_splashes;
    let repointed_pawn = local_result.repointed_pawn;
    #[cfg(test)]
    let weapon_impact_points = local_result.weapon_impact_points;
    weapon.extend(remote_weapon_result.weapon_events);
    weapon.extend(trap_output.emissions);
    // Projectile contacts fire `impact` as hitscan contacts do, one per
    // shot per tick, whoever fired them.
    weapon.extend(projectile_stage::projectile_impact_emissions(
        &local_projectile_contacts,
    ));
    // AI and weapon stages can both launch after the flight pass. Consume the
    // launch tick's grace without moving those projectiles; next tick's
    // pre-AI flight pass advances them exactly once.
    projectile_stage::finish_spawn_tick(
        &mut registry.borrow_mut(),
        enemy_projectile_spawns
            .iter()
            .chain(&trap_output.projectile_spawns)
            .map(|spawn| spawn.projectile)
            .chain(local_result.projectile_spawns.iter().copied()),
    );
    if let Some(nanos) = environment_timer.fluid_crossing_nanos() {
        cpu.add_nanos(SimStage::WeaponFluidCrossings, nanos);
    }
    drop(stage);
    // Statuses run after every in-tick application and before the sweep, so
    // a lethal tick latches and credits on its own tick.
    let stage = cpu.scope(SimStage::Statuses);
    super::status_stage::run_status_stage(&mut registry.borrow_mut(), &mut on_impact);
    drop(stage);
    let death = run_death_sweep(&registry);
    // The assist accumulator reads the tick after the Weapons stage and the
    // death sweep, so an in-tick self-revive or kill is seen.
    let assist_completions = assist_state.run_host_pass(
        &mut registry.borrow_mut(),
        assist_slot_table
            .as_ref()
            .map(|table| table.borrow())
            .as_deref(),
        command,
        &remote_pawn_commands,
    );

    let mut repointed_pawns = touch_events.repointed_pawns;
    if let Some(pawn) = repointed_pawn {
        repointed_pawns.push(pawn);
    }
    repointed_pawns.sort_unstable();
    repointed_pawns.dedup();
    if let Some(nanos) = environment_timer.body_resolve_nanos() {
        cpu.add_nanos(SimStage::Environment, nanos);
    }
    if let Some(nanos) = collision_world.overlap_timer().finish() {
        cpu.add_nanos(SimStage::OverlapCheck, nanos);
    }
    registry
        .borrow_mut()
        .set_status_applications_immediate(false);
    drop(tick_scope);

    TickEvents {
        cpu,
        movement: movement.emissions,
        movement_edges: movement.state_edges,
        teleports,
        teleport_residuals,
        ai,
        weapon,
        weapon_fluid_splashes,
        #[cfg(test)]
        weapon_impact_points,
        mover: mover_events,
        death,
        remote_activation_progress: remote_weapon_result.activation_progress,
        authorized_shots: remote_weapon_result.authorized_shots,
        remote_projectile_presentation_launches: remote_weapon_result
            .projectile_presentation_launches,
        rejected_remote_projectile_fires: remote_weapon_result.rejected_projectile_fires,
        local_projectile_spawns: local_result.projectile_spawns,
        enemy_projectile_spawns,
        trap_projectile_spawns: trap_output.projectile_spawns,
        local_projectile_contacts,
        reload_deliveries,
        reload,
        repointed_pawns,
        dropped_item_meshes: touch_events.dropped_item_meshes,
        trigger_residuals,
        trigger_exit_fires,
        assist_completions,
        trigger_stage_pawns,
        #[cfg(test)]
        trigger_fires,
        #[cfg(test)]
        trigger_command_fires,
    }
}

pub(super) fn trigger_fire_context(
    event: &crate::trigger_system::TriggerEvent,
    occupancy: usize,
    canonical_player_pawns: &BTreeMap<PlayerId, EntityId>,
) -> TriggerFireContext {
    let activator = match event.fire.player {
        PlayerId::Local(pawn) => Some(pawn),
        PlayerId::Remote(_) => canonical_player_pawns.get(&event.fire.player).copied(),
    };
    if activator.is_none() && matches!(event.fire.player, PlayerId::Remote(_)) {
        log::warn!(
            "[Trigger] remote activator {:?} is absent from this tick; @activators is empty",
            event.fire.player
        );
    }
    TriggerFireContext {
        fired_trigger: Some(event.fire.trigger),
        fired_teleporter: None,
        activator,
        occupancy,
        event_player: None,
    }
}

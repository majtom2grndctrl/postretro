// The windowed frame loop: one `RedrawRequested` frame from timing through present.
// See: context/lib/rendering_pipeline.md §1 · context/lib/entity_model.md §5

use std::collections::HashMap;
use std::fmt::Write as _;
use std::time::{Duration, Instant};

use glam::Vec3;
use postretro_foundation::resolve_weapon_placement;
use postretro_scripting_core::reaction_dispatch::{
    ResidualOrigin, dispatch_deferred_named_events_with_sequences,
    fire_prepartitioned_reactions_with_sequences,
};
use postretro_visibility::{CameraCullVisibility, VisibilityPath, VisibleCells};
use winit::event_loop::ActiveEventLoop;

use crate::frame_timing::InterpolableState;
use crate::input::{Action, ButtonState};
use crate::netcode::frame_order;
use crate::startup::BootState;
use crate::{
    App, GamepadPollVotes, append_tick_weapon_script_events, apply_mover_yaw_carry,
    build_post_movement_command, build_sim_command, camera, cpu_timing,
    drain_named_events_with_sequences, follow_camera_to_local_pawn, followed_player_pawn,
    frame_eye, frontend_root_is_pushed, gameplay_capture_gate_for_frame,
    gameplay_snapshot_for_capture_state, has_player_pawn, impact_effects, input,
    local_active_wieldable, local_player_ground, local_viewmodel_asset, local_wieldable_occupancy,
    maybe_save_connected_client_per_owner_state, netcode, poll_gamepad, rebuild_blocked_portals,
    render, render_preparation, resolve_crouch_intent, resolve_mesh_entity_bindings_for_entities,
    resolve_sprint_intent, scripting_systems, sim, sound_events, trigger_system, view_feel,
    viewmodel_asset_for_archetype, viewmodel_world_transform,
};
#[cfg(feature = "dev-tools")]
use crate::{
    agent_diagnostics, door_occluder_diagnostics, drawable_visible_cell_mask, mover_diagnostics,
    trigger_diagnostics, update_debug_chase_agent_destination,
};

/// Runs one windowed frame. `window_event` dispatches `RedrawRequested` here
/// and does nothing after it, so an early `return` ends the frame.
///
/// A free function, not an `App` method: rustc codegens every method in its
/// self type's module, which would put this body back in the crate-root CGU
/// and recompile it on every main.rs edit.
pub(crate) fn redraw(app: &mut App, event_loop: &ActiveEventLoop) {
    // Fixed-timestep loop: accumulate wall-clock time, tick at
    // constant rate, interpolate for rendering.
    // See: context/lib/rendering_pipeline.md §1
    let now = Instant::now();
    let frame_result = app.frame_timing.begin_frame(now);
    let tick_dt = app.frame_timing.tick_dt();
    let frame_dt = frame_result.frame_dt;
    let ticks = frame_result.ticks;

    // CPU stage timing: frontend and early-returned frames never
    // commit. See: context/lib/rendering_pipeline.md §12
    app.cpu_timer.begin_frame(now);
    let cpu_stages = app.cpu_timer.stages();
    let stage_scope = cpu_stages.scope(cpu_timing::FrameStage::Housekeeping);
    // OS preference replies land ahead of the Input stage, so a
    // player write later this frame wins over them.
    app.poll_os_preferences();
    app.poll_window_mode_readback();

    #[cfg(feature = "observe-live")]
    app.drain_observe_live_requests();

    // Seat holds measure elapsed rendered time rather than fixed
    // simulation time: Frontend, Loading and Settling keep polling a
    // host even though none runs the simulation loop. Advance exactly
    // here, once per frame, because a Splash/install frame can drain the
    // transport more than once.
    app.advance_seat_hold_clock(frame_dt);

    // Drain changed paths every frame so the watcher channel does
    // not back up even when the summary is empty. ScriptRuntime
    // checks them against the active dependency set before queuing
    // the serialized staged build.
    //
    // Guarded behind the per-boot signal "the splash logo frame has
    // presented this boot cycle" (`splash_frame >= 2`: frame 0 = black,
    // frame 1 = logo) — so reload draining never runs before the splash
    // logo paints, and a suspend→resume re-blocks it until the resumed
    // logo repaints (suspend resets `splash_frame` to 0). Past the logo
    // also guarantees the script runtime exists: the watcher starts in
    // the deferred mod init on the logo frame, and the runtime is
    // session-lifetime. See: context/lib/boot_sequence.md §1.
    if crate::startup::boot_allows_reload_drain(app.splash_frame >= 2) {
        app.drain_script_reload_requests();
    }

    // A Settling redraw that reaches the Running path below is the reveal
    // frame: its release was decided on the previous frame.
    let reveal_frame = app.boot_state == BootState::Settling;
    if !app.drive_boot_state_for_redraw(event_loop, frame_dt) {
        app.service_window_modes();
        return;
    }

    // Advance the timed-reaction scheduler's monotonic frame counter
    // after the boot/install boundary but before any same-frame UI
    // dispatch or gameplay ticks. A `levelLoad` wait enrolled at install,
    // held through Settling, therefore advances on the reveal redraw's
    // first tick. A UI wait enrolled below stamps the new counter and
    // remains protected from this redraw's ticks.
    // Distinct from `frame_timing.begin_frame`.
    if let Some(session) = app.session.as_ref() {
        session.scripting.scheduler.begin_frame();
    }
    let options_menu_was_open = app.options_menu_is_top();

    if matches!(
        app.boot_state,
        BootState::Frontend | BootState::FirstLaunchHold
    ) {
        // Frontend has no world but is not a peerless state: keep an
        // installed endpoint alive before frontend-only game logic.
        // The first-launch hold runs the same world-less frame with
        // only the accessibility panel on the stack.
        let _ = app.poll_world_less_transport(frame_dt);
        if !app.run_frontend_ui_logic(event_loop, frame_dt, options_menu_was_open) {
            return;
        }
        app.render_frontend_frame(event_loop, now);
        app.finish_first_launch_hold_if_closed();
        return;
    }

    drop(stage_scope);
    let stage_scope = cpu_stages.scope(cpu_timing::FrameStage::Input);
    // Registry, layer, or host-tuning changes from earlier frames
    // rebuild the binding table before this frame's input reads it.
    app.refresh_effective_bindings();

    // The frame's animation sample clock is a single value shared by
    // game-side hit-zone pose resolution and render collection. It is
    // computed before game logic so same-tick animation switches hit
    // with the exact stamp the visible frame will resolve below.
    #[cfg(feature = "dev-tools")]
    let frozen = app
        .renderer
        .as_ref()
        .is_some_and(|renderer| renderer.freeze_time());
    #[cfg(not(feature = "dev-tools"))]
    let frozen = false;
    let frame_anim_time =
        App::frame_anim_time(app.anim_time, frame_dt as f64, app.anim_time_scale, frozen);

    // Tail of the Input stage: poll the gamepad, BEFORE the
    // `take_ready`/`advance_frame` pair below (see `poll_gamepad`).
    // Reached only in Running (Frontend returned above), so the
    // session is installed. Disjoint borrows of the session group and
    // the non-session `nav_stick_tracker`; mode-signal and menu-toggle
    // votes are collected and applied after the borrow ends.
    let gamepad_votes = {
        let App {
            session,
            nav_stick_tracker,
            ..
        } = app;
        session
            .as_mut()
            .map_or_else(GamepadPollVotes::default, |session| {
                poll_gamepad(session, nav_stick_tracker, frame_dt)
            })
    };
    if gamepad_votes.nav_seen {
        app.record_mode_signal(scripting_systems::input_mode::ModeSignal::NavInput);
    }
    // `nav.menu` (gamepad Start) toggles the pause menu via the
    // punch-through flag (Passthrough queues nothing).
    if gamepad_votes.menu_toggle {
        app.pending_menu_toggle = true;
    }
    // A pad lost or switched mid-charge cancels the activation, as
    // focus loss does: neutral input is never a charge release.
    if gamepad_votes.weapon_lifted
        && let Some(session) = app.session.as_mut()
    {
        app.client_weapon
            .suspend(&session.scripting.script_ctx.registry.borrow());
        session.gameplay_input_latch.activation.suspend();
    }

    // Resolve this frame's input-mode signal into the engine-owned
    // `input.mode` slot (app composition — the input subsystem's
    // contract output stays the action snapshot). Mouse motion votes
    // `pointer`, nav input votes `focus`, debounced so jitter doesn't
    // flap. Drives `ui_input_mode` (the focus engine's hover gate). The
    // mode is observation-only here; its cursor/ring EFFECT is gated on
    // a capturing tree being on the stack (applied in `reconcile_ui_focus`).
    // See: context/lib/input.md §7.
    let mode_signal = app.pending_mode_signal.take();
    if let Some(session) = app.session.as_mut() {
        let resolved_input_mode = session
            .scripting
            .input_mode_tracker
            .update(mode_signal, frame_dt);
        // Mouse motion moves glyphs to keyboard-and-mouse only once
        // it passes the pointer-mode debounce.
        if resolved_input_mode == input::InputMode::Pointer
            && session.ui_input_mode != input::InputMode::Pointer
        {
            session.device_family.note_keyboard_mouse();
        }
        session.ui_input_mode = resolved_input_mode;
        session.device_family.end_frame();
    }

    // Game-logic phase begins here. Read the UI captures made
    // available by the *previous* frame, THEN promote this frame's
    // freshly captured events for the next frame. Taking before
    // advancing is what enforces the N→N+1 contract: events captured
    // during THIS frame's Input stage (keyboard via `dispatch_event`,
    // gamepad via the poll just above) land in `pending` and are only
    // promoted to `ready` by this `advance_frame` call — so they
    // first become visible at the next frame's `take_ready`, never
    // this frame. This holds regardless of winit's event/redraw
    // ordering because both calls run here at game-logic time. The
    // modal stack consumes the drained intents; the drain marks the
    // seam where game logic reads them. See: context/lib/input.md
    let (ui_intents, ui_captured_gameplay_at_frame_start) = {
        let session = app.session.as_mut().expect("running session installed");
        let ui_intents = session.ui_dispatch.take_ready();
        session.ui_dispatch.advance_frame();
        let captured = session.ui_dispatch.mode() == input::UiCaptureMode::Capture;
        (ui_intents, captured)
    };

    // Text-entry resolution (M13 Text-Entry, Task 3): while a text-entry
    // tree is the top of the modal stack, the drained intents drive the
    // edit surface. `Text` appends and `Backspace` deletes against the
    // tree's `text_entry_target` slot (through Task 1's text-edit command
    // path); `nav.confirm` commits (fires the opener's `on_commit`, then
    // pops) and `nav.cancel` cancels (pops, no commit). Those confirm /
    // cancel intents are CONSUMED here so they never reach the focus
    // engine (no stray key-button activation) or the pause-menu logic
    // below. Returns whether a commit or cancel fired so the pause-menu
    // path is skipped this frame.
    let text_entry_consumed_nav = app.resolve_text_entry_intents(&ui_intents);
    // Shortcuts resolve after a commit or cancel, so one landing on
    // the frame text entry closes does nothing.
    app.apply_text_shortcuts(&ui_intents, frame_dt);

    // Focus engine (game-logic phase): split the drained intents into
    // nav (directional/confirm/cancel/next/prev) and pointer clicks,
    // then move focus through the TOP stack tree against the focus rect
    // list the renderer exported LAST frame (reverse N→N+1). The
    // focused id is published on this frame's snapshot below so the UI
    // pass draws the ring (it may trail a focus change by one frame).
    // Only the top tree takes focus; lower trees freeze. While text entry
    // is open, confirm/cancel were consumed above and are filtered out so
    // the focus engine sees only directional/next/prev moves (Task 4's
    // on-screen keyboard still navigates between keys).
    let mut nav_intents: Vec<input::NavIntent> = Vec::new();
    let mut click_positions: Vec<input::PointerPos> = Vec::new();
    for intent in &ui_intents {
        match &intent.payload {
            input::UiIntentPayload::Nav(nav) => {
                if text_entry_consumed_nav
                    && matches!(nav, input::NavIntent::Confirm | input::NavIntent::Cancel)
                {
                    // Consumed by the text-entry commit/cancel above.
                    continue;
                }
                nav_intents.push(*nav);
            }
            input::UiIntentPayload::PointerClick { pos } => click_positions.push(*pos),
            // Text / Backspace are text-entry edits, resolved above.
            input::UiIntentPayload::Text(_)
            | input::UiIntentPayload::Backspace
            | input::UiIntentPayload::TextShortcut(_) => {}
        }
    }
    // Slider nav-capture (M13 Goal F, Task 4): the focused slider gets
    // first refusal on its `capturesNav` wire names. A captured nav step
    // adjusts the slider's bound value by `step` within `[min, max]` and
    // emits a `setState` write (applied at the game-logic command drain
    // below → the bound slot changes on the N+1 frame). Captured intents
    // are removed so the focus engine never sees them (focus stays put).
    app.apply_slider_nav_capture(&mut nav_intents);

    // The active (top) tree key: the modal stack's top entry name, else
    // the always-on HUD. `None` is never the gameplay case (the HUD is
    // always present), but the engine handles it.
    let cursor = app.cursor_pos;
    let focus_result = {
        let session = app.session.as_mut().expect("running session installed");
        let (active_key, active_name) = session.ui_focus_target(postretro_ui::tree_asset::HUD_NAME);
        session.prune_ui_focus();
        let rects = crate::session::focus_rects_for(session.ui_focus_rects.as_ref(), &active_name);
        session.ui_focus.tick(
            Some(active_key.as_str()),
            rects,
            &nav_intents,
            cursor,
            &click_positions,
            session.ui_input_mode,
            frame_dt,
        )
    };
    app.ui_focused_id = focus_result.focused.clone();
    app.apply_slider_repeat_steps(focus_result.slider_steps);
    for tab in &focus_result.activations {
        app.fire_focused_button_activation(Some(tab));
    }

    // Button activation: a `confirm` (gamepad
    // confirm or pointer click — the focus engine reports both as
    // `confirmed`) on a focused button resolves its `onPress` as either
    // a reserved UI action or an ordinary named reaction, so a click and
    // a gamepad confirm have an identical observable effect.
    if focus_result.confirmed {
        app.fire_focused_button_activation(focus_result.focused.as_deref());
    }

    if app.pending_exit_to_desktop {
        app.pending_exit_to_desktop = false;
        app.release_cursor_for_exit();
        log::info!("[Engine] Shutting down");
        event_loop.exit();
        return;
    }

    // Pause-menu toggle: `nav.menu` (gamepad Start /
    // Escape-from-gameplay) opens the registered `pauseMenu` only from
    // an empty modal stack, closes it when it is active, and is ignored
    // while another modal is active. A `nav.cancel` (Escape / B inside
    // the menu) closes the active pause menu or a frontend submenu,
    // but never removes the frontend root. The capture-mode
    // + cursor effect follows on this frame's `reconcile_ui_focus`
    // below. The toggle flag is a punch-through from gameplay;
    // `cancelled` rides the captured-intent queue. While the
    // accessibility panel is the active tree, `nav.cancel` closes it
    // too.
    if app.pending_menu_toggle {
        app.pending_menu_toggle = false;
        app.toggle_pause_menu();
    } else if focus_result.cancelled && !text_entry_consumed_nav {
        let close_frontend_submenu = app.frontend_menu_is_present() && !app.frontend_menu_is_top();
        if let Some(session) = app.session.as_mut() {
            crate::app::ui_actions::apply_running_cancel_policy(
                &mut session.modal_stack,
                close_frontend_submenu,
            );
        }
    }

    // After the frame's activations: resolve a capture and refresh
    // the controls panel.
    app.update_controls_panel();
    app.sync_glyph_art();

    let ui_captures_gameplay = {
        let session = app.session.as_ref().expect("running session installed");
        gameplay_capture_gate_for_frame(ui_captured_gameplay_at_frame_start, &session.modal_stack)
    };

    // drain_look_inputs() must precede snapshot(); both touch
    // mouse_axes and look state belongs to the render-rate path.
    // Capturing UI still drains raw input to prevent stale deltas from
    // replaying later, but the consumed look is neutral so player aim
    // cannot move while a modal owns input. The reveal frame's look is
    // neutral too, so it presents the settled orientation even under a
    // stick held through the hold.
    let gameplay_snapshot = {
        let session = app.session.as_mut().expect("running session installed");
        let drained_look = session.input_system.drain_look_inputs();
        let look = if ui_captures_gameplay || reveal_frame {
            input::LookInputs::default()
        } else {
            drained_look
        };
        let frame_snapshot = session.input_system.snapshot();
        if ui_captures_gameplay {
            let registry = session.scripting.script_ctx.registry.borrow();
            app.client_weapon.suspend(&registry);
            let token = app.client_weapon.suppressed.map(|(_, token)| token);
            session.gameplay_input_latch.activation.set_active(token);
        }
        let gameplay_snapshot = gameplay_snapshot_for_capture_state(
            &mut session.gameplay_input_latch,
            &frame_snapshot,
            ticks,
            ui_captures_gameplay,
        );
        if !ui_captures_gameplay {
            let (occupied, active_slot) = {
                let registry = session.scripting.script_ctx.registry.borrow();
                local_wieldable_occupancy(&registry)
            };
            let cycle_dwell_ms = session
                .player_options
                .switch_cycle_dwell_ms
                .map(|dwell| dwell as f32)
                .unwrap_or(app.switching.cycle_commit_dwell_ms);
            session
                .gameplay_input_latch
                .wieldable_selection_mut()
                .advance_frame(
                    &frame_snapshot,
                    &occupied,
                    active_slot,
                    input::WieldableSelectionPolicy {
                        commit_on_direct_select: app.switching.commit_on_direct_select,
                        cycle_dwell_ms,
                    },
                    frame_dt * 1000.0,
                );
        }
        let pending_weapon_slot = session
            .gameplay_input_latch
            .wieldable_selection()
            .cursor_slot();
        session
            .scripting
            .player_hud_state
            .set_pending_weapon_slot(pending_weapon_slot);
        // Apply look rotation once at render rate, not once per tick —
        // so zero-tick frames still consume accumulated mouse motion.
        app.camera
            .rotate(look.yaw_delta(frame_dt), look.pitch_delta(frame_dt));
        gameplay_snapshot
    };

    if ticks == 0 && app.is_connected_client() {
        let session = app.session.as_mut().expect("running session installed");
        if let Some(token) = session.gameplay_input_latch.activation.take_cancel() {
            let mut command = build_sim_command(
                &input::ActionSnapshot::neutral(),
                &app.camera,
                false,
                false,
                false,
                false,
                false,
                false,
                false,
            );
            command.activation.cancel = Some(token);
            let _ = netcode::client_send_input_command(
                session.net_endpoint.as_mut(),
                &command,
                app.camera.pitch,
            );
        }
    }

    // The script tranche lives on `Session` (built post-first-pixel).
    // Clone the `ScriptCtx` handle once for this Game-logic phase (cheap
    // `Rc` bump) so the many `script_ctx.*` reads below borrow nothing of
    // `app`; the non-`Clone` session subsystems are reached through
    // disjoint scoped `app.session.as_mut()` borrows at each site.
    let script_ctx = app
        .session
        .as_ref()
        .expect("running session installed")
        .scripting
        .script_ctx
        .clone();

    // Bump the engine frame counter once per Game logic phase.
    // Reserved for primitives that need a per-frame ordering stamp.
    // See: context/lib/scripting.md
    script_ctx.frame.set(script_ctx.frame.get().wrapping_add(1));
    let engine_frame = script_ctx.frame.get();

    // Net poll (M15 Phase 1): non-blocking, once per frame, BEFORE
    // the catch-up tick loop. The client applies received
    // host-authoritative snapshots into the registry here so the
    // render below reflects this frame's replicated state. The host's
    // serialize + send runs AFTER the tick loop (post-loop, beside
    // the other drains). Single-player → inert no-op. See
    // `context/lib/entity_model.md` §6, development_guide §4.3.
    //
    // Driven through `netcode::frame_order` so the apply-before-detect
    // order is owned by one seam: the witness minted here is the only key
    // to the crossing stage below, so inverting the two is a type error.
    drop(stage_scope);
    let stage_scope = cpu_stages.scope(cpu_timing::FrameStage::SnapshotApply);
    let applied = frame_order::run_snapshot_apply_stage(app, engine_frame, frame_dt);
    drop(stage_scope);
    let stage_scope = cpu_stages.scope(cpu_timing::FrameStage::FixedStep);

    // Accumulate app-side residual and post-tick events across all ticks;
    // drain after the loop against fully-settled world state. Direct trigger
    // consequential work instead executes and rechecks inside each fixed tick.
    // Weapon and reload events share one stream so catch-up ticks stay ordered.
    // See: context/lib/entity_model.md §5
    let mut pending_movement_events: Vec<postretro_sim::emission::MovementEmission> = Vec::new();
    let mut pending_movement_edges: Vec<view_feel::TimedMovementEdge> = Vec::new();
    let mut pending_ai_events: Vec<postretro_sim::emission::AiEmission> = Vec::new();
    let mut pending_weapon_script_events = Vec::new();
    // Connected-client sounds derived outside the event drains (reload edges).
    let mut client_sounds: Vec<postretro_audio::SoundRequest> = Vec::new();
    // These edges are populated only by the authoritative simulation
    // branch below. Connected clients run the shared mover driver but
    // never enqueue host-local mover audio.
    let mut pending_mover_events = Vec::new();
    let mut pending_trigger_residuals = Vec::new();
    let mut repointed_pawns = Vec::new();

    let mut host_snapshot_due = false;
    // Death-event names accumulate here and join the sequence-aware
    // post-tick batch below, so a `progress` reaction naming a sequence
    // resolves. Frame-end removals append to the session buffer after
    // this drain, so take that carryover now rather than running game
    // logic during render.
    // Per-tick stage values, summed over this frame's ticks.
    let sim_cpu =
        postretro_stage_timing::StageFrame::<postretro_sim::sim::cpu_stages::SimStage>::new(
            app.cpu_timer.gate(),
        );
    let prediction_cpu = postretro_stage_timing::StageFrame::<cpu_timing::PredictionStage>::new(
        app.cpu_timer.gate(),
    );
    let mut pending_death_events = std::mem::take(
        &mut app
            .session
            .as_mut()
            .expect("running session installed")
            .pending_death_events,
    );

    // Fix B: restore connected-client pawns to their authoritative poses
    // before any fixed tick and before snapshot serialization (`net_serialize_and_send`
    // below) read them, undoing the previous frame's delayed presentation write.
    // Unconditional so it runs even on zero-tick / no-gameplay-snapshot frames —
    // serialization never ingests a delayed pose. A no-op off the host path.
    app.host_restore_client_pawn_authoritative_poses();

    if let Some(snapshot) = gameplay_snapshot.as_ref() {
        // `player_options` is session-owned; copy the crouch mode out
        // before the `&mut app.crouch_toggle_active` borrow.
        let crouch_mode = app
            .session
            .as_ref()
            .map(|session| session.player_options.crouch_mode)
            .unwrap_or_default();
        let crouch_intent = resolve_crouch_intent(
            crouch_mode,
            snapshot.button(Action::Crouch),
            &mut app.crouch_toggle_active,
        );
        let sprint_mode = app
            .session
            .as_ref()
            .map(|session| session.player_options.sprint_mode)
            .unwrap_or_default();
        let sprint_intent = resolve_sprint_intent(
            sprint_mode,
            snapshot.button(Action::Sprint),
            &mut app.sprint_toggle_active,
        );

        for tick_index in 0..ticks {
            let forward_axis = snapshot.axis_value(Action::MoveForward);
            let right_axis = snapshot.axis_value(Action::MoveRight);
            let up_axis = snapshot.axis_value(Action::MoveUp);
            let sprint = sprint_intent;

            let speed = if sprint {
                camera::MOVE_SPEED * camera::SPRINT_MULTIPLIER
            } else {
                camera::MOVE_SPEED
            };

            // Camera-vs-pawn split (entity_model.md §5/§7):
            //   - If a PlayerMovementComponent entity exists, its
            //     position drives `camera.position` (yaw/pitch stay
            //     mouse-driven).
            //   - Otherwise, fly-cam moves the camera directly so the
            //     engine is navigable without a player spawn (dev maps,
            //     levels without a player descriptor).
            let has_player_pawn = {
                let registry = script_ctx.registry.borrow();
                has_player_pawn(&registry)
            };

            // A connected client owns ZERO PlayerMovement pawns until the
            // host's `local_player` baseline arms one (M15 Phase 3). During
            // that pre-arm window it must NOT fly-cam: it holds the map's
            // first-spawn pose (seeded at install) so the view is steady
            // until its net pawn arrives. Without this guard the pawnless
            // branch below would drift the camera with movement input.
            let pre_arm_client = app.is_connected_client();

            if !has_player_pawn && !pre_arm_client {
                let forward = app.camera.forward();
                let right = app.camera.right();
                let mut move_dir = forward * forward_axis + right * right_axis + Vec3::Y * up_axis;

                // Normalize to prevent faster diagonal movement, but only
                // if there's actual movement input.
                if move_dir.length_squared() > 0.0 {
                    move_dir = move_dir.normalize();
                }

                app.camera.position += move_dir * speed * tick_dt;
            }

            let dash_pressed =
                tick_index == 0 && matches!(snapshot.button(Action::Dash), ButtonState::Pressed);
            let shoot_pressed =
                tick_index == 0 && matches!(snapshot.button(Action::Shoot), ButtonState::Pressed);
            let use_pressed =
                tick_index == 0 && matches!(snapshot.button(Action::Use), ButtonState::Pressed);
            let drop_pressed =
                tick_index == 0 && matches!(snapshot.button(Action::Drop), ButtonState::Pressed);
            let mut trigger_use_edges = HashMap::new();
            if use_pressed {
                let registry = script_ctx.registry.borrow();
                if let Some(pawn) = followed_player_pawn(&registry) {
                    trigger_use_edges.insert(trigger_system::PlayerId::Local(pawn), true);
                }
            }
            let mut touch_drop_edges = HashMap::new();
            if drop_pressed {
                let registry = script_ctx.registry.borrow();
                if let Some(pawn) = followed_player_pawn(&registry) {
                    touch_drop_edges.insert(trigger_system::PlayerId::Local(pawn), true);
                }
            }
            {
                let registry = script_ctx.registry.borrow();
                apply_mover_yaw_carry(
                    &mut app.camera,
                    app.mover_yaw_carry_ground,
                    &app.kinematic_mover_tick_states,
                );
                app.mover_yaw_carry_ground = local_player_ground(&registry);
            }
            let select_slot = if tick_index == 0 {
                let (occupied, active_slot) = {
                    let registry = script_ctx.registry.borrow();
                    local_wieldable_occupancy(&registry)
                };
                app.session
                    .as_mut()
                    .expect("running session installed")
                    .gameplay_input_latch
                    .wieldable_selection_mut()
                    .take_pending_commit(&occupied, active_slot)
            } else {
                None
            };
            let mut command = build_sim_command(
                snapshot,
                &app.camera,
                crouch_intent,
                sprint_intent,
                dash_pressed,
                shoot_pressed,
                false,
                use_pressed,
                drop_pressed,
            );
            command.select_slot = select_slot;
            let input_tick = netcode::client_peek_next_command_tick(
                app.session.as_ref().and_then(|s| s.net_endpoint.as_ref()),
            );
            let active_token = {
                let registry = script_ctx.registry.borrow();
                local_active_wieldable(&registry).and_then(|(_, id)| registry.get_component::<postretro_entities::components::weapon::WeaponComponent>(id).ok()).and_then(|component| component.state.activation_cursor()).map(|cursor| cursor.token)
            };
            let capture = &mut app
                .session
                .as_mut()
                .expect("running session installed")
                .gameplay_input_latch
                .activation;
            capture.set_active(active_token);
            let input_tick = input_tick.unwrap_or_else(|| capture.next_local_tick());
            command.input_tick = input_tick;
            command.activation = capture.command(input_tick);
            if tick_index > 0 {
                command.secondary_button.pressed = false;
            }

            // Connected-client prediction (M15 Phase 3 Task 3): send one
            // Input command and advance ONLY the local pawn's movement
            // through the movement-only replay helper — never the full
            // `simulate_tick` (AI / weapons / death stay host-authoritative
            // and arrive via snapshots). The camera follows the predicted
            // pawn; frame timing pushes the predicted camera pose. Task 5
            // adds reconciliation/smoothing on top of this seam.
            if app.is_connected_client() {
                let local_pawn = {
                    let registry = script_ctx.registry.borrow();
                    registry.local_player_movement_pawn()
                };
                let predict_scope = prediction_cpu.scope(cpu_timing::PredictionStage::Wieldable);
                let (switch_accepted, repointed) = {
                    let hit_zone_store = &app
                        .session
                        .as_ref()
                        .expect("connected client session installed")
                        .hit_zone_store;
                    sim::simulate_client_wieldable_tick(
                        script_ctx.registry.clone(),
                        &app.collision_world,
                        hit_zone_store,
                        local_pawn,
                        app.switching.block_during_reload,
                        command.select_slot,
                        command.fire_button,
                        command.reload,
                        frame_anim_time,
                        tick_dt,
                    )
                };
                drop(predict_scope);
                if let Some(pawn) = repointed {
                    repointed_pawns.push(pawn);
                }
                if switch_accepted && let Some(slot) = command.select_slot {
                    app.client_declare_switch(slot, input_tick);
                }
                {
                    let _scope = prediction_cpu.scope(cpu_timing::PredictionStage::Movers);
                    app.client_predict_loaded_movers_tick(tick_dt);
                }
                let prediction_tick = {
                    let _scope = prediction_cpu.scope(cpu_timing::PredictionStage::Movement);
                    app.predict_client_weapon_command(&mut command, input_tick, tick_dt);
                    app.client_predict_movement_tick(&command, tick_dt)
                };
                if let Some(prediction_tick) = prediction_tick {
                    let mut addresses = Vec::new();
                    prediction_tick
                        .movement_events
                        .append_named_events(&mut addresses);
                    // Predicted movement sounds from the local pawn.
                    if let Some(emitter) = app.session.as_ref().and_then(|session| {
                        let registry = session.scripting.script_ctx.registry.borrow();
                        registry
                            .local_player_movement_pawn()
                            .map(|pawn| postretro_sim::emission::entity_emitter(&registry, pawn))
                    }) {
                        pending_movement_events.extend(addresses.into_iter().map(|address| {
                            postretro_sim::emission::MovementEmission {
                                address,
                                emitter: emitter.clone(),
                            }
                        }));
                    }
                    pending_movement_edges.extend(
                        prediction_tick
                            .movement_events
                            .state_edges
                            .iter()
                            .copied()
                            .map(|edge| view_feel::TimedMovementEdge {
                                edge,
                                age: (ticks - tick_index - 1) as f32 * tick_dt,
                            }),
                    );
                }
                // Tick-rate camera follow tracks the PRESENTED local pose:
                // the gameplay-authoritative (snapped) registry pose plus the
                // decaying presentation offset. Folding the offset in HERE —
                // before `frame_timing.push_state` — is the fix for the
                // velocity-proportional first-person shake (M15 Phase 3
                // playtest bug). Reconcile snaps the registry backward by the
                // correction each snapshot and seeds the offset forward by the
                // same amount, so `registry + offset` is continuous across the
                // snap. If `frame_timing` instead carried the bare (snapped)
                // registry pose and the offset were re-added only at render,
                // `frame_timing` would interpolate ACROSS the snap (a backward
                // arc) while a constant offset over-corrected at alpha 0 — the
                // exact ∝-velocity oscillation. With the presented pose pushed,
                // both `frame_timing` endpoints sit in presented space and the
                // render-rate interpolation between consecutive presented poses
                // IS the smoother; the offset decays once per tick here.
                let presentation_offset = netcode::client_local_presentation_offset(
                    app.session
                        .as_ref()
                        .and_then(|session| session.net_endpoint.as_ref()),
                );
                if has_player_pawn {
                    let registry_ref = script_ctx.registry.borrow();
                    follow_camera_to_local_pawn(
                        &mut app.camera,
                        &registry_ref,
                        presentation_offset,
                    );
                }
                // Decay the offset one step now that this tick's camera pose
                // has baked in the current value. Tick-rate decay (paired with
                // the presented-pose push) keeps `frame_timing` continuous;
                // the render stage reads the interpolated presented eye
                // directly and must NOT re-add the offset (it is already in
                // the pose), so there is no double-count.
                netcode::client_decay_local_correction(
                    app.session
                        .as_mut()
                        .and_then(|session| session.net_endpoint.as_mut()),
                );
                app.frame_timing
                    .push_state(InterpolableState::new(app.camera.position));
                continue;
            }

            // Inventory liveness is entity lifecycle, not an input event.
            // Normalize every authoritative pawn even when a remote queue
            // produces no command this tick; active changes reuse the
            // ordinary repoint attachment-dirty path below.
            repointed_pawns.extend(sim::normalize_wieldable_inventories(
                &mut script_ctx.registry.borrow_mut(),
            ));

            // Freeze the declaration set against the pre-command
            // authorization state. The sim ingests this batch before
            // AI; declarations waiting on this tick's FIRE stay in
            // the pending queue for the existing post-sim drain.
            let mut ready_hit_declarations = app.host_take_ready_hit_declarations();
            // Remote clients' validated shots, each one `impact`.
            let mut remote_impacts: Vec<postretro_sim::emission::WeaponEmission> = Vec::new();

            // Host: resolve remote (owned) pawn inputs up front, then the
            // shared `simulate_tick` runs loaded movers and every player
            // movement consumer against the same combined collision query.
            let resolved_remote_commands = app.host_resolve_remote_commands();
            let remote_pawn_commands =
                app.host_prepare_remote_pawn_commands(&resolved_remote_commands);
            trigger_use_edges.extend(remote_pawn_commands.iter().filter_map(|remote| {
                remote.command.use_pressed.then_some((
                    trigger_system::PlayerId::Remote(remote.owner_client_id),
                    true,
                ))
            }));
            touch_drop_edges.extend(remote_pawn_commands.iter().filter_map(|remote| {
                remote.command.drop_pressed.then_some((
                    trigger_system::PlayerId::Remote(remote.owner_client_id),
                    true,
                ))
            }));

            // Borrow the two session-owned `simulate_tick` inputs
            // (hit-zone store, progress tracker) and the boot-owned
            // `camera` as disjoint field borrows; the post-movement
            // closure captures these locals (not `app`) so it does not
            // re-borrow `app.session`.
            let data_registry = script_ctx.data_registry.borrow();
            let descriptors = &data_registry.entities;
            let descriptor_generation = data_registry.entity_types_generation();
            let default_weapon_placement = data_registry.default_weapon_placement.as_ref();
            let collision_world = &app.collision_world;
            let session = app.session.as_mut().expect("running session installed");
            let net_endpoint = &mut session.net_endpoint;
            let hit_zone_store = &session.hit_zone_store;
            let progress_tracker = &mut session.progress_tracker;
            let scripting = &mut session.scripting;
            let trigger_system = &mut session.trigger_system;
            let touch_system = &mut session.touch_system;
            let trigger_volume_bridge = &session.trigger_volume_bridge;
            let trigger_bindings = &app.trigger_bindings;
            let presentation_camera_aim = (app.camera.pitch, app.camera.yaw);
            let camera = &mut app.camera;
            #[cfg(feature = "dev-tools")]
            let debug_chase_agent = app.debug_chase_agent;
            let tick_events = sim::simulate_tick_with_presentation_aim(
                script_ctx.registry.clone(),
                collision_world,
                hit_zone_store,
                app.nav_graph.as_ref(),
                script_ctx.gravity.get(),
                app.switching.block_during_reload,
                frame_anim_time,
                presentation_camera_aim,
                progress_tracker,
                postretro_ai::tick_runner!(&mut app.ai_runtime),
                &app.kinematic_mover_colliders,
                &mut app.kinematic_mover_tick_states,
                &remote_pawn_commands,
                &command,
                |registry| {
                    // Camera follows the selected local pawn before
                    // weapon fire resolves its aim ray.
                    if has_player_pawn {
                        let registry_ref = registry.borrow();
                        // Host / single-player: no client-side correction
                        // offset (the host pawn is authoritative).
                        follow_camera_to_local_pawn(camera, &registry_ref, Vec3::ZERO);
                    }

                    #[cfg(feature = "dev-tools")]
                    {
                        let mut registry_ref = registry.borrow_mut();
                        update_debug_chase_agent_destination(
                            &mut registry_ref,
                            debug_chase_agent,
                            camera.position,
                        );
                    }

                    build_post_movement_command(camera)
                },
                tick_dt,
                touch_system,
                descriptors,
                descriptor_generation,
                &data_registry.factions,
                script_ctx.faction_sentiment.as_ref(),
                default_weapon_placement,
                &trigger_use_edges,
                &touch_drop_edges,
                Some(sim::TriggerTickContext {
                    system: trigger_system,
                    bridge: trigger_volume_bridge,
                    bindings: trigger_bindings,
                    slot_table: script_ctx.slot_table.clone(),
                    script_ctx: Some(script_ctx.clone()),
                    auto_close_timers: Some(scripting.auto_close_timers.clone()),
                    use_edges: &trigger_use_edges,
                }),
                |registry, on_impact| {
                    let Some(netcode::NetEndpoint::Host {
                        server,
                        allocator,
                        owners,
                        open_shots,
                        projectile_presentations,
                        tick,
                        ..
                    }) = net_endpoint.as_mut()
                    else {
                        return;
                    };
                    let _ = netcode::host_ingest_ready_hit_declarations(
                        server,
                        registry,
                        collision_world,
                        hit_zone_store,
                        allocator,
                        owners,
                        open_shots,
                        *tick,
                        frame_anim_time,
                        std::mem::take(&mut ready_hit_declarations),
                        |registry| on_impact(registry),
                        |shot_id, point| projectile_presentations.note_contact(shot_id, point),
                        |impact| remote_impacts.push(impact),
                    );
                },
                |registry| scripting.evaluate_pending_in_tick_impacts(registry),
                sim_cpu.gate(),
            );
            sim_cpu.absorb(&tick_events.cpu);
            // Advance timed-reaction countdowns for this tick. Position
            // relative to `evaluate_slot_accumulators` is not
            // behaviourally load-bearing: landings execute at the
            // frame-end drain, after every tick's accumulator pass. An
            // instance enrolled this frame is skipped via its stamp.
            // This tick's paired-trigger Exit fires cancel matching
            // interruptible instances before the countdown advances,
            // so an Exit on the exact landing tick wins.
            scripting
                .scheduler
                .evaluate(&tick_events.trigger_exit_fires);
            // A runtime-spawned host enemy receives a mesh only
            // after the install-time whole-registry clip resolve.
            // Drain its one-shot queue now: its archetype model and
            // clip table were preloaded from the map spawner, so this
            // is solely an animation-index fill, never a GPU upload.
            let mut spawned_meshes = session
                .scripting
                .spawn_context
                .take_pending_mesh_clip_resolves();
            spawned_meshes.extend(tick_events.dropped_item_meshes.iter().copied());
            resolve_mesh_entity_bindings_for_entities(
                &mut script_ctx.registry.borrow_mut(),
                &session.mesh_clip_tables,
                &session.hit_zone_store,
                spawned_meshes,
            );
            // Runtime descriptor spawns can carry dynamic lights.
            // Enroll them after the fixed tick; the renderer still
            // receives only the bridge's CPU-packed update.
            session
                .light_bridge
                .absorb_dynamic_lights(&script_ctx.registry.borrow());
            scripting_systems::slot_accumulators::evaluate_slot_accumulators(
                &mut session.scripting.slot_accumulator_bindings,
                tick_dt,
            );
            app.host_record_activation_progress(&tick_events.remote_activation_progress);
            app.host_record_authorized_shots(&tick_events.authorized_shots);
            app.host_spawn_projectile_presentations(
                &script_ctx.registry,
                &tick_events.remote_projectile_presentation_launches,
                &tick_events.local_projectile_spawns,
                &tick_events.enemy_projectile_spawns,
            );
            app.host_note_local_projectile_contacts(&tick_events.local_projectile_contacts);
            if app.host_flush_pending_hit_declarations(frame_anim_time, &mut remote_impacts) {
                pending_death_events.extend(app.host_run_remote_hit_death_sweep());
            }
            app.host_advance_projectile_presentations(&script_ctx.registry, tick_dt);
            pending_movement_events.extend(tick_events.movement);
            pending_movement_edges.extend(tick_events.movement_edges.into_iter().map(|edge| {
                view_feel::TimedMovementEdge {
                    edge,
                    age: (ticks - tick_index - 1) as f32 * tick_dt,
                }
            }));
            app.publish_observer_weapon_cues(&tick_events.weapon, &tick_events.ai);
            pending_ai_events.extend(tick_events.ai);
            append_tick_weapon_script_events(
                &mut pending_weapon_script_events,
                tick_events.weapon,
                tick_events.reload,
            );
            pending_weapon_script_events.append(&mut remote_impacts);
            pending_mover_events.extend(tick_events.mover);
            repointed_pawns.extend(tick_events.repointed_pawns);
            pending_death_events.extend(tick_events.death);
            pending_trigger_residuals.extend(tick_events.trigger_residuals);

            app.frame_timing
                .push_state(InterpolableState::new(app.camera.position));
            // Fix B: capture each connected-client pawn's authoritative pose
            // for this tick before the tick stamp advances, so the buffered
            // sample is keyed to the tick whose end-of-tick pose it carries.
            app.host_record_client_pawn_poses();
            app.host_advance_fixed_sim_tick(&mut host_snapshot_due);
            // Unconditional per-tick registration sweeps, not gated on "did a spawn
            // happen this tick": they catch runtime enemies and component-driven
            // world-item acquisition/drop changes before post-loop serialization.
            app.host_register_map_enemies_after_fixed_sim_tick();
            app.host_register_world_items_after_fixed_sim_tick();
        }
    }

    // Fixed ticks run this frame. A UI-captured frame still ticks
    // (on a neutral snapshot), so this is the accumulator's count.
    let ticks_run = ticks;
    cpu_stages.add_count(cpu_timing::FrameStage::Ticks, u64::from(ticks_run));
    let fixed_step_label = Some(postretro_stage_timing::StageSet::label(
        cpu_timing::FrameStage::FixedStep,
    ));
    let nested_cpu = app.cpu_timer.nested_mut();
    nested_cpu.extend_from(&sim_cpu, fixed_step_label);
    nested_cpu.extend_from(&prediction_cpu, fixed_step_label);
    drop(stage_scope);
    let stage_scope = cpu_stages.scope(cpu_timing::FrameStage::Presentation);

    // Regression: a turntable's transform slerps through this tick while
    // carry_yaw previously held the local view until the next input seam.
    let presented_aim = app.presented_aim_pose(frame_result.alpha);
    let render_camera_yaw = presented_aim.yaw;

    // Task 6 client remote interpolation: sample each remote entity's
    // buffer at `estimated_server_tick - interpolation_delay` and write the
    // interpolated pose through the registry's remote-presentation helper.
    // Runs AFTER the tick loop (so the stage-0 `snapshot_transforms` does
    // not clobber the previous/current pair this writes) and BEFORE the
    // render stage reads entities, so the renderer stays read-only.
    // No-op for single-player and the host.
    app.net_sample_remote_interpolation(frame_dt, frame_anim_time);
    app.update_repointed_weapon_attachments(&script_ctx, &repointed_pawns);
    // Connected clients skip the authoritative `simulate_tick`, so
    // generate renderer-facing pose inputs here from the freshly
    // interpolated displayed transforms. These transient mesh fields
    // are client presentation only and never enter replication.
    app.update_client_presentation_pose_inputs(frame_anim_time, render_camera_yaw);
    app.update_client_overlay_anchors(&script_ctx, frame_anim_time);
    app.run_client_fire_path_post_loop(
        frame_dt,
        frame_anim_time,
        presented_aim,
        &mut pending_weapon_script_events,
    );
    if app.is_connected_client() {
        app.observe_client_weapon_edges(&mut client_sounds);
    }
    // Observed impact cues burst here, once, in the frame they arrived: the
    // cue list is rebuilt every frame in `net_poll_and_apply`. Only a connected
    // client receives cues, so other roles skip the registry borrow.
    if app.is_connected_client() {
        app.spawn_observer_impact_bursts(&mut script_ctx.registry.borrow_mut());
    }

    // Status overlays are host/single-player presentation facts.
    // This runs once after every fixed tick (including zero-tick
    // frames), so a same-frame damage refresh is stamped before
    // Render while a create-then-kill is removed before it draws.
    if !app.is_connected_client() {
        let session = app.session.as_mut().expect("running session installed");
        let overlay_config = session
            .scripting
            .impact_policy_runtime
            .client_overlay_config();
        if let Some(config) = overlay_config.as_ref()
            && let Some(netcode::NetEndpoint::Host {
                server,
                allocator,
                replicable,
                owners,
                ..
            }) = session.net_endpoint.as_mut()
        {
            session
                .host_overlay_fact_tracker
                .begin_frame(frame_dt, config.linger_seconds, owners);
            let overlay_frame = {
                let registry = script_ctx.registry.borrow();
                session
                    .scripting
                    .impact_policy_runtime
                    .update_damaged_enemy_overlays(
                        &mut session.presentation_pool,
                        &registry,
                        &session.hit_zone_store,
                        frame_anim_time,
                        session.host_overlay_fact_tracker.tracked_entities(),
                        |source| source.is_some_and(|source| owners.owner_of(source).is_some()),
                    )
            };

            // The host renderer pool contains only host-local feedback.
            // Each remote recipient has an independently capped fact
            // stream on the unreliable presentation channel.
            netcode::send_host_overlay_facts(
                &mut session.host_overlay_fact_tracker,
                server,
                allocator,
                replicable,
                owners,
                &overlay_frame,
                config.max_visible,
            );
        } else {
            session.host_overlay_fact_tracker.clear();
            let registry = script_ctx.registry.borrow();
            let _ = session
                .scripting
                .impact_policy_runtime
                .update_damaged_enemy_overlays(
                    &mut session.presentation_pool,
                    &registry,
                    &session.hit_zone_store,
                    frame_anim_time,
                    [],
                    |_| false,
                );
        }
    }

    drop(stage_scope);
    let stage_scope = cpu_stages.scope(cpu_timing::FrameStage::Audio);

    // Descriptor sounds for this frame's events, placed at fire time.
    // They play alongside any reaction addressed to the same event.
    // The listener's pawn is named here, before any play this frame,
    // with the key the audio step's listener carries: own-pawn
    // treatment is decided at `play`, ahead of `audio.update`.
    let (pending_mover_edges, descriptor_sounds, listener_attached) = {
        let registry = script_ctx.registry.borrow();
        let listener_attached = sound_events::listener_attached_key(&registry);
        let mut scene = sound_events::AnchorScene {
            registry: &registry,
            world: app.level.as_ref(),
            movers: &mut app.kinematic_mover_render,
        };
        let edges = sound_events::resolve_mover_edges(&pending_mover_events, &mut scene);
        let mut requests: Vec<postretro_audio::SoundRequest> = edges
            .iter()
            .filter_map(sound_events::MoverEdge::sound_request)
            .collect();
        if let Some(session) = app.session.as_ref() {
            let table = &session.scripting.descriptor_sounds;
            requests.extend(
                pending_movement_events
                    .iter()
                    .filter_map(|emission| sound_events::movement_sound(emission, &mut scene)),
            );
            for emission in &pending_ai_events {
                requests.extend(sound_events::ai_sounds(table, emission, &mut scene));
            }
            requests.extend(pending_weapon_script_events.iter().filter_map(|emission| {
                sound_events::weapon_emission_sound(table, emission, &mut scene)
            }));
        }
        for cue in &app.observer_weapon_cues {
            for key in [cue.sound.as_deref(), cue.additional_sound.as_deref()]
                .into_iter()
                .flatten()
            {
                requests.push(sound_events::frozen_sound(key, &cue.emitter, &mut scene));
            }
        }
        requests.append(&mut client_sounds);
        (edges, requests, listener_attached)
    };
    if let Some(audio) = app
        .session
        .as_mut()
        .and_then(|session| session.audio.as_mut())
    {
        audio.set_listener_attached(listener_attached);
        for request in descriptor_sounds {
            audio.play(request);
        }
    }
    drop(stage_scope);
    let stage_scope = cpu_stages.scope(cpu_timing::FrameStage::ScriptDrain);

    // A mover edge's reactions may play `at: on.emitter`: the mover.
    // An edge with no point publishes no emitter, so such a reaction
    // is skipped, as the edge's own descriptor sound is dropped.
    let pending_mover_event_names: Vec<(String, Option<postretro_entities::Emitter>)> =
        pending_mover_edges
            .into_iter()
            .filter_map(|edge| {
                let emitter = edge.reaction_emitter();
                edge.address.map(|address| (address, emitter))
            })
            .collect();
    if let Some(session) = app.session.as_ref() {
        let mut pending_trigger_follow_ups = Vec::new();
        // Every post-tick named source uses the executing path, then
        // contributes `fire`/`on_complete` names to one bounded deferred
        // batch. This keeps movement, AI, weapon, mover, and death events
        // semantically aligned and lets waits enroll through the common
        // sequence control arm.
        pending_trigger_follow_ups.extend(drain_named_events_with_sequences(
            pending_movement_events
                .iter()
                .map(|emission| (emission.address, Some(emission.emitter.clone()))),
            &script_ctx.data_registry.borrow(),
            &session.scripting.sequence_registry,
            &session.scripting.reaction_registry,
            &session.scripting.system_registry,
            &script_ctx,
        ));
        pending_trigger_follow_ups.extend(drain_named_events_with_sequences(
            pending_ai_events.iter().filter_map(|emission| {
                emission
                    .address
                    .as_deref()
                    .map(|address| (address, Some(emission.emitter.clone())))
            }),
            &script_ctx.data_registry.borrow(),
            &session.scripting.sequence_registry,
            &session.scripting.reaction_registry,
            &session.scripting.system_registry,
            &script_ctx,
        ));
        pending_trigger_follow_ups.extend(drain_named_events_with_sequences(
            pending_ai_events
                .iter()
                .filter(|emission| emission.shot_id.is_some())
                .flat_map(|emission| {
                    std::iter::once("activate")
                        .chain(
                            emission
                                .action
                                .as_ref()
                                .and_then(|a| a.emits.as_ref())
                                .and_then(|e| e.activate.as_deref()),
                        )
                        .map(move |address| (address, Some(emission.emitter.clone())))
                }),
            &script_ctx.data_registry.borrow(),
            &session.scripting.sequence_registry,
            &session.scripting.reaction_registry,
            &session.scripting.system_registry,
            &script_ctx,
        ));
        pending_trigger_follow_ups.extend(drain_named_events_with_sequences(
            app.observer_weapon_cues.iter().flat_map(|cue| {
                let builtin = match cue.kind {
                    netcode::weapon_cues::WeaponCueKind::Activate => "activate",
                    netcode::weapon_cues::WeaponCueKind::Impact => "impact",
                };
                std::iter::once(builtin)
                    .chain(cue.alias.as_deref())
                    .map(move |address| (address, Some(cue.emitter.clone())))
            }),
            &script_ctx.data_registry.borrow(),
            &session.scripting.sequence_registry,
            &session.scripting.reaction_registry,
            &session.scripting.system_registry,
            &script_ctx,
        ));
        pending_trigger_follow_ups.extend(drain_named_events_with_sequences(
            pending_weapon_script_events.iter().flat_map(|emission| {
                sound_events::weapon_emission_addresses(emission)
                    .map(move |address| (address, Some(emission.emitter.clone())))
            }),
            &script_ctx.data_registry.borrow(),
            &session.scripting.sequence_registry,
            &session.scripting.reaction_registry,
            &session.scripting.system_registry,
            &script_ctx,
        ));
        pending_trigger_follow_ups.extend(drain_named_events_with_sequences(
            pending_mover_event_names,
            &script_ctx.data_registry.borrow(),
            &session.scripting.sequence_registry,
            &session.scripting.reaction_registry,
            &session.scripting.system_registry,
            &script_ctx,
        ));
        pending_trigger_follow_ups.extend(drain_named_events_with_sequences(
            pending_death_events.iter().map(|name| (name, None)),
            &script_ctx.data_registry.borrow(),
            &session.scripting.sequence_registry,
            &session.scripting.reaction_registry,
            &session.scripting.system_registry,
            &script_ctx,
        ));
        for (handle, trigger, player) in &pending_trigger_residuals {
            let Some(residual) = app.trigger_bindings.residual(*handle) else {
                log::warn!("[Trigger] residual handle {handle:?} was not bound at install");
                continue;
            };
            // Scope the origin guard to THIS residual iteration only,
            // released before the deferred batch below: a `wait`
            // reached synchronously here keys its instance to this
            // `(trigger, player)`, while a batch-seeded `fire` stays
            // sourceless. The paired-enter standing check reads the
            // trigger system from the session the drain already
            // holds — an interruptible instance parks only while its
            // origin's enter is live, so a player who left within
            // the frame does not park an uncancellable beat.
            let paired_enter_standing = session
                .trigger_system
                .paired_enters()
                .contains(&(*trigger, *player));
            let _origin =
                session
                    .scripting
                    .scheduler
                    .begin_origin(*trigger, *player, paired_enter_standing);
            pending_trigger_follow_ups.extend(fire_prepartitioned_reactions_with_sequences(
                residual.steps(),
                &session.scripting.sequence_registry,
                &session.scripting.reaction_registry,
                &session.scripting.system_registry,
                &script_ctx,
                ResidualOrigin::TriggerBinding,
            ));
        }
        if !pending_trigger_follow_ups.is_empty() {
            // Direct residual work has already been partitioned and
            // run above. Follow-up names advance by bounded FIFO
            // hops so authored onComplete order is never flattened.
            dispatch_deferred_named_events_with_sequences(
                pending_trigger_follow_ups,
                &script_ctx.data_registry.borrow(),
                &session.scripting.sequence_registry,
                &session.scripting.reaction_registry,
                &session.scripting.system_registry,
                &script_ctx,
            );
        }
        // Resume timed-reaction landings AFTER the trigger follow-up
        // dispatch and OUTSIDE any origin guard: a resumed tail runs
        // where a trigger residual runs, but each landing gets its own
        // deferred-dispatch call so a `fire`-seeded child's depth is
        // attributable per instance. The scheduler owns its tails as
        // `Vec<SequenceStep>` and never mints a `TriggerResidualHandle`,
        // so this never resolves through `app.trigger_bindings`.
        // `take_landings` (inside `drain_landings`) `mem::take`s the
        // queue, so nothing borrows it across the block — no need to
        // move it onto `App`.
        //
        // Before draining, drop any interruptible instance whose keyed
        // trigger left the level mid-wait: `paired_enters` retains only
        // live triggers, so a surviving parked interruptible instance
        // absent from it has no Exit to ever cancel on and must not land
        // uncancelled.
        session
            .scripting
            .scheduler
            .drop_orphaned_interruptible_instances(session.trigger_system.paired_enters());
        session.scripting.scheduler.drain_landings(
            &script_ctx.data_registry.borrow(),
            &session.scripting.sequence_registry,
            &session.scripting.reaction_registry,
            &session.scripting.system_registry,
            &script_ctx,
        );
    }

    // System-reaction command drain — runs AFTER every post-tick
    // event drain so commands enqueued by movement/AI/weapon/death
    // reactions (and, later, crossing watchers) are taken in one
    // batch. The typed queue keeps audio/input/UI services out of
    // the scripting surface; the dispatcher routes each command to
    // its subsystem consumer. See: scripting.md §10.4.
    // NOTE: a SECOND drain runs later this frame, after the state
    // crossings fire (see the crossing-detection block below), so
    // crossing-enqueued commands land this frame, not the next.
    if !script_ctx.system_commands.is_empty() {
        app.dispatch_system_commands();
    }

    // Player HUD state: republish engine-owned health/ammo/reload slots
    // after game logic settles and before crossing detection / UI
    // snapshot construction, so same-frame consumers see the
    // settled pawn and weapon state. In-tick impact evaluation has
    // already published health at each fire seam. See:
    // context/lib/scripting.md §5.
    //
    // A connected client skips host-authoritative HUD slot writes:
    // those values arrive through state-slot apply. It still samples
    // a materialized local weapon so reload-feedback acknowledgement
    // cannot accumulate. A missing local weapon is a safe no-op.
    let is_connected_client = app.is_connected_client();
    let hud_sampled_weapon = if let Some(session) = app.session.as_mut() {
        session
            .scripting
            .player_hud_state
            .set_charge_presentation_suppression(app.client_weapon.suppressed);
        session
            .scripting
            .player_hud_state
            .tick_for_role_and_report_sampled_weapon(is_connected_client, None)
    } else {
        None
    };
    // Flash-decay state writes the engine-owned `screen.flash`
    // surface at the same game-logic stage as the HUD publisher, so
    // the UI snapshot below freezes this frame's flash color. Runs
    // after the first command drain so a flash started this frame
    // publishes immediately; the crossing drain below may start
    // another, decayed starting next frame.
    if let Some(session) = app.session.as_mut() {
        session.scripting.flash_decay.tick(frame_dt);
        // Vignette- and shake-decay drivers (SE) write the engine-owned
        // `screen.vignette` and `screen.shake` surfaces at the same
        // game-logic stage as `flash_decay.tick`, so the UI snapshot
        // below freezes this frame's vignette color and shake offset.
        // Delta-driven from `frame_dt` (not wall-clock) like the flash
        // decay.
        session.scripting.vignette_decay.tick(frame_dt);
        session.scripting.shake_decay.tick(frame_dt);
    }

    // State-crossing detection (M13 HUD dynamics). Runs AFTER the
    // frame's slot writes (game logic + HUD publisher) settle, so
    // it compares the authoritative slot value — distinct from the
    // eased display value styleRanges read mid-tween. Each watched
    // slot's threshold crossing fires its reaction list synchronously
    // through Task 2's shared named-reaction path; any system
    // reactions thereby enqueued are drained immediately below so
    // crossing-fired commands land in this frame, not the next.
    //
    // Consumes this frame's `SnapshotsApplied` witness: on a connected
    // client the replicated slot writes this frame's snapshots carried
    // have already landed, so a crossing fires on the SAME frame its
    // authoritative value arrives, never a frame late.
    let _crossings = frame_order::run_crossing_stage(app, engine_frame, applied);
    if !script_ctx.system_commands.is_empty() {
        app.dispatch_system_commands();
    }

    app.update_player_options(frame_dt, options_menu_was_open);
    app.commit_render_extents();

    // Connected-client per-owner persistence runs exactly after the
    // second command drain: every fixed tick and same-frame crossing
    // write has settled, and neither the SlotTable nor registry
    // RefCell is borrowed. Keep this synchronous on the main thread
    // so it cannot race clean exit or the retained state document.
    if let Some(session) = app.session.as_mut() {
        maybe_save_connected_client_per_owner_state(
            session,
            std::time::Duration::from_secs_f32(frame_dt),
        );
    }

    if let Some(session) = app.session.as_mut() {
        session
            .scripting
            .impact_policy_runtime
            .discard_app_drain_pending();
    }

    drop(stage_scope);
    let stage_scope = cpu_stages.scope(cpu_timing::FrameStage::FrameEndRemoval);

    // Terminal impact effects stay live through every post-catch-up
    // presentation/reaction drain above. Reap them exactly once per
    // rendered frame, before replication and render observe state.
    impact_effects::run_end_of_frame_removal_pass(
        &mut script_ctx.registry.borrow_mut(),
        |removal| {
            let session = app.session.as_mut().expect("running session installed");
            let fired = removal.report_to_progress(&mut session.progress_tracker);
            session.pending_death_events.extend(fired);
        },
    );

    drop(stage_scope);
    let stage_scope = cpu_stages.scope(cpu_timing::FrameStage::HostSend);

    // Host serialize + send after terminal removals, so the
    // authoritative snapshot cannot carry an entity already reaped
    // this frame. No-op for the client and single-player.
    let owner_projected_weapons = app.net_serialize_and_send(host_snapshot_due);

    // Fix B: present connected-client pawns from the delay buffer at a delayed
    // fractional target, AFTER serialization read the authoritative poses and
    // BEFORE the render collectors read entities. Clocked off the host's own
    // authoritative tick plus the render sub-tick `alpha`, so the presented
    // pose varies smoothly per render frame instead of stepping at 60 Hz.
    app.host_present_client_pawns(frame_result.alpha);

    // Advance each reload-endpoint consumer only after it sampled
    // this frame. Catch-up endpoints remain queued in tick order.
    if let Some(weapon) = hud_sampled_weapon {
        sim::clear_reload_feedback_for_weapon(&mut script_ctx.registry.borrow_mut(), weapon);
    }
    sim::clear_owner_reload_feedback_for_weapons(
        &mut script_ctx.registry.borrow_mut(),
        &owner_projected_weapons,
    );

    drop(stage_scope);
    let stage_scope = cpu_stages.scope(cpu_timing::FrameStage::Eye);

    // Reconcile the input seam + focus with the modal stack's top
    // capture mode, now that every command drain this frame has
    // settled the stack. A capturing top tree gates player controls,
    // freezes lower UI layers, and releases the cursor (`InputFocus::Menu`);
    // an empty/passthrough top hands input back to gameplay.
    app.reconcile_ui_focus();
    app.apply_frontend_menu_camera_pose_if_present();

    // Gameplay rendering shares the pose used by post-loop shots.
    // A frontend menu opened by this frame's reactions has its own
    // camera hold, applied above after the command drains settle.
    let presented_aim = app.render_aim_pose(presented_aim);

    // M15 Phase 3 Task 5: the connected client's local-pawn presentation
    // offset is already baked into the camera pose `frame_timing` carries
    // (folded in at the tick-rate camera-follow seam above, where the offset
    // also decays once per tick). So the interpolated eye IS the presented
    // eye — re-adding the offset here would double-count it and re-introduce
    // the ∝-velocity oscillation it was moved to fix. `frame_timing`
    // interpolates between consecutive PRESENTED poses, so the smoothed
    // correction reaches the view matrix, camera uniforms, cell locator,
    // and portal apex continuously across each reconcile snap.
    // Single-player and the host carry a ZERO offset, so this is the bare
    // interpolated eye for them, unchanged.
    let presented_eye = presented_aim.position;

    // View-feel assembly (movement.md D1/D5/D6) runs once per frame,
    // here, ahead of the audio step: render and the audio listener
    // read this one evaluated eye (`audio.md` §3). View feel only runs
    // when the camera-following pawn carries `view_feel`; another
    // pawn's preset must not leak onto the selected camera.
    let view_feel_driver = {
        let registry = script_ctx.registry.borrow();
        followed_player_pawn(&registry).and_then(|pawn| {
            registry
                .get_component::<postretro_foundation::PlayerMovementComponent>(pawn)
                .ok()
                .and_then(|component| {
                    component
                        .view_feel
                        .as_ref()
                        .map(|params| frame_eye::ViewFeelDriver {
                            pawn,
                            params: params.clone(),
                            velocity: component.velocity,
                            is_grounded: component.is_grounded(),
                            movement_state: component.movement_state.kind(),
                            eye_height_above_feet: frame_eye::presented_eye_clearance(
                                presented_eye.y,
                                app.camera.position.y,
                                component.capsule.eye_height
                                    + component.capsule.half_height
                                    + component.capsule.radius,
                            ),
                        })
                })
        })
    };
    let view_feel_scale = app
        .session
        .as_ref()
        .and_then(|session| session.options_bridge.resolved())
        .map(|resolved| resolved.presented_view_feel_scale())
        .unwrap_or(1.0);
    let eye = frame_eye::assemble_frame_eye(
        presented_aim.frame_eye_inputs(
            app.camera.aspect(),
            view_feel_driver,
            &pending_movement_edges,
            frame_dt,
            view_feel_scale,
        ),
        frame_eye::ViewFeelTracking {
            state: &mut app.view_feel_state,
            followed_pawn: &mut app.view_feel_followed_pawn,
            descriptor: &mut app.view_feel_descriptor,
        },
    );
    let render_camera = eye.camera;
    let view_proj = render_camera.view_projection;
    // The render eye and matrix are assembled together.
    // Portal traversal, camera uniforms, and every render-stage
    // distance/cell query must use the same point. Using the
    // unbobbed interpolated position here can put the visibility
    // apex in a different cell or on the opposite side of a
    // portal plane, causing one-frame clear-color holes.
    let render_eye_position = render_camera.eye_position;

    drop(stage_scope);
    let stage_scope = cpu_stages.scope(cpu_timing::FrameStage::Audio);

    // Audio step — third in frame order (Input → Game logic →
    // Audio → Render → Present, development_guide.md §4.3). Runs after
    // game logic settles every entity and before render. The listener
    // is the rendered eye assembled above, converted to the primitive
    // `ListenerState` at this call site (the boundary carries no glam).
    // Guarded for the silent (init-failed) case. The anchor resolver
    // places each positional voice at its emitter's
    // render-interpolated pose this frame.
    let listener_registry = app
        .session
        .as_ref()
        .map(|session| session.scripting.script_ctx.registry.clone());
    if let (Some(registry), Some(audio)) = (
        listener_registry,
        app.session
            .as_mut()
            .and_then(|session| session.audio.as_mut()),
    ) {
        let registry = registry.borrow();
        let listener = frame_eye::listener_for(
            &render_camera,
            sound_events::listener_attached_key(&registry),
        );
        let mut scene = sound_events::AnchorScene {
            registry: &registry,
            world: app.level.as_ref(),
            movers: &mut app.kinematic_mover_render,
        };
        audio.update(listener, frame_dt, |key| {
            scene.presented_point(key, frame_result.alpha)
        });
    }

    drop(stage_scope);
    let stage_scope = cpu_stages.scope(cpu_timing::FrameStage::Visibility);

    // Level-relative monotonic clock consumed by light_bridge.update,
    // the emitter sim, and the map-light collector.
    // Widen to f64 at the accumulation boundary so summing across
    // long sessions (30+ min at 144 Hz) doesn't quantize the
    // millisecond-precision clock the fog volume bridge consumes.
    //
    // Dev-tools freeze must stop BOTH clocks together. The GPU `time`
    // uniform is fed `script_time`, and the CPU light bridge computes
    // `effective_brightness` (which gates shadow-pool eligibility)
    // from the same clock. Freezing only the GPU uniform would let
    // the CPU clock advance, re-creating the CPU/GPU animation-phase
    // desync this branch fixed. Read the freeze flag from the
    // renderer — it owns the toggle (driven by the debug panel) — and
    // skip the increment while frozen so both sides hold one phase.
    if !frozen {
        app.script_time += frame_dt as f64;
        // Animation clock accumulates scaled dt at the same site,
        // under the same freeze gate. Accumulation (not absolute-time
        // scaling) keeps a mid-fade scale change from jumping poses;
        // scale 0 holds every clip and fade. See scripting.md §10.3.
        app.anim_time = frame_anim_time;
    }

    let capture_portal_walk = std::mem::take(&mut app.capture_portal_walk_next_frame);

    {
        let registry = script_ctx.registry.borrow();
        rebuild_blocked_portals(&mut app.blocked_portals, app.level.as_ref(), &registry);
    }

    // Portal DFS → cell IDs → renderer inputs. This stays after
    // game and audio work; the shared preparation seam also feeds
    // VM-free capture without changing frame order.
    let visible_render = match app.level.as_ref() {
        Some(world) => render_preparation::VisibleRenderPreparation::for_level(
            world,
            render_eye_position,
            view_proj,
            &app.blocked_portals,
            capture_portal_walk,
            &mut app.scratch_cells,
            app.cpu_timer.gate(),
        ),
        None => render_preparation::VisibleRenderPreparation::empty_world(),
    };
    let render_preparation::VisibleRenderPreparation {
        visible_cells,
        fog_reachable,
        light_reachable_cell_mask,
        reachable_cell_aabbs,
        stats,
    } = visible_render;
    // Walk time and counters (or the fallback marker) sit under the
    // binary's visibility stage; staged until the frame commits.
    app.cpu_timer.nested_mut().extend_from(
        &stats.cpu,
        Some(postretro_stage_timing::StageSet::label(
            cpu_timing::FrameStage::Visibility,
        )),
    );
    drop(stage_scope);
    // `Option` so the renderer block can hand over to `Render`.
    let mut stage_scope = Some(cpu_stages.scope(cpu_timing::FrameStage::RenderPrep));

    // A streamed map retains only its validated manifest. Keep the
    // application-side controller keyed to this exact load before
    // the renderer records the frame; legacy storage is `None` and
    // bypasses the developer mode gate entirely.
    let sh_stream_manifest = app
        .level
        .as_ref()
        .and_then(|world| world.sh_stream_manifest())
        .cloned();

    #[cfg(feature = "dev-tools")]
    if let Some(renderer) = app.renderer.as_mut() {
        let locator = match app.level.as_ref() {
            Some(world) => {
                render::LocatorDiagnostics::Trace(world.trace_locate_cell(render_eye_position))
            }
            None => render::LocatorDiagnostics::NoLevel,
        };
        renderer.set_spatial_diagnostics(render::SpatialDiagnostics {
            current_cell: app.level.as_ref().map(|_| stats.camera_cell),
            portal_drawable_cells: render::SpatialCellSetDiagnostics::from_visible_cells(
                &visible_cells,
            ),
            fog_reachable_cells: render::SpatialCellSetDiagnostics::from_cell_slice(&fog_reachable),
            locator,
        });
        renderer.refresh_camera_cull_diagnostics(
            CameraCullVisibility {
                cells: &visible_cells,
                path: stats.path,
            },
            view_proj,
        );
    }

    // World anchors project into the span the upscaled scene
    // covers (scene × divisor, anchored top-left), so each lands on
    // the scene pixel it marks even when the surface does not
    // divide evenly. Committed this frame, so never a 0×0 minimize.
    let presentation_viewport = app
        .renderer
        .as_ref()
        .map(|renderer| renderer.render_extents().upscaled_scene())
        .map(|span| [span.width, span.height])
        .unwrap_or([0, 0]);
    let is_connected_client = app.is_connected_client();

    if let Some(renderer) = app.renderer.as_mut() {
        // The render-stage bridges + collectors live on `Session`;
        // borrow it once here (disjoint from the `renderer` borrow of
        // `app.renderer` and from the other `app` fields read below).
        let session = app.session.as_mut().expect("running session installed");
        // The player's reduce-motion switch reaches presentation
        // here, at the frame-time call site; simulation never reads it.
        let motion = if session
            .options_bridge
            .resolved()
            .is_some_and(|resolved| resolved.reduce_motion)
        {
            crate::presentation_pool::MotionPreference::Reduced
        } else {
            crate::presentation_pool::MotionPreference::Full
        };
        let presentation_inputs = {
            let mut registry = script_ctx.registry.borrow_mut();
            session.presentation_pool.advance_and_collect_inputs(
                &mut registry,
                frame_dt,
                view_proj,
                presentation_viewport,
                motion,
            )
        };
        let recycled_inputs = renderer.set_presentation_draw_inputs(presentation_inputs);
        session
            .presentation_pool
            .recycle_draw_inputs(recycled_inputs);
        // Emitter bridge — after script `tick` handler, before particle
        // sim. Spawns new particles; the sim advances them the same
        // frame so they don't appear stuck at origin.
        {
            let mut registry = script_ctx.registry.borrow_mut();
            // Cap headroom comes from the previous frame's sim tally
            // (see particle_sim::tick) — the bridge no longer walks the
            // ParticleState column itself.
            session.emitter_bridge.update(
                &mut registry,
                frame_dt,
                app.script_time as f32,
                &app.particle_live_counts,
            );
        }

        // Particle sim — after emitter bridge, before light bridge.
        // Pure Rust; scripts never observe individual particles.
        // Refills `particle_live_counts` with this tick's per-emitter
        // survivor count for the next frame's bridge headroom.
        {
            let mut registry = script_ctx.registry.borrow_mut();
            scripting_systems::particle_sim::tick(
                &mut registry,
                frame_dt,
                script_ctx.gravity.get(),
                &mut app.particle_live_counts,
            );
        }

        // Light bridge — between Game Logic and Render. Uploads
        // mutated `LightComponent` data before `render_frame_indirect`
        // allocates slots, so scripted lights reflect their new state.
        {
            let mut registry = script_ctx.registry.borrow_mut();
            // Connected clients materialize predicted and remote
            // projectile lights locally from shared descriptors. They
            // skip the host tick's enrollment path, so absorb before
            // this render-frame update makes those lights visible.
            if is_connected_client {
                session.light_bridge.absorb_dynamic_lights(&registry);
            }
            if let Some(update) = session.light_bridge.update(
                &mut registry,
                app.script_time as f32,
                frame_result.alpha,
            ) {
                let snapshot_committed = if update.has_dirty_data {
                    renderer.upload_light_bridge_snapshot(
                        update.lights_bytes,
                        update.influence_bytes,
                        update.descriptor_bytes,
                        update.samples_bytes,
                        update.effective_brightness,
                        update.animated_window_brightness,
                        update.compose_descriptor_writes,
                    )
                } else {
                    renderer.set_light_effective_brightness(update.effective_brightness);
                    renderer
                        .set_animated_light_window_brightness(update.animated_window_brightness);
                    true
                };
                if !snapshot_committed {
                    // Renderer retained its prior coherent GPU and
                    // promotion-gate state. Retry the entire bridge
                    // transaction next frame instead of advancing
                    // only the CPU-side dirty generation.
                    session.light_bridge.retry_snapshot_upload();
                }
            }
        }

        // Fog volume bridge — alongside the light bridge. Volume
        // packing reads `FogVolumeComponent`; point-light packing
        // pre-culls dynamic point lights against fog AABBs. Upload
        // happens unconditionally so an empty list zeroes the GPU
        // volume count and skips the pass for the rest of the frame.
        // The light bridge tracks the full authored light list plus
        // script-spawned dynamic lights. The fog bridge filters that
        // snapshot to the dynamic point-light subset it consumes.
        // `collect_all_as_map_lights` pairs each light with its
        // brightness multiplier so the two cannot drift out of alignment
        // when a `LightComponent` lookup fails.
        {
            // Evaluate fog animation curves (density and saturation)
            // before `update_volumes` packs the GPU buffer — `tick`
            // writes sampled values into each `FogVolumeComponent`
            // so the existing pack path picks them up unchanged.
            let mut registry = script_ctx.registry.borrow_mut();
            session
                .fog_volume_bridge
                .tick(&mut registry, app.script_time);
        }
        let all_lights = {
            let registry = script_ctx.registry.borrow();
            if let Some((bytes, planes, live_mask)) =
                session.fog_volume_bridge.update_volumes(&registry)
            {
                renderer.upload_fog_volumes(bytes, planes, live_mask);
            } else {
                renderer.upload_fog_volumes(&[], &[], 0);
            }
            renderer.set_fog_aabbs(session.fog_volume_bridge.active_aabbs());
            session
                .light_bridge
                .collect_all_as_map_lights(&registry, app.script_time as f32)
        };
        let point_bytes = session.fog_volume_bridge.update_points(&all_lights);
        renderer.upload_fog_points(point_bytes);

        renderer.update_per_frame_uniforms(view_proj, render_eye_position, app.script_time as f32);
        renderer.update_viewmodel_view_projection(app.camera.aspect(), render_camera.view_matrix);

        // This gameplay block runs only in Running (the redraw
        // path reaches here solely when `boot_state == Running`,
        // set after full renderer init), so the renderer is always
        // full-ready; the mesh-collect + draw submission below runs
        // unconditionally, like the `full_mut`-backed uploads above.
        // Particle render — packs `SpriteInstance` bytes per
        // collection; the collector never touches wgpu directly.
        {
            let registry = script_ctx.registry.borrow();
            // Cull non-visible emitters at render-collect, mirroring
            // the mesh path below: thread the level world + this
            // frame's visible-cell set so off-screen / adjacent-room
            // smoke is never packed for drawing. `visible_cells` is
            // still live here (reclaimed after the frame).
            let presentation_tick = match session.net_endpoint.as_ref() {
                Some(netcode::NetEndpoint::Host { tick, .. }) => f64::from(*tick),
                Some(netcode::NetEndpoint::Client {
                    time_sync,
                    replication,
                    ..
                }) => time_sync
                    .estimated_server_tick()
                    .unwrap_or_else(|| replication.latest_server_tick().map_or(0.0, f64::from)),
                None => 0.0,
            };
            session.particle_render.collect_at_tick(
                &registry,
                app.level.as_ref(),
                &visible_cells,
                presentation_tick,
            );
        }
        // One level-scope streaming step for SH and lightmap blocks (one read
        // issuer, one shared install budget), run while no borrowed draw
        // collection is live: it prepares SH's batch, which drains as the first
        // step inside `render_frame_indirect`, and drains the lightmap now, so
        // the frame samples what it made resident. Its CPU folds under
        // `render_prep` below.
        let streaming_cpu = postretro_stage_timing::StageFrame::<cpu_timing::StreamingStage>::new(
            app.cpu_timer.gate(),
        );
        let sh_drain_batch = match session.run_level_streaming_step(
            sh_stream_manifest.as_ref(),
            app.level.as_ref(),
            renderer,
            crate::session::level_streaming::StreamingFrame {
                visible_cells: &visible_cells,
                camera_cell: app.level.as_ref().map(|_| stats.camera_cell as usize),
                path: stats.path,
                monotonic_seconds: app.script_time,
                settling: false,
                cpu: &streaming_cpu,
            },
        ) {
            Ok(batch) => batch,
            Err(err) => {
                app.exit_result = Err(err);
                event_loop.exit();
                return;
            }
        };
        app.cpu_timer.nested_mut().extend_from(
            &streaming_cpu,
            Some(postretro_stage_timing::StageSet::label(
                cpu_timing::FrameStage::RenderPrep,
            )),
        );
        let particle_collections: Vec<(&str, &[u8])> =
            session.particle_render.iter_collections().collect();

        // Mesh render — emits per-instance inputs (model handle +
        // interpolated transform + phase seed) for skinned-mesh
        // entities. Forward visibility comes from
        // `mesh_pass::mesh_visible`; selected-static shadow casters
        // can be retained as non-forward instances. Like the particle
        // collector it never touches wgpu; the renderer consumes the
        // inputs via `set_mesh_draws`. Runs before
        // `render_frame_indirect`, while `visible_cells` is still live
        // (it is reclaimed into scratch after).
        if let Some(world) = app.level.as_ref() {
            // Resolve pass: fill every pending animation entry
            // stamp from this frame's post-advance animation clock
            // before the collector samples poses. Runs with a
            // mutable registry, immediately before the (read-only)
            // collector, so same-tick switches have all landed and
            // the last target's stamp is concrete. See mesh.rs.
            {
                let mut registry = script_ctx.registry.borrow_mut();
                postretro_entities::components::mesh::resolve_pending_animation_stamps(
                    &mut registry,
                    app.anim_time,
                );
            }
            let registry = script_ctx.registry.borrow();
            // Same frame alpha the player camera reads from
            // `frame_timing` — interpolate each mesh between its
            // previous- and current-tick transforms.
            session.mesh_render.collect_with_hit_zones(
                &registry,
                world,
                &visible_cells,
                frame_result.alpha,
                app.anim_time,
                &session.mesh_clip_tables,
                // Camera eye position — the same value that seeds
                // the portal flood-fill — drives the per-instance
                // animation time-slicing distance bucket.
                render_eye_position,
                &session.hit_zone_store,
            );

            // The first-person model is not an entity attachment. A connected
            // client resolves its local asset by inventory or replicated
            // archetype, but always takes effective placement from host tuning.
            let descriptors = script_ctx.data_registry.borrow();
            if let Some(local_pawn) = followed_player_pawn(&registry) {
                let viewmodel = match session.net_endpoint.as_ref() {
                    Some(netcode::NetEndpoint::Client {
                        replication,
                        tuning,
                        ..
                    }) => local_viewmodel_asset(&registry, local_pawn, &descriptors.entities)
                        .and_then(|(weapon, model, _, active_slot)| {
                            Some((
                                weapon.to_raw(),
                                model,
                                tuning.as_deref()?.placement_for_slot(active_slot)?.clone(),
                            ))
                        })
                        .or_else(|| {
                            let archetype = replication.local_active_weapon_archetype()?;
                            let (model, _) =
                                viewmodel_asset_for_archetype(archetype, &descriptors.entities)?;
                            let placement = tuning
                                .as_deref()?
                                .placement_for_archetype(archetype)?
                                .clone();
                            Some((local_pawn.to_raw(), model, placement))
                        }),
                    _ => local_viewmodel_asset(&registry, local_pawn, &descriptors.entities).map(
                        |(weapon, model, placement, _)| {
                            (
                                weapon.to_raw(),
                                model,
                                resolve_weapon_placement(
                                    descriptors.default_weapon_placement.as_ref(),
                                    None,
                                    placement.as_ref(),
                                    None,
                                ),
                            )
                        },
                    ),
                };
                if let Some((weapon_seed, model, placement)) = viewmodel {
                    session.mesh_render.collect_viewmodel(
                        model,
                        viewmodel_world_transform(
                            render_camera.view_matrix,
                            eye.camera_right,
                            eye.eye_offset,
                            eye.roll,
                            eye.yaw_offset,
                            eye.pitch_offset,
                            &placement,
                        ),
                        weapon_seed,
                    );
                }
            }
            renderer.set_mesh_draws(session.mesh_render.instances());

            app.kinematic_mover_render.collect(
                &registry,
                world,
                &visible_cells,
                frame_result.alpha,
            );
            renderer.set_kinematic_mover_draws(
                app.kinematic_mover_render.instances(),
                app.kinematic_mover_render.shadow_instances(),
            );
            renderer.set_mover_occluder_aabbs(app.kinematic_mover_render.occluder_aabbs());
        }

        #[cfg(feature = "dev-tools")]
        let (agent_overlay_geometry, agent_overlay_labels) = {
            let agent_overlay_state = renderer.agent_overlay_state();
            let diagnostics_visible = session
                .debug_ui
                .as_ref()
                .is_some_and(|debug_ui| debug_ui.is_visible());
            let include_geometry = agent_overlay_state.enabled
                && (agent_overlay_state.paths
                    || agent_overlay_state.velocities
                    || agent_overlay_state.destinations);
            let include_labels =
                diagnostics_visible || (agent_overlay_state.enabled && agent_overlay_state.labels);
            if include_geometry || include_labels {
                let registry = script_ctx.registry.borrow();
                let viewport_size_points = app
                    .window_state
                    .as_ref()
                    .map(|ws| {
                        let size = ws.window.inner_size();
                        let scale_factor = ws.window.scale_factor() as f32;
                        egui::vec2(
                            size.width as f32 / scale_factor,
                            size.height as f32 / scale_factor,
                        )
                    })
                    .unwrap_or(egui::Vec2::ZERO);
                agent_diagnostics::collect_agent_overlay_snapshots_for_view(
                    &registry,
                    view_proj,
                    viewport_size_points,
                    include_geometry,
                    include_labels,
                )
            } else {
                (Vec::new(), Vec::new())
            }
        };
        #[cfg(feature = "dev-tools")]
        let agent_rows = agent_diagnostics::agent_overlay_diagnostics_rows(&agent_overlay_labels);
        #[cfg(feature = "dev-tools")]
        let (trigger_rows, trigger_overlay_labels) = {
            let diagnostics_visible = session
                .debug_ui
                .as_ref()
                .is_some_and(|debug_ui| debug_ui.is_visible());
            if diagnostics_visible {
                let registry = script_ctx.registry.borrow();
                let viewport_size_points = app
                    .window_state
                    .as_ref()
                    .map(|ws| {
                        let size = ws.window.inner_size();
                        let scale_factor = ws.window.scale_factor() as f32;
                        egui::vec2(
                            size.width as f32 / scale_factor,
                            size.height as f32 / scale_factor,
                        )
                    })
                    .unwrap_or(egui::Vec2::ZERO);
                (
                    trigger_diagnostics::collect_trigger_diagnostics_rows(
                        &registry,
                        &session.trigger_volume_bridge,
                        &session.trigger_system,
                        &app.trigger_bindings,
                        &app.trigger_pool_report,
                    ),
                    trigger_diagnostics::collect_trigger_overlay_labels(
                        &registry,
                        &session.trigger_volume_bridge,
                        &session.trigger_system,
                        view_proj,
                        viewport_size_points,
                    ),
                )
            } else {
                (Vec::new(), Vec::new())
            }
        };
        #[cfg(feature = "dev-tools")]
        let door_occluder_diagnostics = {
            let diagnostics_visible = session
                .debug_ui
                .as_ref()
                .is_some_and(|debug_ui| debug_ui.is_visible());
            if diagnostics_visible {
                let registry = script_ctx.registry.borrow();
                door_occluder_diagnostics::collect(&registry, &app.blocked_portals)
            } else {
                door_occluder_diagnostics::DoorOccluderDiagnostics::default()
            }
        };

        // Build the egui UI before `render_frame_indirect` so
        // the SH diagnostic overlay can push debug lines that
        // the frame's debug-line pass will pick up. Tessellated
        // paint jobs are stashed and consumed after the frame
        // by `render_debug_ui`; texture deltas queue on the
        // `DebugUi` so a frame that never presents carries them.
        #[cfg(feature = "dev-tools")]
        let debug_ui_frame: Option<(Vec<egui::epaint::ClippedPrimitive>, f32)> = {
            let mut out = None;
            // `debug_ui` is session-owned; reach it through the
            // already-held `session` borrow (the window is a disjoint
            // `app` field).
            if let (Some(debug_ui), Some(ws)) =
                (session.debug_ui.as_mut(), app.window_state.as_ref())
            {
                let agent_label_state = renderer.agent_overlay_state();
                let paint_agent_labels = agent_label_state.enabled && agent_label_state.labels;
                let diagnostics_visible = debug_ui.is_visible();
                if diagnostics_visible || paint_agent_labels {
                    let window = &ws.window;
                    let raw_input = debug_ui.winit_state.take_egui_input(window);
                    let timing_snapshot = renderer.frame_timing_snapshot().cloned();
                    let cpu_timing_panel = if !app.cpu_timer.gate().is_enabled() {
                        render::debug_ui::CpuTimingPanel::Off
                    } else if let Some(window) = app.cpu_timer.last_window() {
                        render::debug_ui::CpuTimingPanel::Window(window)
                    } else {
                        render::debug_ui::CpuTimingPanel::NotYetWindowed
                    };
                    let panel_state = &mut debug_ui.panel_state;
                    let sh_state = &mut debug_ui.sh_diagnostics_state;
                    let sh_streaming_live = session
                        .sh_streaming
                        .as_ref()
                        .map(|streaming| streaming.live_diagnostics());
                    // The tab edits a copy of the levers; changes
                    // are written back after the UI runs.
                    let lightmap_streaming = session.level_streaming.lightmap().map(|streaming| {
                        (*streaming.live_diagnostics(), streaming.slider_levers())
                    });
                    let mut lightmap_levers = lightmap_streaming.map(|(_, levers)| levers);
                    let reach_before = session.level_streaming.cell_demand().map(|stage| {
                        render::StreamingReachLever {
                            lead_metres: stage.lead_metres(),
                            max_lead_metres: stage.max_lead_metres(),
                        }
                    });
                    let mut reach = reach_before;
                    let ctx_clone = debug_ui.ctx.clone();
                    let full_output = ctx_clone.run_ui(raw_input, |ui| {
                        let ctx = ui.ctx();
                        if paint_agent_labels {
                            agent_diagnostics::paint_agent_overlay_labels(
                                ctx,
                                &agent_overlay_labels,
                            );
                        }
                        if diagnostics_visible {
                            trigger_diagnostics::paint_trigger_overlay_labels(
                                ctx,
                                &trigger_overlay_labels,
                            );
                            render::debug_ui::draw_diagnostics_panel(
                                ctx,
                                panel_state,
                                sh_state,
                                renderer,
                                timing_snapshot.as_ref(),
                                cpu_timing_panel,
                                &agent_rows,
                                &trigger_rows,
                                &door_occluder_diagnostics.mover_rows,
                                &door_occluder_diagnostics.blocked_portal_ids,
                                sh_streaming_live,
                                lightmap_streaming
                                    .as_ref()
                                    .zip(lightmap_levers.as_mut())
                                    .map(|((diagnostics, _), levers)| {
                                        render::debug_ui::LightmapStreamingTab {
                                            diagnostics,
                                            levers,
                                        }
                                    }),
                                reach.as_mut(),
                            );
                        }
                    });
                    if let (Some((_, before)), Some(after)) = (lightmap_streaming, lightmap_levers)
                        && before != after
                        && let Some(streaming) = session.level_streaming.lightmap_mut()
                    {
                        streaming.set_slider_levers(after);
                    }
                    if let Some(after) = reach
                        && reach_before != Some(after)
                        && let Some(stage) = session.level_streaming.cell_demand_mut()
                    {
                        stage.set_lead_metres(after.lead_metres);
                    }
                    debug_ui
                        .winit_state
                        .handle_platform_output(window, full_output.platform_output);
                    let paint_jobs = debug_ui
                        .ctx
                        .tessellate(full_output.shapes, full_output.pixels_per_point);
                    debug_ui.pending_textures.push(full_output.textures_delta);
                    out = Some((paint_jobs, window.scale_factor() as f32));
                }
            }
            // Clear the debug-line buffer unconditionally each
            // frame so any producer starts fresh. This is the
            // single lifecycle owner of the buffer: it handles
            // early-returns in `render_frame_indirect`
            // (Timeout/Occluded/Outdated) and level unloads
            // cleanly, and keeps any future debug-line producer
            // from colliding with the SH diagnostic pass.
            renderer.clear_debug_lines();
            // Emit SH diagnostic debug lines now — after UI
            // mutated state, before `render_frame_indirect`
            // draws the debug-line pass.
            if let Some(world) = app.level.as_ref() {
                if let Some(debug_ui) = session.debug_ui.as_ref() {
                    renderer.emit_sh_diagnostics(
                        &debug_ui.sh_diagnostics_state,
                        render_eye_position,
                        world,
                        &light_reachable_cell_mask,
                    );
                }
                let bvh_visible_cell_mask =
                    drawable_visible_cell_mask(world.cell_count(), &visible_cells);
                renderer.emit_bvh_overlay_diagnostics(bvh_visible_cell_mask.as_deref());
                renderer.emit_cell_overlay_diagnostics(world, &visible_cells);
                renderer.emit_portal_overlay_diagnostics(world);
                if session
                    .debug_ui
                    .as_ref()
                    .is_some_and(|debug_ui| debug_ui.is_visible())
                {
                    door_occluder_diagnostics::emit_blocked_portal_geometry(
                        renderer,
                        world,
                        &app.blocked_portals,
                    );
                }
            }
            // Navmesh overlay: append region rectangles + portal
            // edges. No-op unless the `Alt+Shift+N` toggle is on
            // and the map carried a baked navmesh.
            if let Some(nav_graph) = app.nav_graph.as_ref() {
                render::nav_diagnostics::emit(renderer, nav_graph);
            }
            // Rotating-mover spin axes and orientation. The app owns the
            // registry read and line geometry; renderer only consumes the
            // established debug-line primitive.
            if session
                .debug_ui
                .as_ref()
                .is_some_and(|debug_ui| debug_ui.is_visible())
            {
                let registry = script_ctx.registry.borrow();
                mover_diagnostics::emit(renderer, &registry);
            }
            // All-agent path/velocity/destination overlay. The
            // registry was read once before egui; this emit pass
            // emits from owned plain geometry through renderer
            // debug-line surfaces.
            agent_diagnostics::emit_agent_overlay_geometry(renderer, &agent_overlay_geometry);
            // Replicated-entity fallback wireframe: on a host or client,
            // draw capsules only for replicated entities that still lack
            // mesh presentation. Thin delegation — `netcode` collects
            // centers (registry read, no wgpu); the renderer owns the draw.
            // No-op in single-player and once every replicated entity here
            // has materialized its descriptor mesh.
            if let Some(endpoint) = session.net_endpoint.as_ref() {
                let registry = script_ctx.registry.borrow();
                let centers = netcode::remote_entity_positions(endpoint, &registry);
                renderer.emit_remote_entity_markers(
                    &centers,
                    netcode::REMOTE_CAPSULE_RADIUS,
                    netcode::REMOTE_CAPSULE_HALF_HEIGHT,
                );
            }
            out
        };

        // Publish the once-per-frame read snapshot just before
        // the gameplay render call, mirroring the splash path so
        // the once-per-frame contract holds on both. Game logic and
        // audio have already run this frame, so the slot snapshot
        // freezes the settled store state (frame order: Input →
        // Game logic → Audio → Render). The renderer reads these
        // cloned values, never the live `SlotTable`.
        //
        // Modal stack compose stays behind one helper so normal
        // gameplay gets always-on HUD/base layers, while an active
        // frontend stack suppresses those layers and presents only
        // its root menu and pushed submenu over the optional backdrop.
        let frontend_menu_name = session
            .frontend
            .as_ref()
            .map(|frontend| frontend.menu_tree.as_str())
            .unwrap_or(postretro_ui::demo::FRONTEND_MENU_NAME);
        // Reuse the `session` borrow taken at the top of this render
        // block (the `particle_collections` borrow keeps it alive); a
        // second `app.session.as_mut()` here would alias it.
        let frontend_menu_is_present =
            frontend_root_is_pushed(&session.modal_stack, frontend_menu_name);
        let mut ui_snapshot = App::build_ui_read_snapshot(
            &session.modal_stack,
            &mut session.presentation_cells,
            &script_ctx.slot_table.borrow(),
            app.script_time,
            session.ui_input_mode,
            app.ui_focused_id.clone(),
            frontend_menu_is_present,
        );
        ui_snapshot.wheel = app.ui_wheel.take();
        crate::app::glyph_art::resolve_snapshot_glyphs(&mut ui_snapshot, session);
        renderer.set_ui_snapshot(ui_snapshot);
        let limiter_frame = App::next_limiter_frame(&mut app.last_resolve_at, now);
        renderer.set_limiter_frame(limiter_frame);

        drop(stage_scope.take());
        let render_scope = cpu_stages.scope(cpu_timing::FrameStage::Render);
        let sh_frame_result = match renderer.render_frame_indirect(
            &mut session.font_system,
            CameraCullVisibility {
                cells: &visible_cells,
                path: stats.path,
            },
            &light_reachable_cell_mask,
            &reachable_cell_aabbs,
            &fog_reachable,
            render::ShSampleRegionSets {
                visible_cells: &visible_cells,
                fog_cells: &fog_reachable,
                movers: app.kinematic_mover_render.sh_sample_regions(),
            },
            Some(stats.camera_cell),
            view_proj,
            &particle_collections,
            app.script_time,
            render::ClearColor {
                r: 0.05,
                g: 0.05,
                b: 0.08,
                a: 1.0,
            },
            render::FrameScene::World,
            sh_drain_batch,
        ) {
            Ok(result) => result,
            Err(err) => {
                app.exit_result = Err(err.into());
                event_loop.exit();
                return;
            }
        };
        // The surface request is where a vsync block lands: wait, not render.
        if let Some(acquire) = sh_frame_result.acquire_nanos {
            app.cpu_timer.add_wait_within(
                cpu_timing::FrameStage::Render,
                cpu_timing::WaitSource::Acquire,
                acquire,
            );
        }
        let compose_submitted = sh_frame_result.compose_submitted;
        if let Err(err) = session.apply_sh_streaming_outcome(sh_frame_result.outcome, renderer) {
            app.exit_result = Err(err);
            event_loop.exit();
            return;
        }
        let present_handle = match sh_frame_result.frame {
            Ok(present_handle) => present_handle,
            Err(err) => {
                app.exit_result = Err(err);
                event_loop.exit();
                return;
            }
        };
        session.mark_sh_streaming_compose_submitted(compose_submitted);
        // Read back the focus rect list the renderer just exported
        // for the top stack layer (the gameplay render above laid it
        // out). The focus engine consumes it next frame's game-logic
        // phase — the reverse N→N+1 the focus ring's one-frame trail
        // comes from. See: context/lib/ui.md §4.
        let exported_rects = renderer.export_ui_focus_rects();
        if let Some(session) = app.session.as_mut() {
            session.ui_focus_rects = Some(exported_rects);
        }
        if let Some(present_handle) = present_handle {
            #[cfg(feature = "dev-tools")]
            let mut present_handle = present_handle;

            #[cfg(feature = "dev-tools")]
            {
                if let Some((paint_jobs, scale)) = debug_ui_frame
                    && let Some(debug_ui) = app.session.as_mut().and_then(|s| s.debug_ui.as_mut())
                    && let Err(err) = renderer.render_debug_ui(
                        &mut present_handle,
                        debug_ui.pending_textures.delta_mut(),
                        paint_jobs,
                        scale,
                    )
                {
                    app.exit_result = Err(err);
                    event_loop.exit();
                    return;
                }
            }
            let present_start = app.cpu_timer.gate().is_enabled().then(Instant::now);
            renderer.present(present_handle);
            if let Some(start) = present_start {
                let nanos = u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX);
                app.cpu_timer.add_wait_within(
                    cpu_timing::FrameStage::Render,
                    cpu_timing::WaitSource::Present,
                    nanos,
                );
            }
            if app.pending_level_log {
                // Reveal frame just presented — close out
                // log line C with the present-cost of the frame
                // the user is about to see.
                app.level_timings.record("first_level_frame");
                log::info!("{}", app.level_timings.summary());
                app.pending_level_log = false;
            }
        } else {
            // No surface this frame: nothing presented, nothing counted.
            app.cpu_timer.exclude_frame();
        }
        drop(render_scope);
        app.cpu_timer.nested_mut().extend_from(
            renderer.cpu_stages(),
            Some(postretro_stage_timing::StageSet::label(
                cpu_timing::FrameStage::Render,
            )),
        );
    }

    drop(stage_scope);
    let stage_scope = cpu_stages.scope(cpu_timing::FrameStage::FrameTail);
    app.poll_staged_manifest_results();

    if let VisibleCells::Culled(mut cells) = visible_cells {
        cells.clear();
        app.scratch_cells = cells;
    }

    let pos = render_eye_position;
    let region_label = "cell";
    let path_label = match stats.path {
        VisibilityPath::PrlPortal { .. } => "prl-portal",
        VisibilityPath::NoPortalsFallback => "no-portals",
        VisibilityPath::EmptyWorldFallback => "empty",
        VisibilityPath::SolidCellFallback => "solid-cell",
        VisibilityPath::ExteriorCellFallback => "exterior",
        VisibilityPath::PortalStepLimitFallback { .. } => "portal-step-limit",
    };
    let walk_reach_col = match stats.walk_reach() {
        Some(walk) => format!(" walk:{walk}"),
        None => String::new(),
    };
    log::debug!(
        "[Diagnostics] {region_label}:{} path:{path_label} | draw:{} all:{}{walk_reach_col} | pos: ({:.0}, {:.0}, {:.0})",
        stats.camera_cell,
        stats.drawn_faces,
        stats.total_faces,
        pos.x,
        pos.y,
        pos.z,
    );

    // `vsync:` label always present (not toggled) so it's grep-able
    // and the diagnostic toggle's effect is immediately visible.
    let vsync_label = app
        .renderer
        .as_ref()
        .map(|r| if r.vsync_enabled() { "on" } else { "off" });
    if let Some(ws) = app.window_state.as_ref()
        && app.last_title_update.elapsed() >= Duration::from_millis(250)
    {
        app.last_title_update = Instant::now();
        app.title_buffer.clear();
        let _ = write!(
            &mut app.title_buffer,
            "Postretro | {region_label}:{} path:{path_label} | draw:{} all:{}{walk_reach_col} | pos: ({:.0}, {:.0}, {:.0})",
            stats.camera_cell, stats.drawn_faces, stats.total_faces, pos.x, pos.y, pos.z,
        );
        if let Some(label) = vsync_label {
            let _ = write!(&mut app.title_buffer, " | vsync:{label}");
        }
        if let Some(ft) = app.frame_rate_meter.stats() {
            let _ = write!(
                &mut app.title_buffer,
                " frame: {:.1}/{:.1}/{:.1} ms",
                ft.min_ms, ft.avg_ms, ft.max_ms,
            );
        }
        ws.window.set_title(&app.title_buffer);
    }

    // Measure from `now` at handler entry so the sample spans all
    // CPU work. Wall-clock tick-to-tick is useless under vsync
    // (pinned to ~16.6ms); this shows actual load.
    let frame_cpu = Instant::now().duration_since(now);
    app.frame_rate_meter.record(frame_cpu);
    drop(stage_scope);
    drop(cpu_stages);
    app.cpu_timer.finish_frame(Instant::now());
    postretro_stage_timing::mark_frame();
}

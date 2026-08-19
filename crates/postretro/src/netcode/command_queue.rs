// Host-side authoritative command queues and the deterministic input-gap policy.
// Per-client queues hold sanitized inbound full remote commands keyed by client
// id; the per-pawn resolved cursor (`last_processed_client_tick`) drives a
// hold-then-neutral gap policy so a missing command tick never stalls locomotion.
//
// Bounded playout buffer with a standing playout floor. The resolved cursor consumes
// one command per 60 Hz tick, the same rate the client produces them. Two mechanisms
// keep playout smooth:
//
// (1) Standing playout floor (buildup latch). The two 60 Hz clocks free-run ~1 tick
// out of phase, so on a clean link the awaited command is usually not-yet-arrived when
// its tick resolves. Rather than neutral-fill and advance past it (which drop-stales
// the real command when it lands), the resolver holds the cursor on a genuine
// late-arrival — no advance, no drop_stale — until the command lands within the hold
// grace, and it withholds the FIRST real command at stream start / after an underrun
// until `pending` has built to INPUT_BUFFER_TARGET. Steady state then trails the newest
// received tick by ~INPUT_BUFFER_TARGET - 1 ticks, absorbing the phase offset.
//
// (2) Depth-keyed catch-up. Drain-rate == produce-rate, so a deep backlog would become
// PERMANENT latency. Two backlogs matter: (a) the client streams input at 60 Hz on
// connect, but the host can't drain a pawn until `owners.set()` runs at the end of
// accept+spawn — so a handshake/spawn-window backlog (tens of ticks ≈ hundreds of ms)
// accumulates; (b) a mid-session host frame hitch stalls the drain while commands keep
// arriving. When `pending` depth exceeds INPUT_BUFFER_MAX, fast-forward — drop all but
// the newest INPUT_BUFFER_TARGET commands and reseat the cursor on the new oldest, so
// playout converges to the small bounded buffer and stays there.
//
// Why depth-keyed (number of buffered commands), NOT tick-distance to the newest: both
// the catch-up trigger and the buildup latch key on `pending.len()`. A continuous-stream
// backlog holds MANY commands queued ahead (catch up), but a client that went silent then
// RESUMED at a far-future tick holds exactly ONE command far ahead (must NOT catch up, and
// must NOT read as "buffer full" — the hold→neutral→real resume path must stay intact).
// Tick-distance can't tell those apart; pending depth can.
// See: context/lib/networking.md
//
// Boundary: this is engine-side game logic, not the net crate. The net crate is
// registry-blind and only moves typed messages; intake/selection/gap policy live
// here because they bridge the client-id keyed wire stream to the per-pawn movement
// seam (`sim::host_movement`). Intake runs `wire_convert::sanitize_input_command`
// before queueing — an invalid command never mutates a queue.

use std::collections::{HashMap, HashSet, VecDeque};

use postretro_net::wire::InputCommand;

use crate::netcode::netdiag::{HostQueueDiag, QueueEvent};
use crate::netcode::prediction::client_tick_le;
use crate::netcode::wire_convert::{input_command_to_sim, sanitize_input_command};
use crate::sim::SimCommand;
use postretro_entities::components::inventory::Inventory;
use postretro_entities::{EntityId, EntityRegistry};

/// Host-side movement-authority owner map: `EntityId -> owning client id`. The
/// engine-side metadata snapshot production stamps onto each owned pawn's
/// `EntitySnapshot.owner_client_id`. Kept here (engine side) — the net crate never
/// sees an `EntityId`. Owned by the `Host` endpoint alongside the command queues.
#[derive(Debug, Default)]
pub(crate) struct MovementOwners {
    owners: HashMap<EntityId, u64>,
}

impl MovementOwners {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Record `client_id` as the movement-authority owner of `pawn`.
    pub(crate) fn set(&mut self, pawn: EntityId, client_id: u64) {
        self.owners.insert(pawn, client_id);
    }

    /// The owning client of `pawn`, if any.
    pub(crate) fn owner_of(&self, pawn: EntityId) -> Option<u64> {
        self.owners.get(&pawn).copied()
    }

    /// Forget a pawn's ownership (on slot close / despawn). Idempotent.
    pub(crate) fn remove_pawn(&mut self, pawn: EntityId) {
        self.owners.remove(&pawn);
    }

    /// Iterate `(pawn, client_id)` owner pairs. Used by snapshot production to stamp
    /// authority metadata onto each owned pawn.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (EntityId, u64)> + '_ {
        self.owners.iter().map(|(&id, &cid)| (id, cid))
    }
}

/// Explicitly-marked work queue for third-person weapon attachment updates.
///
/// Active wieldable identity is pawn-owned state. This tracker deliberately keeps
/// only the presentation dirties; draining it resolves the current active instance
/// from the pawn's [`Inventory`] rather than retaining a second pawn-to-weapon map.
#[derive(Debug, Default)]
pub(crate) struct WeaponOwners {
    attachment_dirty: HashSet<EntityId>,
}

impl WeaponOwners {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Mark `pawn` for an attachment refresh. Call this after inventory
    /// materialization, repoint, and pawn removal; the queue intentionally does no
    /// implicit change detection because the inventory is the sole active source.
    pub(crate) fn mark_attachment_dirty(&mut self, pawn: EntityId) {
        self.attachment_dirty.insert(pawn);
    }

    /// Mark a pawn's attachment as removed. The pawn may already be gone by the
    /// time the queue drains, which resolves naturally to no active wieldable.
    pub(crate) fn remove_pawn(&mut self, pawn: EntityId) {
        self.mark_attachment_dirty(pawn);
    }

    /// Drain pawn attachment changes, resolving each active wieldable at the moment
    /// of consumption from live pawn inventory.
    pub(crate) fn take_attachment_changes(
        &mut self,
        registry: &EntityRegistry,
    ) -> Vec<(EntityId, Option<EntityId>)> {
        let dirty: Vec<EntityId> = self.attachment_dirty.drain().collect();
        dirty
            .into_iter()
            .map(|pawn| (pawn, active_wieldable_for_pawn(registry, pawn)))
            .collect()
    }

    pub(crate) fn has_attachment_changes(&self) -> bool {
        !self.attachment_dirty.is_empty()
    }
}

/// Resolve the active inventory instance for a live pawn. This is the one shared
/// lookup for fire, replication, HUD projections, and presentation plumbing.
pub(crate) fn active_wieldable_for_pawn(
    registry: &EntityRegistry,
    pawn: EntityId,
) -> Option<EntityId> {
    registry
        .get_component::<Inventory>(pawn)
        .ok()
        .and_then(Inventory::active_wieldable)
}

/// Hold the last resolved command for at most this many missing ticks before
/// synthesizing neutral input. Deterministic gap policy (Task 4 §C): a short hold
/// rides out a single dropped/late packet; a longer gap falls back to neutral so a
/// disconnected-but-not-yet-closed client cannot keep its pawn coasting on stale
/// intent.
pub(crate) const INPUT_HOLD_TICKS: u32 = 3;

/// Standing playout depth: both the buildup latch's *disarm depth* and the pending
/// depth a catch-up fast-forward trims back to. ~2 ticks ≈ 33 ms at 60 Hz. The buildup
/// latch withholds the first real command until `pending` first reaches this depth;
/// after the first consume drops one command, the resolved cursor trails the newest
/// received tick by ~`INPUT_BUFFER_TARGET - 1` ticks (≈ 1 tick / 16 ms) — the standing
/// margin that absorbs the sub-tick phase offset between the client's send clock and the
/// host's resolve clock. It complements [`INPUT_HOLD_TICKS`]: the buffer rides out jitter
/// on the way in, the hold rides it out on the way out. Kept well below
/// [`INPUT_BUFFER_MAX`] so catch-up restores real headroom, and strictly below
/// [`INPUT_HOLD_TICKS`] (the standing invariant `INPUT_BUFFER_TARGET < INPUT_HOLD_TICKS`)
/// so a normal buildup completes before the hold grace can give up on it.
pub(crate) const INPUT_BUFFER_TARGET: usize = 2;

/// Catch-up trigger: the pending depth above which `resolve_tick` fast-forwards,
/// trimming the buffer back to [`INPUT_BUFFER_TARGET`]. ~8 ticks ≈ 133 ms at 60 Hz.
/// Two constraints pin it:
/// - It MUST exceed the largest in-order burst legitimate usage/tests reach, so a
///   normal small-gap regime never trips catch-up. The hottest existing tests ingest
///   4 (`ordered_input_resolves_each_tick_real`) and 3
///   (`stale_command_at_or_below_cursor_is_dropped`) commands before resolving; 8 is
///   strictly greater, so they resolve exactly as before.
/// - It MUST exceed [`INPUT_BUFFER_TARGET`] (hysteresis) so a fast-forward leaves the
///   buffer comfortably below the trigger and catch-up does not thrash tick-to-tick.
pub(crate) const INPUT_BUFFER_MAX: usize = 8;

/// One client's resolved-command state on the host: its pending inbound queue and
/// the gap-policy cursor. Keyed in [`HostCommandQueues`] by client id.
#[derive(Debug, Default)]
struct ClientCommandState {
    /// Pending sanitized commands, kept sorted-ascending and deduplicated by
    /// `client_tick`. Normally small (steady state holds ~[`INPUT_BUFFER_TARGET`]
    /// commands; `resolve_tick`'s catch-up bounds it back down whenever a handshake or
    /// hitch backlog pushes it past [`INPUT_BUFFER_MAX`]), so a `Vec` with binary-search
    /// insert beats a heap's overhead and keeps stale-drop / duplicate-collapse trivial
    /// to reason about. The fast-forward drains the stale prefix in one `drain` call.
    pending: Vec<InputCommand>,
    /// The latest client command tick this pawn has *resolved* (consumed a real
    /// command for, held the previous through, or synthesized neutral for). `None`
    /// until the first command resolves. A later real command at or below this is
    /// stale and dropped at intake.
    resolved_cursor: Option<u32>,
    /// The last command actually resolved (real or held). Held for up to
    /// [`INPUT_HOLD_TICKS`] consecutive missing ticks before neutral takes over.
    /// `None` before the first command and after a hold lapses to neutral.
    last_resolved: Option<InputCommand>,
    /// Latest finite aim pitch accepted by authoritative command playout. Movement,
    /// fire, and use neutralize after a long outage, but torso aim is presentation
    /// state and remains at the last valid orientation until a newer real command.
    latest_aim_pitch: Option<f32>,
    /// Latest finite facing yaw accepted by authoritative command playout. Long
    /// outages neutralize gameplay intent without snapping the replicated body aim.
    latest_facing_yaw: Option<f32>,
    /// Consecutive ticks the previous command has been held across a gap. Reset to 0
    /// whenever a real command resolves; once it reaches [`INPUT_HOLD_TICKS`] the gap
    /// policy synthesizes neutral input.
    held_ticks: u32,
    /// Reload is a level bit at the wire/sim boundary but an edge-triggered action at
    /// the weapon. Record rising edges from the reliable-ordered receive stream before
    /// stale-drop or catch-up trimming so a delayed tap survives command recovery.
    pending_reload_presses: VecDeque<u32>,
    /// Newest command observed at intake and its reload level. Only strictly newer
    /// ticks advance this pair, so duplicate/stale retransmits cannot mint another edge.
    latest_observed_reload: Option<(u32, bool)>,
    /// Reload level emitted on the previous authoritative resolution. A recovered
    /// press waits behind one false tick when necessary so the weapon's level-to-edge
    /// dedup sees a genuine rising edge.
    last_emitted_reload: bool,
    /// One-shot buildup latch for the standing playout floor. Armed at stream begin
    /// (`resolved_cursor == None`) and re-armed by a give-up that empties `pending`;
    /// disarmed the instant `pending.len()` first reaches [`INPUT_BUFFER_TARGET`].
    /// While armed, `resolve_tick` withholds the first real command — holding without
    /// consuming or advancing — until the queue has built to the disarm depth, so the
    /// resolved cursor establishes a small playout margin behind the newest received
    /// tick proactively (not only reactively via catch-up). Depth-keyed on
    /// `pending.len()` alone, never tick-distance. `Default` is disarmed.
    building_playout: bool,
}

impl ClientCommandState {
    fn observe_reload_level(&mut self, cmd: &InputCommand) {
        if self
            .latest_observed_reload
            .is_some_and(|(tick, _)| client_tick_le(cmd.client_tick, tick))
        {
            return;
        }

        let previous_level = self
            .latest_observed_reload
            .map(|(_, reload)| reload)
            .unwrap_or(false);
        if cmd.reload && !previous_level {
            self.pending_reload_presses.push_back(cmd.client_tick);
        }
        self.latest_observed_reload = Some((cmd.client_tick, cmd.reload));
    }

    fn preserve_due_reload_press(&mut self, resolved_tick: u32, command: &mut SimCommand) {
        let press_due = self
            .pending_reload_presses
            .front()
            .is_some_and(|tick| client_tick_le(*tick, resolved_tick));
        if press_due {
            if self.last_emitted_reload {
                // The false tick clears `WeaponComponent::reload_press_consumed`; keep
                // the press queued for the next authoritative resolution.
                command.reload = false;
            } else {
                command.reload = true;
                self.pending_reload_presses.pop_front();
            }
        }
        self.last_emitted_reload = command.reload;
    }

    /// Insert a sanitized command into the pending queue with stale-drop and
    /// exact-duplicate collapse. Returns `true` if the command was queued, `false`
    /// if it was dropped (stale or duplicate). Invalid commands never reach here —
    /// sanitization happens at the [`HostCommandQueues::ingest`] boundary.
    fn enqueue(&mut self, cmd: InputCommand) -> bool {
        // Stale: a command at or below the resolved cursor describes a tick the host
        // already settled authoritatively. Drop it. Wrap-aware `<=` (serial-number
        // arithmetic) so the comparison stays correct across the u32 client_tick wrap
        // — the allocator advances with `wrapping_add`, so a plain `<=` would freeze
        // the pawn to neutral for the half-range straddling u32::MAX.
        if let Some(cursor) = self.resolved_cursor
            && client_tick_le(cmd.client_tick, cursor)
        {
            return false;
        }
        match self
            .pending
            .binary_search_by_key(&cmd.client_tick, |c| c.client_tick)
        {
            // Exact duplicate tick already queued: collapse to one. The first arrival
            // wins; a duplicate is a retransmit of the same logical command.
            Ok(_) => false,
            Err(idx) => {
                self.pending.insert(idx, cmd);
                true
            }
        }
    }

    /// Pop the queued command for exactly `tick`, if present. The queue is sorted
    /// ascending, so the target — when present — is at the front once stale entries
    /// below it are gone; but a reordered arrival can leave a lower tick ahead, so
    /// search by key.
    fn take_exact(&mut self, tick: u32) -> Option<InputCommand> {
        let idx = self
            .pending
            .binary_search_by_key(&tick, |c| c.client_tick)
            .ok()?;
        Some(self.pending.remove(idx))
    }

    /// Drop every queued command at or below `cursor` — they are stale once the
    /// cursor advances past them (e.g. after a hold/neutral resolves a tick that a
    /// late real command targeted). Wrap-aware (serial-number arithmetic), matching
    /// the [`enqueue`](Self::enqueue) stale-check so both agree across the u32 wrap.
    fn drop_stale(&mut self, cursor: u32) {
        self.pending
            .retain(|c| !client_tick_le(c.client_tick, cursor));
    }
}

/// The host's per-client authoritative command queues. Owned by the `Host`
/// endpoint variant. Intake sanitizes and queues; the movement stage resolves one
/// command per pawn per fixed tick through the deterministic gap policy.
#[derive(Debug, Default)]
pub(crate) struct HostCommandQueues {
    clients: HashMap<u64, ClientCommandState>,
    /// Off-by-default per-client resolution/jump diagnostics (see `netdiag`). Reset
    /// with the queue on level unload; inert unless `postretro::netdiag=debug`.
    diag: HostQueueDiag,
}

/// What the gap policy resolved for one pawn this fixed tick: the command to apply
/// and whether it was a real client command (vs. a held repeat or synthesized
/// neutral). The resolved command always advances the pawn's `last_processed_client_tick`.
#[derive(Debug, Clone)]
pub(crate) struct ResolvedCommand {
    /// The sim command to feed the movement seam this tick.
    pub(crate) command: SimCommand,
    /// The client tick this resolution advances the cursor to. Read by this module's
    /// tests and the Task 5/6 reconciliation/harness consumers; staged dead-code-
    /// allowed (like the Task 2 helpers) until a non-test caller reads it.
    #[allow(dead_code)]
    pub(crate) client_tick: u32,
    /// How the command was resolved (real / held / neutral) — diagnostic and
    /// test-observable; the movement seam treats all three identically. Staged for
    /// the Task 6 harness's stale/duplicate assertions.
    #[allow(dead_code)]
    pub(crate) source: ResolutionSource,
}

/// A resolved host command bound to one remote-owned pawn for this fixed tick.
/// Movement consumes `command.movement`; host FIRE/reload consumes the same command's
/// weapon intent later in the sim weapon stage.
#[derive(Debug, Clone)]
pub(crate) struct ResolvedPawnCommand {
    pub(crate) pawn: EntityId,
    pub(crate) client_id: u64,
    pub(crate) command: SimCommand,
    /// Camera pitch from the resolved, host-authorized input command. Presentation
    /// consumes it locally; snapshot production reads the same queue state.
    pub(crate) aim_pitch: f32,
    pub(crate) client_tick: u32,
    #[allow(dead_code)]
    pub(crate) source: ResolutionSource,
}

/// How a fixed tick's command was resolved by the gap policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResolutionSource {
    /// A real queued command for the expected tick.
    Real,
    /// The previous command, held across a missing tick (within [`INPUT_HOLD_TICKS`]).
    Held,
    /// Synthesized neutral input after the hold lapsed.
    Neutral,
}

impl HostCommandQueues {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Ingest one raw inbound `InputCommand` for `client_id`: sanitize it (Task 2),
    /// then queue with stale-drop and duplicate-collapse. Returns `true` if the
    /// command was sanitized AND queued; `false` if it was rejected (non-finite),
    /// stale, or a duplicate. Invalid input mutates no state. A strictly newer stale
    /// command may still contribute a reload rising edge to the recovery lane; its
    /// movement, look, and fire fields remain dropped.
    pub(crate) fn ingest(&mut self, client_id: u64, raw: &InputCommand) -> bool {
        let Some(sanitized) = sanitize_input_command(raw) else {
            // Non-finite: never touch any queue or cursor. The client's state is not
            // even created on a rejected first command.
            return false;
        };
        let state = self.clients.entry(client_id).or_default();
        state.observe_reload_level(&sanitized);
        state.enqueue(sanitized)
    }

    /// Resolve exactly one command for `client_id`'s pawn this fixed tick, applying
    /// the deterministic gap policy, and advance the pawn's resolved cursor. Returns
    /// `None` only for a client that has never sent a command AND has no prior
    /// resolution — there is nothing to drive its pawn with yet (the pawn holds its
    /// authoritative pose). Once any command has resolved, this always returns a
    /// command (held or neutral) so the pawn advances deterministically.
    ///
    /// Cursor model: the host expects the tick immediately after the resolved cursor.
    /// If that exact tick is queued, consume it (`Real`). If it is missing, the cursor
    /// either HOLDS in place or ADVANCES, depending on the case:
    /// - **Hold (no advance):** a genuine late-arrival — a real command is being held
    ///   (`last_resolved.is_some()`) and the grace is not exhausted — holds the cursor
    ///   so the awaited tick, arriving within [`INPUT_HOLD_TICKS`], still resolves
    ///   `Real`. The one-shot buildup latch (`building_playout`) also holds, withholding
    ///   the first `Real` until `pending` reaches [`INPUT_BUFFER_TARGET`].
    /// - **Advance (+1):** the give-up after the hold grace (`Neutral`, so an absent
    ///   client cannot stall the host) and the post-give-up neutral-walk toward a
    ///   far-future resumed stream both synthesize `Neutral` and advance one tick.
    ///
    /// Held commands no longer advance the cursor; that establishes a standing playout
    /// depth trailing the newest received tick by ~[`INPUT_BUFFER_TARGET`] − 1 ticks.
    ///
    /// Bounded playout + catch-up: BEFORE picking the expected tick, if the pending
    /// queue has grown past [`INPUT_BUFFER_MAX`] real buffered commands, fast-forward —
    /// keep only the newest [`INPUT_BUFFER_TARGET`] and reseat the cursor on the new
    /// oldest. Because drain-rate == produce-rate (both 60 Hz), a backlog that builds
    /// during the accept/spawn handshake window (the client streams on connect before
    /// the host can drain) or a mid-session host hitch would otherwise become permanent
    /// latency; this single path drains it back to a small buffer and keeps it there. It
    /// is depth-keyed (count of buffered commands), NOT tick-distance to the newest, so a
    /// single far-future command after a silence does NOT trip it — the resume path stays
    /// intact.
    pub(crate) fn resolve_tick(&mut self, client_id: u64) -> Option<ResolvedCommand> {
        let state = self.clients.get_mut(&client_id)?;

        // Diagnostics-only: whether this resolution performed a catch-up trim and how
        // many trimmed commands carried a pressed jump. Pure observability.
        let mut diag_trims: u32 = 0;
        let mut diag_trimmed_jump: u32 = 0;

        // Catch-up fast-forward: a deep pending queue means real commands are stacking
        // up faster than the +1-per-tick cursor consumes them — a startup-handshake or
        // hitch backlog. Drop all but the newest INPUT_BUFFER_TARGET so the resolved
        // cursor never sits more than a small bounded buffer behind the newest received
        // command. Reload edges from the discarded prefix remain in their independent
        // recovery lane. Wrap-aware throughout: the new oldest's `client_tick - 1`
        // (serial arithmetic) is the cursor the normal exact-tick path then consumes as
        // `Real`.
        if state.pending.len() > INPUT_BUFFER_MAX {
            let drop_count = state.pending.len() - INPUT_BUFFER_TARGET;
            diag_trims = 1;
            diag_trimmed_jump = state.pending[0..drop_count]
                .iter()
                .filter(|c| c.movement.jump_pressed)
                .count() as u32;
            state.pending.drain(0..drop_count);
            // `pending` is non-empty here (INPUT_BUFFER_TARGET >= 1), so `first()` holds.
            let new_first = state.pending[0].client_tick;
            state.resolved_cursor = Some(new_first.wrapping_sub(1));
            // The trajectory jumped; any held intent is stale. Reset the hold so the
            // upcoming exact-tick hit resolves cleanly as the new `Real` baseline.
            state.held_ticks = 0;
        }

        let expected = match state.resolved_cursor {
            // First resolution: the next tick we want is the oldest queued command's
            // tick (the client's command stream may not start at 0). With nothing
            // queued and nothing prior resolved, there is nothing to drive yet. This is
            // the stream-begin path: arm the one-shot buildup latch so the first real
            // command is withheld until a small playout depth accumulates.
            None => {
                let first = state.pending.first().map(|c| c.client_tick)?;
                state.building_playout = true;
                first
            }
            Some(cursor) => cursor.wrapping_add(1),
        };

        // Disarm the buildup latch the instant the pending queue first reaches the
        // target depth — depth-keyed on `pending.len()` ALONE (never tick-distance), so
        // a lone far-future command after a silence stays at depth 1 and keeps the latch
        // armed rather than reading as "buffer full".
        if state.building_playout && state.pending.len() >= INPUT_BUFFER_TARGET {
            state.building_playout = false;
        }

        // Exact-tick hit: a real command resolves this tick. Skipped while the buildup
        // latch is armed — evaluating the latch BEFORE `take_exact` is load-bearing:
        // `take_exact` removes the command, so a buildup check after it would pop the
        // awaited command and still resolve `Neutral`, and the buffer could never build.
        if !state.building_playout
            && let Some(cmd) = state.take_exact(expected)
        {
            let mut sim = input_command_to_sim(&cmd);
            state.latest_aim_pitch = Some(cmd.movement.aim_pitch);
            state.latest_facing_yaw = Some(cmd.movement.facing_yaw);
            state.last_resolved = Some(cmd);
            state.held_ticks = 0;
            state.resolved_cursor = Some(expected);
            state.drop_stale(expected);
            state.preserve_due_reload_press(expected, &mut sim);
            let diag_lead = state
                .latest_observed_reload
                .map(|(newest, _)| expected.wrapping_sub(newest) as i32);
            self.diag.record(QueueEvent {
                client_id,
                source: ResolutionSource::Real,
                lead: diag_lead,
                yaw: Some(sim.movement.facing_yaw),
                jump_pressed: sim.movement.jump_pressed,
                trims: diag_trims,
                trimmed_jump: diag_trimmed_jump,
            });
            return Some(ResolvedCommand {
                command: sim,
                client_tick: expected,
                source: ResolutionSource::Real,
            });
        }

        // Gap resolution. Two outcomes: HOLD the cursor in place (a genuine late-arrival
        // wait, or an armed buildup withhold) or ADVANCE it (+1) with synthesized neutral
        // (a give-up after the grace, or the post-give-up neutral-walk).
        let within_grace = state.held_ticks < INPUT_HOLD_TICKS;
        // Hold without advancing iff the grace is not exhausted AND either the buildup
        // latch is armed (withhold the first real command, regardless of `last_resolved`)
        // or a real command is being held across the gap (`last_resolved.is_some()`). A
        // disarmed gap with no command to hold is the neutral-walk — it must advance.
        let hold_without_advance =
            within_grace && (state.building_playout || state.last_resolved.is_some());

        if hold_without_advance {
            // Hold: leave `resolved_cursor` unchanged, do NOT `drop_stale`, do NOT
            // `take_exact`, and do NOT thread `preserve_due_reload_press` (a non-advancing
            // hold keeps `expected` constant, so a due reload press waits for an advancing
            // tick). `held_ticks` still increments so the grace stays bounded.
            state.held_ticks += 1;
            let (sim, source) = match &state.last_resolved {
                Some(prev) => (held_gap_sim_command(prev), ResolutionSource::Held),
                None => (
                    neutral_sim_command(state.latest_facing_yaw.unwrap_or(0.0)),
                    ResolutionSource::Neutral,
                ),
            };
            let diag_lead = state
                .latest_observed_reload
                .map(|(newest, _)| expected.wrapping_sub(newest) as i32);
            self.diag.record(QueueEvent {
                client_id,
                source,
                lead: diag_lead,
                yaw: Some(sim.movement.facing_yaw),
                jump_pressed: sim.movement.jump_pressed,
                trims: diag_trims,
                trimmed_jump: diag_trimmed_jump,
            });
            return Some(ResolvedCommand {
                command: sim,
                client_tick: expected,
                source,
            });
        }

        // Advancing gap resolution: give-up (grace exhausted while a command was held)
        // or neutral-walk (no command to hold — the coast toward a far-future resume).
        // Both synthesize neutral and advance the cursor one tick.
        if state.held_ticks >= INPUT_HOLD_TICKS {
            // Give-up: clear the held command so a later real command at a still-higher
            // tick resumes cleanly rather than re-holding stale intent.
            state.last_resolved = None;
        }
        let mut sim = neutral_sim_command(state.latest_facing_yaw.unwrap_or(0.0));
        state.held_ticks = 0;
        state.resolved_cursor = Some(expected);
        state.drop_stale(expected);
        // Give-up latch recompute: re-arm buildup iff the give-up emptied the buffer (a
        // fresh stream must build depth again); a give-up that leaves commands buffered
        // disarms so the neutral-walk advances toward them. For a neutral-walk, `pending`
        // still holds the command being walked toward, so this is a no-op (stays false).
        state.building_playout = state.pending.is_empty();
        state.preserve_due_reload_press(expected, &mut sim);
        let diag_lead = state
            .latest_observed_reload
            .map(|(newest, _)| expected.wrapping_sub(newest) as i32);
        self.diag.record(QueueEvent {
            client_id,
            source: ResolutionSource::Neutral,
            lead: diag_lead,
            yaw: Some(sim.movement.facing_yaw),
            jump_pressed: sim.movement.jump_pressed,
            trims: diag_trims,
            trimmed_jump: diag_trimmed_jump,
        });
        Some(ResolvedCommand {
            command: sim,
            client_tick: expected,
            source: ResolutionSource::Neutral,
        })
    }

    /// The pawn's resolved cursor (`last_processed_client_tick`) for snapshot
    /// authority metadata. `None` until the first command resolves.
    pub(crate) fn resolved_cursor(&self, client_id: u64) -> Option<u32> {
        self.clients.get(&client_id).and_then(|s| s.resolved_cursor)
    }

    /// Latest finite aim pitch accepted by authoritative command playout. Input
    /// neutralization does not reset presentation orientation during an outage.
    pub(crate) fn current_aim_pitch(&self, client_id: u64) -> Option<f32> {
        self.clients
            .get(&client_id)
            .and_then(|state| state.latest_aim_pitch)
    }

    /// Drop a client's queue + cursor on slot close. Idempotent.
    pub(crate) fn remove_client(&mut self, client_id: u64) {
        self.clients.remove(&client_id);
    }
}

/// Resolve one movement command per owned pawn for this fixed tick and build the
/// explicit `(EntityId, MovementInput)` list the host multi-pawn seam
/// (`sim::run_host_movement_tick`) consumes. Game-logic-owned selection: it routes
/// each owner's resolved command through the `EntityId -> client_id` map and applies
/// the deterministic gap policy per pawn. A pawn whose owner has never sent a command
/// (and has no prior resolution) is omitted — its authoritative pose holds. This is
/// the host's substitute for `local_movement_pawn`: every authoritative pawn is named
/// explicitly, including the listen host's own pawn (which the caller appends
/// separately with its locally-sampled input).
pub(crate) fn host_resolve_remote_commands(
    owners: &MovementOwners,
    command_queues: &mut HostCommandQueues,
) -> Vec<ResolvedPawnCommand> {
    let mut commands = Vec::new();
    // Snapshot the owner pairs first so the mutable queue borrow does not alias the
    // owners borrow.
    let owner_pairs: Vec<(EntityId, u64)> = owners.iter().collect();
    for (pawn, client_id) in owner_pairs {
        if let Some(resolved) = command_queues.resolve_tick(client_id) {
            let aim_pitch = command_queues.current_aim_pitch(client_id).unwrap_or(0.0);
            commands.push(ResolvedPawnCommand {
                pawn,
                client_id,
                command: resolved.command,
                aim_pitch,
                client_tick: resolved.client_tick,
                source: resolved.source,
            });
        }
    }
    commands
}

/// A neutral (no-intent) sim command: no wish direction or buttons, with the last
/// finite facing yaw retained for remote-avatar presentation.
fn neutral_sim_command(facing_yaw: f32) -> SimCommand {
    use crate::movement::MovementInput;
    use crate::weapon::FireButtonState;
    use glam::Vec2;
    SimCommand {
        movement: MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw,
            use_pressed: false,
            drop_pressed: false,
        },
        fire_button: FireButtonState {
            pressed: false,
            active: false,
        },
        reload: false,
        firing_slot: 0,
        select_slot: None,
        use_pressed: false,
        drop_pressed: false,
    }
}

/// Build a held-gap command from the previous resolved command, clearing FIRE and
/// one-tick use/drop edges but carrying movement and `reload` forward unchanged.
/// The two level fields diverge on purpose: `fire_button` authorizes cooldown and
/// ammo consumption whenever it resolves `active`, so a held command must not
/// re-authorize FIRE. `reload` is a level bit; weapon-owned `reload_press_consumed`
/// deduplicates it while held. Carrying that bit preserves reload intent across a
/// packet gap without synthesizing another press.
fn held_gap_sim_command(prev: &InputCommand) -> SimCommand {
    let mut sim = input_command_to_sim(prev);
    sim.fire_button = crate::weapon::FireButtonState {
        pressed: false,
        active: false,
    };
    sim.movement.use_pressed = false;
    sim.use_pressed = false;
    sim.movement.drop_pressed = false;
    sim.drop_pressed = false;
    sim
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_net::wire::{WireFireButtonState, WireMovementInput};

    const EPSILON: f32 = 1e-6;
    const CLIENT: u64 = 7;

    /// A forward-walking command at the given client tick. `wish` lets a test vary
    /// the intent so a held/neutral resolution is distinguishable from a real one.
    fn command(client_tick: u32, wish_forward: f32) -> InputCommand {
        InputCommand {
            client_tick,
            movement: WireMovementInput {
                wish_dir: [0.0, wish_forward],
                jump_pressed: false,
                dash_pressed: false,
                running: true,
                crouch_intent: false,
                facing_yaw: 0.5,
                use_pressed: false,
                drop_pressed: false,
                aim_pitch: 0.0,
                firing_slot: 0,
            },
            fire_button: WireFireButtonState {
                pressed: false,
                active: false,
            },
            reload: false,
        }
    }

    /// A reload-pressed command at `client_tick` (rising edge on the wire level bit).
    fn reload_command(client_tick: u32, wish_forward: f32) -> InputCommand {
        let mut cmd = command(client_tick, wish_forward);
        cmd.reload = true;
        cmd
    }

    /// Drive the buildup latch to DISARMED steady state: ingest the two consecutive
    /// commands [`base`, `base + 1`] and resolve both `Real`, leaving `resolved_cursor`
    /// at `base + 1` with an empty pending queue, the latch disarmed, and a real command
    /// held. A single command can never disarm the one-shot latch (depth 1 <
    /// [`INPUT_BUFFER_TARGET`]), so priming a "first Real resolves" state needs two.
    fn prime_disarmed(queues: &mut HostCommandQueues, base: u32) {
        assert!(queues.ingest(CLIENT, &command(base, 1.0)));
        assert!(queues.ingest(CLIENT, &command(base + 1, 1.0)));
        let r0 = queues.resolve_tick(CLIENT).expect("primed real 0");
        assert_eq!(r0.source, ResolutionSource::Real);
        assert_eq!(r0.client_tick, base);
        let r1 = queues.resolve_tick(CLIENT).expect("primed real 1");
        assert_eq!(r1.source, ResolutionSource::Real);
        assert_eq!(r1.client_tick, base + 1);
    }

    // === Intake / sanitize (unchanged by the playout fix) ===

    // Intake sanitizes and queues a finite command; a non-finite command is rejected
    // and mutates no queue state (no client entry is even created).
    #[test]
    fn ingest_sanitizes_and_queues_finite_rejects_non_finite() {
        let mut queues = HostCommandQueues::new();
        assert!(queues.ingest(CLIENT, &command(0, 1.0)), "finite queued");

        let mut bad = command(1, 1.0);
        bad.movement.wish_dir[1] = f32::NAN;
        assert!(!queues.ingest(CLIENT, &bad), "non-finite rejected");

        // A different client whose only command was rejected has no state at all.
        const OTHER: u64 = 99;
        let mut bad2 = command(0, 1.0);
        bad2.movement.facing_yaw = f32::INFINITY;
        assert!(!queues.ingest(OTHER, &bad2));
        assert!(
            queues.resolved_cursor(OTHER).is_none(),
            "a rejected-only client created no queue/cursor state"
        );
    }

    // Out-of-range finite wish_dir is clamped by sanitize before queueing (the
    // sanitizer's contract); the queued+resolved command reflects the clamp. A second
    // command brings depth to INPUT_BUFFER_TARGET so the buildup latch disarms and the
    // first command resolves Real.
    #[test]
    fn ingest_clamps_out_of_range_wish_dir_before_queueing() {
        let mut queues = HostCommandQueues::new();
        let mut cmd = command(0, 5.0); // forward 5.0 -> clamp to 1.0
        cmd.movement.wish_dir[0] = -3.0; // right -3.0 -> clamp to -1.0
        assert!(queues.ingest(CLIENT, &cmd));
        assert!(queues.ingest(CLIENT, &command(1, 0.0)));
        let resolved = queues.resolve_tick(CLIENT).expect("a command resolves");
        assert_eq!(resolved.source, ResolutionSource::Real);
        assert!((resolved.command.movement.wish_dir.x - (-1.0)).abs() < EPSILON);
        assert!((resolved.command.movement.wish_dir.y - 1.0).abs() < EPSILON);
    }

    #[test]
    fn current_aim_pitch_tracks_the_last_resolved_input() {
        let mut queues = HostCommandQueues::new();
        let mut a = command(0, 1.0);
        a.movement.aim_pitch = -0.42;
        let mut b = command(1, 1.0);
        b.movement.aim_pitch = -0.42;
        assert!(queues.ingest(CLIENT, &a));
        assert!(queues.ingest(CLIENT, &b));
        assert_eq!(
            queues.resolve_tick(CLIENT).unwrap().source,
            ResolutionSource::Real
        );
        assert_eq!(
            queues.resolve_tick(CLIENT).unwrap().source,
            ResolutionSource::Real
        );
        assert_eq!(queues.current_aim_pitch(CLIENT), Some(-0.42));

        // Regression: an outage longer than INPUT_HOLD_TICKS neutralizes gameplay
        // intent but must not snap the remote torso back to zero pitch.
        for _ in 0..(INPUT_HOLD_TICKS + 3) {
            assert!(queues.resolve_tick(CLIENT).is_some());
        }
        assert_eq!(queues.current_aim_pitch(CLIENT), Some(-0.42));
        let neutral = queues.resolve_tick(CLIENT).expect("playout stays active");
        assert_eq!(neutral.source, ResolutionSource::Neutral);
        assert!(neutral.command.movement.wish_dir.length_squared() <= EPSILON);
        assert!(
            (neutral.command.movement.facing_yaw - 0.5).abs() <= EPSILON,
            "neutral gameplay input retains the last finite facing yaw"
        );
    }

    // An exact duplicate tick collapses to one queued command; a stale command at or
    // below the resolved cursor is dropped. Neither mutates unrelated state.
    #[test]
    fn ingest_collapses_duplicates_and_drops_stale() {
        let mut queues = HostCommandQueues::new();
        assert!(queues.ingest(CLIENT, &command(0, 1.0)));
        // Exact duplicate of tick 0: collapsed.
        assert!(!queues.ingest(CLIENT, &command(0, 0.5)));
        // A second command brings depth to INPUT_BUFFER_TARGET so tick 0 resolves Real.
        assert!(queues.ingest(CLIENT, &command(1, 1.0)));
        let r = queues.resolve_tick(CLIENT).unwrap();
        assert_eq!(r.client_tick, 0);
        assert_eq!(r.source, ResolutionSource::Real);
        // First-arrival wins the duplicate collapse: forward intent is 1.0, not 0.5.
        assert!((r.command.movement.wish_dir.y - 1.0).abs() < EPSILON);

        // A late command at the resolved cursor (0) is stale -> dropped.
        assert!(!queues.ingest(CLIENT, &command(0, 0.0)));
        assert_eq!(queues.resolved_cursor(CLIENT), Some(0));
    }

    // A real command at or below the resolved cursor is stale and dropped at intake —
    // it never resurrects an already-settled tick. Three commands pre-buffered
    // (depth >= INPUT_BUFFER_TARGET) so the latch disarms immediately.
    #[test]
    fn stale_command_at_or_below_cursor_is_dropped() {
        let mut queues = HostCommandQueues::new();
        for t in 0..3u32 {
            queues.ingest(CLIENT, &command(t, 1.0));
        }
        for _ in 0..3 {
            let _ = queues.resolve_tick(CLIENT);
        }
        assert_eq!(queues.resolved_cursor(CLIENT), Some(2));

        // A duplicate/old command for tick 1 (<= cursor 2) is dropped, not re-applied.
        assert!(!queues.ingest(CLIENT, &command(1, -1.0)));
        // And tick 2 (== cursor) is also stale.
        assert!(!queues.ingest(CLIENT, &command(2, -1.0)));
    }

    // A client that never sent a command resolves to None — its pawn holds its
    // authoritative pose, the gap policy never fabricates input out of nothing.
    #[test]
    fn no_commands_resolves_none() {
        let mut queues = HostCommandQueues::new();
        assert!(queues.resolve_tick(CLIENT).is_none());
        // Injecting then removing the client clears state cleanly.
        queues.ingest(CLIENT, &command(0, 1.0));
        queues.remove_client(CLIENT);
        assert!(queues.resolve_tick(CLIENT).is_none());
    }

    // Duplicate `ClientMessage::Input` injected at the drain/queue seam does not mutate
    // unrelated clients' state and does not panic. Both clients are primed to
    // INPUT_BUFFER_TARGET depth so their first commands resolve Real.
    #[test]
    fn duplicate_injection_does_not_disturb_other_clients() {
        let mut queues = HostCommandQueues::new();
        const A: u64 = 1;
        const B: u64 = 2;
        assert!(queues.ingest(A, &command(0, 1.0)));
        assert!(queues.ingest(A, &command(1, 1.0)));
        assert!(queues.ingest(B, &command(0, -1.0)));
        assert!(queues.ingest(B, &command(1, -1.0)));

        // Flood A with duplicates and stale commands.
        for _ in 0..10 {
            let _ = queues.ingest(A, &command(0, 0.0));
        }
        // B is untouched: its command resolves with its own intent.
        let rb = queues.resolve_tick(B).unwrap();
        assert_eq!(rb.source, ResolutionSource::Real);
        assert!((rb.command.movement.wish_dir.y - (-1.0)).abs() < EPSILON);
        // A still resolves its first-arrival command, not a duplicate's 0.0 intent.
        let ra = queues.resolve_tick(A).unwrap();
        assert_eq!(ra.source, ResolutionSource::Real);
        assert!((ra.command.movement.wish_dir.y - 1.0).abs() < EPSILON);
    }

    #[test]
    fn host_resolve_remote_commands_preserves_full_sim_command_per_pawn() {
        let mut queues = HostCommandQueues::new();
        let mut owners = MovementOwners::new();
        let pawn = EntityId::from_raw(2);
        owners.set(pawn, CLIENT);

        // Prime the client to a disarmed state so the full command resolves Real.
        prime_disarmed(&mut queues, 0);

        let mut cmd = command(2, 0.75);
        cmd.fire_button = WireFireButtonState {
            pressed: true,
            active: true,
        };
        cmd.reload = true;
        assert!(queues.ingest(CLIENT, &cmd));

        let resolved = host_resolve_remote_commands(&owners, &mut queues);

        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].pawn, pawn);
        assert_eq!(resolved[0].client_id, CLIENT);
        assert_eq!(resolved[0].client_tick, 2);
        assert_eq!(resolved[0].source, ResolutionSource::Real);
        assert!(resolved[0].command.fire_button.pressed);
        assert!(resolved[0].command.fire_button.active);
        assert!(resolved[0].command.reload);
        assert!((resolved[0].command.movement.wish_dir.y - 0.75).abs() < EPSILON);
    }

    #[test]
    fn held_gap_command_preserves_locomotion_but_clears_fire_authorization() {
        let mut queues = HostCommandQueues::new();
        prime_disarmed(&mut queues, 0); // cursor 1, disarmed
        let mut cmd = command(2, 1.0);
        cmd.fire_button = WireFireButtonState {
            pressed: true,
            active: true,
        };
        assert!(queues.ingest(CLIENT, &cmd));

        let real = queues.resolve_tick(CLIENT).expect("real command resolves");
        assert_eq!(real.source, ResolutionSource::Real);
        assert!(real.command.fire_button.active);

        // Tick 3 is missing: a hold (no advance) carries movement but not FIRE.
        let held = queues.resolve_tick(CLIENT).expect("held command resolves");
        assert_eq!(held.source, ResolutionSource::Held);
        assert!((held.command.movement.wish_dir.y - 1.0).abs() < EPSILON);
        assert!(
            !held.command.fire_button.pressed && !held.command.fire_button.active,
            "gap-filled movement hold must not synthesize remote FIRE"
        );
    }

    #[test]
    fn one_drop_press_crossing_a_packet_gap_resolves_exactly_once() {
        let mut queues = HostCommandQueues::new();
        prime_disarmed(&mut queues, 0); // cursor 1, disarmed
        let mut cmd = command(2, 1.0);
        cmd.movement.drop_pressed = true;
        assert!(queues.ingest(CLIENT, &cmd));

        let real = queues.resolve_tick(CLIENT).expect("real command resolves");
        assert!(real.command.drop_pressed);
        assert!(real.command.movement.drop_pressed);
        let mut resolved_drop_edges = usize::from(real.command.drop_pressed);

        // Holds (no advance) carry no drop edge...
        for _ in 0..INPUT_HOLD_TICKS {
            let held = queues.resolve_tick(CLIENT).expect("held command resolves");
            assert_eq!(held.source, ResolutionSource::Held);
            resolved_drop_edges += usize::from(held.command.drop_pressed);
        }
        // ...and neither does the give-up neutral.
        let neutral = queues
            .resolve_tick(CLIENT)
            .expect("neutral fallback resolves");
        assert_eq!(neutral.source, ResolutionSource::Neutral);
        assert!(!neutral.command.drop_pressed);
        resolved_drop_edges += usize::from(neutral.command.drop_pressed);
        assert_eq!(
            resolved_drop_edges, 1,
            "a held packet gap cannot replay the one-tick drop action"
        );
    }

    // === Orderings table: playout behavior (P-labeled rows) ===

    // Ordering "Command on time": a command buffered before its tick resolves is Real,
    // in disarmed steady state.
    #[test]
    fn command_on_time_resolves_real() {
        let mut queues = HostCommandQueues::new();
        prime_disarmed(&mut queues, 0); // cursor 1
        assert!(queues.ingest(CLIENT, &command(2, -1.0))); // buffered before tick 2 resolves
        let r = queues.resolve_tick(CLIENT).unwrap();
        assert_eq!(r.source, ResolutionSource::Real);
        assert_eq!(r.client_tick, 2);
        assert_eq!(queues.resolved_cursor(CLIENT), Some(2));
    }

    // Ordering "Command slightly late" (the bug): the cursor HOLDS without advancing on
    // an unfilled tick, and the command arriving within the grace resolves Real.
    // Regression: the host advanced the cursor past an unfilled tick and drop-staled the
    // client's on-time-but-slightly-late command, discarding ~75% of input on a clean link.
    #[test]
    fn slightly_late_command_holds_without_advancing_then_resolves_real() {
        let mut queues = HostCommandQueues::new();
        prime_disarmed(&mut queues, 0); // cursor 1, last_resolved Some

        // Tick 2's command has not arrived: HOLD (no advance), not neutral-fill-and-advance.
        let held = queues.resolve_tick(CLIENT).unwrap();
        assert_eq!(held.source, ResolutionSource::Held);
        assert_eq!(held.client_tick, 2);
        assert_eq!(
            queues.resolved_cursor(CLIENT),
            Some(1),
            "a held tick does not advance the cursor"
        );

        // It lands within the hold grace and resolves Real at its own tick, not stale.
        assert!(queues.ingest(CLIENT, &command(2, -1.0)));
        let real = queues.resolve_tick(CLIENT).unwrap();
        assert_eq!(real.source, ResolutionSource::Real);
        assert_eq!(real.client_tick, 2);
        assert!((real.command.movement.wish_dir.y - (-1.0)).abs() < EPSILON);
        assert_eq!(queues.resolved_cursor(CLIENT), Some(2));
    }

    // Ordering "Command never arrives": held for the whole grace (no advance), then a
    // single Neutral give-up that DOES advance past the absent tick.
    #[test]
    fn never_arriving_command_holds_then_neutral_give_up_advances() {
        let mut queues = HostCommandQueues::new();
        prime_disarmed(&mut queues, 0); // cursor 1, last_resolved Some (wish 1.0)

        for _ in 0..INPUT_HOLD_TICKS {
            let held = queues.resolve_tick(CLIENT).unwrap();
            assert_eq!(held.source, ResolutionSource::Held);
            assert_eq!(held.client_tick, 2, "the held tick keeps awaiting tick 2");
            assert!((held.command.movement.wish_dir.y - 1.0).abs() < EPSILON);
            assert_eq!(
                queues.resolved_cursor(CLIENT),
                Some(1),
                "no advance on hold"
            );
        }

        let neutral = queues.resolve_tick(CLIENT).unwrap();
        assert_eq!(neutral.source, ResolutionSource::Neutral);
        assert_eq!(neutral.client_tick, 2);
        assert!(neutral.command.movement.wish_dir.y.abs() < EPSILON);
        assert_eq!(
            queues.resolved_cursor(CLIENT),
            Some(2),
            "the give-up advances past the absent tick"
        );
    }

    // P1: after a give-up that empties pending and re-arms the latch, a far-future resume
    // holds until depth reaches INPUT_BUFFER_TARGET, then neutral-walks +1 per tick and
    // resolves Real — it must NOT freeze at the give-up cursor.
    // Regression: a naive "don't advance on any within-grace gap" freezes the resume.
    #[test]
    fn neutral_walk_after_give_up_advances_to_far_future_resume() {
        let mut queues = HostCommandQueues::new();
        prime_disarmed(&mut queues, 0); // cursor 1, last_resolved Some

        // Silence -> hold grace -> give-up (cursor 2, last_resolved None, pending empty
        // -> latch re-arms).
        for _ in 0..INPUT_HOLD_TICKS {
            assert_eq!(
                queues.resolve_tick(CLIENT).unwrap().source,
                ResolutionSource::Held
            );
        }
        let giveup = queues.resolve_tick(CLIENT).unwrap();
        assert_eq!(giveup.source, ResolutionSource::Neutral);
        assert_eq!(queues.resolved_cursor(CLIENT), Some(2));

        // A single far-future command keeps the re-armed latch armed (depth 1 < target).
        assert!(queues.ingest(CLIENT, &command(60, -1.0)));
        let h = queues.resolve_tick(CLIENT).unwrap();
        assert_eq!(h.source, ResolutionSource::Neutral);
        assert_eq!(
            queues.resolved_cursor(CLIENT),
            Some(2),
            "armed buildup holds the lone far command in place"
        );

        // A second far-future command brings depth to the target: the latch disarms and
        // the neutral-walk marches to the resumed Real at tick 60.
        assert!(queues.ingest(CLIENT, &command(61, -1.0)));
        let mut resolved_real = None;
        for _ in 0..70 {
            let r = queues.resolve_tick(CLIENT).unwrap();
            if r.source == ResolutionSource::Real {
                resolved_real = Some(r);
                break;
            }
        }
        let resolved = resolved_real.expect("neutral-walk reaches the resumed Real");
        assert_eq!(resolved.client_tick, 60);
        assert!((resolved.command.movement.wish_dir.y - (-1.0)).abs() < EPSILON);
    }

    // P1b: a give-up that leaves pending non-empty does NOT re-arm the latch; the
    // neutral-walk advances immediately (no buildup withhold) to the buffered Real.
    #[test]
    fn give_up_with_cushion_intact_does_not_re_arm_buildup() {
        let mut queues = HostCommandQueues::new();
        prime_disarmed(&mut queues, 0); // cursor 1, last_resolved Some

        // A far-future command is buffered early; the stream is DISARMED, so it does not
        // resolve until the cursor walks to it.
        assert!(queues.ingest(CLIENT, &command(60, -1.0)));

        // Ticks 2.. miss: hold the grace (awaiting tick 2), then give up.
        for _ in 0..INPUT_HOLD_TICKS {
            assert_eq!(
                queues.resolve_tick(CLIENT).unwrap().source,
                ResolutionSource::Held
            );
        }
        let giveup = queues.resolve_tick(CLIENT).unwrap();
        assert_eq!(giveup.source, ResolutionSource::Neutral);
        assert_eq!(queues.resolved_cursor(CLIENT), Some(2));

        // The latch did NOT re-arm (pending held [60]); no buildup hold — straight to the
        // neutral-walk and Real at 60.
        let mut resolved_real = None;
        for _ in 0..70 {
            let r = queues.resolve_tick(CLIENT).unwrap();
            assert_ne!(
                r.source,
                ResolutionSource::Held,
                "no buildup hold after a cushion-intact give-up"
            );
            if r.source == ResolutionSource::Real {
                resolved_real = Some(r);
                break;
            }
        }
        assert_eq!(
            resolved_real
                .expect("neutral-walk reaches Real")
                .client_tick,
            60
        );
    }

    // P2: a steady 1-in/1-out stream where every resolve sees pending depth 1 must still
    // resolve Real and advance — the one-shot latch, once disarmed, never re-fires.
    #[test]
    fn steady_low_water_stream_stays_real_and_never_re_arms() {
        let mut queues = HostCommandQueues::new();
        prime_disarmed(&mut queues, 0); // cursor 1, disarmed
        for t in 2..40u32 {
            assert!(queues.ingest(CLIENT, &command(t, 1.0)));
            let r = queues.resolve_tick(CLIENT).unwrap();
            assert_eq!(
                r.source,
                ResolutionSource::Real,
                "steady low-water resolve stays Real"
            );
            assert_eq!(r.client_tick, t);
            assert_eq!(queues.resolved_cursor(CLIENT), Some(t));
        }
    }

    // P3: a single packet delayed past the grace costs at most the hold grace — the
    // give-up leaves the later commands buffered, so the latch does not re-arm and the
    // next resolves are Real (no fresh buildup stall).
    #[test]
    fn one_late_packet_costs_only_the_grace_no_fresh_buildup() {
        let mut queues = HostCommandQueues::new();
        prime_disarmed(&mut queues, 0); // cursor 1, last_resolved Some

        // Tick 2 is delayed; ticks 3 and 4 arrive and buffer.
        assert!(queues.ingest(CLIENT, &command(3, 1.0)));
        assert!(queues.ingest(CLIENT, &command(4, 1.0)));

        for _ in 0..INPUT_HOLD_TICKS {
            assert_eq!(
                queues.resolve_tick(CLIENT).unwrap().source,
                ResolutionSource::Held
            );
        }
        let giveup = queues.resolve_tick(CLIENT).unwrap();
        assert_eq!(giveup.source, ResolutionSource::Neutral);
        assert_eq!(queues.resolved_cursor(CLIENT), Some(2));

        // The very next resolves are Real (tick 3 then 4) — no fresh buildup withhold.
        let r3 = queues.resolve_tick(CLIENT).unwrap();
        assert_eq!(r3.source, ResolutionSource::Real);
        assert_eq!(r3.client_tick, 3);
        let r4 = queues.resolve_tick(CLIENT).unwrap();
        assert_eq!(r4.source, ResolutionSource::Real);
        assert_eq!(r4.client_tick, 4);
    }

    // P4: at stream start the buildup latch withholds the first command WITHOUT consuming
    // it (no take_exact), keeping the cursor unset, until depth reaches INPUT_BUFFER_TARGET.
    #[test]
    fn buildup_at_stream_start_withholds_without_consuming() {
        let mut queues = HostCommandQueues::new();
        assert!(queues.ingest(CLIENT, &command(0, 1.0))); // depth 1 < INPUT_BUFFER_TARGET
        let n = queues.resolve_tick(CLIENT).unwrap();
        assert_eq!(n.source, ResolutionSource::Neutral);
        assert_eq!(
            queues.resolved_cursor(CLIENT),
            None,
            "buildup withholds: the cursor is not advanced"
        );

        // The command was NOT consumed: when a second command makes depth reach the
        // target, the next resolve consumes tick 0 as the first Real.
        assert!(queues.ingest(CLIENT, &command(1, 1.0)));
        let r = queues.resolve_tick(CLIENT).unwrap();
        assert_eq!(r.source, ResolutionSource::Real);
        assert_eq!(r.client_tick, 0);
        assert_eq!(queues.resolved_cursor(CLIENT), Some(0));
    }

    // P4b: a lone command then silence during buildup cannot pin the pawn armed forever —
    // armed holds increment held_ticks and the grace give-up fires (AC "absent client").
    #[test]
    fn lone_command_then_silence_during_buildup_gives_up() {
        let mut queues = HostCommandQueues::new();
        assert!(queues.ingest(CLIENT, &command(0, 1.0))); // depth 1, then silence

        for _ in 0..INPUT_HOLD_TICKS {
            let n = queues.resolve_tick(CLIENT).unwrap();
            assert_eq!(n.source, ResolutionSource::Neutral);
            assert_eq!(
                queues.resolved_cursor(CLIENT),
                None,
                "armed buildup does not advance while within grace"
            );
        }
        let giveup = queues.resolve_tick(CLIENT).unwrap();
        assert_eq!(giveup.source, ResolutionSource::Neutral);
        assert_eq!(
            queues.resolved_cursor(CLIENT),
            Some(0),
            "the grace give-up advances past the withheld command"
        );
    }

    // P5: a lone far-future command after a silence sits at depth 1 and keeps the re-armed
    // latch armed — it is NOT read as "buffer full" by any tick-distance. With no second
    // command, the grace give-up disarms and the neutral-walk then advances toward it.
    // Regression: a tick-distance readiness check would silently resume, skipping the fill.
    #[test]
    fn far_future_resume_is_depth_one_not_buffer_full() {
        let mut queues = HostCommandQueues::new();
        prime_disarmed(&mut queues, 0); // cursor 1

        for _ in 0..INPUT_HOLD_TICKS {
            assert_eq!(
                queues.resolve_tick(CLIENT).unwrap().source,
                ResolutionSource::Held
            );
        }
        let giveup = queues.resolve_tick(CLIENT).unwrap();
        assert_eq!(giveup.source, ResolutionSource::Neutral);
        assert_eq!(queues.resolved_cursor(CLIENT), Some(2));

        // A single far-future command: depth 1, latch stays armed (no advance).
        assert!(queues.ingest(CLIENT, &command(200, -1.0)));
        let held = queues.resolve_tick(CLIENT).unwrap();
        assert_eq!(held.source, ResolutionSource::Neutral);
        assert_eq!(
            queues.resolved_cursor(CLIENT),
            Some(2),
            "the lone far command does not disarm the latch"
        );

        // With no second command, the armed grace give-up disarms (pending still [200])
        // and the neutral-walk advances to the resumed Real at 200.
        let mut resolved_real = None;
        for _ in 0..260 {
            let r = queues.resolve_tick(CLIENT).unwrap();
            if r.source == ResolutionSource::Real {
                resolved_real = Some(r);
                break;
            }
        }
        assert_eq!(resolved_real.expect("resumes").client_tick, 200);
    }

    // P6: a command landing on the grace-edge tick is consumed Real (take_exact runs
    // before the give-up), not turned into a Neutral give-up.
    #[test]
    fn command_on_grace_edge_resolves_real_not_give_up() {
        let mut queues = HostCommandQueues::new();
        prime_disarmed(&mut queues, 0); // cursor 1 (T-1 = 1, T = 2)

        for _ in 0..INPUT_HOLD_TICKS {
            assert_eq!(
                queues.resolve_tick(CLIENT).unwrap().source,
                ResolutionSource::Held
            );
        }
        // The frame that would give up instead ingests tick 2's command first.
        assert!(queues.ingest(CLIENT, &command(2, -1.0)));
        let r = queues.resolve_tick(CLIENT).unwrap();
        assert_eq!(
            r.source,
            ResolutionSource::Real,
            "the grace-edge tick resolves Real, not a give-up Neutral"
        );
        assert_eq!(r.client_tick, 2);
        assert_eq!(queues.resolved_cursor(CLIENT), Some(2));
    }

    // P8: a multi-tick hitch frame (no ingest between resolves) burns the whole grace and
    // gives up in one frame; a command for the drop-staled tick in the next frame's ingest
    // is stale. Bounded loss on hitch frames is accepted.
    #[test]
    fn hitch_frame_burns_the_whole_grace() {
        let mut queues = HostCommandQueues::new();
        prime_disarmed(&mut queues, 0); // cursor 1

        for _ in 0..INPUT_HOLD_TICKS {
            assert_eq!(
                queues.resolve_tick(CLIENT).unwrap().source,
                ResolutionSource::Held
            );
        }
        assert_eq!(
            queues.resolve_tick(CLIENT).unwrap().source,
            ResolutionSource::Neutral
        );
        assert_eq!(queues.resolved_cursor(CLIENT), Some(2));

        // Tick 2's command, arriving in the next frame's ingest, is drop-staled.
        assert!(
            !queues.ingest(CLIENT, &command(2, -1.0)),
            "tick 2 was advanced past and is drop-staled"
        );
    }

    // P14: the standing invariant that a normal buildup completes before the hold grace
    // can give up on it — else buildup self-triggers a give-up.
    #[test]
    fn buildup_target_is_below_the_hold_grace() {
        assert!(
            INPUT_BUFFER_TARGET < INPUT_HOLD_TICKS as usize,
            "INPUT_BUFFER_TARGET ({INPUT_BUFFER_TARGET}) must be < INPUT_HOLD_TICKS ({INPUT_HOLD_TICKS}) \
             so buildup completes before the grace give-up fires"
        );
    }

    // P16: in steady state the resolved cursor TRAILS the newest received tick by
    // INPUT_BUFFER_TARGET - 1 (the signed netdiag cursor_lead reads a small negative).
    #[test]
    fn steady_state_cursor_trails_newest_by_the_playout_margin() {
        let mut queues = HostCommandQueues::new();
        // Fresh stream: buildup withholds until depth INPUT_BUFFER_TARGET, then the first
        // consume leaves the cursor trailing newest by INPUT_BUFFER_TARGET - 1.
        for t in 0..(INPUT_BUFFER_TARGET as u32) {
            assert!(queues.ingest(CLIENT, &command(t, 1.0)));
        }
        let r = queues.resolve_tick(CLIENT).unwrap();
        assert_eq!(r.source, ResolutionSource::Real);
        assert_eq!(r.client_tick, 0);

        let expected_lead = -((INPUT_BUFFER_TARGET as i32) - 1);
        let newest = INPUT_BUFFER_TARGET as u32 - 1;
        let cursor = queues.resolved_cursor(CLIENT).unwrap();
        assert_eq!(
            cursor.wrapping_sub(newest) as i32,
            expected_lead,
            "first consume trails newest by INPUT_BUFFER_TARGET - 1 (negative cursor_lead)"
        );

        // The margin stays negative under continued 1-in/1-out streaming.
        for t in (INPUT_BUFFER_TARGET as u32)..40 {
            assert!(queues.ingest(CLIENT, &command(t, 1.0)));
            let r = queues.resolve_tick(CLIENT).unwrap();
            assert_eq!(r.source, ResolutionSource::Real);
            let cursor = queues.resolved_cursor(CLIENT).unwrap();
            assert_eq!(
                cursor.wrapping_sub(t) as i32,
                expected_lead,
                "steady-state cursor_lead stays negative"
            );
        }
    }

    // P17: reset_level_scoped_host_state replaces HostCommandQueues wholesale
    // (netcode/endpoint.rs), so all per-client state resets to Default. A mid-buildup
    // stream carries nothing across the reset: the next stream re-enters buildup from None
    // and no stale reload edge survives.
    #[test]
    fn level_reset_clears_all_per_client_state_and_re_enters_buildup() {
        let mut queues = HostCommandQueues::new();
        assert!(queues.ingest(CLIENT, &reload_command(0, 1.0))); // mid-buildup, reload edge pending
        assert_eq!(
            queues.resolve_tick(CLIENT).unwrap().source,
            ResolutionSource::Neutral,
            "armed buildup withholds"
        );

        // Level change: the endpoint drops the whole queue and installs a fresh one.
        queues = HostCommandQueues::new();

        // The fresh stream re-enters buildup from None and carries no reload edge.
        assert!(queues.ingest(CLIENT, &command(5, 1.0)));
        let n = queues.resolve_tick(CLIENT).unwrap();
        assert_eq!(
            n.source,
            ResolutionSource::Neutral,
            "fresh stream re-enters buildup from None"
        );
        assert_eq!(queues.resolved_cursor(CLIENT), None);
        assert!(
            !n.command.reload,
            "no stale reload edge carried across the reset"
        );
    }

    // Ordering "Multiple fixed ticks in one frame": ingest a burst once, then resolve
    // several times; the playout depth absorbs it and resolutions are Real.
    #[test]
    fn multiple_fixed_ticks_in_one_frame_resolve_real() {
        let mut queues = HostCommandQueues::new();
        for t in 0..6u32 {
            assert!(queues.ingest(CLIENT, &command(t, 1.0)));
        }
        for t in 0..6u32 {
            let r = queues.resolve_tick(CLIENT).unwrap();
            assert_eq!(r.source, ResolutionSource::Real);
            assert_eq!(r.client_tick, t);
        }
        assert_eq!(queues.resolved_cursor(CLIENT), Some(5));
    }

    // Ordered input: consecutive ticks resolve Real and advance the cursor by one each.
    // Four commands pre-buffered (depth >= INPUT_BUFFER_TARGET) so the latch disarms
    // immediately.
    #[test]
    fn ordered_input_resolves_each_tick_real() {
        let mut queues = HostCommandQueues::new();
        for t in 0..4u32 {
            queues.ingest(CLIENT, &command(t, 1.0));
        }
        for t in 0..4u32 {
            let r = queues.resolve_tick(CLIENT).expect("a command per tick");
            assert_eq!(r.client_tick, t);
            assert_eq!(r.source, ResolutionSource::Real);
        }
        assert_eq!(queues.resolved_cursor(CLIENT), Some(3));
    }

    // === client_tick wrap ===

    // Ordering "client_tick wraps": all cursor/stale/hold comparisons stay wrap-aware; a
    // session crossing the u32 client_tick boundary resolves without a spurious flush.
    // Regression: a plain `<=` stale-check mis-ordered across the wrap, freezing the pawn
    // to neutral for the half-range past u32::MAX.
    #[test]
    fn client_tick_wrap_resolves_without_a_spurious_flush() {
        let mut queues = HostCommandQueues::new();

        // Prime disarmed just before the wrap (two commands so the latch disarms).
        assert!(queues.ingest(CLIENT, &command(u32::MAX - 1, 1.0)));
        assert!(queues.ingest(CLIENT, &command(u32::MAX, 1.0)));
        let a = queues.resolve_tick(CLIENT).unwrap();
        assert_eq!(a.source, ResolutionSource::Real);
        assert_eq!(a.client_tick, u32::MAX - 1);
        let b = queues.resolve_tick(CLIENT).unwrap();
        assert_eq!(b.source, ResolutionSource::Real);
        assert_eq!(b.client_tick, u32::MAX);
        assert_eq!(queues.resolved_cursor(CLIENT), Some(u32::MAX));

        // Post-wrap commands (ticks 0, 1) are AHEAD of the cursor in serial order — they
        // queue; a plain `0 <= u32::MAX` would wrongly drop them as stale.
        assert!(
            queues.ingest(CLIENT, &command(0, -1.0)),
            "a post-wrap command is not stale against a pre-wrap cursor"
        );
        assert!(queues.ingest(CLIENT, &command(1, -1.0)));

        // The expected tick wraps u32::MAX -> 0 and resolves Real without a flush.
        let c = queues.resolve_tick(CLIENT).unwrap();
        assert_eq!(c.source, ResolutionSource::Real);
        assert_eq!(c.client_tick, 0, "cursor+1 wraps to 0 cleanly");
        let d = queues.resolve_tick(CLIENT).unwrap();
        assert_eq!(d.source, ResolutionSource::Real);
        assert_eq!(d.client_tick, 1);
        assert_eq!(queues.resolved_cursor(CLIENT), Some(1));

        // And a genuinely stale pre-wrap command (<= cursor across the wrap) is dropped.
        assert!(
            !queues.ingest(CLIENT, &command(u32::MAX, 0.0)),
            "a pre-wrap command below the post-wrap cursor is stale"
        );
    }

    // === Reload recovery lane (logic unchanged; call sites re-threaded off the hold path) ===

    // A real reload resolves Real (edge delivered), a hold carries the held command's
    // reload level forward without advancing, and the give-up neutral clears it.
    #[test]
    fn reload_survives_real_then_hold_then_neutral_fallback() {
        let mut queues = HostCommandQueues::new();
        prime_disarmed(&mut queues, 0); // cursor 1, disarmed (non-reload)

        assert!(queues.ingest(CLIENT, &reload_command(2, 1.0)));
        let real = queues.resolve_tick(CLIENT).expect("real command resolves");
        assert_eq!(real.source, ResolutionSource::Real);
        assert!(real.command.reload, "real command preserves reload");

        for _ in 0..INPUT_HOLD_TICKS {
            let held = queues.resolve_tick(CLIENT).expect("held command resolves");
            assert_eq!(held.source, ResolutionSource::Held);
            assert!(
                held.command.reload,
                "the held tick carries the held command's reload level forward"
            );
        }

        let neutral = queues
            .resolve_tick(CLIENT)
            .expect("neutral command resolves");
        assert_eq!(neutral.source, ResolutionSource::Neutral);
        assert!(
            !neutral.command.reload,
            "the neutral give-up clears reload intent"
        );
    }

    /// Drive the cursor several ticks past the reload marker via silence give-ups (the
    /// marker stays at the prime), then record a stale-but-newer reload rising edge whose
    /// tick the give-up already advanced past. Leaves the latch armed with the recovered
    /// press queued. `tap_wish` is the (never-replayed) movement on the stale tap.
    fn setup_drop_staled_reload_edge(queues: &mut HostCommandQueues, tap_tick: u32, tap_wish: f32) {
        prime_disarmed(queues, 0); // cursor 1, reload marker (1, false)
        // Give-ups advance the cursor while the marker stays at 1.
        while queues.resolved_cursor(CLIENT).unwrap() < tap_tick {
            let _ = queues.resolve_tick(CLIENT);
        }
        // The tap is newer than the marker (records an edge) but <= the cursor (stale).
        let mut tap = reload_command(tap_tick, tap_wish);
        tap.movement.wish_dir[1] = tap_wish;
        assert!(
            !queues.ingest(CLIENT, &tap),
            "the reload tap is stale for movement (dropped), but records its edge"
        );
    }

    // P11: a recovered reload press is never delivered on a non-advancing hold (which
    // would re-test the same tick each hold and risk early delivery); it delivers once on
    // the next advancing resolution.
    #[test]
    fn recovered_reload_press_waits_for_an_advancing_resolution() {
        let mut queues = HostCommandQueues::new();
        setup_drop_staled_reload_edge(&mut queues, 2, 0.0);

        // The armed holds must NOT deliver the recovered press.
        for _ in 0..INPUT_HOLD_TICKS {
            let h = queues.resolve_tick(CLIENT).unwrap();
            assert!(
                !h.command.reload,
                "a non-advancing hold never delivers a recovered reload press"
            );
        }
        // The next advancing resolution (the give-up) delivers it, exactly once.
        let adv = queues.resolve_tick(CLIENT).unwrap();
        assert!(
            adv.command.reload,
            "the recovered press delivers on the advancing resolution"
        );
        assert!(
            !queues.resolve_tick(CLIENT).unwrap().command.reload,
            "the recovered press is delivered exactly once"
        );
    }

    // P12: a reload edge whose tick a give-up advanced past is recorded before the
    // stale-drop and delivered once on the next advancing resolution; the stale tap's
    // movement is never replayed.
    // Regression: a delayed true->false reload tap arriving after gap recovery had already
    // advanced past both ticks discarded the entire press.
    #[test]
    fn drop_staled_reload_edge_delivers_once_without_replaying_movement() {
        let mut queues = HostCommandQueues::new();
        setup_drop_staled_reload_edge(&mut queues, 2, 0.9); // stale tap carries wish 0.9

        let mut reload_count = 0;
        for _ in 0..(INPUT_HOLD_TICKS + 2) {
            let r = queues.resolve_tick(CLIENT).unwrap();
            if r.command.reload {
                reload_count += 1;
            }
            assert!(
                (r.command.movement.wish_dir.y - 0.9).abs() > EPSILON,
                "the stale tap's movement is never replayed"
            );
        }
        assert_eq!(
            reload_count, 1,
            "the drop-staled reload edge delivers exactly once"
        );
    }

    // Regression: retransmitting the stale tap after recovery must not re-latch the same
    // reload edge if intake tracked only the latest Boolean level.
    #[test]
    fn stale_reload_retransmit_does_not_deliver_duplicate_press() {
        let mut queues = HostCommandQueues::new();
        setup_drop_staled_reload_edge(&mut queues, 2, 0.0);

        // Deliver the recovered press once.
        let mut delivered = false;
        for _ in 0..(INPUT_HOLD_TICKS + 1) {
            if queues.resolve_tick(CLIENT).unwrap().command.reload {
                delivered = true;
                break;
            }
        }
        assert!(delivered, "the recovered press was delivered once");

        // Retransmit the same stale tap: dedup (tick == marker) blocks a new edge, and the
        // command is stale-dropped. No second press may be minted.
        assert!(!queues.ingest(CLIENT, &reload_command(2, 0.0)));
        for _ in 0..(INPUT_HOLD_TICKS + 2) {
            assert!(
                !queues.resolve_tick(CLIENT).unwrap().command.reload,
                "a duplicate/stale retransmit cannot mint another reload press"
            );
        }
    }

    // Regression: catch-up trimmed a reload tap along with the old movement prefix. The
    // catch-up path reseats the cursor (so the buildup latch is bypassed) and the reload
    // edge from the discarded prefix survives in the independent recovery lane.
    #[test]
    fn backlog_trim_preserves_reload_press_from_dropped_prefix() {
        let mut queues = HostCommandQueues::new();
        for tick in 0..=(INPUT_BUFFER_MAX as u32 + 2) {
            let mut cmd = command(tick, tick as f32 / 10.0);
            cmd.reload = tick == 3;
            assert!(queues.ingest(CLIENT, &cmd));
        }

        let recovered = queues.resolve_tick(CLIENT).expect("catch-up resolves");
        assert_eq!(recovered.source, ResolutionSource::Real);
        assert_eq!(recovered.client_tick, INPUT_BUFFER_MAX as u32 + 1);
        assert!(
            recovered.command.reload,
            "reload edge survives the trimmed command prefix"
        );
        assert!(
            (recovered.command.movement.wish_dir.y - (INPUT_BUFFER_MAX as f32 + 1.0) / 10.0).abs()
                < EPSILON,
            "catch-up still uses the retained real command's movement"
        );
        assert!(
            !queues.resolve_tick(CLIENT).unwrap().command.reload,
            "trimmed tap is delivered only once"
        );
    }

    // === Catch-up / bounded playout (depth-keyed; unchanged) ===

    /// Lag (in ticks) between the newest received command and the resolved cursor.
    /// `None` cursor (never resolved) reports the full depth from tick 0. Wrap-safe via
    /// the same serial-number subtraction the queue uses.
    fn lag(queues: &HostCommandQueues, client: u64, newest_received: u32) -> u32 {
        let cursor = queues.resolved_cursor(client).unwrap_or(0);
        newest_received.wrapping_sub(cursor)
    }

    // Regression (Ordering "Backlog burst"): a backlog accumulated during the accept/spawn
    // handshake window became PERMANENT ~800 ms latency, because the cursor seeded at the
    // oldest queued command and only advanced +1 per tick. The depth-keyed catch-up must
    // converge the lag to a small bounded buffer within a tick or two and keep it there.
    #[test]
    fn startup_backlog_converges_and_stays_bounded() {
        let mut queues = HostCommandQueues::new();

        const BACKLOG: u32 = 48;
        for t in 0..BACKLOG {
            assert!(queues.ingest(CLIENT, &command(t, 1.0)));
        }

        // First resolve fast-forwards: depth 48 > INPUT_BUFFER_MAX. Lag drops into the
        // bounded range immediately, NOT staying at 47.
        let newest = BACKLOG - 1;
        let r = queues.resolve_tick(CLIENT).expect("a command resolves");
        assert_eq!(
            r.source,
            ResolutionSource::Real,
            "the fast-forward consumes a recent real command, not a held/neutral"
        );
        assert!(
            lag(&queues, CLIENT, newest) <= INPUT_BUFFER_MAX as u32,
            "lag collapses to the bounded buffer on the first catch-up (lag={})",
            lag(&queues, CLIENT, newest)
        );

        // Steady state: one fresh command ingested per tick, one resolved. Lag stays
        // bounded forever — never creeps back toward 48.
        for next_tick in BACKLOG..(BACKLOG + 200) {
            assert!(queues.ingest(CLIENT, &command(next_tick, 1.0)));
            let r = queues.resolve_tick(CLIENT).expect("steady-state resolve");
            assert_eq!(
                r.source,
                ResolutionSource::Real,
                "steady 1-in/1-out resolves the expected real command"
            );
            assert!(
                lag(&queues, CLIENT, next_tick) <= INPUT_BUFFER_MAX as u32,
                "lag stays bounded under steady streaming (lag={})",
                lag(&queues, CLIENT, next_tick)
            );
        }
    }

    // Regression: a mid-session host frame hitch stalls the drain while the client keeps
    // streaming, deepening `pending`. The catch-up path must re-converge the lag after the
    // burst lands in one go.
    #[test]
    fn mid_session_hitch_catches_up() {
        let mut queues = HostCommandQueues::new();

        // Reach a disarmed steady state, then run a few clean ticks.
        prime_disarmed(&mut queues, 0); // cursor 1
        let mut next_tick = 2u32;
        for _ in 0..3 {
            assert!(queues.ingest(CLIENT, &command(next_tick, 1.0)));
            queues.resolve_tick(CLIENT).expect("steady resolve");
            next_tick += 1;
        }
        let steady_newest = next_tick - 1;
        assert!(lag(&queues, CLIENT, steady_newest) <= INPUT_BUFFER_MAX as u32);

        // The host stalls for a long frame: BURST commands arrive before the next resolve.
        const BURST: u32 = 30;
        for _ in 0..BURST {
            assert!(queues.ingest(CLIENT, &command(next_tick, -1.0)));
            next_tick += 1;
        }
        let newest_after_burst = next_tick - 1;

        // The very next resolve fast-forwards back into the bounded range.
        let r = queues.resolve_tick(CLIENT).expect("post-hitch resolve");
        assert_eq!(r.source, ResolutionSource::Real);
        assert!(
            lag(&queues, CLIENT, newest_after_burst) <= INPUT_BUFFER_MAX as u32,
            "the hitch backlog re-converges to the bounded buffer (lag={})",
            lag(&queues, CLIENT, newest_after_burst)
        );

        // And it stays bounded under resumed steady streaming.
        for _ in 0..100 {
            let newest_received = next_tick;
            assert!(queues.ingest(CLIENT, &command(newest_received, -1.0)));
            next_tick += 1;
            queues.resolve_tick(CLIENT).expect("resumed steady resolve");
            assert!(
                lag(&queues, CLIENT, newest_received) <= INPUT_BUFFER_MAX as u32,
                "lag stays bounded after the hitch (lag={})",
                lag(&queues, CLIENT, newest_received)
            );
        }
    }
}

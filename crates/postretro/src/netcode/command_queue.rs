// Host-side authoritative command queues and the deterministic input-gap policy
// (M15 Phase 3 Task 4). Per-client queues hold sanitized inbound `InputCommand`s
// keyed by client id; the per-pawn resolved cursor (`last_processed_client_tick`)
// drives a hold-then-neutral gap policy so a missing command tick never stalls the
// authoritative movement seam.
//
// Bounded playout buffer + depth-keyed catch-up: the resolved cursor consumes one
// command per 60 Hz tick, the same rate the client produces them. Without catch-up,
// any backlog that builds up in `pending` becomes PERMANENT latency, because
// drain-rate == produce-rate and the cursor only advances +1 per tick. Two backlogs
// matter: (1) the client streams input at 60 Hz immediately on connect, but the host
// can't drain a pawn until `owners.set()` runs at the end of accept+spawn — so a
// handshake/spawn-window backlog (tens of ticks ≈ hundreds of ms) accumulates; (2) a
// mid-session host frame hitch stalls the drain while commands keep arriving. Either
// way, a seed at the oldest queued command with +1-only advance locks that backlog in
// forever (a 48-command startup backlog → a rock-steady ~800 ms lag that never
// shrinks). The fix: when `pending` depth exceeds INPUT_BUFFER_MAX, fast-forward —
// drop all but the newest INPUT_BUFFER_TARGET commands and reseat the cursor on the
// new oldest, so playout converges to a small bounded buffer and stays there.
//
// Why depth-keyed (number of buffered commands), NOT tick-distance to the newest:
// a continuous-stream backlog holds MANY commands queued ahead (catch up), but a
// client that went silent then RESUMED at a far-future tick holds exactly ONE command
// far ahead (must NOT catch up — the hold→neutral→real resume path must stay intact).
// Tick-distance can't tell those apart; pending depth can.
// See: context/lib/networking.md
//
// Boundary: this is engine-side game logic, not the net crate. The net crate is
// registry-blind and only moves typed messages; intake/selection/gap policy live
// here because they bridge the client-id keyed wire stream to the per-pawn movement
// seam (`sim::host_movement`). Intake runs `wire_convert::sanitize_input_command`
// before queueing — an invalid command never mutates a queue.

use std::collections::HashMap;

use postretro_net::wire::InputCommand;

use crate::netcode::prediction::client_tick_le;
use crate::netcode::wire_convert::{input_command_to_sim, sanitize_input_command};
use crate::scripting::registry::EntityId;
use crate::sim::SimCommand;

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

/// Hold the last resolved command for at most this many missing ticks before
/// synthesizing neutral input. Deterministic gap policy (Task 4 §C): a short hold
/// rides out a single dropped/late packet; a longer gap falls back to neutral so a
/// disconnected-but-not-yet-closed client cannot keep its pawn coasting on stale
/// intent.
pub(crate) const INPUT_HOLD_TICKS: u32 = 3;

/// Steady-state playout floor: the pending depth a catch-up fast-forward trims back
/// to. ~2 ticks ≈ 33 ms at 60 Hz — a small buffer that absorbs one late/dropped
/// packet (it complements [`INPUT_HOLD_TICKS`]: the buffer rides out jitter on the
/// way in, the hold rides it out on the way out) without re-introducing perceptible
/// latency. Kept well below [`INPUT_BUFFER_MAX`] so catch-up restores real headroom.
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
    /// Consecutive ticks the previous command has been held across a gap. Reset to 0
    /// whenever a real command resolves; once it reaches [`INPUT_HOLD_TICKS`] the gap
    /// policy synthesizes neutral input.
    held_ticks: u32,
}

impl ClientCommandState {
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
    /// stale, or a duplicate. A rejected command mutates no queue state — the
    /// invalid-input invariant the task requires.
    pub(crate) fn ingest(&mut self, client_id: u64, raw: &InputCommand) -> bool {
        let Some(sanitized) = sanitize_input_command(raw) else {
            // Non-finite: never touch any queue or cursor. The client's state is not
            // even created on a rejected first command.
            return false;
        };
        self.clients
            .entry(client_id)
            .or_default()
            .enqueue(sanitized)
    }

    /// Resolve exactly one command for `client_id`'s pawn this fixed tick, applying
    /// the deterministic gap policy, and advance the pawn's resolved cursor. Returns
    /// `None` only for a client that has never sent a command AND has no prior
    /// resolution — there is nothing to drive its pawn with yet (the pawn holds its
    /// authoritative pose). Once any command has resolved, this always returns a
    /// command (held or neutral) so the pawn advances deterministically.
    ///
    /// Cursor model: the host expects the tick immediately after the resolved cursor.
    /// If that exact tick is queued, consume it (`Real`). Otherwise hold the previous
    /// command for up to [`INPUT_HOLD_TICKS`] ticks (`Held`), then synthesize neutral
    /// (`Neutral`). Real and synthetic resolutions both advance the cursor.
    ///
    /// Bounded playout + catch-up: BEFORE picking the expected tick, if the pending
    /// queue has grown past [`INPUT_BUFFER_MAX`] real buffered commands, fast-forward —
    /// keep only the newest [`INPUT_BUFFER_TARGET`] and reseat the cursor on the new
    /// oldest. Because drain-rate == produce-rate (both 60 Hz), a backlog that builds
    /// during the accept/spawn handshake window (the client streams on connect before
    /// the host can drain) or a mid-session host hitch would otherwise become permanent
    /// latency under +1-only advance; this single path drains it back to a small buffer
    /// and keeps it there. It is depth-keyed (count of buffered commands), NOT
    /// tick-distance to the newest, so a single far-future command after a silence does
    /// NOT trip it — the hold→neutral→real resume path stays intact.
    pub(crate) fn resolve_tick(&mut self, client_id: u64) -> Option<ResolvedCommand> {
        let state = self.clients.get_mut(&client_id)?;

        // Catch-up fast-forward: a deep pending queue means real commands are stacking
        // up faster than the +1-per-tick cursor consumes them — a startup-handshake or
        // hitch backlog. Drop all but the newest INPUT_BUFFER_TARGET so the resolved
        // cursor never sits more than a small bounded buffer behind the newest received
        // command. Wrap-aware throughout: the new oldest's `client_tick - 1` (serial
        // arithmetic) is the cursor the normal exact-tick path then consumes as `Real`.
        if state.pending.len() > INPUT_BUFFER_MAX {
            let drop_count = state.pending.len() - INPUT_BUFFER_TARGET;
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
            // queued and nothing prior resolved, there is nothing to drive yet.
            None => state.pending.first().map(|c| c.client_tick)?,
            Some(cursor) => cursor.wrapping_add(1),
        };

        // Exact-tick hit: a real command resolves this tick.
        if let Some(cmd) = state.take_exact(expected) {
            let sim = input_command_to_sim(&cmd);
            state.last_resolved = Some(cmd);
            state.held_ticks = 0;
            state.resolved_cursor = Some(expected);
            state.drop_stale(expected);
            return Some(ResolvedCommand {
                command: sim,
                client_tick: expected,
                source: ResolutionSource::Real,
            });
        }

        // Gap: hold the previous command for up to INPUT_HOLD_TICKS, then neutral.
        let (sim, source) = if state.held_ticks < INPUT_HOLD_TICKS {
            match &state.last_resolved {
                Some(prev) => {
                    state.held_ticks += 1;
                    (input_command_to_sim(prev), ResolutionSource::Held)
                }
                // No previous command to hold (cursor advanced via neutral only):
                // neutral immediately.
                None => (neutral_sim_command(), ResolutionSource::Neutral),
            }
        } else {
            // Hold lapsed: neutral. Clear the held command so a later real command at
            // a still-higher tick resumes cleanly rather than re-holding stale intent.
            state.last_resolved = None;
            (neutral_sim_command(), ResolutionSource::Neutral)
        };

        state.resolved_cursor = Some(expected);
        state.drop_stale(expected);
        Some(ResolvedCommand {
            command: sim,
            client_tick: expected,
            source,
        })
    }

    /// The pawn's resolved cursor (`last_processed_client_tick`) for snapshot
    /// authority metadata. `None` until the first command resolves.
    pub(crate) fn resolved_cursor(&self, client_id: u64) -> Option<u32> {
        self.clients.get(&client_id).and_then(|s| s.resolved_cursor)
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
pub(crate) fn host_resolve_movement_inputs(
    owners: &MovementOwners,
    command_queues: &mut HostCommandQueues,
) -> Vec<(EntityId, crate::movement::MovementInput)> {
    let mut pairs = Vec::new();
    // Snapshot the owner pairs first so the mutable queue borrow does not alias the
    // owners borrow.
    let owner_pairs: Vec<(EntityId, u64)> = owners.iter().collect();
    for (pawn, client_id) in owner_pairs {
        if let Some(resolved) = command_queues.resolve_tick(client_id) {
            pairs.push((pawn, resolved.command.movement));
        }
    }
    pairs
}

/// A neutral (no-intent) sim command: no wish direction, no buttons, facing held at
/// zero. The deterministic fallback when the gap policy exhausts the hold window.
/// Facing 0.0 is acceptable for Phase 3's movement-only scope — a neutral tick
/// applies no locomotion, so the held facing does not visibly snap; Task 5/6 may
/// refine to hold the last facing if needed.
fn neutral_sim_command() -> SimCommand {
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
            facing_yaw: 0.0,
        },
        fire_button: FireButtonState {
            pressed: false,
            active: false,
        },
    }
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
            },
            fire_button: WireFireButtonState {
                pressed: false,
                active: false,
            },
        }
    }

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
    // sanitizer's contract); the queued+resolved command reflects the clamp.
    #[test]
    fn ingest_clamps_out_of_range_wish_dir_before_queueing() {
        let mut queues = HostCommandQueues::new();
        let mut cmd = command(0, 5.0); // forward 5.0 -> clamp to 1.0
        cmd.movement.wish_dir[0] = -3.0; // right -3.0 -> clamp to -1.0
        assert!(queues.ingest(CLIENT, &cmd));
        let resolved = queues.resolve_tick(CLIENT).expect("a command resolves");
        assert!((resolved.command.movement.wish_dir.x - (-1.0)).abs() < EPSILON);
        assert!((resolved.command.movement.wish_dir.y - 1.0).abs() < EPSILON);
    }

    // An exact duplicate tick collapses to one queued command; a stale command at or
    // below the resolved cursor is dropped. Neither mutates unrelated state.
    #[test]
    fn ingest_collapses_duplicates_and_drops_stale() {
        let mut queues = HostCommandQueues::new();
        assert!(queues.ingest(CLIENT, &command(0, 1.0)));
        // Exact duplicate of tick 0: collapsed.
        assert!(!queues.ingest(CLIENT, &command(0, 0.5)));
        // Resolve tick 0 so the cursor advances to 0.
        let r = queues.resolve_tick(CLIENT).unwrap();
        assert_eq!(r.client_tick, 0);
        assert_eq!(r.source, ResolutionSource::Real);
        // The first-arrival wins the duplicate collapse: forward intent is 1.0, not 0.5.
        assert!((r.command.movement.wish_dir.y - 1.0).abs() < EPSILON);

        // A late command at the resolved cursor (0) is stale -> dropped.
        assert!(!queues.ingest(CLIENT, &command(0, 0.0)));
        assert_eq!(queues.resolved_cursor(CLIENT), Some(0));
    }

    // Regression: the enqueue stale-check used a plain `<=`, which mis-ordered the
    // comparison straddling the u32 client_tick wrap (the allocator wraps with
    // wrapping_add) — freezing the pawn to neutral for the half-range past u32::MAX.
    // The wrap-aware predicate keeps a post-wrap command live against a pre-wrap cursor.
    #[test]
    fn enqueue_stale_check_is_wrap_aware_at_the_u32_boundary() {
        let mut queues = HostCommandQueues::new();

        // Resolve a command just before the wrap so the cursor sits at u32::MAX - 1.
        assert!(queues.ingest(CLIENT, &command(u32::MAX - 1, 1.0)));
        let r = queues.resolve_tick(CLIENT).unwrap();
        assert_eq!(r.client_tick, u32::MAX - 1);
        assert_eq!(queues.resolved_cursor(CLIENT), Some(u32::MAX - 1));

        // A post-wrap command (tick 1) is AHEAD of the cursor in serial-number order,
        // so it must queue — a plain `1 <= u32::MAX-1` would wrongly drop it as stale.
        assert!(
            queues.ingest(CLIENT, &command(1, -1.0)),
            "a post-wrap command is not stale against a pre-wrap cursor"
        );

        // And a genuinely stale pre-wrap command (== cursor) is still dropped.
        assert!(
            !queues.ingest(CLIENT, &command(u32::MAX - 1, 0.0)),
            "a command at the cursor is stale across the wrap too"
        );
    }

    // Ordered input: consecutive ticks resolve as Real and advance the cursor by one
    // each, returning the matching command.
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

    // Gap policy: a missing tick holds the last intent for INPUT_HOLD_TICKS ticks,
    // then synthesizes neutral. Every synthetic tick advances the cursor.
    #[test]
    fn missing_tick_holds_then_neutral_advancing_cursor() {
        let mut queues = HostCommandQueues::new();
        // Tick 0 arrives with distinctive forward intent; ticks 1.. are missing.
        queues.ingest(CLIENT, &command(0, 1.0));
        let r0 = queues.resolve_tick(CLIENT).unwrap();
        assert_eq!(r0.source, ResolutionSource::Real);

        // Ticks 1..=3: held (INPUT_HOLD_TICKS = 3), each carrying the held forward
        // intent and advancing the cursor.
        for expected_tick in 1..=INPUT_HOLD_TICKS {
            let r = queues.resolve_tick(CLIENT).unwrap();
            assert_eq!(r.source, ResolutionSource::Held);
            assert_eq!(r.client_tick, expected_tick);
            assert!(
                (r.command.movement.wish_dir.y - 1.0).abs() < EPSILON,
                "held command repeats the last real intent"
            );
        }

        // Tick 4: hold lapsed -> neutral (no forward intent), cursor still advances.
        let r = queues.resolve_tick(CLIENT).unwrap();
        assert_eq!(r.source, ResolutionSource::Neutral);
        assert_eq!(r.client_tick, INPUT_HOLD_TICKS + 1);
        assert!(
            r.command.movement.wish_dir.y.abs() < EPSILON,
            "neutral synthesizes no movement intent"
        );
        assert_eq!(queues.resolved_cursor(CLIENT), Some(INPUT_HOLD_TICKS + 1));
    }

    // Late arrival: a command that arrives for a tick still ahead of the cursor (a
    // bounded delay within the hold window) is consumed as Real once the cursor
    // reaches it, even though intervening ticks were held.
    #[test]
    fn late_arrival_within_hold_resolves_real_when_cursor_reaches_it() {
        let mut queues = HostCommandQueues::new();
        queues.ingest(CLIENT, &command(0, 1.0));
        let _ = queues.resolve_tick(CLIENT); // resolve tick 0 (Real)

        // Tick 1 is missing now -> hold.
        let held = queues.resolve_tick(CLIENT).unwrap();
        assert_eq!(held.source, ResolutionSource::Held);
        assert_eq!(held.client_tick, 1);

        // Tick 2's command arrives late (but still ahead of the cursor). The next
        // resolve targets tick 2 and consumes it as Real.
        queues.ingest(CLIENT, &command(2, -1.0));
        let real = queues.resolve_tick(CLIENT).unwrap();
        assert_eq!(real.source, ResolutionSource::Real);
        assert_eq!(real.client_tick, 2);
        assert!((real.command.movement.wish_dir.y - (-1.0)).abs() < EPSILON);
    }

    // A real command at or below the resolved cursor is stale and dropped at intake —
    // it never resurrects an already-settled tick.
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

    // Resumed input after a long gap: once the hold lapses to neutral and a fresh
    // command arrives at a higher tick, the cursor advances (neutral) up to the new
    // command and then resolves it Real. The pawn resumes cleanly without replaying
    // stale held intent.
    #[test]
    fn resumed_input_after_gap_resolves_cleanly() {
        let mut queues = HostCommandQueues::new();
        queues.ingest(CLIENT, &command(0, 1.0));
        let _ = queues.resolve_tick(CLIENT); // tick 0 Real, cursor 0

        // Long silence: resolve enough ticks to exhaust the hold and go neutral.
        // Ticks 1..=3 held, tick 4 neutral.
        for _ in 0..(INPUT_HOLD_TICKS + 1) {
            let _ = queues.resolve_tick(CLIENT);
        }
        assert_eq!(queues.resolved_cursor(CLIENT), Some(INPUT_HOLD_TICKS + 1));

        // Client resumes at a fresh higher tick (e.g. 10). It is queued, not stale.
        let resume_tick = 10u32;
        assert!(queues.ingest(CLIENT, &command(resume_tick, -1.0)));

        // The next resolutions synthesize neutral for the still-missing ticks
        // (cursor+1 .. resume_tick-1), then resolve tick `resume_tick` Real. The
        // cursor sits at INPUT_HOLD_TICKS+1, so it takes exactly
        // `resume_tick - (INPUT_HOLD_TICKS + 1)` resolutions to land on resume_tick.
        let mut last = None;
        for _ in 0..(resume_tick - (INPUT_HOLD_TICKS + 1)) {
            last = queues.resolve_tick(CLIENT);
        }
        let resolved = last.expect("resumed command resolves");
        assert_eq!(resolved.client_tick, resume_tick);
        assert_eq!(resolved.source, ResolutionSource::Real);
        assert!((resolved.command.movement.wish_dir.y - (-1.0)).abs() < EPSILON);
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

    // Duplicate `ClientMessage::Input` injected at the drain/queue seam does not
    // mutate unrelated clients' state and does not panic. (Task gate: duplicate/old
    // injected at the drain/queue seam are inert against other entities.)
    #[test]
    fn duplicate_injection_does_not_disturb_other_clients() {
        let mut queues = HostCommandQueues::new();
        const A: u64 = 1;
        const B: u64 = 2;
        queues.ingest(A, &command(0, 1.0));
        queues.ingest(B, &command(0, -1.0));

        // Flood A with duplicates and stale commands.
        for _ in 0..10 {
            let _ = queues.ingest(A, &command(0, 0.0));
        }
        // B is untouched: its command resolves with its own intent.
        let rb = queues.resolve_tick(B).unwrap();
        assert!((rb.command.movement.wish_dir.y - (-1.0)).abs() < EPSILON);
        // A still resolves its first-arrival command, not a duplicate's 0.0 intent.
        let ra = queues.resolve_tick(A).unwrap();
        assert!((ra.command.movement.wish_dir.y - 1.0).abs() < EPSILON);
    }

    /// Lag (in ticks) between the newest received command and the resolved cursor.
    /// `None` cursor (never resolved) reports the full depth from tick 0. Wrap-safe via
    /// the same serial-number subtraction the queue uses.
    fn lag(queues: &HostCommandQueues, client: u64, newest_received: u32) -> u32 {
        let cursor = queues.resolved_cursor(client).unwrap_or(0);
        newest_received.wrapping_sub(cursor)
    }

    // Regression: a backlog accumulated during the accept/spawn handshake window (the
    // client streams at 60 Hz on connect before the host can drain) became PERMANENT
    // ~800 ms latency, because the cursor seeded at the oldest queued command and only
    // advanced +1 per tick — drain-rate == produce-rate, so the backlog never shrank.
    // The depth-keyed catch-up must converge the lag to a small bounded buffer within a
    // tick or two and keep it there under steady 1-in/1-out streaming.
    #[test]
    fn startup_backlog_converges_and_stays_bounded() {
        let mut queues = HostCommandQueues::new();

        // The host couldn't drain this pawn until ownership was set: a 48-command
        // backlog (≈ 800 ms at 60 Hz) piled up in `pending`. Nothing resolved yet.
        const BACKLOG: u32 = 48;
        for t in 0..BACKLOG {
            assert!(queues.ingest(CLIENT, &command(t, 1.0)));
        }

        // First resolve fast-forwards: depth 48 > INPUT_BUFFER_MAX. The lag must
        // immediately drop into the bounded range (≤ INPUT_BUFFER_MAX), NOT stay at 47.
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

        // Now run steady state: one fresh command ingested per simulated tick, one
        // resolved. The lag must stay bounded forever — never creep back toward 48.
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

    // Regression: a mid-session host frame hitch stalls the drain while the client
    // keeps streaming, deepening `pending` the same way the startup backlog did. The
    // same catch-up path must re-converge the lag after the burst lands in one go.
    #[test]
    fn mid_session_hitch_catches_up() {
        let mut queues = HostCommandQueues::new();

        // Reach steady state cleanly: a few ordered ticks, one resolved each.
        let mut next_tick = 0u32;
        for _ in 0..5 {
            assert!(queues.ingest(CLIENT, &command(next_tick, 1.0)));
            queues.resolve_tick(CLIENT).expect("steady resolve");
            next_tick += 1;
        }
        let steady_newest = next_tick - 1;
        assert!(lag(&queues, CLIENT, steady_newest) <= INPUT_BUFFER_MAX as u32);

        // The host stalls for a long frame: BURST commands arrive before the next
        // resolve (depth jumps well past INPUT_BUFFER_MAX).
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

    // Resume-after-silence must NOT trip catch-up: a single far-future command after a
    // long gap holds exactly ONE entry in `pending`, so depth never exceeds
    // INPUT_BUFFER_MAX. This guards that the depth-keyed (not tick-distance) trigger
    // preserves the hold→neutral→real resume semantics — the inverse failure mode the
    // catch-up must avoid. Mirrors `resumed_input_after_gap_resolves_cleanly`.
    #[test]
    fn resume_after_silence_does_not_trigger_catchup() {
        let mut queues = HostCommandQueues::new();
        queues.ingest(CLIENT, &command(0, 1.0));
        let _ = queues.resolve_tick(CLIENT); // tick 0 Real, cursor 0

        // Long silence: exhaust the hold and go neutral (ticks 1..=3 held, tick 4
        // neutral).
        for _ in 0..(INPUT_HOLD_TICKS + 1) {
            let _ = queues.resolve_tick(CLIENT);
        }
        assert_eq!(queues.resolved_cursor(CLIENT), Some(INPUT_HOLD_TICKS + 1));

        // Client resumes at a far-future tick. Pending depth is exactly 1 — far below
        // INPUT_BUFFER_MAX — so catch-up must NOT fire and discard it.
        let resume_tick = 200u32;
        assert!(queues.ingest(CLIENT, &command(resume_tick, -1.0)));

        // Walk the neutral fill up to the resume tick, then resolve it Real. If catch-up
        // had wrongly fired, the single far-future command would have been kept (depth 1
        // is already <= INPUT_BUFFER_TARGET) but the cursor would have JUMPED forward to
        // resume_tick-1, skipping the deterministic neutral fill — so the first resolve
        // would already be Real. Assert the gap is filled with neutral first.
        let first = queues.resolve_tick(CLIENT).unwrap();
        assert_eq!(
            first.source,
            ResolutionSource::Neutral,
            "a far-future single command does NOT fast-forward the cursor; the gap fills neutral"
        );
        assert_eq!(first.client_tick, INPUT_HOLD_TICKS + 2);

        let mut last = Some(first);
        for _ in 0..(resume_tick - (INPUT_HOLD_TICKS + 2)) {
            last = queues.resolve_tick(CLIENT);
        }
        let resolved = last.expect("resumed command resolves");
        assert_eq!(resolved.client_tick, resume_tick);
        assert_eq!(resolved.source, ResolutionSource::Real);
        assert!((resolved.command.movement.wish_dir.y - (-1.0)).abs() < EPSILON);
    }
}

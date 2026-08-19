# Co-op Client Movement Feel

## Goal

Restore smooth client-player movement in authoritative co-op. The host command queue currently discards the majority of a connected client's input on a clean link, causing constant micro-stutter, a jump that must be held ~0.2–0.5 s before it fires, and stepped rotation as seen by the host. Fix the host's input resolution so a client's on-time-but-slightly-late input is applied rather than dropped, then re-measure to decide whether any presentation-side smoothing remains warranted.

## Scope

### In scope

- **Fix A — host input playout buffer.** Change `HostCommandQueues::resolve_tick` so it does not advance the resolve cursor past a tick whose command has not yet arrived (within the hold grace), and so it maintains a small standing playout depth behind the newest received command. This is the primary, evidence-backed fix and it delivers the stutter, hold-to-jump, and (very likely) the host-side rotation fixes at once.
- **Re-measurement gate.** After Fix A, capture the `netdiag` counters (vsync-on and vsync-off) and decide, from data, whether a host-side presentation defect survives.
- **Fix B — host-side remote-pawn delay presentation (conditional).** If re-measurement shows the host still steps the client pawn's orientation after Fix A, give the host the same delay-buffered presentation the client already uses for remotes. Design carried here; implementation gated on the measurement.
- **Fix C — orientation sampling cadence (conditional).** If re-measurement shows the sending client's sub-60-fps render rate still starves orientation after Fix A, address the render-rate sampling of `facing_yaw`. Gated, and must argue divergence from the `input.md` §3 render-rate-look contract.

### Out of scope

- Splitting `main.rs` (11 161 lines) or `interpolation.rs` (1 428 lines). Fix A lives in `command_queue.rs`; Fix B/C add a new `netcode` submodule with only thin `main.rs` wiring. A split of these monoliths is its own work, not gated on this bug.
- Wire-format changes. The client already sends one `ClientMessage::Input` per fixed tick at 60/s; the send path is correct and untouched.
- Client-side prediction, reconciliation, or the interpolation buffer's internals — all verified correct.
- The `--release` build skipping the scripts-build regeneration (a separate, unrelated launch-tooling bug).

## Direction

**Problem.** `resolve_tick` (`crates/postretro/src/netcode/command_queue.rs:397`) advances `resolved_cursor` and calls `drop_stale(expected)` on the gap path (`:490–491`), so when the client's command for the expected tick has not yet arrived — which, on a clean loopback, is most ticks, because the client's 60 Hz send and the host's 60 Hz resolve run ~1 tick out of phase — the host Neutral/Held-fills the tick, advances past it, and discards the real command as stale when it lands. The cause is *cursor advancement on an unfilled tick*, not loss, not a rate limit, not the catch-up trim (`trims = 0`, `cursor_lead ≈ 0` in the data; see `research.md`).

**Prior commitments.** The prediction/reconciliation and interpolation model in `context/lib/networking.md` is preserved: the client still predicts locally and reconciles against `last_processed_client_tick` (stamped from `resolved_cursor`, `netcode/replication.rs:142`). Fix A makes `resolved_cursor` trail the newest received tick by a small playout margin (~`INPUT_BUFFER_TARGET`), so the client's ack lags ~2 ticks more and its reconcile replay tail grows by ~2 ticks — bounded and within the existing model. The existing gap policy (`INPUT_HOLD_TICKS = 3`), catch-up trim (`INPUT_BUFFER_MAX = 8`, `INPUT_BUFFER_TARGET = 2`), and reload recovery lane (`pending_reload_presses`, `preserve_due_reload_press`) are retained; Fix A changes *when the cursor advances*, not those constants' meaning. Fix C would touch the `input.md` §3 render-rate-look contract — an explicit divergence it must argue, which is why it is gated.

**Alternatives rejected.** (1) *Minimal change only* — stop advancing the cursor during the Hold grace, but keep resolving from the queue edge (no standing playout depth). This recovers late commands but, under a steady ~1-tick phase offset, still alternates Held/Real (~50 % Real) because each freshly-expected tick is again not-yet-arrived. Rejected as the sole fix: it halves the loss but does not close it. Fix A keeps this "don't advance on hold" behavior *and* adds the standing playout depth that gets steady-state resolution to ~100 % Real. (2) *Timestamp-based input reordering / client send-with-lead* — larger surface, touches the wire and the client, and buys little over a host-side playout buffer, which is the standard placement for absorbing receive jitter. Rejected as disproportionate. (3) *Raise the snapshot rate or extrapolate on the host* — addresses presentation, not the input-loss root; does nothing for the dropped commands.

Foreclosures and one-way doors: Fix A commits to a buffered-input playout model and adds ~2 ticks (~33 ms) of latency to *authoritative* input application; client prediction hides this locally, and it is tunable via `INPUT_BUFFER_TARGET`, so undoing or retuning is a constant change — not a one-way door. Fix B commits the host to delay-buffered remote presentation (adds presentation latency to how the host sees remotes, already true on the client side); localized and reversible.

## Acceptance criteria

- [ ] On a clean loopback co-op session with continuous client movement, host `netdiag::queue` shows `real` ≥ ~55 of ~60 per second and `neutral` ≈ 0 (was `real` ~8–15, `neutral` ~30–45). Verified from a captured host log.
- [ ] Across a client jump-test of ~10 taps, host `netdiag::jump` shows `dropped_neutral = 0` and `dropped_trim = 0`, and a clean single tap fires a jump without a hold. (Was `dropped_neutral ≈ attempts`, `max_hold` 67–217 ms.)
- [ ] `cursor_lead` (host `netdiag::queue`) settles at a small positive playout margin (~`INPUT_BUFFER_TARGET`) behind the newest received tick, not at 0 and not growing unbounded.
- [ ] A command that arrives up to `INPUT_HOLD_TICKS` ticks after its expected tick resolves as `Real`, not `Neutral` — the awaited tick is not drop-staled while still within the hold grace.
- [ ] A command that never arrives still yields `Neutral` and the cursor still advances after `INPUT_HOLD_TICKS`, so a genuinely absent client cannot stall the host indefinitely.
- [ ] The reload recovery lane still fires exactly once per rising reload edge under late/duplicate/out-of-order arrival (existing reload behavior unchanged by the cursor-advance change).
- [ ] `client_tick` wrap: all cursor/stale/hold comparisons remain wrap-aware; a session that crosses the `u32` `client_tick` boundary resolves without a spurious flush.
- [ ] Re-measurement captured post-Fix-A (vsync-on and vsync-off) and a written go/no-go recorded for Fix B and Fix C against the criteria in Task 2.
- [ ] (Fix B, only if triggered) With the host presenting client pawns through the delay buffer, host-observed `distinct_yaw` during continuous client turning tracks the client's send rate rather than stepping, with no regression to position smoothness.
- [ ] (Fix C, only if triggered) Host-observed orientation update rate for a client rendering below 60 fps is decoupled from that render rate, with the `input.md` §3 divergence documented.

## Tasks

### Task 1: Host input playout buffer

Rework the gap path of `HostCommandQueues::resolve_tick` (`crates/postretro/src/netcode/command_queue.rs:397`) so a slightly-late client command resolves `Real` instead of being drop-staled, and so the resolver maintains a small standing playout depth behind the newest received tick. Two behaviors, both in `resolve_tick` and the `ClientCommandState` it mutates (`command_queue.rs:167`):

(a) **Do not advance the cursor past an unfilled tick within the hold grace.** When `take_exact(expected)` returns `None` and `held_ticks < INPUT_HOLD_TICKS`, resolve `Held` (as today, via `held_gap_sim_command`) but leave `resolved_cursor` unchanged and do **not** call `drop_stale(expected)` — so `expected` is retried next tick and its command, when it lands within the grace, resolves `Real`. Only when the hold lapses (`held_ticks` reaches `INPUT_HOLD_TICKS`) does the resolver give up: advance the cursor past `expected`, `drop_stale`, resolve `Neutral`, and reset the hold so a later real command at a higher tick resumes cleanly. The reload recovery lane (`preserve_due_reload_press`, `pending_reload_presses`) must continue to observe the resolved tick only on ticks where the cursor actually advances — thread it through the give-up and Real paths, not the hold-without-advance path.

(b) **Maintain a standing playout depth.** So the resolver runs a small fixed margin behind the newest received tick (`latest_observed_reload.0`) rather than at the queue edge, hold the first Real resolution of a fresh or post-underrun stream until `pending` has built to ~`INPUT_BUFFER_TARGET` (the existing steady-state floor constant). During buildup, resolve `Neutral`/`Held` without advancing past un-arrived ticks. This makes `resolved_cursor` trail the newest received command by ~`INPUT_BUFFER_TARGET` in steady state, so every expected tick is already buffered and resolves `Real`. Keep the existing catch-up trim (`INPUT_BUFFER_MAX`) for the over-buffered/hitch case unchanged. Access: `latest_observed_reload.0` is the newest received `client_tick` (updated unconditionally at `ingest`, `command_queue.rs:371`); read it in `resolve_tick` to compute the playout margin. Do not add a new wire field. The diagnostics already recorded in `resolve_tick` (`self.diag.record(...)`) stay and become the verification instrument for the ACs.

### Task 2: Re-measurement gate

With Task 1 landed, run the `netdiag`-instrumented two-process session (`RUST_LOG=postretro::netdiag=debug`) on `content/dev/maps/movement-feel.prl`, once vsync-on and once vsync-off, driving continuous client movement, a mouse-look sweep, and a ~10-tap jump test. Confirm the Task-1 ACs from the host log. Then read the residual, and record a written go/no-go: **Fix B is triggered** iff, after Fix A, the host still steps the client pawn's orientation — operationally, host `distinct_yaw` during a continuous client turn stays well below the client's `input_sends` rate despite `real ≈ 60/60`. **Fix C is triggered** iff the sending client's render rate is below 60 fps and host-observed orientation tracks that render rate rather than 60 Hz (compare client render fps against host `distinct_yaw` during a turn). If neither triggers, Fix B and Fix C drop from scope and the spec closes at Task 1. This task is the spec's falsification step: it decides whether Tasks 3–4 exist.

### Task 3 (conditional on Task 2): Host-side remote-pawn delay presentation

Only if Task 2 triggers Fix B. Give the host the delay-buffered presentation the client already uses. Record each connected client pawn's authoritative `Transform` per fixed tick into a `RemoteInterpolationBuffer` (`crates/postretro/src/netcode/interpolation.rs`) keyed by the pawn, then once per host render frame sample it at a delayed target tick and write the result via `EntityRegistry::set_presentation_transform` (`crates/entities/src/registry.rs:1349`) — mirroring `client_sample_interpolation` (`netcode/mod.rs:1303`) but clocked off the host's own authoritative tick (`current_tick − delay`) rather than a `ClientTimeSync` estimate, since the host is the clock. The host presentation path today reads the live registry with a single-tick slerp (`interpolated_transform`, used at `scripting/systems/mesh_render.rs:309`); the buffered pose must feed that same presentation read via `set_presentation_transform`, which seeds `previous_transforms` so any-alpha blends reproduce it. Gate the new path so it runs only on the `NetEndpoint::Host` endpoint for pawns the host does not treat as the local player (parallel to the client-only gate at `main.rs:6533`). Plumbing: the host owns no `RemoteInterpolationBuffer` today — add one to the host endpoint state in `netcode/endpoint.rs`, written from the tick loop after `run_host_movement_tick` and read from the per-frame presentation stage. New logic goes in a new `netcode` submodule, not appended to `main.rs` or `interpolation.rs`.

### Task 4 (conditional on Task 2): Orientation sampling cadence

Only if Task 2 triggers Fix C. Decouple the host-observed client orientation from the sending client's render rate. The client samples `facing_yaw`/`aim_pitch` once per render frame (`main.rs:2411` camera update, `:1166` capture), per the `input.md` §3 contract. Options to weigh at implementation: re-sample the camera yaw per fixed tick before `build_sim_command`, or interpolate viewangles across the ticks of a multi-tick frame. Either diverges from the documented render-rate-look contract; this task must state the divergence in `input.md` and argue it, or, if the contract wins, record that the residual is accepted and close Fix C without code. Do not silently change the contract.

## Sequencing

**Phase 1 (sequential):** Task 1 — the fix and its own ACs; blocks everything.
**Phase 2 (sequential):** Task 2 — re-measurement gate; consumes Task 1 and decides whether Phase 3 exists. This is the falsification step, deliberately placed before any presentation work.
**Phase 3 (concurrent, conditional):** Task 3, Task 4 — independent of each other; each runs only if Task 2 triggered it. Task 3 touches `netcode` presentation state; Task 4 touches the input/look path; no shared files.

## Rough sketch

Fix A centers on the gap branch of `resolve_tick` (`command_queue.rs:466–491`). Today both the exact-hit path (`:438–464`) and the gap path (`:466–504`) end with `resolved_cursor = Some(expected)` + `drop_stale(expected)`. The change makes cursor advancement conditional: advance on a `Real` hit, or on a `Neutral` give-up after `INPUT_HOLD_TICKS`, but not on a within-grace `Held`. A small helper (e.g. `playout_ready(&self) -> bool` on `ClientCommandState`, comparing `pending` depth / `latest_observed_reload.0` against `INPUT_BUFFER_TARGET`) governs the initial/underrun buildup. `held_ticks` already tracks the grace count; its reset points move to the give-up and Real paths. No new constants required — reuse `INPUT_HOLD_TICKS`, `INPUT_BUFFER_TARGET`, `INPUT_BUFFER_MAX`. Verification uses the existing `HostQueueDiag` counters.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| The resolve cursor never advances past a tick whose command has not arrived, within the hold grace | Task 1 (gap-path cursor hold) | Threatened wherever `resolved_cursor`/`drop_stale` are written; the exact-hit and give-up paths still advance | AC 1, AC 4 |
| A genuinely absent client cannot stall the host: the cursor advances (Neutral) after `INPUT_HOLD_TICKS` | Task 1 (give-up path) | Threatened by (a) if the give-up path is missed | AC 5 |
| `resolved_cursor` = `last_processed_client_tick` trails newest-received by a bounded playout margin (~`INPUT_BUFFER_TARGET`), never unbounded | Task 1 (playout depth) | Reconcile reads it via `netcode/replication.rs:142`; unbounded lag would grow the client replay tail without limit | AC 3 |
| Reload rising edge delivered exactly once under late/duplicate/out-of-order arrival | existing reload lane; re-threaded by Task 1 (a) | Threatened by moving cursor-advance/`preserve_due_reload_press` off the hold path | AC 6 |
| All cursor/stale/hold comparisons wrap-aware across the `u32` `client_tick` boundary | existing `client_tick_le`; preserved by Task 1 | Threatened by any new arithmetic on `expected`/margin | AC 7 |

## Orderings

| Scenario | Ordering | Expected outcome |
|---|---|---|
| Command on time | Input(T) buffered before tick T resolves | `Real` at T |
| Command slightly late (the bug) | tick T resolves, then Input(T) lands ≤ `INPUT_HOLD_TICKS` later | `Held` without advancing, then `Real` at T when it lands |
| Command never arrives | Input(T) absent > `INPUT_HOLD_TICKS` | `Held`×`INPUT_HOLD_TICKS` then `Neutral`, cursor advances past T |
| Backlog burst | `pending.len() > INPUT_BUFFER_MAX` | existing catch-up trim to `INPUT_BUFFER_TARGET`, unchanged |
| Fresh / post-underrun stream | `pending` below playout floor | hold buildup to ~`INPUT_BUFFER_TARGET` before first `Real`, no advance past un-arrived ticks |
| Reload press on a late/held command | reload rising edge on a tick resolved `Held`-without-advance | edge preserved in the recovery lane; delivered once when its tick advances |
| Multiple fixed ticks in one frame | ingest once per frame, `resolve_tick` N times | playout depth absorbs the burst; steady `Real` |
| `client_tick` wraps mid-session | newest-received wraps past `u32::MAX` | wrap-aware comparisons resolve without a spurious flush |

## Open questions

- **Exact playout margin.** `INPUT_BUFFER_TARGET = 2` (~33 ms) is the natural reuse and matches the observed ~1-tick phase offset with headroom. If Task 2 shows residual `Neutral` under normal jitter, the margin may need to be 2–3; if input latency feels heavy, 1–2. Owner tuning call, bounded by the AC that steady-state `neutral ≈ 0`.
- **Fix B host clock source.** Task 3 assumes the host presents remotes at `current_tick − delay` off its own authoritative tick. If a wall-clock playout (as the client uses) turns out cleaner for cross-frame smoothness at high host render rates, that is an implementation choice for Task 3 — recorded here so it is decided, not discovered.
- **Does Fix A alone close everything?** The evidence says Fix A removes the dominant cause of all three symptoms, including rotation. Tasks 3–4 exist only if Task 2's measurement disproves that. This is deliberate: the spec does not pre-commit presentation work the data has not yet justified.

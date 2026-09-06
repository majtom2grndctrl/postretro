# movement--state-transition-feel — plan of record

status: approved
read at: 04970869 (source base b53eeccb3)

## Corrections

- `content/dev/scripts/player.ts` had no `slide` descriptor → it already enables and tunes `slide`; the first slice starts at tick-edge emission, preserves the existing playable slide, and adds the requested reaction proof instead of re-enabling it.
- `view_feel.rs` is now 1,065 lines (mostly its co-located tests), while `main.rs` is 13,056 lines → split the evaluator by its existing bob/tilt/sway responsibility seam before adding impulse behavior. The render call site itself is a narrow coordinator around state already owned by `App`; no self-contained extraction seam exists there without moving unrelated render-loop dependencies.
- `sim::host_movement::run_host_movement_tick` still collapses all pawns into `Vec<&'static str>` → change it to return pawn-identified `MovementEvents`, then have each caller select its own local pawn before mapping state edges and landing/jump flags to addresses.
- `MIN_FOV_DEG`/`MAX_FOV_DEG` remain private to `capture::scene`, and capture bypasses `RenderCamera::new` because it always uses `HFOV` → relocate the common FOV band beside `HFOV`, keep capture's independent scene FOV, and update its now-stale comment.
- A slide's `natural_exit` can reach `Crouching` on blocked headroom as well as the exits enumerated in the brief → cover every natural-exit branch in the edge-order matrix, not only the listed examples.
- `ViewFeelState` has no followed-pawn identity and lifecycle only resets the existing flash/vignette/shake effects → Task 3 must invalidate impulse state on a followed-pawn change and descriptor hot reload, in addition to level install.

## Review findings — resolved

The brief names tilt's fixed under-damped `0.8` damping ratio as the impulse spring model, but its acceptance requires monotonic decay. An under-damped spring necessarily overshoots for some impulses, so these contracts cannot both be proven.

- **Owner decision:** retain the current monotonic-decay acceptance criterion and use critical damping for impulse springs. This deliberately departs from tilt's fixed `0.8` under-damped ratio; the author-facing descriptor remains unchanged, and `tension` is an intuitive settle-speed control with no rebound.

The catch-up AC also needs a fixture-qualified proof: signed, opposing impulses cannot be universally "strictly less" after aging. Proposed wording: "A frame draining a full catch-up backlog produces every event. For a positive, same-sign fixture, each edge's age attenuates the presented offset below an otherwise identical unaged batch; the separate-frame equivalence is proved independently."

## Delegated answers

- None; the brief has no open questions.

## AC-to-proof

| AC | Proof | Status |
|---|---|---|
| `Normal` → `Sliding` fires `slide_started` once; every named slide exit fires `slide_ended` once | focused movement transition-exit tests | achievable as stated |
| Slide → crouch orders `slide_ended`, then `crouch_started`; slide → normal has only its exit | focused movement edge-order tests | achievable as stated |
| Slide exit plus target entry applies both impulses in one frame | `view_feel::impulse_transition_edges_sum` | achievable as stated |
| Dash/crouch edges fire only on transitions; a sustained slide is silent | focused movement edge tests | achievable as stated |
| A guest's slide/land/jump never reaches the host registry; remote observing clients have no movement-tick source | `sim::host_movement` local-pawn filtering test; source inspection | achievable as stated |
| Forward client prediction fires local movement edges once; reconcile replay is silent; wire/digest versions stay unchanged | prediction/reconcile focused tests; source inspection | achievable as stated |
| Hot reload during a slide emits no exit and leaves no orphan impulse | refresh + impulse reset focused test | achievable as stated |
| An authoritative state correction emits no edge; later locally-ticked exit can be unpaired | reconcile focused test | achievable as stated |
| Consecutive tick edges survive one render frame | `view_feel::impulse_consecutive_edges_are_not_coalesced` | achievable as stated |
| Catch-up backlog drains all events and age-attenuates its rendered impulse | `view_feel::impulse_backlog_edges_age_before_sum` with a positive, same-sign fixture | achievable as stated |
| Presented per-channel offset never exceeds authored `max` | `view_feel::impulse_output_is_clamped_per_channel` | achievable as stated |
| Level load or pawn change clears any impulse | lifecycle/reset focused test | achievable as stated |
| Present slide entry applies FOV/pitch/roll on its first render frame; absent entry is zero | `view_feel::slide_entry_impulse_is_optional` | achievable as stated |
| Present slide exit applies its authored FOV/pitch/roll; absent exit is zero | `view_feel::slide_exit_impulse_is_optional` | achievable as stated |
| Dash and crouch use the identical state-generic impulse mechanism | `view_feel::dash_and_crouch_impulses_use_state_keys` | achievable as stated |
| Impulse affects only FOV, pitch and roll, never eye position | view-feel/camera composition test | achievable as stated |
| State tension overrides the default and simultaneous state springs settle independently | `view_feel::impulse_state_tension_overrides_default` | achievable as stated |
| Springs decay monotonically and a higher tension settles faster | `view_feel::impulse_spring_decay_is_monotonic` | achievable as stated |
| Spring state is frame-rate independent and the ceiling is presentation-only | `view_feel::impulse_spring_is_frame_rate_independent` | achievable as stated |
| A later edge adds to, rather than replaces, an in-flight spring | `view_feel::impulse_edges_accumulate` | achievable as stated |
| Zero frame dt holds an idle integrator but still applies a pending displacement | `view_feel::impulse_zero_dt_applies_pending_edge` | achievable as stated |
| `view_feel_scale = 0` zeroes output while integration continues | `view_feel::impulse_respects_accessibility_scale` | achievable as stated |
| Zero FOV offset preserves the old projection bit-for-bit | `camera::zero_fov_offset_preserves_projection` | achievable as stated |
| Nonzero FOV widens the frustum and clamps to the authored band | `camera::fov_offset_widens_and_clamps_projection` | achievable as stated |
| Viewmodel inherits pitch/roll but its projection excludes FOV | viewmodel transform test; source inspection | achievable as stated |
| JS and Luau parsers enforce all impulse shapes/ranges and sparse inheritance | scripting-core movement descriptor tests | achievable as stated |
| Generated TS/Luau SDK types contain the impulse surface | `committed_sdk_types_match_current_registry` | achievable as stated |
| Scripting reference lists every 21 reserved engine-fired address | documentation inspection | achievable as stated |
| Dev player retains slide, authors impulse, and a dev level manifest registers the sample reactions | scripts build/load test and in-engine check | achievable as stated |
| Slide entry is visually distinct from crouch | user, in-engine | manual-visual |
| Slide exit springs through recovery instead of cutting | user, in-engine | manual-visual |
| Slide → jump → slide remains rhythmically legible | user, in-engine | manual-visual |
| A stall-recovery backlog is not disorienting | user, in-engine | manual-visual |
| Scale 0.5 halves the effect; scale 0 matches pre-impulse slide | user, in-engine | manual-visual |

## Tasks

| # | Task | Status |
|---|---|---|
| 1 | Add a payload-free movement-state discriminant and ordered per-tick edge list at the tick transition write; extend `MovementEvents`, preserve landing/jump behavior, return pawn-identified events from the host seam, and map only the local pawn's events to reaction addresses. Thread forward-prediction events into the same local drain while keeping reconciliation replay silent. Prove every natural slide-exit branch, same-tick exit/entry ordering, dash/crouch edges, guest filtering, correction/hot-reload silence, and no wire/digest changes. | done · 259963e5 · `CARGO_PROFILE_TEST_SPLIT_DEBUGINFO=off cargo test -p postretro --bin postretro movement::tests:: -- --nocapture`; `CARGO_PROFILE_TEST_SPLIT_DEBUGINFO=off cargo test -p postretro forward_prediction_cannot_reach_registry_driven_systems -- --nocapture`; `CARGO_PROFILE_TEST_SPLIT_DEBUGINFO=off cargo check -p postretro` |
| 2 | Split `view_feel.rs` behavior-preservingly into evaluator-owned modules (shared state/output and bob, tilt, sway calculations); retain its public-in-crate API and establish focused parity tests before adding impulses. | done · 61bec157 · `CARGO_PROFILE_TEST_SPLIT_DEBUGINFO=off cargo test -p postretro --bin postretro view_feel::tests:: -- --nocapture`; `CARGO_PROFILE_TEST_SPLIT_DEBUGINFO=off cargo check -p postretro` |
| 3 | Add the optional `viewFeel.impulse` descriptor across foundation, JS, Luau, primitive registry, typedefs, SDK fixtures, and generated files. Implement one bounded linear spring per closed state key, age queued edges before summing, preserve zero-scale integration, and invalidate it on level install, followed-pawn identity change, and descriptor hot reload. Drain the frame's ordered edge list before view-feel evaluation, compose pitch/roll into the existing camera/viewmodel path, and prove all spring, scale, absence, and reset contracts. | |
| 4 | Add FOV impulse to `RenderCamera::new`: relocate the shared 60°–130° FOV band from capture, clamp the final FOV, keep zero-offset projection bit-identical, and leave the dedicated viewmodel projection untouched. Update camera/capture callers and tests for culling-compatible projection behavior. | |
| 5 | Add dev impulse tuning and a manifest-registered reaction example, publish the complete reserved-address list, run scripts/type generation and focused integration checks, then perform the required in-engine visual trial at scales 1.0, 0.5, and 0.0. | |

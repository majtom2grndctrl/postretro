# E10 — Speed-Scaled Walk Playback (clip rate follows ground speed)

> **Conditional — entry gate.** Build after `E10--enemy-steering-feel` lands, and only if its acceleration/deceleration ramps make the fixed-rate walk clip visibly foot-slide on the movement-feel fixture (the follow-up named in `E10--enemy-locomotion-animation` § Open questions). The gate is a playtest observation, not a metric; this spec exists so firing it is a promotion, not a design session.
>
> **Composes with:** `E10--enemy-locomotion-animation` (its locomotion intent already carries the measured XZ speed to the animation site — this spec consumes that value) and `E10--enemy-steering-feel` (the source of the speed ramps that expose the problem).

## Goal

Scale the walk clip's playback rate from the agent's measured ground speed so foot cadence tracks the floor during acceleration, deceleration, and arrival easing — removing the treadmill-slide that a fixed-rate loop shows under ramped speeds.

## Background (the time model)

- **There is no playback-rate concept in the runtime.** Clip-local time is a pure function of a single global animation clock: `state_time` (`crates/postretro/src/scripting/systems/mesh_anim.rs:148`) computes `elapsed = anim_time − entered_at (+ phase for loops)`. No dt is integrated in the animation module; the only rate knob is the global slow-mo clock multiplier. A per-entity rate is net-new capability.
- **Animation state is a component.** `MeshAnimation` (`crates/entities/src/components/mesh.rs:112`) on `MeshComponent` carries `states`, `current_state`, `entered_at`, crossfade bookkeeping. The state machine (`switch_animation_state`, `SwitchResult` — `mesh.rs:320`, `mesh.rs:286`) lives there; per-frame sampling lives in `mesh_anim.rs` via `animate_entity` (`mesh_anim.rs:195`), called from the render-frame collector (`crates/postretro/src/scripting/systems/mesh_render.rs`).
- **Order:** sim tick (AI switches states, steering moves the agent) → render frame (resolve pending stamps, sample poses). The measured speed is produced in the sim tick and consumed at sampling time.
- **Time-slicing constraint.** Distant agents resample poses at stride 1/2/4 (`mesh_render.rs:29-66`). The current stateless time model makes skipped frames free — the next sample lands at the correct absolute time. Any rate mechanism must preserve that property; a naive per-frame accumulator advanced only on resampled frames would stall distant agents' phase and jump on resample.
- **Reference speed exists.** `AgentComponent::move_speed` is the speed at which the authored walk cycle reads correctly (the enemy's full chase speed); `rate = speed_xz / move_speed` needs no authored data.

## Scope

### In scope

- **Rate computation.** Per agent, per sim tick: `rate = clamp(speed_xz / move_speed, RATE_MIN, RATE_MAX)`, from the same resolved XZ speed the locomotion-intent read already produces (`agent_steering::path_state(..).velocity`; `AgentComponent::move_speed` read at the same site). Applies only while the current animation state is the looping locomotion (walk) state; all other states run at rate 1.
- **Rebased scaled time (the design).** Store `(rate, rebase_time, rebase_elapsed)` on `MeshAnimation` (runtime-only, `#[serde(skip)]`). When the sim tick updates the rate, rebase: `rebase_elapsed += (now − rebase_time) × old_rate; rebase_time = now`. At sampling, the walk state's clip-local time is `rebase_elapsed + (anim_time − rebase_time) × rate` — continuous across rate changes by construction, a pure function of the clock between changes, and therefore stride-safe with **no per-frame mutation** in the render collector (the collector borrows the registry immutably; all writes stay in the sim tick).
- **Clamps.** `RATE_MIN`/`RATE_MAX` module constants (e.g. 0.5–1.5): below-epsilon speeds never reach this path (the locomotion intent selects idle there); the clamp guards mid-band distortion.
- **Crossfades unscaled.** Fade weights keep real-clock timing (`active_fade`, `mesh_anim.rs:227`); only the walk state's sampled clip time scales.
- **Pre-split.** `mesh.rs` is 1294 lines — over the split threshold. Behavior-preserving extraction of the animation state machine (`MeshAnimation`, `AnimationState`, switch/restart/resolve fns) into its own module before extending it.

### Out of scope

- Authored rate fields, reference-speed overrides, or rate curves in descriptors — the typedef is codegen'd (`gen-script-types`); promote an authored surface only if modders ask, as its own boundary-inventoried follow-up.
- Walk/run blend trees, directional locomotion blends, additional clips.
- Scaling attack, death, or idle states; player/viewmodel animation.
- Changing the time-slicing strides or forcing resamples on rate change — continuity of the scaled-time function makes that unnecessary.

## Acceptance criteria

- [ ] An agent moving at its full `move_speed` samples the walk clip at the authored rate (rate = 1 within float tolerance); at half speed the clip-local time advances at half rate (runnable unit test on the rate function and the rebased-time evaluation over a simulated clock).
- [ ] A step change in speed produces no discontinuity in clip-local time: evaluating scaled time immediately before and after a rebase yields the same value, and the sampled pose phase is monotone through an accel→decel ramp (runnable unit test on the rebase math).
- [ ] Rate is clamped to the configured min/max at extreme speeds (runnable unit test at near-zero and above-`move_speed` inputs).
- [ ] Sampling at arbitrary clock times — including stride patterns that skip 2 and 4 frames — matches continuous evaluation of the same rebased function (runnable unit test evaluating the scaled-time function at strided vs. dense time points).
- [ ] Crossfade timing is unchanged: fade weight for a walk↔idle transition under a scaled walk state progresses on the real clock (existing crossfade tests remain green; one new test with an active rate ≠ 1).
- [ ] Non-walk states are byte-identical in behavior: attack/death/idle sampling paths untouched (existing `mesh_anim` and FSM animation tests remain green).
- [ ] Pre-split lands behavior-preserving: all existing `mesh.rs` and animation tests green before the rate change begins.

## Tasks

### Task 1: Pre-split mesh.rs animation half
Behavior-preserving extraction: move `MeshAnimation`, `AnimationState`, `AnimStamp`, `SwitchResult`/`switch_animation_state`, `RestartResult`/`restart_animation_clip`, and `resolve_pending_animation_stamps` from `crates/entities/src/components/mesh.rs` (1294 lines) into a sibling animation module, re-exporting so call sites (`ai.rs`, `mesh_anim.rs`, `mesh_render.rs`) compile unchanged or with import-path-only edits. All tests green; no logic changes.

### Task 2: Rebased scaled-time substrate
Add the runtime-only `(rate, rebase_time, rebase_elapsed)` fields to `MeshAnimation` (seeded rate 1, `#[serde(skip)]`), a rebase method, and a scaled-elapsed evaluation used by the walk state's `state_time` path in `mesh_anim.rs`. Reset the rebase state on state entry (`switch_animation_state`) so a new state starts at elapsed 0 regardless of prior rate. Unit tests: rebase continuity, clamped rates, stride-vs-dense equivalence, crossfade-unscaled.

### Task 3: Sim-tick rate update
In the sim tick, after steering resolves (so the tick's final velocity is read), update each brain-driven agent's animation rate: compute `clamp(speed_xz / move_speed, RATE_MIN, RATE_MAX)` from `agent_steering::path_state` velocity and `AgentComponent::move_speed`, and rebase the entity's `MeshAnimation` when the rate changes beyond a small epsilon (avoids rebasing every tick on noise). Applies only when `current_state` is the locomotion state; otherwise the rate rests at 1. Constants live beside the module's existing tuning constants.

## Sequencing

**Phase 1 (sequential):** Task 1 — file split; Tasks 2–3 touch the extracted module.
**Phase 2 (sequential):** Task 2 — the substrate Task 3 writes to.
**Phase 3 (sequential):** Task 3 — consumes Task 2's rebase method and the steering-resolved velocity.

## Open questions

- **Rebase epsilon.** Rebasing on every tick is correct but noisy; a small rate-delta epsilon (e.g. 0.02) trades imperceptible cadence error for fewer writes. Tune during implementation; both are correct by the continuity AC.
- **Walk-state identification.** "The looping locomotion state" is today exactly the alert-mapped walk state from the reference descriptor. If future archetypes name multiple locomotion states, identifying them may need a state-table flag — deferred until such an archetype exists.

# M10 — Enemy Locomotion Animation (velocity-driven idle/walk)

> **Wave:** M10 enemy-AI follow-up (refinement of the shipped `M10--enemy-ai-behavior` foundation — "a foundation to refine, not a stub"). Surfaced in manual play-testing.
>
> **Builds on:** the enemy-facing + attack-animation-replay blocks that have landed in `run_ai_tick` (ai.rs ~519-592); this spec layers locomotion selection over the same animation region.

## Goal

Make the enemy play its idle pose when it is standing still and its walk clip only when it is actually moving, instead of walking-in-place. Today the animation is selected purely from the logical FSM state, so an `Alert` enemy shows the looping walk clip even when stationary. Select idle-vs-walk from the agent's real motion, layered over the logical state.

## Background (the bug)

The FSM requests an animation only on a logical-state change, mapping each state to one fixed clip:

- `crates/postretro/src/scripting/systems/ai.rs` (~line 598): the apply pass calls `switch_animation_state(registry, id, name)` **only when `outcome.state_changed`**, with `name = brain.tuning.states.animation_for(brain.state)`.
- `AiStateMap::animation_for` (`scripting/components/brain.rs:79`) is a pure `LogicalState → &str` lookup; the agent's live `velocity` is never consulted.
- The reference descriptor maps `alert → "walk"` (`sdk/behaviors/reference/entities.ts:91`), and `walk` is a **looping** clip (`Walking_A`, `loop: true`, line 62). So whenever the brain is in `LogicalState::Alert` the looping walk clip plays — including when the agent has arrived, is blocked, or is holding while the player loiters in range.
- Compounding tuning: `leashRange: 24` (`entities.ts:84`) ≫ `detectionRange: 16` (`entities.ts:82`; line 83 is `attackRange: 2`), so the brain stays `Alert` (and thus "walking") for any player within 24 m, well past the 16 m at which it stops chasing meaningfully. The `.luau` twin mirrors these at a one-line offset (`detectionRange` at line 81, `leashRange` at line 83).

Clip names are **not** the cause: the KayKit glTF clips (`Idle`, `Walking_A`, `1H_Melee_Attack_Slice_Horizontal`, `Death_A`) match the descriptor exactly, so the idle switch succeeds when it is requested — it just is not requested while the enemy stands in `Alert`.

## Scope

### In scope

- A **locomotion-intent signal** derived from the agent's actual horizontal (XZ) speed, read through the existing steering read surface (`agent_steering::path_state(registry, id)` → `AgentPathState::velocity`). The landed facing block already reads `path_state` at ai.rs ~535, but that read is gated inside `if matches!(state, Alert|Attack)` and inside `if let Some(path)`, so the `path` binding is NOT in scope at the animation site (~598). `moving` (or the XZ speed) must be computed from a SINGLE `path_state` read hoisted above BOTH the facing block and the animation block — near the top of the per-outcome apply iteration — and reused by both. (The Chase arm's `path_state` call consumes only `blocked`, not velocity.) Compared against a small speed epsilon.
- **Future-safe ground-speed data.** Keep the horizontal speed value available at the animation-selection site, not only a buried bool. Current behavior derives `moving` from that speed and selects idle/walk. A later speed-scaled walk playback pass can map the same speed to clip rate without adding a second steering read or changing descriptor shape.
- **Idle-vs-walk selection layered over `LogicalState::Alert`:** in `Alert`, request the idle animation when stationary and the alert/walk animation when moving. `Idle`, `Attack`, and `Death` animation selection are unchanged (attack/death keep their mapped clips regardless of speed; idle is already idle).
- **Broaden the animation-switch trigger** from "logical state changed" to "logical state **or** locomotion intent changed," with a small per-brain latch so the switch fires once on a stop/resume and is not re-requested every tick (keeps the warn-once unresolved-clip path honest).
- **Resolve the `leashRange ≫ detectionRange` tuning** so the enemy de-aggros (returns to `Idle`) at a sensible distance rather than holding `Alert`/locomotion across a 24 m bubble. Pin `leashRange` to ~20 (detectionRange 16 + a 4 m hysteresis margin). Adjust the reference descriptor (`entities.ts` + the `.luau` twin in parity).

### Out of scope

- Enemy facing/orientation and attack-animation-replay — landing separately on this branch.
- Full locomotion animation blending: walk↔run blend trees, directional blends, multi-clip locomotion graphs.
- Implementing speed-scaled walk playback. This draft keeps the ground-speed data shape ready for it but ships a single idle/walk switch.
- Auto-returning to a neutral pose when a one-shot clip (attack/death) completes via `state_elapsed` — noted as a future hook (see Open questions), not built here.
- Steering-dynamics smoothing / movement feel — separate spec (`M10--enemy-steering-feel`).
- Any change to the FSM transition set or to damage/attack timing.

## Acceptance criteria

- [ ] An enemy in the `Alert` state whose agent horizontal speed is above the locomotion epsilon requests the walk (alert-mapped) animation; the same enemy with horizontal speed below the epsilon requests the idle-mapped animation — asserted on the FSM's selected animation-state name given a stubbed agent velocity, with no real multi-clip model required (runnable unit test).
- [ ] The animation apply path computes horizontal speed once, derives `moving` from that value, and leaves the speed available beside the selection helper. During a gradual acceleration ramp from `M10--enemy-steering-feel`, frames below the epsilon select idle and frames above it select walk from the same measured ground speed; no separate logical-state estimate can drift from actual movement (runnable helper test with below/near/above-epsilon speeds).
- [ ] A stop while remaining in `Alert` triggers exactly one walk→idle animation switch, and a resume triggers exactly one idle→walk switch; while locomotion intent is unchanged no further switch is requested (latch asserted over several ticks — the switch is not re-issued every tick).
- [ ] `Attack` and `Death` select their mapped animation (`attack`, `death`) regardless of agent speed; an `Idle` enemy keeps the idle animation — selection for the non-`Alert` states is unchanged (runnable unit test).
- [ ] A player that walks out to the resolved de-aggro distance returns the enemy to `Idle` (steering cleared), and a stationary in-range-but-not-chasing enemy shows the idle animation rather than the walk clip (transition + selection asserted via the steering read surface, not an internal call count). The existing FSM already performs the Alert→Idle de-aggro on `leash_range` at ai.rs:208 (`if evaluate_acquisition && distance > tuning.leash_range`), clearing steering; AC4 is therefore achieved by the `leashRange` descriptor literal change alone with no FSM edit (consistent with "no change to the FSM transition set" in Out of scope).
- [ ] Reference descriptor tuning updated in both `entities.ts` and the `.luau` twin with parity preserved; no typedef shape change (raw number only); the two literals are mirrored by hand with parity verified by inspection (no automated parity gate exists between the two files).

## Tasks

### Task 1: Locomotion Intent Helper
Hoist one `path_state` read in the AI apply pass, derive horizontal speed once, and build a small `LocomotionIntent` value. Add pure helper tests for below/near/above-epsilon speeds and non-`Alert` state selection.

### Task 2: Animation Switch Trigger + Latch
Broaden the animation switch condition from state-changed only to state-or-locomotion intent changed. Persist the latch after the animation block so stop/resume switches fire once and unresolved clips do not re-request every tick.

### Task 3: Reference Tuning
Set `leashRange` to 20 in the TypeScript and Luau reference descriptors, preserving parity and leaving typedef shape unchanged.

## Sequencing

**Phase 1 (sequential):** Task 1 — establishes the speed-derived intent surface.
**Phase 2 (sequential):** Task 2 — consumes Task 1's intent and persists the latch.
**Phase 3 (concurrent):** Task 3 — descriptor-only tuning; can land with either phase if merge coordination is simple.

## Rough sketch

- **Locomotion intent.** In `run_ai_tick`'s apply pass, reuse the `path_state(registry, id).velocity` XZ magnitude already read by the landed facing block (ai.rs ~535). REUSE `FACING_MOVE_SPEED_EPSILON: f32 = 0.05` (ai.rs:79) — promote/rename it to a shared `MOVE_SPEED_EPSILON` (0.05 m/s) so facing and locomotion animation agree by construction on what "moving" means; this rename also updates the existing squared comparison at ai.rs:538. Do NOT introduce a second `LOCOMOTION_SPEED_EPSILON`; a mismatched threshold would cause facing and animation to disagree. Use the SAME squared form as the existing facing comparison: `moving = vel_xz_sq > MOVE_SPEED_EPSILON * MOVE_SPEED_EPSILON` (no sqrt), so "agree by construction" is literal. Store the local as `LocomotionIntent { moving, speed_xz_sq: vel_xz_sq }`; only `moving` affects selection in this draft. Note the AI tick runs before the steering tick (`run_ai_tick` precedes `run_agent_tick`), so `velocity` reflects last tick's movement — a one-tick lag, imperceptible for animation.
- **Selection.** Replace the single `animation_for(state)` call with a small helper that returns the idle-mapped name when `state == Alert && !intent.moving`, else `animation_for(state)`. Keep it a pure function over `(LogicalState, LocomotionIntent, &AiStateMap)` so the AC tests drive it without the `App`.
- **Trigger + latch.** Add a small field to `BrainComponent` (e.g. `locomotion_moving: bool`, `#[serde(default)]`, seeded `false` at `from_descriptor`) recording the last-applied locomotion intent. EVERY animation request — both the existing `state_changed` branch AND the new locomotion-intent branch — writes `brain.locomotion_moving = moving` as part of the same apply step. Unify the two into ONE switch site: `if state_changed || moving != brain.locomotion_moving { resolve name via the helper; switch_animation_state(...); brain.locomotion_moving = moving; }`. This single call must share the existing warn-once `warned` set and `anim:{name}` key, and write the latch unconditionally after the switch call regardless of `SwitchResult` arm — including `UnknownState`/`NotAnimated` failure arms — so an unresolved clip does not re-request the switch every tick (this is what "keeps the warn-once unresolved-clip path honest" requires). This prevents a redundant re-switch on the tick after an Idle→Alert(moving) entry. The latch write (`brain.locomotion_moving = moving`) is written to the local owned brain; for the latch to persist, the brain must be re-persisted after the animation block via `registry.set_component(outcome.id, outcome.brain.clone())`. The current single persist at ai.rs:483 runs BEFORE the animation block and the unified switch site (~598) is the last block in the loop with no subsequent persist, so either relocate the ai.rs:483 persist to after the animation block, or add a second `set_component` call after the switch.
- **Tuning.** Set `leashRange` to 20 (detectionRange 16 + 4 m hysteresis margin). Mirror in the `.luau` twin. This is descriptor data only; no parser change.
- **Testability of the latch (AC2).** Factor the trigger as a small pure `fn should_switch(state_changed: bool, moving: bool, latch: bool) -> bool` plus the latch write, so AC2 ("exactly one switch on intent flip, none while unchanged") is unit-testable without the App, parallel to the pure name helper for AC1/AC3.
- **`state_elapsed` hook (not built):** `mesh_anim::state_elapsed` already reports `{ state, elapsed, complete }` for the current clip (both the `StateElapsed` struct and the `state_elapsed` fn carry `#[cfg_attr(not(test), allow(dead_code))]` — mesh_anim.rs:374/386). It is the natural future seam for "leave the attack/death one-shot pose when the clip completes"; left for a follow-up.

## Open questions

- **De-aggro distance.** `leashRange` is pinned to 20 (detectionRange 16 + 4 m hysteresis margin). Open question: whether the hysteresis margin should become a named descriptor field rather than two raw numbers.
- **Speed-scaled walk playback.** If steering acceleration makes the fixed-rate walk clip visibly slide during ramp-up/ramp-down, add a follow-up that scales walk playback from measured ground speed. That follow-up should use the speed value carried here; it should not introduce full blend trees unless walk/run content exists.
- **Between-swing attack pose.** With attack-animation-replay landing separately, the attack clip restarts each swing and otherwise holds its last frame between swings. Whether to neutralize that gap via the `state_elapsed` completion hook is a separate decision (kept out of scope here).

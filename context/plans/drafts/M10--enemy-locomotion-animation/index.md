# M10 — Enemy Locomotion Animation (velocity-driven idle/walk)

> **Wave:** M10 enemy-AI follow-up (refinement of the shipped `M10--enemy-ai-behavior` foundation — "a foundation to refine, not a stub"). Surfaced in manual play-testing.
>
> **Depends on:** the in-flight enemy-facing + attack-animation-replay fixes on the same branch (they touch the same animation block in `run_ai_tick`); sequence this after they land.

## Goal

Make the enemy play its idle pose when it is standing still and its walk clip only when it is actually moving, instead of walking-in-place. Today the animation is selected purely from the logical FSM state, so an `Alert` enemy shows the looping walk clip even when stationary. Select idle-vs-walk from the agent's real motion, layered over the logical state.

## Background (the bug)

The FSM requests an animation only on a logical-state change, mapping each state to one fixed clip:

- `crates/postretro/src/scripting/systems/ai.rs` (~line 484): the apply pass calls `switch_animation_state(registry, id, name)` **only when `outcome.state_changed`**, with `name = brain.tuning.states.animation_for(brain.state)`.
- `AiStateMap::animation_for` (`scripting/components/brain.rs:79`) is a pure `LogicalState → &str` lookup; the agent's live `velocity` is never consulted.
- The reference descriptor maps `alert → "walk"` (`sdk/behaviors/reference/entities.ts:91`), and `walk` is a **looping** clip (`Walking_A`, `loop: true`, line 62). So whenever the brain is in `LogicalState::Alert` the looping walk clip plays — including when the agent has arrived, is blocked, or is holding while the player loiters in range.
- Compounding tuning: `leashRange: 24` ≫ `detectionRange: 16` (`entities.ts:82-84`), so the brain stays `Alert` (and thus "walking") for any player within 24 m, well past the 16 m at which it stops chasing meaningfully.

Clip names are **not** the cause: the KayKit glTF clips (`Idle`, `Walking_A`, `1H_Melee_Attack_Slice_Horizontal`, `Death_A`) match the descriptor exactly, so the idle switch succeeds when it is requested — it just is not requested while the enemy stands in `Alert`.

## Scope

### In scope

- A **locomotion-intent signal** derived from the agent's actual horizontal (XZ) speed, read through the existing steering read surface (`agent_steering::path_state(registry, id)` → `AgentPathState::velocity`, already consumed in the apply pass's `Chase` arm), compared against a small speed epsilon.
- **Idle-vs-walk selection layered over `LogicalState::Alert`:** in `Alert`, request the idle animation when stationary and the alert/walk animation when moving. `Idle`, `Attack`, and `Death` animation selection are unchanged (attack/death keep their mapped clips regardless of speed; idle is already idle).
- **Broaden the animation-switch trigger** from "logical state changed" to "logical state **or** locomotion intent changed," with a small per-brain latch so the switch fires once on a stop/resume and is not re-requested every tick (keeps the warn-once unresolved-clip path honest).
- **Resolve the `leashRange ≫ detectionRange` tuning** so the enemy de-aggros (returns to `Idle`) at a sensible distance rather than holding `Alert`/locomotion across a 24 m bubble. Adjust the reference descriptor (`entities.ts` + the `.luau` twin in parity).

### Out of scope

- Enemy facing/orientation and attack-animation-replay — landing separately on this branch.
- Locomotion animation **blending** (speed-scaled playback rate, walk↔run blend trees) — single idle/walk switch only.
- Auto-returning to a neutral pose when a one-shot clip (attack/death) completes via `state_elapsed` — noted as a future hook (see Open questions), not built here.
- Steering-dynamics smoothing / movement feel — separate spec (`M10--enemy-steering-feel`).
- Any change to the FSM transition set or to damage/attack timing.

## Acceptance criteria

- [ ] An enemy in the `Alert` state whose agent horizontal speed is above the locomotion epsilon requests the walk (alert-mapped) animation; the same enemy with horizontal speed below the epsilon requests the idle-mapped animation — asserted on the FSM's selected animation-state name given a stubbed agent velocity, with no real multi-clip model required (runnable unit test).
- [ ] A stop while remaining in `Alert` triggers exactly one walk→idle animation switch, and a resume triggers exactly one idle→walk switch; while locomotion intent is unchanged no further switch is requested (latch asserted over several ticks — the switch is not re-issued every tick).
- [ ] `Attack` and `Death` select their mapped animation (`attack`, `death`) regardless of agent speed; an `Idle` enemy keeps the idle animation — selection for the non-`Alert` states is unchanged (runnable unit test).
- [ ] A player that walks out to the resolved de-aggro distance returns the enemy to `Idle` (steering cleared), and a stationary in-range-but-not-chasing enemy shows the idle animation rather than the walk clip (transition + selection asserted via the steering read surface, not an internal call count).
- [ ] Reference descriptor tuning updated in both `entities.ts` and the `.luau` twin with parity preserved; regenerated typedefs (if the shape changed) keep the drift test green.

## Rough sketch

- **Locomotion intent.** In `run_ai_tick`'s apply pass, after resolving steering, read `agent_steering::path_state(registry, outcome.id)` and take the XZ magnitude of `AgentPathState::velocity`. Define a module const (e.g. `LOCOMOTION_SPEED_EPSILON`, a small fraction of a walk speed — sized against `move_speed`, ~5–15%). `moving = xz_speed > LOCOMOTION_SPEED_EPSILON`. Note the AI tick runs before the steering tick (`run_ai_tick` precedes `run_agent_tick`), so `velocity` reflects last tick's movement — a one-tick lag, imperceptible for animation.
- **Selection.** Replace the single `animation_for(state)` call with a small helper that returns the idle-mapped name when `state == Alert && !moving`, else `animation_for(state)`. Keep it a pure function over `(LogicalState, moving, &AiStateMap)` so the AC tests drive it without the `App`.
- **Trigger + latch.** Add a small field to `BrainComponent` (e.g. `locomotion_moving: bool`, `#[serde(default)]`, seeded `false` at `from_descriptor`) recording the last-applied locomotion intent. Request the animation when `state_changed || moving != brain.locomotion_moving`; update the latch after a successful/attempted switch. This composes with the existing `state_changed` gate rather than replacing it.
- **Tuning.** Reduce `leashRange` toward `detectionRange` (a small hysteresis margin keeps it from flapping at the detection edge — e.g. leash ≈ detection + a few metres). Mirror in the `.luau` twin. This is descriptor data only; no parser change.
- **`state_elapsed` hook (not built):** `mesh_anim::state_elapsed` already reports `{ state, elapsed, complete }` for the current clip (currently `#[allow(dead_code)]` off-test). It is the natural future seam for "leave the attack/death one-shot pose when the clip completes"; left for a follow-up.

## Open questions

- **De-aggro distance.** Exact `leashRange` value and whether a detection/leash hysteresis margin should be a named descriptor field rather than two raw numbers — resolve during implementation/review against the tuning in `entities.ts`.
- **Between-swing attack pose.** With attack-animation-replay landing separately, the attack clip restarts each swing and otherwise holds its last frame between swings. Whether to neutralize that gap via the `state_elapsed` completion hook is a separate decision (kept out of scope here).

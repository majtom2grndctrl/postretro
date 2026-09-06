# movement--state-transition-feel

Brief · Epic 11 · reads: `context/lib/movement.md` §1 §2 §3 §4, `context/lib/scripting.md` §12, `context/lib/networking.md` §"Presentation events vs. replicated state", `context/lib/player_options.md` §5 · read at 2a9128d

## Problem

The project owner, playing the dev build, cannot tell when a slide is happening. That observation is over-determined and its cheaper half is a content gap: `content/dev/scripts/player.ts` is the repo's only movement descriptor and authors no `slide` block, `try_enter_slide` opens on `component.slide.as_ref()?`, so `MovementState::Sliding` has never once been entered. Enabling it is a precondition of this brief, not its subject. The cause that survives is structural and verified independent of slide ever running: every shipped presentation channel is continuous and velocity-driven — `view_feel::evaluate` takes a horizontal speed, a signed lateral speed and a grounded bool — and no engine surface emits or renders a movement *state transition*, so nothing marks the moment a state begins or ends, and slide's one sustained cue, the eye dropping to the crouched height, is the cue crouch owns. When this is done, a state change announces itself at both edges: the camera kicks and settles on entry and springs back on exit, and a mod script can hang a sound, a rumble, a screen shake or an emitter burst on the same two moments without the engine shipping a state-specific effect.

## Decisions

- **Two mechanisms, one for each side of the seam.** Tick-side, the movement tick emits named state-edge events scripts react to. Render-side, a transient camera displacement springs back to neutral. A script cannot drive a per-frame camera value (`ui.md` §6 makes per-frame script callbacks a non-goal), and an engine effect cannot author a mod's sound design.
- **One per-frame edge list feeds both consumers.** `movement::tick` records each transition it applies as a `(from, to)` pair; the frame collects them in order, the reaction drain maps them to addresses, and the render read sums them into the spring. A counter stamped on `PlayerMovementComponent` was the rival: it cannot carry two edges in one frame, it is written a third time by reconciliation's authoritative merge, it resets with a fresh pawn, and it lands on a `Serialize`/`PartialEq`-deriving component. The list is none of those things, and the reaction drain is already generic over what it iterates, so nothing shared with AI, weapon or death events changes.
- **Edges are emitted where `tick` writes the transition, not in the state intents.** `dispatch_state_intent` returns the next state and writes the component only on the no-transition path, so `tick`'s transition write is the sole edge; it takes the outgoing state by `mem::replace` and leaves a `Normal` placeholder, so `tick` snapshots `from` before dispatch runs. One site covers the whole vocabulary and gives wall-run and vault their edges for free — the reason cross-cutting-policies D6 put velocity carry at the transition layer.
- **The edge carries both endpoints, and both `<from>.exit` and `<to>.enter` fire.** `movement.md` §2 models a transition as `{ from, to, when, carry }`; an edge that knew only the state entered could not resolve `slide.exit` at any exit. Most slide exits enter `Crouching`, so an exit edge and the next entry edge routinely land in one frame and their impulses sum. Transition-specific tuning is not authorable in this vocabulary — a per-transition map would reach it, and is the extension point if authors ask.
- **Event addresses are per-state and payload-free**, extending the shipped `MovementEvents { landed, jumped }` seam. This resolves `movement--state-machine`'s deferral of `dashed` ("defer until a consumer exists") — the consumer is this brief.
- **The whole state vocabulary ships its impulse keys; the three non-baseline states ship addresses.** The emit site is state-generic, so `dash` and `crouch` cost nothing beyond their tuning values, and shipping only `slide` would leave the surface looking slide-shaped to the first modder who reads it. `normal` carries impulse keys so a recovery settle is authorable, but no address of its own — `normal_started` would fire on every dash, crouch and slide exit, which a modder already gets from the paired `<state>_ended`. Wall-run and vault then inherit both mechanisms without a spec.
- **Addresses stay bare and snake_case, and the engine-fired set is published as a reserved list.** The `ui.*` button actions are the codebase's one deliberate act about reserved names and they are namespaced, so the collision objection stands rather than fails to apply; it is overridden because all fifteen engine-fired addresses are bare, `scripting.md` §12 already calls `levelLoad` a reserved address in the flat space, and consistency inside the set a modder subscribes to outweighs consistency with a surface pointing the other way. Casing inside that set is mixed — seven multi-word addresses are snake_case and three are camelCase — and this brief follows the majority rather than inventing a fourth style. Publishing states the hazard rather than fixing it — `activate` and `impact` are words an author would pick, and no bind-time diagnostic is added.
- **All twenty-one engine-fired addresses are documented together**, not just the movement family. A partial list reads as complete and becomes a trap. `landed` and `jumped` ship today, are absent from `sdk/` and `docs/`, and are used by no script.
- **The impulse is a displacement that relaxes to zero, not a velocity kick.** It appears on the first render frame after its edge, including a frame with `frame_dt = 0`, which a velocity impulse could not do. Authored degrees are therefore peak offsets.
- **One spring per state key, with `tension` inherited from a character default.** Authors set `impulse.tension` once and override it on a state wanting a different settle — the sparse-override shape `player-descriptor-composition` gives its modifiers. A single shared spring could not carry per-state rates, because a `Sliding` → `Crouching` frame supplies two; the key set is closed and small, so one spring per key stays bounded and static. Their positions sum at read time and the ceiling clamps that sum, leaving every spring linear.
- **Impulses sum, bounded by an authored ceiling.** `shake_decay`'s latest-wins is the shipped precedent for a transient decaying effect, and this diverges from it: a chained transition should accumulate, because erasing the in-flight kick makes a chain read flat. The ceiling is authored, not an engine constant — `movement--slide` D10 rejected "a hidden engine ceiling" on flexibility-band grounds (§3), and a capped impulse would cap the same band. Only the renderer's own limit is engine-owned.
- **Each edge decays by its own age before it sums.** A frame draining a catch-up backlog holds edges the player never lived through, so advancing each edge's displacement and velocity by the age of its tick attenuates the backlog in proportion to staleness. Each spring is linear about a zero target and the ceiling clamps only the presented sum, so an aged edge lands exactly where separate frames would have put it. Dropping backlog edges instead would eat every second edge on a machine steadily running two ticks per frame, and scaling by tick count would make magnitude a function of frame rate.
- **Edges fire client-local from forward prediction; reconciliation replay stays silent.** `netcode::prediction::replay` already returns `MovementEvents` and both client sites discard it, so suppression on replay is structural rather than a rule to enforce. Forward prediction must start returning what it already computes. This also turns on `landed` and `jumped` for connected clients, which have never fired there.
- **Two writers change `movement_state` outside a transition, and neither emits an edge.** Reconciliation's authoritative merge writes it before the replay loop, and a descriptor hot reload forces `Normal` so no in-flight state survives the swap. A correction or a reload therefore ends a slide silently, and the edge it implies is dropped rather than fired — a mis-predicted cue is better lost than doubled.
- **Every movement edge is the local pawn's only — including `landed` and `jumped`, whose current fan-out is a bug this brief fixes.** The host runs `run_host_movement_tick` over remote pawns and collects every pawn's events into one identity-less list, so a guest's landing already fires the host's reactions. Both shipped addresses are undocumented and consumed by nothing, so scoping them costs nothing and avoids publishing twenty-one addresses with two behaviours. A mod wanting a partner's slide audible needs the Presentation channel (`E16--combat-presentation-substrate`), not this seam.
- **The camera impulse is a new channel on `view_feel`, engine-owned integrators, read-only on the pawn** (`movement.md` §1). Pitch and roll ride `ViewFeelOutput` to the existing chokepoint; FOV takes the camera parameter instead, so "one chokepoint" covers two of the three channels.
- **In-flight impulse does not cross a pawn change.** `ViewFeelState` is app-level and reset nowhere, while flash, vignette and shake decay already clear together at level install — the impulse joins that block. Without it a displacement that relaxes on its own would ride a level load into the next pawn's camera, which bob, tilt and sway never could. A respawn of the same pawn may keep it.
- **World FOV becomes a parameter of `camera::RenderCamera::new`, which today derives it from the `HFOV` const.** That is the single site in the player render stage producing `eye_position`, `view_matrix` and `view_projection`, so one signature change reaches the matrix, frustum culling and camera uniforms together. Clamping and unit conversion happen before the call, and a zero offset passes `HFOV` itself — the same expression on the same input, so bit-identity needs no matrix branch. That identity is an accessibility guarantee rather than a test convenience: at `view_feel_scale = 0` the offset is exactly zero.
- **The viewmodel's own projection does not follow the FOV punch.** E21 gave it a dedicated ~70° projection precisely so player FOV changes do not distort the weapon. A reader would expect the gun to inherit the kick because it already inherits bob, tilt and sway; it inherits the angular channels and not the FOV.
- **Accessibility is the shipped `PlayerOptions.view_feel_scale`.** The impulse multiplies by the same `global_scale`, is exactly zero at `scale = 0`, and keeps integrating at zero scale so re-enabling does not snap.
- **This brief adds no sustained per-state `viewFeel` tuning.** `player-descriptor-composition` owns that as `viewFeel.layers`, whose keys are input tiers plus states and are not this brief's key set; impulses are transient and additive, not overrides. That draft also forbids new top-level `viewFeel` keys, and `impulse` is one. It lands beside `base` and `layers` when the reshape arrives, and the reshape carries the migration row.
- **Not here:** the slide scrape loop and surface-dependent audio (script audio is one-shot; no `SoundHandle` reaches the VM and no runtime ground-material query exists — two subsystems, their own brief); speed lines and motion blur (no such post-process exists); script-spawned dust (engine-owned spawn is an architectural rule, `scripting.md` §10.1); a canted viewmodel pose, which is sustained-while-sliding and therefore layer-stack work.
- **No `player.stance` HUD slot.** A reader would expect one, since `GameStateRefs` exposes no movement state and this brief exists to make stance legible. The view is the readout here: a HUD label reporting the same fact would confound the manual-visual criteria that judge whether the camera work landed. Stance earns a slot once it gains a consequence the view cannot show — a sliding accuracy penalty, a crouch detection radius — and it publishes sim-side through `write_hud_slot`, so it is a separate integration regardless.
- **Camera height drop is already shipped and is not re-owned here.** Crouch D9 makes eye-drop and view-feel offsets independent additive contributions at the chokepoint, and slide carries its own `eye_current`. A third owner of the eye offset would have undefined ordering against that sum.

### Scripting surface

```typescript
// Entity descriptor — `content/dev/scripts/player.ts`.
viewFeel: {
  bob: { /* unchanged */ },
  tilt: { /* unchanged */ },
  impulse: {
    tension: 12.0,                              // character default, 1/sec
    max: { fov: 20.0, pitch: 8.0, roll: 6.0 },  // ceiling on the summed offset
    states: {
      slide:  { tension: 9.0,                   // sparse override
                enter: { fov: 8.0, pitch: -2.5, roll: 1.5 },
                exit:  { fov: -2.0, pitch: 1.5, roll: 0.0 } },
      dash:   { tension: 16.0, enter: { fov: 12.0, pitch: -1.0, roll: 0.0 } },
      normal: { enter: { fov: 0.0, pitch: 1.0, roll: 0.0 } },  // inherits 12.0
    },
  },
},
```

```typescript
// Level script — `defineReaction` is pure; the manifest `setupLevel` returns
// is what registers.
export function setupLevel() {
  const reactions = [
    defineReaction("slide_started", playSound("sfx/slide_start", "sfx")),
    defineReaction("slide_started", rumble(0.35, 180)),
    defineReaction("slide_ended", screenShake(6, 150)),
  ];
  return { reactions };
}
```

A present `impulse` requires `tension`, `max` and `states`. Each state key is optional and sparse: `tension` overrides the character default, `enter` and `exit` are each optional, and a present `enter`/`exit` requires all of `fov`/`pitch`/`roll`, where `0` means no kick on that channel. State keys are `normal`, `dash`, `crouch`, `slide`; `normal` has no addresses. Addresses are `dash_started`/`dash_ended`, `crouch_started`/`crouch_ended`, `slide_started`/`slide_ended`. Both reactions bound at `slide_started` fire.

## Acceptance

### Automated

- [ ] Entering `Sliding` from `Normal` emits `slide_started` exactly once, and leaving it emits `slide_ended` exactly once, for every exit slide has: jump, the committed-window exit on released crouch, the committed-window exit on speed decay, ledge handoff, and the `SLIDE_MAX_MS` backstop.
- [ ] Every slide exit that lands in `Crouching` — blocked-headroom jump, ledge handoff, and the committed-window exit with crouch held — emits `slide_ended` then `crouch_started`, in that order, from the single transition that tick applies. Exits that reach `Normal` emit `slide_ended` alone.
- [ ] A `Sliding` → `Crouching` transition applies `impulse.states.slide.exit` and `impulse.states.crouch.enter` in the same frame, summed. A `Sliding` → `Normal` transition applies `slide.exit` and `normal.enter`.
- [ ] `dash_started`/`dash_ended` and `crouch_started`/`crouch_ended` fire on their own edges; no edge event fires on a tick with no transition, including a sustained slide of many ticks.
- [ ] A guest pawn sliding, landing or jumping contributes no event — `slide_started`, `landed` or `jumped` — to the list `run_host_movement_tick` returns, so the host's reaction registry never sees it. The observing-client half is structural: remote pawns never run through the movement tick, so no edge can be derived for them — a source-inspection gate, not a test.
- [ ] On a connected co-op client, forward prediction fires each edge once locally, `landed` and `jumped` included; a reconciliation replay over the same ticks fires none. No edge appears in any snapshot, wire type or content digest, and `SNAPSHOT_VERSION`/`WIRE_VERSION` are unchanged — a source-inspection gate.
- [ ] A descriptor hot reload during a live slide fires no `slide_ended`, and leaves no impulse relaxing under a state that no longer exists.
- [ ] An authoritative correction that flips a client's predicted state into or out of `Sliding` fires no edge. A correction into `Sliding` — including a mid-slide join, whose first snapshot carries `Sliding` and no entry edge — later fires `slide_ended` alone; an unpaired exit edge is the accepted outcome, not a defect (pin O4).
- [ ] Two transitions on consecutive ticks spanned by one render frame produce both events and both impulses; neither is coalesced away (pin O1).
- [ ] A frame draining a full catch-up backlog produces every event, and an impulse whose summed offset is strictly less than the same edges arriving one per frame — each edge decays by the age of its tick before it sums (pin O5).
- [ ] The summed offset never exceeds `impulse.max` on any channel, however many edges arrive in one frame.
- [ ] A level load or pawn change leaves no in-flight impulse on the new pawn's first render frames (pin O3).
- [ ] With `impulse.states.slide.enter` authored, the first render frame after entry produces a nonzero FOV offset and nonzero pitch and roll; with `enter` absent, entry produces zero on all three.
- [ ] With `impulse.states.slide.exit` authored, the first render frame after the exit edge produces the authored `fov`, `pitch` and `roll`; with `exit` absent the exit produces zero on all three while `enter` still fires.
- [ ] `impulse.states.dash` and `impulse.states.crouch` drive the same channels as `slide` on their own edges, with the same present/absent behaviour — the surface is state-generic, not slide-shaped.
- [ ] The impulse contributes to `fov`, `pitch` and `roll` only. It adds no eye-position offset, so crouch's eye drop and slide's `eye_current` remain the sole owners of eye height.
- [ ] A state's `tension` overrides the character default for that state's spring; a state without one settles at the default rate, and two states with different `tension` settle at different rates in the same frame.
- [ ] The impulse decays monotonically toward zero after an edge and reaches within a small tolerance of zero given enough frames; a higher `tension` reaches a fixed fraction of the decay in fewer frames than a lower one.
- [ ] The impulse springs are frame-rate independent: two edges one tick apart converge to the same spring state whether they arrive in two frames or one (pin O1), and advancing to a fixed wall-clock time in many small `frame_dt` steps and in few large ones agrees within tolerance. The ceiling clamps the presented sum only, so it does not disturb either comparison.
- [ ] A second edge arriving mid-decay sums into the spring rather than replacing it.
- [ ] `frame_dt = 0` with no pending edge leaves the integrator unchanged; `frame_dt = 0` with a pending edge still applies the displacement, since only the spring advance is skipped.
- [ ] At `view_feel_scale = 0` the impulse output is exactly zero on all channels; advancing many frames at zero scale then restoring `1.0` yields the value an unscaled run would have reached at that frame, not the value from the frame the scale dropped. At `1.0` it is the unscaled value.
- [ ] `RenderCamera::new` with a zero FOV offset reproduces a pinned pre-change `view_projection` bit for bit.
- [ ] A nonzero FOV offset widens the frustum, and an offset beyond the band resolves to the clamp limit — compared as a tolerance on the FOV recovered from the projection, which adds no accessor.
- [ ] `viewmodel_camera_space_transform` still tracks the impulse's pitch and roll. The viewmodel's own projection takes no FOV argument and is unchanged by construction — a source-inspection gate.
- [ ] Parsers reject invalid `impulse` fields in JS and in Luau, with at least one Luau twin per rejection class: `tension` rejects missing/non-finite/non-positive; `fov`, `pitch` and `roll` reject missing/non-finite and accept negative and zero; `max` fields reject missing/non-finite/negative; a missing `tension`, `max` or `states` on a present `impulse` is rejected; a per-state `tension` is optional and inherits when absent.
- [ ] `committed_sdk_types_match_current_registry` passes with the impulse types in `sdk/types/postretro.d.ts` and `.d.luau`, and `impulse?` on `ViewFeelParams`.
- [ ] `docs/scripting-reference.md` carries one reserved-address list naming all twenty-one engine-fired addresses, `playerDied` and `enemyAttack` included. A documentation gate, checked by reading.
- [ ] `content/dev/scripts/player.ts` enables `slide` and authors `viewFeel.impulse`; a dev level script registers the Scripting surface reactions through its `setupLevel` manifest. Both load and play.

### Manual-visual

- [ ] Entering a slide reads as a distinct event, not as a fast crouch: the view punches wide and tips before settling.
- [ ] Exiting a slide reads as a recovery — the camera springs back through neutral rather than cutting.
- [ ] Chaining slide → jump → slide reads as a rhythm; the second entry's kick is legible over the first's decay.
- [ ] A frame that recovers from a stall does not punch the camera hard enough to disorient.
- [ ] At `view_feel_scale = 0.5` the effect is halved rather than switched off, and at `0` the slide is visually identical to today's.

## Path

Non-binding.

- Seams: `view_feel::evaluate` and `ViewFeelState`/`ViewFeelOutput`; `camera::RenderCamera::new` and the `HFOV` const; `MovementEvents` and its address mapping in `sim::host_movement`; the render-assembly pawn read in `main.rs`, which already borrows the whole `PlayerMovementComponent`; `netcode::prediction::replay` and `netcode::reconcile`, whose `_events` bindings are the client-local firing site.
- `MovementState` carries `f32` payloads and derives neither `Eq` nor `Hash`, so the edge needs a payload-free discriminant type that does not exist yet. The descriptor key set and the edge list both want it.
- Returning `Vec<(EntityId, MovementEvents)>` from `run_host_movement_tick` lets each caller scope its own pawns, which satisfies the local-pawn rule without a lookup inside the seam and yields the structured edge list as a side effect.
- Per-edge age decay reuses `advance_spring` unchanged: advance the edge from `(x0 = kick, v0 = 0)` by its tick's age, then add the aged position and velocity into its own state's spring, then advance every spring by `frame_dt`.
- Precedents: `tilt`'s spring and its fixed internal damping ratio are the impulse spring's model; `reload_started`/`reload_completed` are the address-naming model; crouch D9 is the composition rule at the chokepoint.
- Shape chosen: a state-keyed impulse map beside the shipped motions, driven by one per-frame edge list. The strongest rival is a counter stamped on the component, which needs no new list but cannot carry two edges in a frame and has four writers to reconcile.
- First slice: enable `slide` in `content/dev/scripts/player.ts` and emit the edge events. That falsifies the riskiest assumption — that the difficulty is a presentation gap and not simply that slide never entered — and is observable through `playSound` with no camera work. Report what it reads like before tuning the impulse magnitudes.
- `MIN_FOV_DEG`/`MAX_FOV_DEG` carry the right names and values but are private to the `capture`-gated module, so sharing them is a relocation beside `HFOV` with capture importing them back — not the no-op "reuse" it reads as. The capture driver keeps its own FOV, and its comment about `RenderCamera::new` always using `HFOV` goes stale.
- FOV is separately landable, and worth ordering after pitch and roll are playing: those two ride shipped plumbing, while FOV brings the constructor signature, the clamp band, the viewmodel exclusion and the capture-comment staleness.
- `view_feel.rs` is past the split threshold and `main.rs`'s render assembly far past it. Split `view_feel.rs` behavior-preservingly in its own commit before extending it.
- `E19--render-stack-decomposition` may relocate the render-assembly call site; passing the edge slice as a parameter to whatever it extracts keeps the drain-before-read order a signature rather than a coincidence.

## Boundary inventory

Impulse tuning crosses Rust ↔ wire (JS/Lua object) ↔ TS ↔ Luau. camelCase on every script-facing side; snake_case in Rust. No FGD KVP and no PRL section — the descriptor is a script object, never map-overridable (`movement.md` §7). Addresses are plain strings on both runtimes and appear in no typedef. Wire-casing mechanism mirrors `viewFeel`: author each key literally in the JS parser, the Luau parser, the `register_type().field(...)` chain, both `typedef/common.rs` type-name maps, and the `EXPECTED_TS`/`EXPECTED_LUAU` constants.

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| impulse sub-descriptor (optional) | `Option<ImpulseParams>` | optional object under `viewFeel` | `impulse?: ImpulseParams` | `impulse?` | n/a |
| summed-offset ceiling | `max: ImpulseChannels` | `max` | `max` | `max` | n/a |
| per-state block | `states: ImpulseStates` | `states` | `states` | `states` | n/a |
| one state's impulse (optional, ×4) | `normal`/`dash`/`crouch`/`slide`: `Option<ImpulseStateParams>` | same names | `normal?` … `slide?` | `normal?` … `slide?` | n/a |
| character spring rate (1/sec) | `tension: f32` | `tension` | `tension` | `tension` | n/a |
| per-state spring rate (optional) | `tension: Option<f32>` | `tension` | `tension?` | `tension?` | n/a |
| entry displacement | `enter: Option<ImpulseChannels>` | `enter` | `enter?` | `enter?` | n/a |
| exit displacement | `exit: Option<ImpulseChannels>` | `exit` | `exit?` | `exit?` | n/a |
| FOV channel (degrees, signed) | `fov: f32` | `fov` | `fov` | `fov` | n/a |
| pitch channel (degrees, signed) | `pitch: f32` | `pitch` | `pitch` | `pitch` | n/a |
| roll channel (degrees, signed) | `roll: f32` | `roll` | `roll` | `roll` | n/a |

Units: `tension` 1/sec (finite > 0) at both levels; `fov`/`pitch`/`roll` degrees (finite, signed, zero permitted) under `enter`/`exit`, and finite ≥ 0 under `max`. `states` carries one optional field per movement state rather than a map, matching how `viewFeel` models `bob`/`tilt`/`sway` — so an unrecognised key is ignored, as it is there.

## Open questions

None.

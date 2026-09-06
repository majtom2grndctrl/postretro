# movement--state-transition-feel — research

Source read at 2a9128d. Findings that informed the brief but do not decide it.

## Why the owner saw nothing: two independent gaps

1. **`slide` is not enabled anywhere in content.** `content/dev/scripts/player.ts` is the repo's only `PlayerMovementDescriptor` and authors `capsule`, `ground`, `air`, `fall`, `dash`, `crouch`, `viewFeel` — no `slide` block. `slide` is present-then-all-required, absent ⇒ disabled, so crouch-at-speed takes the `Normal` → `Crouching` path. Grep for `slide` across `content/` returns only an unrelated `.map` comment and a `@ts-expect-error` about UI sliders.
2. **No presentation channel keys off movement state.** `view_feel::evaluate(params, horizontal_speed, lateral_velocity, is_grounded, state, frame_dt, global_scale)` takes no `MovementState`. `main.rs` names `MovementState` exactly once, in an unrelated netcode comment.

Gap 1 is a one-line content fix and is why the first slice starts there. Gap 2 is the brief.

## Presentation-layer inventory, priced

| Layer | State | Evidence |
|---|---|---|
| Viewmodel | **Exists**, and already composes view-feel | `viewmodel_camera_space_transform(camera_right, eye_offset, roll, yaw, pitch, placement)`; E21 shipped it with a dedicated ~70° projection |
| Haptics | **Exists**, script-callable | `rumble(strong, durationMs, weak?)` → `Gamepad::rumble` via gilrs; undocumented in `input.md` |
| Screen effects | **Exists**, one-shot with engine-owned decay | `flashScreen`/`vignette`/`screenShake` → `FlashDecay`/`VignetteDecay`/`ShakeDecay`; three mod-readonly `screen.*` slots |
| HUD | **Exists**, but no movement state to bind | `GameStateRefs` covers input/player/screen/session/ui; no stance, speed or grounded slot. Slots publish sim-side through `write_hud_slot`, so a stance slot is a separate integration from this brief's render-side read, not a rider on it. `hud.reticle` is already its own always-on tree |
| Particles | **Partial** — modulate only | `setEmitterRate`/`setSpinRate` on placed emitters. `scripting.md` §10.1: "Scripts configure, Rust simulates… scripts never call spawn or despawn" |
| Audio | **Partial** — one-shot only | `playSound(sound, bus?)` is the whole script contract; `main.rs` hardcodes `looping: false` with the comment "no per-voice volume or looping yet (deferred)". `Audio::play`/`stop` and `SoundRequest.looping` exist in Rust; no `SoundHandle` escapes the VM. No pitch anywhere |
| Surface material | **Absent at runtime** | `Material` enum (`render-data`) is renderer-only; its own doc calls footsteps/impacts "future consumers". No collision or movement result carries a ground material |
| Motion blur / chromatic aberration | **Absent** | Zero hits across `.rs`, `.wgsl`, `.md` |

The ranking that produced the brief's scope: the state-edge events are worth more than any single effect, because `playSound`, `rumble`, `screenShake`, `vignette`, `flashScreen` and `setEmitterRate` all already ship — the events are the missing wire, not the missing effect.

## FOV: unclaimed, unpriced, and already half-documented

`pub const HFOV: f32 = 100.0 * PI / 180.0` (`camera.rs`), consumed at two sites inside the module. `E20--frame-capture` records that "`RenderCamera::new` derives vfov from the `HFOV` const and takes no FOV argument", and works around it with `capture_view_projection(camera, width, height)` reading `CameraPose.fov_deg` (default 100, clamped 60–130) — the only place FOV is a runtime value today. `rendering_pipeline.md` §11 documents FOV as "Configurable 60°–130°", a capability with no runtime path; the brief's clamp band adopts it rather than inventing one.

Nearest owner is the unbuilt Weapon Feel roadmap bullet: "**ADS** — an aim-down-sights view/FOV transition plus an accuracy modifier". ADS is a *sustained* transition; the impulse is transient. They would compose additively on the same parameter, which is the argument for putting FOV on `RenderCamera::new` rather than inside `view_feel`.

Known fallout of the signature change: frame-capture and PVS fixtures pin `horizontal_fov_degrees`, and `E19--visibility` inlines test-local `HFOV`/`NEAR`/`FAR` consts.

## Movement events: the shipped seam and its client-side hole

`MovementEvents { landed, jumped }` (`movement/mod.rs`) → `sim::host_movement::run_host_movement_tick` pushes `"landed"`/`"jumped"` into a `Vec<&'static str>` → `main.rs` drains `pending_movement_events` through `drain_named_events_with_sequences`. Both are undocumented: absent from `sdk/`, `docs/scripting-reference.md`, `context/lib/`, and unused by any script in `content/`.

`movement--state-machine` resolved: "do NOT add `dashed` to `MovementEvents` in this spec. Defer until a consumer exists" — and named the extension pattern (add a flag, extend the `main.rs` event-string mapping).

The client hole: `netcode::prediction::replay` returns `MovementEvents`, and both call sites — `prediction.rs` forward prediction and `reconcile.rs` replay — bind `_events` and discard. Only `tick_events.movement` from the host sim path reaches script. Without the brief's client-local decision, a co-op guest would get no cue.

The host hole, in the other direction: `sim` calls `run_host_movement_tick(&mut registry, …, &remote_pawn_inputs, tick_dt)` and appends the host's own pawn afterwards, while `host_movement.rs` pushes each pawn's events into one `Vec<&'static str>` carrying no pawn identity. So a guest's landing already fires `landed` in the *host's* reaction registry today. Nothing consumes it, which is why the bug is invisible; four more addresses on the same seam would make it audible.

Writers outside a transition: `reconcile.rs` calls `merge_wire_into_movement_state_checked` before the replay loop, and `refresh_plan.rs`'s `plan_movement_replace` forces `MovementState::Normal` on a descriptor hot reload so no in-flight `Dash` survives the swap. Both change the state with no edge to emit, and the hot reload is the one the owner meets while editing `player.ts` in the dev build.

## Relationship to `player-descriptor-composition`

That draft reshapes `viewFeel` into `base` + `layers`, deletes `groundedOnly`, and states that "new specs extend this shape instead of adding more top-level keys," with slide's layer landing with slide's own spec. Its Task 3 says the evaluator "additionally needs the active state and input tier — exposed the same read-only render-side way, never widening script access to the movement component."

One thing follows that the brief acts on: the render-side movement-state read is a shared prerequisite, so building it here removes work from that draft rather than duplicating it.

Two things do not. Its shipping layer keys are `walk | run | crouch | air | dash` — input tiers plus states, with `slide` deferred to slide's own spec — so they are not this brief's key set and the reshape gains a rename, not a no-op row. And `viewFeel.impulse` is exactly the new top-level key that draft forbids; it lands beside `base` and `layers`, and the reshape carries the migration row.

What stays that draft's: sustained per-state overrides — bob amplitude while sliding, a lower-frequency sway curve, tilt tension by stance — and the wieldable overlay that a canted gun pose belongs to. Its open question about layer-transition smoothing ("amplitude/frequency pop") is evidence the layer stack does not express transitions, which is the gap the impulse fills.

## Constraints the brief must not break

- `movement.md` §1: view feel "reads the followed pawn's velocity and grounded flag, writes nothing back… Its integrator state lives engine-side, never on the movement component or the per-tick interpolation state." Also: "A positional view-feel offset defines the effective render eye for the whole render stage: the view matrix, runtime cell lookup, portal-traversal apex, camera uniforms, and render diagnostics must all consume that same position." That clause binds the eye *position*, which FOV does not move — and the render assembly feeds one `view_projection` to both the image and `determine_visible_cells`, so a punch widens the culling frustum through the same matrix. No pop at the screen edge, and nothing to delegate.
- `movement.md` §7: movement tuning is descriptor-owned, never map-overridable. No FGD keys.
- `networking.md`: "cosmetics never enter a digest or block a join"; "a deterministic input to client prediction belongs in the hash… presentation is not."
- `player_options.md` §5: `view_feel_scale` multiplies **all** view-feel output; `0` zeroes all of it.
- `view_feel.rs` places its zero-scale short-circuit after the sub-evaluators on purpose — "the integrator must keep advancing even when scale is zero, so that re-enabling the scale resumes smoothly with no frozen-state snap."
- Slide D14 is the closest committed precedent for state-driven presentation: reconciliation smoothing widens its band on `matches!(state, MovementState::Sliding{..})`, and the spec calls it "presentation-only — gameplay always snaps to authority."

## Ordering pins

| id | scenario | ordering | expected outcome |
|---|---|---|---|
| O1 | Two transitions on consecutive ticks, one render frame spanning both | tick N records edge A; tick N+1 records edge B; the frame's drain and the render read follow | Both events drain. Both impulses sum. The spring lands exactly where two frames at double the rate would have left it — superposition over a linear solver about a zero target, with the ceiling clamping only the presented sum. |
| O2 | Reconciliation replay re-crosses `Normal` → `Sliding` | the merge writes the authoritative state, N unacked commands replay, the component is snapped in, then forward prediction runs | Only forward prediction's edges reach the list. Replay's are discarded at the binding, as they are today. |
| O3 | Level load, respawn, or pawn re-materialisation | a fresh pawn appears; `ViewFeelState` is app-level and outlives it, as flash/vignette/shake decay did before their level-install reset | No edge exists without a tick recording a transition, so none fires. An impulse in flight across a pawn change does not ride into the new pawn's camera. |
| O4 | Client connects mid-slide | the first snapshot merge writes `Sliding` outside any transition; the slide later ends through a local tick | The exit edge fires alone. A lone `slide_ended` is the accepted outcome. |
| O5 | A catch-up frame drains a backlog | up to fifteen ticks resolve, each possibly recording an edge, before one render read | Every event fires. Each impulse decays by its tick's age before summing, so the frame's offset is strictly below the same edges arriving one per frame, and below `impulse.max` regardless. |
| O6 | An edge lands on a frame with `frame_dt == 0` | the tick records the edge; the render read runs with `frame_dt == 0` | The displacement applies; only the spring advance is skipped. |

## Rejected shapes

- **A generic `state_entered` event with a state-name parameter.** §12 dispatch params are IR published-input membership checked at bind and dispatch; the shipped movement events are payload-free `Reaction<{}>`. Per-state addresses cost nothing and match `reload_started`/`dry_fire`.
- **Emitting edges from each state intent.** Every new state would have to remember to emit. The dispatch point applies all transitions already.
- **A transition counter stamped on `PlayerMovementComponent`.** One slot cannot carry two edges in a frame; reconciliation's authoritative merge is a third writer beside the tick and the dispatch; a fresh pawn resets the writer's counter while an app-level reader's does not; and the field lands on a type deriving `Serialize` and `PartialEq`, where `DashPrograms` is the precedent for excluding it. The per-frame list has none of these, and `drain_named_events_with_sequences` is already generic over what it iterates, so the shared drain is untouched.
- **Render-side edge detection by discriminant comparison.** Needs nothing on the component, and the render assembly already borrows the whole `PlayerMovementComponent`. Rejected for installing a second detector beside the tick-exact one the events use: the two can disagree on a frame, and the render-side one drops any state shorter than a frame gap.
- **Latest-wins impulse composition.** `shake_decay` does this for screen shake, and it is the closer precedent. Rejected because erasing an in-flight kick makes a chained transition read flat, and chaining is what the ceiling reference (`movement.md` §3) asks the surface to compose. Bounded summing keeps the chain legible without the unbounded stack.
- **Dropping or downscaling backlog edges.** Injecting only the last tick's edges eats every second edge on a machine that steadily runs two ticks per frame; scaling by tick count makes magnitude a function of frame rate and contradicts the frame-rate-independence criterion. Per-edge age decay gets the same semantics by computing them.
- **An engine-owned impulse ceiling.** `movement--slide` D10 rejected "a hidden engine ceiling" because the flexibility band wants an Ultrakill-agile and a Titanfall-committed feel both reachable. The same argument caps the same band here, so the ceiling is authored and only the renderer's own FOV limit is engine-owned.
- **Putting the impulse on `SlideParams`.** `CrouchParams.eye_height` is the precedent for presentation values on a state's own block, but it is a gameplay value (capsule attach point) consumed by the tick. The impulse is render-rate and belongs with the other render-rate motions.
- **Letting the viewmodel inherit the FOV punch.** E21 gave it a locked projection for the Destiny reason recorded in `movement--view-feel/research.md`: a separate weapon FOV so player FOV changes do not distort the viewmodel.

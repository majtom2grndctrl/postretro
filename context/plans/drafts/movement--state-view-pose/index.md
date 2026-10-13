# movement--state-view-pose

Brief · compact · reads: `context/lib/movement.md` §1 §3, `context/lib/player_options.md` §5, `context/lib/networking.md` §Presentation events vs. replicated state · read at c06cd6c

## Problem

The project lead play-tested the new vault and asked for the camera to focus on the ledge while the climb is in progress, or at least to look down toward it. This is a requested capability. View feel has no way to hold a camera pose for the length of a movement state. Transition impulses are springs that start decaying at once (`view_feel::impulse::evaluate`), and `viewFeel.slide` is the only sustained channel, wired to one state (`view_feel::slide::evaluate`). The dev player fakes the hold with a low-tension vault kick (`content/dev/scripts/player.ts`, `impulse.states.vault.tension: 6`). That kick is about two-thirds gone by mid-climb, keeps decaying after landing, cannot scale with ledge height, and gets no exit when a reconcile ends the vault without an edge. When this is done, an author can declare a camera pose held while a movement state lasts: pitch, roll, eye drop and FOV, plus a vault-only, engine-aimed ledge focus. The pose eases in on entry, holds for the whole state, and eases out on every exit. It moves only the rendered camera. Mouse look stays live, and aim is untouched.

**Note (vault build, owner feel pass):** `movement.viewFeel.vault { pitch, roll, fov, easeIn, easeOut }` shipped with the vault as an interim sustained block, beside `viewFeel.slide`. Its weight follows the vault's own progress rather than rates, and an early end eases back from the presented weight. It already renders camera-only (the viewmodel does not re-apply it) under a ±89° render-pitch clamp, and content/dev's `tension: 6` vault kick is gone. `pose.vault` should fold it in, as `pose.slide` folds in `viewFeel.slide`. The same pass replaced `path_feet` and `RISE_SHARE` with `physics::movement::vault_path::VaultPath`, whose forward now overlaps the rise (record: `plans/in-progress/movement--vault/plan.md`, Corrections).

## Decisions

- **One state-keyed sustained pose channel, `viewFeel.pose`.** It generalizes the shipped slide block (`plans/done/slide-camera-feel-contract.md`) rather than adding a vault-only key. Rows are keyed by `dash`, `crouch`, `slide`, `swim` and `vault`, each with literal held offsets and its own `enterRate`/`exitRate`. `normal` gets no row, because a pose there is a permanent offset, not a state cue; rivals are in `research.md` §Paths.
- **`viewFeel.slide` folds into `pose.slide` here.** Semantics carry over (`fovIncrease` → `fov`, 0.05 m eye-to-feet floor kept). Unknown `viewFeel` keys are otherwise ignored (`view_feel_params_from_js`), so a leftover `viewFeel.slide` is rejected with a message naming `pose.slide`. The top-level key count is unchanged, as `player-descriptor-composition` asks, and the pre-stable rule (`index.md`) covers the rename.
- **Blend model: the shipped slide blend, per row.** A row's weight eases toward 1 at `enterRate` while its state is active and toward 0 at `exitRate` otherwise, and output is Σ weight × value. A row easing out holds the value it last presented, so geometry terms never pop. The blend follows state kind, not edges, so a reconcile or hot reload that ends a vault silently still eases the pose out.
- **`ledgeFocus`: the vault row's engine-aimed term, an aim-referenced pull downward only.** The ledge line is the angle from the presented eye to the vault's landing spot. Its horizontal distance has a floor, and the angle is clamped to at most level. While aim is above the line, the view is pulled `strength` of the way toward it, capped at `maxPitch`: never past the line, zero once the player already looks at or below it, never touching yaw. Geometry comes from `MovementState::Vault { target, start_offset, mover, .. }`, valid in world space because vault frames are upright (`UPRIGHT_MIN_UP_Y`); derivation is in `research.md` §Focus geometry.
- **Render-only; aim and wire unchanged** (`movement.md` §1). Tick input and aim read `camera.pitch` (`MovementInput.facing_pitch`), and the pose enters only `RenderCamera`. The screen-fixed reticle therefore sits off the shot line by the presented pose. Under the default `allowFire: false`, that matters only for a shot fired right after the vault while the pose eases out; there is no engine coupling to fire. `view_feel` never rides the tuning payload (`movement_state_to_wire`), so no wire or snapshot bump.
- **The composed render pitch clamps to the ±89° camera band.** `RenderCamera::new` adds view-feel pitch to aim without a clamp. Aim at the limit plus a downward kick already flips the view today, and a held pose makes that routine.
- **Pose angles ride the camera and are not re-applied to the viewmodel.** `viewmodel_camera_space_transform` re-rotates the weapon by sway and impulse, and doing that with a 20° hold swings the weapon out of its placement. The pose eye channel keeps slide's shipped composition. Lowering the weapon mid-vault is the vault brief's separate draft.
- **Accessibility and lifecycle follow slide.** The pose scales by the presented `view_feel_scale`, so reduced motion gives zero, and the integrator keeps advancing at zero (`player_options.md` §5). Descriptor refresh, pawn change, no driver and level install clear it with `ViewFeelState::clear_state_effects`.
- **Local pawn only.** View feel follows `local_player_movement_pawn`, and there is no spectator camera. A remote avatar's climb is body animation, and a future follow-cam can evaluate the pose from the replicated vault payload.
- **Doors, not scope.** Wall-run adds a row plus its own wall-signed term in the same slot, which is why `ledgeFocus` is a named vault term rather than a generic look-at. `movement--sustained-fov-feel` shares this model: its slide row is superseded, and its `run` target rebases onto `pose` as an input-tier key; this brief does not edit that draft. Fields are numbers and can widen to `NumberOrIr` later; no transition-specific pose.

### Scripting surface

```typescript
viewFeel: {
  pose: {
    // Held while vaulting; eases out on every exit, early ends included.
    vault: {
      ledgeFocus: { strength: 0.7, maxPitch: 20 }, // 0..1 share toward the ledge line; cap in degrees
      pitch: -2,                                   // optional literal hold, degrees, + looks up
      enterRate: 10, exitRate: 6,                  // 1/sec, [0.1, 240], required
    },
    slide: { eyeDrop: 0.15, fov: 5, enterRate: 18, exitRate: 12 },
  },
},
```

```lua
viewFeel = {
  pose = {
    vault = { ledgeFocus = { strength = 0.7, maxPitch = 20 }, pitch = -2, enterRate = 10, exitRate = 6 },
    slide = { eyeDrop = 0.15, fov = 5, enterRate = 18, exitRate = 12 },
  },
}
```

Every row key and every channel is optional. `enterRate` and `exitRate` are required in a present row. `pitch` and `roll` are degrees in [-90, 90]. `eyeDrop` is metres ≥ 0, measured downward. `fov` is horizontal degrees in [-90, 90]. A missing channel is 0. `ledgeFocus` is valid only on `vault`. It needs both `strength` (in [0, 1]) and `maxPitch` (degrees in (0, 89]). Unknown keys and inherited or `__index` entries are rejected.

## Acceptance

### Automated
- [ ] TypeScript and Luau accept the Scripting surface example. Both reject the same cases and name the field path: a missing rate, an out-of-range or non-finite channel, `normal` or another unknown row key, `ledgeFocus` on a non-vault row, an incomplete `ledgeFocus`, an inherited row, and a leftover `viewFeel.slide`. Omitting `pose` or a row yields no pose.
- [ ] A row approaches its values and holds them for the whole state, across hundreds of frames. Every way out eases monotonically to neutral, starting from the last presented value with no step: completed vault, blocked-path abort, `break_off_vault` on death, a reconcile that writes `Normal` with no edge, and hot reload.
- [ ] Frame-rate independent at 30, 60 and 240 Hz. `frame_dt = 0` leaves output and state unchanged.
- [ ] Ledge focus, both sides of each predicate. Aim above the ledge line pulls the view toward the line by `strength` and never past it. Aim at or below the line adds nothing, and mouse pitch maps 1:1. A ledge top above the eye adds nothing, so the pull is never upward. The pull never exceeds `maxPitch`. It stays finite and capped on the final tick, with the eye straight above the landing spot.
- [ ] While unsaturated above the line, a mouse pitch delta moves the view by (1 − `strength`) of it. Once saturated, or below the line, it moves 1:1. Yaw always moves 1:1.
- [ ] A vault onto a moving mover presents the same focus as the static case at the same relative geometry. A mid-vault correction of the landing target moves the view continuously, never more than one frame's rate-limited step.
- [ ] Aim pitch at the limit plus a full pose and an impulse kick keeps the composed render pitch within ±89°. Today this case flips the view.
- [ ] Movement input facing pitch, host aim pitch and every fired shot's direction are bit-identical over a vault with `pose` present and absent.
- [ ] The viewmodel camera-space transform is unchanged by pose pitch and roll, and still tracks sway and impulse.
- [ ] Scale 1.0, 0.5 and 0.0 (reduced motion included) gives full, half and zero pose. Restoring scale mid-hold resumes from the live blend with no snap. Descriptor refresh, pawn change, no driver and level install clear pose state.
- [ ] `pose.slide` with the dev values reproduces the shipped slide dip and FOV, the 0.05 m clearance floor included.
- [ ] A remote pawn vaulting in co-op adds no pose to the local camera.
- [ ] `WIRE_VERSION`, `SNAPSHOT_VERSION` and the wire fixtures are unchanged. Committed SDK typedefs match the registry.
- [ ] `content/dev/scripts/player.ts` and the `movement-vault.ts`/`.luau` twins carry the example and load. The vault impulse stopgap (`tension: 6`) is removed.
### Manual
- [ ] Vault-test graded ledges: the view settles toward each ledge during the climb, holds, and releases after landing without a pop. A tall ledge dips only as the eye clears the top. A pool exit never pulls upward.
- [ ] Mouse look stays responsive through a climb, and turning during it feels natural rather than fought.
- [ ] Reduced motion on: a vault shows no pose. Slide feel is unchanged after the migration.
- [ ] `allowFire: true` trial: the record says how far the reticle sits off the shot line mid-vault, and whether that reads as acceptable.

## Path

- Seams: `frame_eye::assemble_frame_eye`, which composes slide after bob today; `view_feel::slide`, which generalizes into a per-row pose module; `ViewFeelState`; `map_output_to_camera`; `RenderCamera::new`; `viewmodel_camera_space_transform`; `ViewFeelDriver`, which needs the vault payload, the presented eye and aim pitch rather than only `movement_state.kind()`; `physics::movement::vault::{path_feet, VaultFrame}` for the geometry.
- Mover ledges: either use the mover's presented pose (`kinematic_mover_render`), or work frame-free from the payload and the interpolated progress. Vertical gap and horizontal distance are rotation-invariant under upright frames.
- Shape chosen: per-state pose rows plus named state terms. Strongest rival: a vault-only `viewFeel.vaultFocus` block. It is smaller, but it would make a third sustained mechanism and leave wall-run nowhere to go.
- First slice: the vault row with `ledgeFocus` on the dev player in `vault-test`, before the slide migration. It falsifies the riskiest assumption: that an aim-referenced pull reads as focus rather than as a fought camera.
- `view_feel.rs` (1306 lines) and `frame_eye.rs` grow. Land the pose module beside `slide.rs` and keep `view_feel.rs` to wiring.

## Boundary inventory

camelCase on script sides, snake_case in Rust. No FGD and no PRL. Not on the wire.

| Name | Rust | JS / TS | Luau |
|---|---|---|---|
| pose block | `ViewFeelParams::pose: Option<PoseParams>` (replaces `slide`) | `pose?` | `pose?` |
| rows | `dash`/`crouch`/`slide`/`swim`/`vault: Option<PoseRow>` | same | same |
| channels | `pitch`, `roll`, `eye_drop`, `fov` | `pitch`, `roll`, `eyeDrop`, `fov` | same as TS |
| rates | `enter_rate`, `exit_rate` | `enterRate`, `exitRate` | same as TS |
| ledge focus | `ledge_focus: Option<LedgeFocus { strength, max_pitch }>` (vault only) | `ledgeFocus { strength, maxPitch }` | same as TS |

## Open questions
- Focus reference: aim-referenced pull toward the ledge line (drafted) or a horizontal-referenced additive dip that always keeps mouse pitch 1:1? Recommend aim-referenced: it is true focus, adds no motion when the player already watches the ledge, and is capped. Trade-off: mouse pitch is softened while aim is above the line. — owner: project lead — **blocks build**
- Fold `viewFeel.slide` into `pose.slide` now (drafted) or keep both? Recommend folding: one sustained model, and only dev content uses the slide block. Trade-off: a breaking rename for any external mod. — owner: project lead — **blocks build**
- `allowFire: true` with a held pose: no engine coupling (drafted) or attenuate the pose while firing is allowed? Recommend no coupling, documented: aim stays honest, and an author who allows fire keeps the pose small. Trade-off: the reticle sits off the shot line mid-vault. — owner: project lead — **blocks build**
- Horizontal-distance floor value for the focus angle, and the mover-pose source — **delegated**

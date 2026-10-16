# movement--state-view-pose

Brief · compact · reads: `context/lib/movement.md` §1 §4 (Vault), `context/lib/player_options.md` §5, `context/lib/networking.md` §Presentation events vs. replicated state · read at 3409c2fc

## Problem

The project lead play-tested the vault and asked for the camera to focus on the ledge during the climb, or at least to look down toward it. This is a requested capability. The vault shipped an interim held pose, `viewFeel.vault`, that turns the camera by authored literals over the vault's own progress (`view_feel::vault::evaluate`). It looks down by the same angle at every ledge, because nothing in view feel reads the vault's geometry. It is also a second one-state sustained block beside `viewFeel.slide`, so each further state pose would be another block. When this is done, an author declares in one place a camera pose held while a movement state lasts: pitch, roll, eye drop and FOV. On the vault, they can add an engine-aimed ledge focus that pulls the view down toward the landing spot. The pose eases in, holds for the state, and eases out on every exit. It moves only the rendered camera. Mouse look stays live, and aim is untouched.

## Decisions

- **One state-keyed sustained channel, `viewFeel.pose`.** Rows are keyed `dash`, `crouch`, `slide`, `swim` and `vault`, and each carries literal `pitch`, `roll`, `eyeDrop` and `fov`. `normal` gets no row, because a pose there is a permanent offset, not a state cue. Rows evaluate independently and their outputs add, so a handoff eases one row out while the next eases in; this generalizes the slide contract (`plans/done/slide-camera-feel-contract.md`), and rivals are in `research.md` §Paths.
- **`viewFeel.slide` and `viewFeel.vault` fold into `pose.slide` and `pose.vault`.** Semantics carry over, with `fovIncrease` renamed `fov`; slide's 0.05 m eye-to-feet floor bounds the summed `eyeDrop` of every row. Unknown `viewFeel` keys are otherwise ignored, so each leftover key is rejected with a message naming its replacement, as retired `components.ai` names `components.behavior`. No new top-level key, as `player-descriptor-composition` asks; the pre-stable rule (`index.md`) covers the rename.
- **The `pose` block is strict.** Unknown keys and inherited or `__index` entries are rejected at every level, not skipped, so a typo fails loudly instead of silently posing nothing. This is new: the shipped vault block reads both, and impulse rows skip inherited rows (`has_own_impulse_state_key_js`).
- **Timing splits by row.** `dash`, `crouch`, `slide` and `swim` use the shipped slide blend: weight eases toward 1 at `enterRate` while the state's kind is current and toward 0 at `exitRate` otherwise, so a reconcile that ends a state with no edge still eases it out. `vault` keeps the shipped progress timing, with literal `easeIn`/`easeOut` shares and its join, release and larger-weight overlap rules (`movement.md` §1), so with a nonzero `easeOut` the pose reaches neutral exactly as the pawn reaches the top. A rate on `vault`, or a share on another row, is rejected.
- **A row presents weight × values, and a vault release keeps its values.** Literals and `ledgeFocus` share the one row weight. A release fades the unscaled values it last presented, because the vault's geometry is gone. When a release overlaps a new vault or a rewind, their value sets blend so neither a quick re-vault at a different ledge nor a rewind steps (`research.md` §Overlap value blend).
- **`ledgeFocus { strength [0,1], maxPitch (0,89] }`, vault row only, is an entry-referenced downward offset.** The ledge line is the angle from the rendered eye to the landing spot, with a floor on horizontal distance; a landing spot at or above the eye gives no focus. The reference is the aim pitch on the first frame the pose follows the vault, plus the row's literal pitch; a new vault or a rewind takes a new reference. While the reference is above the line, the offset is `strength` of the gap, capped at `maxPitch`; at or below it, nothing; never yaw. The offset is additive like every other view-feel term, so mouse pitch maps 1:1 for the whole climb; a player who looks further down mid-climb gets the offset on top, bounded by `maxPitch` and the ±89° clamp. PostRetro's movement is a combat tool, and a mouse-gain change mid-climb would be the one view-feel term that fights the player (`research.md` §Product fit). On a mover, the landing spot follows the mover's presented pose, since the vault's frame is re-resolved each tick and may tilt after entry (`research.md` §Focus geometry).
- **Render-only; aim and wire unchanged** (`movement.md` §1). Tick input and host aim read the aim camera, and the pose enters only the render camera, so the screen-fixed reticle sits off the shot line by the presented pose. `allowFire: true` gets no engine coupling, and the reference docs say so. `view_feel` never rides the tuning payload (`movement_state_to_wire`), so there is no wire or snapshot bump.
- **Every row's angles turn the camera only.** Row pitch and roll stay out of the offsets the viewmodel re-applies, as the vault pose ships. `eyeDrop` keeps slide's composition: after bob, in the eye offset that the viewmodel and audio listener follow. Row FOV joins the projection offset under its shipped band, and composed pitch stays under the shipped ±89° clamp.
- **Accessibility and lifecycle follow the shipped blocks.** The pose scales by the presented `view_feel_scale`, reduced motion gives zero, and weights keep advancing at zero (`player_options.md` §5). A descriptor refresh, hot reload included, clears pose state and snaps it to neutral, as it does slide and vault today; a pawn change, no driver and level install reset all view-feel state.
- **Local pawn only.** View feel follows `local_player_movement_pawn`, and there is no spectator camera; a follow-cam is a door (`research.md` §Doors).
- **Doors, not scope.** Wall-run adds a row and its own wall-signed term, which is why `ledgeFocus` is a named vault term rather than a generic look-at. `movement--vault-entry-feel` widens `pose.vault`'s `pitch`, `roll` and `fov` to `motion` expressions evaluated once per vault; the overlap value blend keeps its quick re-vaults from stepping, and that draft's own open question owns its rebase. `pose.slide` owns slide FOV; `movement--sustained-fov-feel` keeps run FOV, because run is input-derived inside `Normal`, which has no row. That draft drops its slide row and states how its run target combines with `pose.slide`'s additive offset before it builds; a note at its head records this. This brief does not edit `movement--vault-entry-feel`. `pose` rows carry their own easing and add, so `player-descriptor-composition`'s override-merged, single state layer could not absorb them as written; that draft would have to accommodate additive rows. No transition-specific pose: transitions are the impulse channel's.

### Scripting surface

```typescript
viewFeel: {
  pose: {
    // Weighted by the climb's progress; releases on every early end.
    vault: {
      ledgeFocus: { strength: 0.7, maxPitch: 30 }, // share toward the ledge line; cap, degrees
      roll: 10, fov: -30,                          // literal holds; pitch, roll in degrees, + looks up
      easeIn: 0.33, easeOut: 0.33,                 // shares of the climb, [0, 1], sum <= 1
    },
    slide: { eyeDrop: 0.15, fov: 5, enterRate: 18, exitRate: 12 }, // 1/sec, [0.1, 240]
  },
},
```

Every row and channel is optional; a missing channel is 0. `pitch` and `roll` are degrees in [-90, 90]. `eyeDrop` is metres ≥ 0, downward. `fov` is horizontal degrees in [-90, 90]. Rate rows require `enterRate` and `exitRate`. `vault` takes `easeIn` and `easeOut` in [0, 1], defaults 0.15 and 0.2, summing to at most 1. `ledgeFocus` needs both fields.

## Acceptance

### Automated
- [ ] TypeScript and Luau accept the Scripting surface example. Both reject the same cases, naming the field path: a missing rate; a rate on `vault` or a share on a rate row; an out-of-range or non-finite channel; shares summing past 1; `normal` or another unknown row; an unknown field in a row or in `ledgeFocus`; `ledgeFocus` on a non-vault row; an incomplete `ledgeFocus`; an inherited `pose` block, row or field. A leftover `viewFeel.slide` is rejected naming `pose.slide`, and a leftover `viewFeel.vault` naming `pose.vault`. Omitting `pose` or a row yields no pose.
#### Migration
- [ ] `pose.slide` with the dev values reproduces the shipped slide dip and FOV frame for frame, the 0.05 m clearance floor included, compared against the shipped evaluator in one build before it is removed.
- [ ] `pose.vault` with the shipped dev literals and no `ledgeFocus` reproduces the shipped vault pose frame for frame: a full climb, an early end, a mid-climb join, a vault during a release, a rewind, a zero `easeOut`, and a zero-length frame on a vault's end, compared against the shipped evaluator in one build before it is removed.
#### Rate rows
- [ ] A rate row approaches its values and holds them for the whole state, across hundreds of frames. Every movement exit eases monotonically to neutral from the last presented value with no step: another state, and a reconcile that writes `Normal` with no edge.
- [ ] A dash or a swim into a vault, and a swim that cuts a vault short, show the leaving row easing out and the entering row easing in at once, each on its own timing. Rows add, and no frame steps. (P1)
- [ ] Two rows dropping the eye at once, as crouch hands to slide, still leave the eye at least 0.05 m above the feet. The floor bounds their sum. (P2)
- [ ] Frame-rate independent at 30, 60 and 240 Hz. `frame_dt = 0` leaves every row's output unchanged, a vault's end frame included.
#### Vault row
- [ ] With a nonzero `easeOut`, the pose reaches neutral on the tick the path reaches the top, at 30, 60, 144 and 240 Hz, and is still easing a tick before.
- [ ] A blocked-path abort, `break_off_vault` on death, and a reconcile that ends the vault each release from the last presented pose, ledge focus included, over the climb's time × `easeOut` and at least 0.1 s, monotonically and with no step.
- [ ] A vault that starts during a release at a different ledge height, and a rewind past 5% of the vault, present no step beyond the new vault's own ease-in.
- [ ] A vault that ends while an earlier release still runs releases the blended pose it last presented, not its own values alone, with no step. (P3)
- [ ] With ledge focus on, two vaults back to back at different ledge heights, with no frame showing the pawn between them, and a rewind past 5% present no step beyond the new vault's own ease-in. (P10)
#### Ledge focus
- [ ] Both sides of each predicate, at full weight and scale with eye and ledge held still. An entry reference above the ledge line offsets the view by `strength` of the gap toward the line; impulse kicks and other rows add on top (P7). An entry reference at or below the line adds nothing. A landing spot at or above the eye adds nothing at any aim, so the pull is never upward. The pull never exceeds `maxPitch`, and it stays finite and capped on the final tick, with the eye straight above the landing spot.
- [ ] A literal `pitch` that already puts the entry reference at or below the line adds no focus; with a literal above the line, the reference plus the offset approaches the line and never passes it.
- [ ] Mouse pitch and yaw move the view 1:1 through the whole climb and its release, at any `strength`, above or below the line. Looking down mid-climb adds to the offset, which stays within `maxPitch` and the ±89° clamp. A new vault or a rewind past 5% takes a new reference. (P5, P14)
- [ ] With ledge focus on and nonzero shares, a full climb starts and ends at neutral, and a vault the pose joins mid-climb eases its focus in from neutral over its own ease-in with no step. (P6)
- [ ] A frame assembled from a vaulting pawn's live movement state shows the focus its own landing spot gives. Two ledge heights at the same aim show different focus.
- [ ] A vault onto a moving mover presents the same focus as the static case at the same relative geometry, and follows the mover if it tilts mid-vault, read at the frame's own interpolation share. (P8, P12)
#### Regression guards
- [ ] Aim pitch at the limit plus a full pose on every row and an impulse kick keeps the composed render pitch within ±89°, and every row's FOV together stays within the projection band.
- [ ] Movement input facing pitch, host aim pitch and every fired shot's direction are bit-identical over a slide, and over a vault with `allowFire: true` firing during the hold with focus active, with `pose` present and absent.
- [ ] The viewmodel camera-space transform is unchanged by any row's pitch and roll, and still tracks sway and impulse. A vault row's eye drop, like slide's, moves the render eye, the audio listener and the viewmodel together.
- [ ] Scale 1.0, 0.5 and 0.0 (reduced motion included) give full, half and zero pose on a rate row and the vault row. Restoring scale mid-hold resumes from the live weight with no snap. Scale 0 through a vault's end, restored mid-release, shows the rest of that release, ledge focus included. (P4)
- [ ] A descriptor refresh and a hot reload mid-vault, mid-slide or mid-release snap the pose to neutral on the first frame assembled after the commit; a pawn change, no driver and level install reset it. A hot reload mid-swim keeps swimming, so the swim row snaps and eases back in. (P9, P11)
- [ ] A remote pawn vaulting or sliding in co-op adds no pose to the local camera.
- [ ] Diff gate: `WIRE_VERSION`, `SNAPSHOT_VERSION` and the wire fixtures are unchanged. Committed SDK typedefs match the registry.
- [ ] `content/dev/scripts/player.ts` authors `pose` and loads. Grep gate: no `viewFeel.slide` or `viewFeel.vault` remains in `content/`, `docs/` or `sdk/`, and `docs/scripting-reference.md` documents `pose` and the reticle offset under `allowFire: true`.
### Manual
- [ ] Vault-test graded ledges: the view settles toward each ledge during the climb, holds, and levels out as the pawn reaches the top, with no pop. A tall ledge dips only as the eye clears the top. A pool exit never pulls upward.
- [ ] Mouse look stays responsive through a climb, and turning during it feels natural rather than fought.
- [ ] Reduced motion on: a vault and a slide show no pose. Slide and vault feel are unchanged after the migration with focus off.

## Path

- Seams: `frame_eye::assemble_frame_eye`, which already composes slide after bob and the vault pose into `RenderCamera::new` only; `view_feel::slide` and `view_feel::vault`, which become one pose module with a rate path and the progress path; `ViewFeelState` and `clear_state_effects`; `ViewFeelDriver`, which needs the vault payload and aim pitch beside `VaultViewInput`, built by `frame_loop`'s `presented_vault`; `vault_view_block::vault_view_params_from_fields` and the `movement_view_feel` parsers; `VaultViewParams`/`SlideViewParams` in foundation, replaced by pose types.
- Geometry: the payload's `target` plus the rendered eye give the line; `VaultFrame::resolve` is the tick's frame. `kinematic_mover_render` runs after the Eye stage, so read the mover pose at the eye's own interpolation share. A reconcile that corrects `target` re-aims focus within a frame; no slew (P13).
- At promotion, correct `movement.md` §1's claim that hot reload releases the vault pose, and the matching `view_feel::vault::evaluate` doc comment.
- Shape chosen: per-state pose rows plus named state terms. Strongest rival: add `ledgeFocus` inside the shipped `viewFeel.vault` block and leave slide alone. It answers the request with no fold, no strict-parsing change and no rate rows. It loses on three counts: wall-run is on the roadmap and would add a third block; `movement--vault-entry-feel` phase 2 builds on one pose row model; and a modder-surface rename is cheapest pre-stable.
- First slice: `pose.vault` with `ledgeFocus` on the dev player in `vault-test`, before the slide migration. It falsifies the riskiest assumption: that an entry-referenced offset reads as focus on the ledge rather than as a dip the player then looks past.
- `view_feel.rs` is past ~800 lines and grows only by wiring; land pose beside `vault.rs`. `docs/scripting-reference.md`'s slide and vault sections merge into one pose section; its accessibility row names the pose.

## Boundary inventory

camelCase on script sides, snake_case in Rust. TypeScript and Luau both ship. No FGD and no PRL. Not on the wire.

| Name | Rust | JS / TS | Luau |
|---|---|---|---|
| pose block | `ViewFeelParams::pose: Option<PoseParams>` (replaces `slide`, `vault`) | `pose?` | `pose?` |
| rate rows | `dash`/`crouch`/`slide`/`swim: Option<PoseRow>` | same | same |
| vault row | `vault: Option<VaultPoseRow>` | `vault?` | `vault?` |
| channels | `pitch`, `roll`, `eye_drop`, `fov` | `pitch`, `roll`, `eyeDrop`, `fov` | same as TS |
| rates | `enter_rate`, `exit_rate` (rate rows) | `enterRate`, `exitRate` | same as TS |
| shares | `ease_in`, `ease_out` (vault) | `easeIn`, `easeOut` | same as TS |
| ledge focus | `ledge_focus: Option<LedgeFocus { strength, max_pitch }>` (vault) | `ledgeFocus { strength, maxPitch }` | same as TS |

## Open questions
- Horizontal-distance floor value for the focus angle, and which presented mover pose to read — **delegated**

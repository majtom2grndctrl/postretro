# Mover-Relative Rider Presentation — Research

Read at 8489a1fe. Symbols, not lines; the executor re-verifies.

## Who reads which mover pose today

| Consumer | Pose it reads | Where |
|---|---|---|
| Mover beauty, shadow instances, occluder AABBs | `registry.interpolated_transform(id, alpha)` | `KinematicMoverRenderCollector::collect_with_mirror` (`crates/postretro/src/runtime_movers.rs`); visibility cull uses the current fixed-tick bounds |
| Mover-attached lights | registry interpolated transform | `crates/postretro/src/light_bridge.rs` |
| Foot probes, single-player and host own pawn | `MoverTickStateTable`, end of this tick, no alpha | `simulate_started_tick_with_presentation_aim` → `update_foot_ground_probes` (`crates/sim/src/sim/mod.rs`) |
| Foot probes, connected client | `App.kinematic_mover_tick_states`, no alpha | `App::update_client_presentation_pose_inputs` (`main.rs`) |
| Foot probes, host seat (client pawns) | live tick table ("deliberately live, not delayed") | `host_presentation::probe_presented_client_pawn_feet` |
| Local camera yaw | tick table + `tick_rotation_delta * alpha` residual | `effective_render_yaw`, `mover_yaw_render_residual` (`main.rs`) |
| Local camera position, local pawn mesh | world-space lerp of tick endpoints | `InterpolableState::lerp` (`crates/sim/src/sim/frame_timing.rs`); `MeshRenderCollector::collect_inner` |
| Remote riders, client view | delayed world-space sample, no ground | `ClientReplication::sample_into_registry` → `RemoteInterpolationBuffer::presented_pose` |
| Remote riders, host seat | delayed world-space sample, `newest_recorded_tick − PRESENTATION_DELAY_TICKS + alpha` | `host_presentation::present_client_pawns` |

`MoverPoseSource` (`crates/physics/src/collision/moving.rs`) is implemented by `MoverTickStateTable`, `MoverTickPoseSource`, `EmptyMoverPoseSource`, `MoverHistoryBuffer` (newest sample only), `ReplayCollisionSource` and `OneMoverPose`. No per-frame shared presentation pose exists.

## Remote riders

- `TransformSample` and `MovementOnlySample` carry no ground. The client reads `WireGroundRef::Mover` only to reject unknown movers (`unknown_ground_mover_id`) and to set the grounded stance bit (`wire_ground_ref_to_foundation_checked`).
- Movers on a client are predicted, not delayed. `prepare_mover_apply` fast-forwards the received phase to `floor(estimated_server_tick)`, and `client_predict_loaded_movers_tick` advances them each fixed tick. Remotes sample `estimated_server_tick − interpolation_delay`, so they run one delay behind their mover.
- The host seat has the same split at a smaller delay (`PRESENTATION_DELAY_TICKS` = `INPUT_BUFFER_TARGET`). Its movers are drawn at the registry blend of the current tick. Its client pawns sample the delayed buffer the host records each tick (`record_client_pawn_poses`). The host owns each client pawn's authoritative movement state, so ground is available where the pose is recorded.
- `MoverHistoryBuffer` (`crates/netcode/src/client.rs`) is keyed by mover id and capped at `MOVER_HISTORY_LIMIT`. It records the authoritative sample and one predicted sample at the fast-forward tick on each apply, not every predicted tick. It answers whole ticks only, through `resolve_replay_tick` / `replay_pose` (`client/replay_poses.rs`). The rebase reads only whole ticks that carried a rider sample. On a client, those are snapshot ticks the history holds as authoritative samples.

### Rebase contract

```mermaid
sequenceDiagram
    participant Rec as "record (client apply / host tick)"
    participant Buf as "delayed rider buffer"
    participant Hist as "authoritative mover pose at each sample's tick"
    participant Now as "presented mover pose"
    participant Out as "presentation transform"
    Rec->>Buf: "pose + ground at tick t"
    Buf->>Buf: "pick samples bracketing delayed tick τ"
    Buf->>Hist: "same mover on both samples?"
    Hist-->>Buf: "mover frame at t0 and at t1"
    Buf->>Buf: "local_i = frame(t_i)⁻¹ · pos_i; blend local_0..local_1 at τ"
    Buf->>Now: "pos' = frame(now) · local"
    Now-->>Out: "yaw' = blended(yaw_i − moverYaw(t_i)) + moverYaw(now) if carry_yaw"
```

A world-space blend across a snapshot interval follows the chord. At the 2-tick snapshot spacing it is r·Δθ²/8 off the arc: 2e-4 m on the fixture (1 m, 1.2 rad/s), and 8e-4 m after one lost snapshot. Blending in the mover's frame is exact at any spacing. Each sample's mover pose is the authoritative one from its own snapshot, so no between-snapshot replay is read, and a trigger between snapshots cannot mislead it. On the host, a rider sample is recorded after the tick's movement under the pre-increment tick. Its mover pose is the end-of-tick pose of the same tick (pin P11).

## Local eye

- `InterpolableState` holds only `position`. `lerp` is `Vec3::lerp`. Every `push_state` site pushes the eye after `follow_camera_to_local_pawn`.
- `interpolated_transform` lerps position and slerps rotation, so a point fixed to the mover sweeps an arc while the eye sweeps the chord. The gap at mid-tick is r·θ²/8 for a turn of θ per tick at radius r. On the carousel (45°/s, 60 Hz, θ ≈ 0.013 rad) it is about 0.1 mm at 4–6 m. At 360°/s it is about 8 mm at 6 m. The gap is not what play-tests saw. The owner kept the arc because one rule, placing the attached eye through the mover's presented pose, also carries a smoothed correction to the eye. Without it, the client needs a separate rule that applies the correction at the eye's point. NPC riders on single-player and the host keep the straight blend at the same millimetre scale.
- The local pawn mesh is `shadowOnly` in first person (`content/dev/scripts/player.ts`).
- `ClientPrediction::presentation_offset` (`crates/netcode/src/prediction.rs`) is added inside `pawn_eye_position` before `push_state`, so both endpoints already include it.
- `apply_authoritative_mover_corrections` turns a carried rider's camera yaw by the correction's yaw at apply. Under smoothing, the view yaw has to take the presented correction's world-up yaw, or the view turns while the deck holds still.

## Where the pose lives

`interpolated_transform` is the render-time accessor every per-entity visual pass reads (`entity_model.md` §5). Presentation writers that resolve a final pose, such as the remote buffer and the host seat, write it through `set_presentation_transform`. A mover's current `Transform` is simulation truth, so smoothing cannot use that writer. It has to be a correction the accessor applies on read.

A per-frame mover pose table was the earlier shape. Whenever smoothing runs, every reader not handed the table draws a different mover than the beauty pass:
- ghosts (`ghost_presented_transform`)
- sound anchors (`sound_events/anchors.rs`)
- E21 attachments and the `E22--kinematic-assemblies` draft
- `mover-aware-reflection-captures`

The registry correction costs one sparse lookup per accessor call and nothing for an entity without a correction. The table cached poses once per frame, but each consumer had to be wired to it.

## Mover corrections

- `ClientReplication::apply_components_to` writes phase and `Transform` with `set_component`, which writes the current slot only. `previous_transforms` keeps the predicted pre-snapshot pose. `run_snapshot_apply_stage` runs before the tick loop, and the client refreshes previous only in `client_predict_loaded_movers_tick`. A zero-tick frame therefore blends an unmatched pair. A frame that ticks still jumps by about the correction.
- `MoverCorrection` records position magnitude only. `apply_authoritative_mover_corrections` uses it to adjust a carried rider's yaw and to publish to `MoverTickStateTable`.
- No test measures the drawn step. `moving_platform_reconciles_under_mandated_profile_without_accumulating_drift` allows corrections up to 0.16 m, eighty times the 0.002 m step the earlier draft would have tolerated. This led the owner to build smoothing outright and use measurement as proof.

### Own-pawn share of a mover correction

The ordinary arm of `reconcile.rs` seeds `presentation_offset` with predicted-minus-reconciled pawn position. Replay carries the pawn on the corrected mover history, so a mover correction d under a standing pawn produces a pawn correction of about d. Suppose the eye stayed on today's path while the drawn deck held its old pose. The eye would glide at the pawn offset's rate and the deck at the mover's, and the rider would slide across the deck in between. The brief keeps the mover-explained share out of the pawn offset, and applies the mover's presented correction at the eye instead. On leaving the mover, whatever correction remains at the eye moves into the world offset, so nothing steps.

## Commitments touched

- `networking.md` §Host-side remote-pawn presentation: host-seat foot probes use the live mover pose. This brief moves every rider's probe to the presented pose, per frame, on every seat. Update the section, and the foot-probe contract it rests on, at promotion.
- `movement.md` §6, Moving bases (Agents): "co-op presentation of NPCs on moving geometry is not built". It stays unbuilt, and the roadmap item owns it.
- `plans/done/E17--rotating-movers/`: intrinsic velocity drives remote locomotion, fixed-tick yaw stays authoritative with an alpha residual, and `carry_yaw` never crosses the wire. All are kept.
- `entity_model.md` §5: the registry accessor stays the one presented pose. It gains a render-only correction term, which goes into §5 at promotion.
- `E22--kinematic-assemblies` (draft): members render at their holder's interpolated transform, so they follow smoothing with no change.

## Oversized sources

| Source | Lines at read | Action |
|---|---|---|
| `crates/netcode/src/client.rs` | ~8,700 | Split remote presentation and mover history out first |
| `crates/postretro/src/main.rs` | ~13,100 | Split camera follow and yaw carry helpers out first |
| `crates/sim/src/sim/mod.rs` | ~4,700 | Split foot probing out first |
| `crates/netcode/src/interpolation.rs`, `scripting/frame_systems/mesh_render.rs`, `runtime_movers.rs` | 2,100–2,800, mostly tests | Extend at the boundary |

## Pins

| id | scenario | ordering | expected outcome |
|---|---|---|---|
| P1 | Mover correction lands on a zero-tick frame; a later frame ticks | Apply writes the corrected current pose before the ticks; previous moves only when a tick runs | No drawn step on the apply frame, later zero-tick frames, or the first ticking frame |
| P2 | Mover correction seeded on frame N | Seed (snapshot apply) → advance → readers after the ticks (view yaw, remote rebase, foot probes, sound anchors, render) | Frame N draws the full residual; first shrink on N+1; one smoothing state per frame |
| P3 | Own pawn on a gliding mover, zero-tick frames | Eye endpoints pushed per tick with the pawn offset baked in; smoothing advances per frame | The mover term at the eye comes from this frame's presented pose at render time |
| P5 | One snapshot corrects a mover and moves the pawn's ground onto or off it | Mover seed and pawn reconcile in one apply | Mover share enters the eye once; eye moves by its own motion only |
| P8 | Rider's tick precedes the oldest retained mover sample | Lookup falls back to the oldest sample's pose | Treated as missing history; sampled world pose |
| P9 | Ground sample arrives after a newer sparse sample | Buffer inserts by tick; sparse fields filled at record time | Ground resolved from the latest at or before each tick, so a late arrival corrects later samples; no stale attach |
| P10 | carry_yaw mover correction under a standing pawn | Apply-time yaw step on the settled yaw that commands read; drawn yaw assembled after the ticks | Settled yaw takes the full step at apply; only the drawn yaw glides |
| P11 | Host pairs a rider sample with its mover pose | Rider samples recorded after movement, before the stamp advances, pre-increment label | Mover pose taken at the same seam and label; rider tick t pairs with the mover's end-of-tick t |
| P4 | Pawn lands on a mover mid-glide | Ground becomes the mover, then the eye takes its presented pose | Residual at the eye's point enters the world offset with the sign that keeps the eye still |
| P6 | Pawn teleports off a mover mid-glide | Teleport clears the world offset and cuts | Hand-off dropped; clean cut |
| P7 | Rotation-only correction moves part of the mover's bounds by the floor | Snap is decided on the mover's bounds, not its origin | Deck snaps and the standing pawn's eye cuts on the same frame |
| P12 | Mover despawns or rebinds under a standing pawn mid-glide | Smoothing clears, then the eye loses the mover term | Residual moves into the world offset; no eye step |
| P13 | A carry_yaw mover correction lands on a frame that runs no fixed tick, under a standing pawn | Apply adds the whole tick-start correction to the settled yaw and publishes the corrected tick delta; the registry rewrites only the mover's current pose; the drawn yaw is assembled after the ticks | View yaw and drawn deck yaw agree at every alpha on the apply frame, later zero-tick frames and the first ticking frame |
| P14 | A mover correction lands on a zero-tick frame under a standing pawn | Eye endpoints pushed per tick; apply rewrites the mover's current pose before the ticks; eye assembled after the ticks | Each eye endpoint keeps the mover pose it was pushed with; the eye stays on the drawn deck until the next push |
| P15 | Attachment starts or ends at a push while a correction is mid-glide | The hand-off enters the world offset, which reaches the eye only through endpoints pushed after it | The hand-off covers both endpoints of the first interval it opens; the eye at alpha 0 equals the last frame's eye |
| P16 | A mover despawns or rebinds under a standing pawn mid-glide, on a zero-tick frame | Snapshot apply removes the mover before the ticks; the world offset reaches the eye only at the next push | The residual at the eye's point is read before the mover leaves the registry and reaches the eye on the despawn frame at any alpha |
| P17 | A world-space pawn offset glides while the pawn stands on a turning or corrected mover | Offset and eye height are folded into each endpoint at push; the mover-frame blend runs at render | Only the capsule centre enters the mover's frame; offset and eye height are added along world axes after placement |
| P18 | A snapping mover correction lands on a zero-tick frame | Apply rewrites only the mover's current pose; previous moves when a tick runs | Deck and eye land on the corrected pose on the apply frame at any alpha, and move only by their own motion on the first ticking frame |
| P19 | A hold or eye cut lands while the pawn stands on a mover, possibly mid-glide | Holds write both endpoints without a ground; the next push may attach | A held interval never attaches; a cut that leaves the pawn on a gliding mover moves the eye once and the next attached interval does not step |
| P20 | A carry_yaw pawn leaves, lands on, or loses a gliding mover | The hand-off moves the residual yaw at the eye between the deck and a render-only view-yaw offset; commands read the settled yaw | Drawn view yaw does not step; command yaw is unchanged by the hand-off |

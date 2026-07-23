# E21 — Co-op Avatar + Weapon Presentation

## Goal

Remote players render as skinned character models with aim tracking, foot planting, and held weapons. The local player sees a first-person viewmodel of the equipped weapon and casts a body shadow. Consumes the shipped pose-modifier stack, bone sockets, and foot-IK contracts.

## Scope

### In scope

- Client view pitch on the wire (input command and movement-state payload).
- Shared-visible active-weapon archetype identity on the wire, distinct from owner-private ammo/cooldown and the host-only `WeaponOwners` map.
- Player descriptor gains a `mesh` block (materializes into `MeshComponent`) with clips, pose masks, sockets, and locomotion descriptor.
- Weapon descriptor gains a third-person prop model path and a viewmodel model path.
- Remote player mesh materialization through the existing descriptor-mesh presentation path.
- Client-local locomotion: idle/walk animation state derived from replicated velocity, stride rate from per-state `travelSpeed` calibration. No new wire traffic.
- Aim-bend and upper/lower-body split modifiers driven from replicated pitch and facing for remote players, from camera for the local player.
- Local player body renders into shadow-depth only (no forward-pass visibility). Descriptor-authored via a `shadowOnly` flag on the mesh block. Dynamic-light filtering includes explicit shadow-only intent without admitting the broader promoted-static relevance set.
- Third-person weapon mesh at the hand socket, resolved from the replicated weapon archetype identity. Runtime attach/detach on weapon change.
- First-person viewmodel rendered through the mesh pass with a dedicated near-plane projection, layered with view-feel.
- Behavior-preserving split of `mesh_pass.rs` (3813 lines) before extending it with viewmodel draw logic.

### Out of scope

- Co-op enemy aim presentation — separate plan (`E21--coop-enemy-aim-presentation`), different wire surface.
- Per-player cosmetics or skins — `weapon-model.md` future.
- Hand IK onto the weapon (roadmap non-goal: socket poses the weapon, not the reverse).
- Cloth/hair sim, facial animation, full-body IK beyond legs.
- Animated (skinned) weapon attachments — third-person weapon is a rigid prop at rest pose.
- Weapon augment `mount` descriptor surface (`SlotKind`, `MountPoint`) — Epic 16.
- Client-side ammo/reload prediction and reconciliation — owner-private state-slot projection handles this.
- Hit-zone authority changes — aim bend is presentation; the authoritative ray uses the replicated pitch/yaw, never the bent spine.
- Weapon-switch input path — no equip/swap mechanic exists. This plan renders whatever weapon the host assigns; the switch mechanic is a separate feature.

## Acceptance criteria

- [ ] A two-player co-op session renders each remote player as a skinned character mesh, not a bare transform.
- [ ] A remote player's torso bends to match the controlling player's camera pitch; the bend tracks live aim changes with no visible snap or lag beyond the interpolation delay.
- [ ] A remote player's upper body faces aim direction while legs face travel direction, using the upper/lower-body split modifier.
- [ ] A remote player's feet plant via foot IK — no skating on flat ground, no floating on slopes — matching the shipped enemy locomotion-descriptor behavior.
- [ ] A remote player's locomotion animation transitions between idle and walk/run states based on replicated velocity, with stride rate calibrated by per-state `travelSpeed`.
- [ ] A changed replicated weapon archetype updates the held third-person model at the hand socket within one snapshot interval.
- [ ] The local player sees a first-person viewmodel of the equipped weapon. The viewmodel does not clip into nearby world geometry (no z-fight with walls/floors at arm's length).
- [ ] The local player's body does not appear in the first-person view but casts shadows from lights that already have shadow pools (dynamic or promoted-static). Lights without shadow pools (low-power, bake-only) are unaffected.
- [ ] In single-player, the viewmodel still renders and the player body still casts shadows — the presentation is not gated on netcode being active.
- [ ] The wire-version bumps gate the new fields behind the two-gate handshake. Mismatched peers (pre-bump client to post-bump host or post-bump client to pre-bump host) cannot connect.
- [ ] The behavior-preserving split of `mesh_pass.rs` (Task 6) does not change any test result or visual output.
- [ ] A descriptor with `shadowOnly: true` on its mesh block renders into shadow-depth passes only; `shadowOnly: false` (the default) preserves current behavior for all existing entities.

## Tasks

### Task 1: Wire bump — aim pitch + weapon archetype identity

Add `aim_pitch: f32` to `WireMovementInput` (after `use_pressed`, the current last field). The client packs camera pitch into the input command; the host receives it, stores it alongside movement state, and echoes it to other clients.

Add `aim_pitch: f32` to `WirePlayerMovementState`. The host writes the latest received aim pitch from the owning client's input command. Clients consume it for pose-modifier input on remote player pawns.

Extend the client-side interpolation buffer to retain replicated aim pitch alongside position. The client snapshot-apply path currently captures remote-pawn velocity into `TransformSample` but discards `PlayerMovementState` for non-local entities. Add `aim_pitch: f32` to the per-entity interpolation sample (`TransformSample`) so it interpolates smoothly between snapshots, matching position interpolation. Add `aim_pitch: f32` to `PresentedPose` so the interpolated value is accessible to callers of `presented_pose()`. The interpolated aim pitch feeds `PoseInputs` in Task 3.

Add `active_weapon_archetype: Option<String>` to the entity record metadata, valid on non-despawn records carrying `PlayerMovementState` (the same validity gate as `local_player` and `last_processed_client_tick`). The host populates it from the `WeaponOwners` pawn→weapon lookup → weapon entity's descriptor `canonicalName`. Clients consume it for third-person weapon attachment resolution. `None` means no weapon equipped. On `RawEntityRecord`, encode as `has_active_weapon_archetype: bool` + `active_weapon_archetype: String`, mirroring the `has_entity_class`/`entity_class` flag pair. The encoding mirrors `entity_class`, but the validity gate is narrower: `active_weapon_archetype` requires `PlayerMovementState`, whereas `entity_class` requires only a finite Transform. `validate` rejects a `false` flag paired with a non-empty string (`MalformedActiveWeaponMetadata`), a `true` flag with an empty string, and any active-weapon metadata on a despawn record (`MetadataOnDespawn`). Extend the existing `MetadataOnDespawn` despawn guard to also check `has_active_weapon_archetype`, alongside `has_entity_class` and the movement-authority flags.

Bump `WIRE_VERSION` and `SNAPSHOT_VERSION`. These are layout changes to existing wire types, not vocabulary changes (no new message variant), so the app-protocol constant is untouched. Update the drift-guard tests on both sides of the crate boundary.

### Task 2: Player mesh descriptor + remote materialization

Add `shadowOnly: bool` to `MeshDescriptor` on both the TS surface (`sdk/types/postretro.d.ts`) and the Rust descriptor (`crates/entities/src/data_descriptors/types/entity.rs`), defaulting to `false`. This field must exist on the type surface before Task 4 threads it into `MeshComponent` and the renderer.

Add a `mesh` block to the player descriptor (`content/dev/scripts/player.ts`): model path pointing to the exo_red character model, animation states (`idle`, `walk_forward`), locomotion with `speedScale: true`, per-state `travelSpeed` on the walk state, `defaultState: "idle"`, and a `hand_r` socket already present on the model. Add `shadowOnly: true` to opt the local player body into shadow-depth-only rendering.

Add `third_person_model` and `viewmodel` fields to the weapon descriptor surface (the `weapon` component schema). The reference pistol descriptor gains a `thirdPersonModel` path pointing to the smg model (or a placeholder). Add a `viewmodel` field (consumed in Task 7); initially optional and unused.

Add `materialize_armed_remote_player` to `remote_materialize.rs`. Mirrors `materialize_armed_remote_enemy`: attaches the descriptor's `MeshComponent` presentation only — never `PlayerMovement`, `Weapon`, or `Health`. Apply player viewer-role policy after generic descriptor materialization: `shadowOnly` affects the owning first-person viewer, while a remote viewer sees the avatar forward. Idempotent; unknown class leaves the entity transform-only.

Wire `materialize_armed_remote_player` into the client snapshot-apply path. At the call site in `netcode/mod.rs` where `outcome.remote_enemies` is iterated, resolve the descriptor by `entity_class` from the shared descriptor table. If the resolved descriptor carries a `movement` component (the durable signal that it's a player-type descriptor, not a name match), call the player materialization path; otherwise the enemy path. Rename the outcome field from `remote_enemies` to `remote_entities` — it now carries both player and enemy materializations. Both share the same presentation-only contract. The local player gets mesh materialization too (for shadow rendering) through the same materialization path; the `shadowOnly` descriptor flag controls forward-pass exclusion at collection time. The local pawn's mesh attaches inside `materialize_armed_local_pawn`, alongside the existing movement materialization — not through the `remote_entities` path. In single-player, the host spawn path in `netcode/mod.rs` (the same path that populates `WeaponOwners`) already materializes the player entity — extend it to attach the descriptor's mesh alongside the existing movement materialization. No snapshot apply is involved.

### Task 3: Player pose inputs — local + remote

Write `PoseInputs` for player mesh entities. The existing `update_pose_inputs` in `sim/mod.rs` already runs for all animated mesh entities each tick, writing zero pitch and heading-yaw fallback for brainless entities. Add a branch inside this loop for player pawns that overrides the default write:

- **Local player:** write `aim_pitch` from the camera's current pitch, `aim_yaw` from the camera's current yaw, `heading_yaw` from the pawn's movement-facing direction. These feed the aim-bend and upper/lower-split modifiers so the local body shadow tracks the player's aim.
- **Remote player:** write `aim_pitch` from the interpolated aim pitch in the interpolation buffer (Task 1), `aim_yaw` from the interpolated transform's yaw, `heading_yaw` derived from the interpolated velocity direction (or transform yaw when stationary). These drive the aim presentation on the remote avatar.

The local player's camera pitch must reach `update_pose_inputs`. The camera state is resolved in the main loop at the render-assembly site. `update_pose_inputs` currently takes only `&mut EntityRegistry` and has no camera access. Add a camera-aim parameter (pitch, yaw) and a per-entity interpolated-aim map (`HashMap<NetworkId, f32>`, extracted from the interpolation buffer's `presented_pose()` results by the caller in `main.rs`) to `update_presentation_pose_inputs`, threaded down to `update_pose_inputs`. The local-player branch reads camera aim; the remote-player branch looks up interpolated aim_pitch from the map by network ID. This is the narrowest change — `update_presentation_pose_inputs` is the public entry point called from the main loop and already accepts multiple parameters. The camera pitch/yaw is captured from `App.camera` before the tick loop and passed down.

Foot-IK ground probes for player pawns activate automatically. The shipped `update_foot_ground_probes` runs for entities with `MeshComponent.pose_inputs` and legs in the pose stack. The exo_red model carries `legL`/`legR`/`footL`/`footR` pose-mask tags on its leg joints, so the loader builds the leg set and the probe loop activates — no entity-type filter to bypass.

Drive client-local locomotion animation from replicated velocity. On each client frame, for each remote player pawn: compute speed from the interpolated velocity, select idle vs walk/run animation state, and set playback rate from `measured_ground_speed / effective_travel_speed` (the existing `update_playback_rate` API). This mirrors the existing enemy locomotion logic — port it rather than re-deriving. The animation state name is already replicated via `WireMeshAnimationState`. Client-local locomotion is the prediction; replicated state is the authoritative correction. On snapshot delivery, if the replicated state disagrees with the client-local derivation, blend toward the server state over a short window (2-3 frames) rather than snapping. Detect mismatch by comparing the client-derived state name against the replicated `WireMeshAnimationState` current state name. The blend uses the existing crossfade API (`switch_animation_state` with a short crossfade duration) — per-entity blend state is implicit in the crossfade timer. Remote player avatars are visually scrutinized — a snap reads as a hitch. The snapshot-correction blending (2-3 frame blend on mismatch) is new — enemies currently snap. This blending model sets the precedent for a future port that replaces the enemy snap-on-mismatch behavior.

### Task 4: Local body shadow-only rendering + descriptor surface

The `shadowOnly: bool` field is on the descriptor surface (added in Task 2). Thread it into `MeshComponent` at materialization. In the mesh render collector, keep forward visibility, dynamic-shadow eligibility, and promoted-static relevance separate. Forward visibility is portal-visible and not shadow-only. Dynamic-shadow eligibility is portal-visible or explicitly shadow-only. Collection retains either dynamic eligibility or selected-static relevance, so an off-PVS local shadow-only body still reaches per-light cone culling while a non-opted off-PVS mesh cannot leak into dynamic slots.

The pre-task shadow-pass infrastructure does not fully support this:
- Promoted-static shadow slots use `MeshDepthInstanceFilter::IncludeShadowOnly`, which includes `forward_visible = false` instances.
- Dynamic shadow slots use `MeshDepthInstanceFilter::ForwardVisibleOnly`, which deliberately excludes them (pinned by test).
- When no promoted-static records exist, the frame planner uses `plan_forward_visible_mesh_frame`, which drops shadow-only instances from the plan and SSBO entirely.

This task must make two changes:
1. Carry dynamic-shadow eligibility through the CPU frame plan. Dynamic spot and cube slots filter on it. Promoted-static slots accept every retained static-relevance instance. Budget forward-visible first, explicit dynamic casters second, and promoted-static-only relevance last.
2. Always use `plan_mesh_frame` regardless of promoted-static record presence. Remove `plan_forward_visible_mesh_frame` and its dedicated tests — the fast path assumes no shadow-only instances exist, and that precondition no longer holds.

Both changes are safe because shadow-only instances are opt-in per descriptor — existing entities default to `shadowOnly: false` and their behavior is unchanged. Dynamic filtering widens only for instances that explicitly requested shadow-only rendering.

Single-player mode: the same shadow-only treatment applies. The player body shadow is not gated on netcode.

### Task 5: Third-person weapon at hand socket — runtime attach/detach

Add a runtime attach/detach mechanism for weapon models at bone sockets. The shipped socket system resolves attachments from the descriptor at load time and stores them in `MeshComponent.attachments`. This task adds a host-side mutation path: when a player's active weapon changes (tracked by `WeaponOwners`), the host updates the pawn's `MeshComponent.attachments` to mount the new weapon's `thirdPersonModel` at the `hand_r` socket, replacing any previous weapon attachment at that socket. The mutation runs in the host command-processing path where `WeaponOwners` is populated, before snapshot production.

On connected clients: the replicated `active_weapon_archetype` (from Task 1) drives attachment resolution. The client tracks the last-applied `active_weapon_archetype` per network ID in a `HashMap<NetworkId, Option<String>>` on `ClientReplication`. On each snapshot apply, the apply path compares the incoming archetype against the stored value; a change (including initial population) surfaces through the `remote_entities` outcome for weapon attachment update. When the outcome delivers a changed archetype string, the client resolves the weapon descriptor by canonical name, looks up its `thirdPersonModel` path, and updates the local `MeshComponent.attachments` for the remote player pawn. The attachment model must already be loaded (pre-uploaded as part of the descriptor's model set at level load). If the model is not in the pre-loaded set (misconfigured descriptor), the weapon attachment clears — same disappear-until-loaded behavior as the viewmodel (Task 7). Socket binding resolution reuses the existing `AttachmentBinding::Skinned` path — the hand socket is a skin joint.

For the local player: the same attachment mutation applies (the third-person weapon renders in the shadow-depth pass alongside the body). The viewmodel (Task 7) renders separately and is not an attachment.

`None` archetype clears the weapon attachment. A weapon archetype whose descriptor has no `thirdPersonModel` also clears it — no phantom weapon.

### Task 6: Split mesh_pass.rs

Behavior-preserving split of `mesh_pass.rs` (3813 lines) before Task 7 extends it. Extract `record_skinned_depth`, `MeshDepthInstanceFilter`, `model_bounds` (an `&self` method on `MeshPass` — moves with its impl block or becomes freestanding with an explicit parameter), and the depth pipeline construction into a sibling module. The forward draw path and the frame-plan consumption stay in `mesh_pass.rs`. The split reduces `mesh_pass.rs` so the viewmodel forward-draw additions (Task 7) land in a shorter, more navigable file.

No functional changes. All existing tests and visual behavior remain identical.

### Task 7: First-person viewmodel

Render the local player's equipped weapon as a first-person viewmodel through the mesh pass. The viewmodel is a separate model asset (the weapon descriptor's `viewmodel` path), placed camera-relatively but carried to the shared shader in world space, with a dedicated near-plane projection that prevents clipping into world geometry.

The viewmodel instance is a `MeshInstanceInput` with `is_viewmodel: bool` (default `false`). The frame planner partitions instances into two separate `MeshFramePlan` objects: a world plan (all non-viewmodel instances) and a viewmodel plan (viewmodel instances only). Both plans pack into the shared palette/instance SSBO — the separation is control flow, not data. Shadow-depth passes receive only the world plan; `record_skinned_depth` never sees viewmodel groups. The dedicated viewmodel pass draws from the viewmodel plan after the forward color pass completes, with a depth-clear and a tighter near/far projection (tunable; starting point: 0.01–2.0 m range, ~70° FOV). The viewmodel reuses the same skinned-mesh pipeline, palette, and material bindings — only the projection, depth state, and plan differ.

View-feel integration: the engine composes the viewmodel's camera-space placement at the render-assembly site in the main loop, applies view-feel bob and tilt, then converts it to a world-space instance transform with the render camera. The renderer applies the matching view plus alternate projection — it does not read view-feel state. This keeps shared mesh SH, dynamic-light, and shadow-receipt coordinates in world space.

Shadow exclusion: shadow-depth passes receive only the world plan, so viewmodel instances are structurally excluded — `record_skinned_depth` never iterates viewmodel groups. Viewmodels use a dedicated near-plane view-projection rather than world shadow matrices; plan-level separation enforces exclusion without a per-group filter check.

Weapon switch: when the local player's weapon changes, the viewmodel swaps to the new weapon's `viewmodel` asset. If the asset is not yet loaded (hot-swap edge case), the viewmodel disappears until the load completes — no stale weapon frame.

Single-player mode: the viewmodel renders identically. `WeaponOwners` is populated by the host even in single-player mode. The viewmodel reads the local player's weapon via `WeaponOwners::weapon_of(local_pawn)` → weapon entity → descriptor's `viewmodel` path, not from the wire.

## Sequencing

**Phase 1 (concurrent):** Task 1 (wire bump), Task 2 (player mesh descriptor + materialization), Task 6 (mesh_pass split) — independent subsystems.

**Phase 2 (concurrent):** Task 3 (pose inputs), Task 4 (shadow-only body), Task 5 (weapon attach/detach) — all consume Phase 1 foundations. Task 3 needs Tasks 1+2. Task 4 needs Task 2. Task 5 needs Tasks 1+2.

**Phase 3 (sequential):** Task 7 (viewmodel) — needs Tasks 5+6 (weapon model loading and the split mesh pass).

## Rough sketch

**Wire additions.** `WireMovementInput` gains `aim_pitch: f32`. `WirePlayerMovementState` gains `aim_pitch: f32`. Entity record metadata gains `active_weapon_archetype: Option<String>` with the same validity gate as movement-authority metadata. The host writes `aim_pitch` from the input command and `active_weapon_archetype` from `WeaponOwners` → weapon entity → descriptor canonical name. The client interpolation buffer gains an `aim_pitch` field on each sample, interpolated alongside position.

**Remote player materialization.** A new `materialize_armed_remote_player` in `remote_materialize.rs` follows the enemy pattern: presentation-only mesh attachment, no gameplay components. The client-apply dispatch in `crate::netcode` branches on whether the descriptor carries a `movement` component (player-type) or not (enemy-type). Viewer-role policy retains descriptor `shadowOnly` for the owner but clears it for peers.

**Pose input plumbing.** A branch inside `update_pose_inputs` in `sim/mod.rs` overrides the default write for player pawns. Local player reads camera pitch/yaw; remote player reads interpolated aim_pitch from a per-entity map extracted from the interpolation buffer's `presented_pose()` results. Both paths produce the same `PoseInputs` shape consumed by the existing modifier stack — no modifier changes needed. Camera pitch and the per-entity interpolated-aim map reach the sim stage via added parameters on `update_presentation_pose_inputs`, threaded down to `update_pose_inputs`. Client-local locomotion is client-predicted with server correction: on snapshot mismatch (compared against `WireMeshAnimationState`), blend via the existing crossfade API over 2-3 frames rather than snapping.

**Shadow-only body.** The `shadowOnly` descriptor flag threads into `MeshComponent`. Collection and planning carry forward visibility separately from dynamic-shadow eligibility. Dynamic slots accept portal-visible or explicit shadow-only casters; promoted-static slots accept the broader retained relevance set. `plan_mesh_frame` replaces the removed `plan_forward_visible_mesh_frame` unconditionally — shadow-only instances must survive in the plan regardless of promoted-static record presence.

**Viewmodel projection.** The frame planner partitions into a world plan and a viewmodel plan — control-flow separation, not data separation. Both pack into the shared palette/instance SSBO. Shadow-depth passes receive only the world plan; `record_skinned_depth` never sees viewmodel groups. The viewmodel pass draws from the viewmodel plan after the forward color pass, behind a depth-clear, with a near-plane projection (~0.01–2.0 m, ~70° FOV). The engine composes camera-relative placement plus view-feel offsets, converts it to world space, and supplies the matching view; the renderer owns the alternate projection.

**Oversized file watch.** `mesh_pass.rs` is 3813 lines. Task 6 splits it before Task 7 extends it. `gltf_loader.rs` is 4810 lines but is not extended by this plan.

## Boundary inventory

| Name | Rust | Wire / serde | TS descriptor |
|---|---|---|---|
| Aim pitch | `aim_pitch: f32` | `aim_pitch: f32` (bitcode) | n/a (engine-derived from camera) |
| Active weapon archetype | `active_weapon_archetype: Option<String>` | `Option<String>` (bitcode) | `canonicalName` on weapon descriptor |
| Third-person model | `third_person_model: Option<String>` | n/a (descriptor-local) | `weapon.thirdPersonModel: "<path>"` |
| Viewmodel | `viewmodel: Option<String>` | n/a (descriptor-local) | `weapon.viewmodel: "<path>"` |
| Shadow-only | `shadow_only: bool` | n/a (descriptor-local) | `mesh.shadowOnly: true` |

## Wire format

Three additions to existing wire surfaces — no new section or message type.

**`WireMovementInput`** gains `aim_pitch: f32` after `use_pressed` (the current last field). Client-to-server on the Input channel. Bitcode layout change bumps `WIRE_VERSION`.

**`WirePlayerMovementState`** gains `aim_pitch: f32` after `capsule_eye_height`. Server-to-client on the Snapshot channel. Bitcode layout change bumps `SNAPSHOT_VERSION`.

**Entity record metadata** gains `active_weapon_archetype: Option<String>` with the same validity gate as movement-authority metadata (valid only on records carrying `PlayerMovementState`). Bitcode layout change is part of the `SNAPSHOT_VERSION` bump.

All three are covered by a single combined bump of `WIRE_VERSION` and `SNAPSHOT_VERSION`. The app-protocol constant is untouched — no message vocabulary change. The two-gate handshake rejects mismatched peers before any payload is decoded.

## Script syntax examples

Player descriptor with mesh (Phase 2):

```typescript
// content/dev/scripts/player.ts — additions
export const playerEntity = defineEntity({
  canonicalName: "player",
  defaultWeapon: "reference_pistol",
  components: {
    health: { max: 100 },
    mesh: {
      model: "dev/models/exo_red",
      shadowOnly: true,
      animations: {
        idle: { clip: "idle", loop: true },
        walk: { clip: "walk_forward", loop: true, travelSpeed: 7.0 },
      },
      defaultState: "idle",
      locomotion: { speedScale: true },
    },
    movement: { /* ... existing ... */ },
  },
});
```

Weapon descriptor with model paths (Phase 2–3):

```typescript
// content/dev/scripts/reference-pistol.ts — additions
export const referencePistolEntity = defineEntity({
  canonicalName: "reference_pistol",
  components: {
    weapon: {
      damage: 12.0,
      range: 64.0,
      fireRateMs: 180.0,
      fireMode: "semi",
      resolution: "hitscan",
      thirdPersonModel: "dev/models/smg",
      viewmodel: "dev/models/smg",
      resource: { /* ... existing ... */ },
    },
  },
});
```

## Content dependencies

- **Viewmodel asset.** No viewmodel model asset exists for the reference pistol. The smg model rendered in camera space is a sufficient placeholder — the engine is asset-agnostic. A dedicated viewmodel mesh is a content task, not an engine task.

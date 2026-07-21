# E21 — Co-op Avatar + Weapon Presentation

## Goal

Remote players render as skinned character models with aim tracking, foot planting, and held weapons. The local player sees a first-person viewmodel of the equipped weapon and casts a body shadow. Consumes the shipped pose-modifier stack, bone sockets, and foot-IK contracts.

## Scope

### In scope

- Client view pitch on the wire (input command and movement-state payload).
- Shared-visible active-weapon archetype identity on the wire, distinct from owner-private ammo/cooldown and the host-only `WeaponOwners` map.
- Player descriptor gains `MeshComponent` with clips, pose masks, sockets, and locomotion descriptor.
- Weapon descriptor gains a third-person prop model path and a viewmodel model path.
- Remote player mesh materialization through the existing descriptor-mesh presentation path.
- Client-local locomotion: idle/walk animation state derived from replicated velocity, stride rate from the shipped locomotion-descriptor `travelSpeed` calibration. No new wire traffic.
- Aim-bend and upper/lower-body split modifiers driven from replicated pitch and facing for remote players, from camera for the local player.
- Local player body renders into shadow-depth only (no forward-pass visibility), using the existing `forward_visible` instance flag.
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

## Acceptance criteria

- [ ] A two-player co-op session renders each remote player as a skinned character mesh, not a bare transform.
- [ ] A remote player's torso bends to match the controlling player's camera pitch; the bend tracks live aim changes with no visible snap or lag beyond the interpolation delay.
- [ ] A remote player's upper body faces aim direction while legs face travel direction, using the upper/lower-body split modifier.
- [ ] A remote player's feet plant via foot IK — no skating on flat ground, no floating on slopes — matching the shipped enemy locomotion-descriptor behavior.
- [ ] A remote player's locomotion animation transitions between idle and walk/run states based on replicated velocity, with stride rate calibrated by `travelSpeed`.
- [ ] Each remote player holds a third-person model of the equipped weapon at the hand socket. Switching weapons updates the held model within one snapshot interval.
- [ ] The local player sees a first-person viewmodel of the equipped weapon. The viewmodel never clips into nearby world geometry.
- [ ] The local player's body does not appear in the first-person view but casts shadows from dynamic lights.
- [ ] In single-player, the viewmodel still renders and the player body still casts shadows — the presentation is not gated on netcode being active.
- [ ] The wire-version and app-protocol bumps gate the new fields behind the two-gate handshake. A pre-bump client cannot connect to a post-bump host.

## Tasks

### Task 1: Wire bump — aim pitch + weapon archetype identity

Add `aim_pitch: f32` to `WireMovementInput` (alongside `facing_yaw`). The client packs camera pitch into the input command; the host receives it, stores it alongside movement state, and echoes it to other clients.

Add `aim_pitch: f32` to `WirePlayerMovementState`. The host writes the latest received aim pitch from the owning client's input command. Clients consume it for pose-modifier input on remote player pawns.

Add `active_weapon_archetype: Option<String>` to the entity record metadata, valid on non-despawn records carrying `PlayerMovementState` (the same validity gate as `local_player` and `last_processed_client_tick`). The host populates it from the `WeaponOwners` pawn→weapon lookup → weapon entity's descriptor `canonicalName`. Clients consume it for third-person weapon attachment resolution. `None` means no weapon equipped.

Bump `WIRE_VERSION` and `SNAPSHOT_VERSION`. Bump the app-protocol constant if the `InputCommand` vocabulary change requires it (new field layout). A single combined bump covers all three additions. Update the drift-guard tests on both sides of the crate boundary.

### Task 2: Player mesh descriptor + remote materialization

Add a `mesh` block to the player descriptor (`content/dev/scripts/player.ts`): model path pointing to the exo_red character model, animation clips (idle, walk), pose mask extras (`aimSpine`, `upperBody`, `lowerBody`), locomotion descriptor with `travelSpeed`, and a `hand_r` socket for weapon attachment. The exo_red model may need glTF `extras` tags added for pose masks and sockets — add them via a content-prep step or document the tagging requirement.

Add a `thirdPersonModel` field to the weapon descriptor surface (the `weapon` component schema). The reference pistol descriptor gains a `thirdPersonModel` path pointing to the smg model (or a placeholder). Add a `viewmodel` field (consumed in Task 7); initially optional and unused.

Add `materialize_armed_remote_player` to `remote_materialize.rs`. Mirrors `materialize_armed_remote_enemy`: attaches the descriptor's `MeshComponent` presentation only — never `PlayerMovement`, `Weapon`, or `Health`. Idempotent; unknown class leaves the entity transform-only. The client apply path in `crate::netcode` calls this for entity records whose `entity_class` resolves to a player-type descriptor (the player descriptor's `canonicalName`), gated on the entity NOT being the local player. The local player gets mesh materialization too (for shadow-only rendering) but through a separate local path that sets `forward_visible = false` on the collector.

Wire `materialize_armed_remote_player` into the client snapshot-apply path. Currently `materialize_armed_remote_enemy` is called for non-local entities with a known class. Extend the dispatch: if the class resolves to a player-type descriptor, call the player path; otherwise the enemy path. Both share the same presentation-only contract.

### Task 3: Player pose inputs — local + remote

Write `PoseInputs` for player mesh entities. Currently `update_pose_inputs` in `sim/mod.rs` writes aim angles from `BrainComponent.acquired_target` for AI entities only. Add a parallel path for player pawns:

- **Local player:** write `aim_pitch` from the camera's current pitch, `aim_yaw` from the camera's current yaw, `heading_yaw` from the pawn's movement-facing direction. These feed the aim-bend and upper/lower-split modifiers so the local body shadow tracks the player's aim.
- **Remote player:** write `aim_pitch` from the replicated `WirePlayerMovementState.aim_pitch`, `aim_yaw` from the replicated transform's yaw, `heading_yaw` derived from the replicated velocity direction (or transform yaw when stationary). These drive the aim presentation on the remote avatar.

Wire foot-IK ground probes for player pawns. The shipped `update_foot_ground_probes` runs for entities with `MeshComponent.pose_inputs` and legs in the pose stack. Once the player descriptor carries a mesh with IK leg chains, the existing probe path should activate — verify and fix if the probe loop filters by entity type.

Drive client-local locomotion animation from replicated velocity. On each client frame, for each remote player pawn: compute speed from the interpolated velocity, select idle vs walk/run animation state, and set playback rate from `speed / travelSpeed`. This mirrors the existing enemy locomotion logic — port it rather than re-deriving. The animation state name is already replicated via `WireMeshAnimationState`; client-local locomotion overrides the replicated state for smoother visual transitions (replicated state is the fallback/correction).

### Task 4: Local body shadow-only rendering

In the mesh render collector, detect the local player's mesh entity. Set `forward_visible = false` on its `MeshInstanceInput`. The existing mesh-pass infrastructure already handles this: `record_draws` skips `forward_visible = false` instances in the color pass, while `record_skinned_depth` includes them via `MeshDepthInstanceFilter::IncludeShadowOnly`.

Verify that the shadow-depth pipeline (`skinned_depth.wgsl`, group 0 light-space + group 3 palette/instance) correctly renders the shadow-only player instance. The shared palette/instance SSBO (written once by `plan_and_upload` before any shadow pass) already includes all instances regardless of `forward_visible`.

Single-player mode: the same shadow-only treatment applies. The player body shadow is not gated on netcode.

### Task 5: Third-person weapon at hand socket — runtime attach/detach

Add a runtime attach/detach mechanism for weapon models at bone sockets. The shipped socket system resolves attachments from the descriptor at load time and stores them in `MeshComponent.attachments`. This task adds a host-side mutation path: when a player's active weapon changes (tracked by `WeaponOwners`), the host updates the pawn's `MeshComponent.attachments` to mount the new weapon's `thirdPersonModel` at the `hand_r` socket, replacing any previous weapon attachment at that socket.

On connected clients: the replicated `active_weapon_archetype` (from Task 1) drives attachment resolution. When snapshot apply delivers a changed archetype string, the client resolves the weapon descriptor by canonical name, looks up its `thirdPersonModel` path, and updates the local `MeshComponent.attachments` for the remote player pawn. The attachment model must already be loaded (pre-uploaded as part of the descriptor's model set at level load). Socket binding resolution reuses the existing `AttachmentBinding::Skinned` path — the hand socket is a skin joint.

For the local player: the same attachment mutation applies (the third-person weapon renders in the shadow-depth pass alongside the body). The viewmodel (Task 7) renders separately and is not an attachment.

`None` archetype clears the weapon attachment. A weapon archetype whose descriptor has no `thirdPersonModel` also clears it — no phantom weapon.

### Task 6: Split mesh_pass.rs

Behavior-preserving split of `mesh_pass.rs` (3813 lines) before Task 7 extends it. Extract the depth-recording functions (`record_skinned_depth`, `record_rigid_depth`, shadow cone-frustum culling) into a sibling module. The forward draw path and the frame-plan consumption stay in `mesh_pass.rs`. The split reduces the file and gives the viewmodel draw logic (Task 7) a focused module to land in.

No functional changes. All existing tests and visual behavior remain identical.

### Task 7: First-person viewmodel

Render the local player's equipped weapon as a first-person viewmodel through the mesh pass. The viewmodel is a separate model asset (the weapon descriptor's `viewmodel` path), rendered in camera space with a dedicated near-plane projection that prevents clipping into world geometry.

The viewmodel instance is a `MeshInstanceInput` with a flag or variant distinguishing it from world-space mesh instances. The mesh pass draws viewmodel instances after the forward color pass completes, with a depth-clear and a tighter near/far projection (typical: 0.01–2.0 m range, ~70° FOV). The viewmodel reuses the same skinned-mesh pipeline, palette, and material bindings — only the projection and depth state differ.

View-feel integration: the viewmodel's transform incorporates the camera bob and tilt from the existing `view_feel.rs` system. Bob and tilt offsets apply to the viewmodel's camera-space position and rotation, so the weapon sways with player movement.

Weapon switch: when the local player's weapon changes, the viewmodel swaps to the new weapon's `viewmodel` asset. If the asset is not yet loaded (hot-swap edge case), the viewmodel disappears until the load completes — no stale weapon frame.

Single-player mode: the viewmodel renders identically. It reads the local player's weapon from the entity registry (direct lookup), not from the wire.

## Sequencing

**Phase 1 (concurrent):** Task 1 (wire bump), Task 2 (player mesh descriptor + materialization), Task 6 (mesh_pass split) — independent subsystems.

**Phase 2 (concurrent):** Task 3 (pose inputs), Task 4 (shadow-only body), Task 5 (weapon attach/detach) — all consume Phase 1 foundations. Task 3 needs Tasks 1+2. Task 4 needs Task 2. Task 5 needs Tasks 1+2.

**Phase 3 (sequential):** Task 7 (viewmodel) — needs Tasks 5+6 (weapon model loading and the split mesh pass).

## Rough sketch

**Wire additions.** `WireMovementInput` gains `aim_pitch: f32`. `WirePlayerMovementState` gains `aim_pitch: f32`. Entity record metadata gains `active_weapon_archetype: Option<String>` with the same validity gate as movement-authority metadata. The host writes `aim_pitch` from the input command and `active_weapon_archetype` from `WeaponOwners` → weapon entity → descriptor canonical name.

**Remote player materialization.** A new `materialize_armed_remote_player` in `remote_materialize.rs` follows the enemy pattern: presentation-only mesh attachment, no gameplay components. The client-apply dispatch in `crate::netcode` branches on whether the entity class is a player-type or enemy-type descriptor.

**Pose input plumbing.** A new `update_player_pose_inputs` function in `sim/mod.rs` writes `PoseInputs` for player mesh entities. Local player reads camera pitch/yaw; remote player reads replicated state. Both paths produce the same `PoseInputs` shape consumed by the existing modifier stack — no modifier changes needed.

**Viewmodel projection.** The viewmodel renders after the forward color pass, behind a depth-clear, with a near-plane projection (~0.01–2.0 m, ~70° FOV). The mesh pass groups viewmodel instances separately during frame planning and records their draws with the alternate projection bound at group 0. The shared palette/instance SSBO includes viewmodel instances alongside world instances — no separate buffer.

**Oversized file watch.** `mesh_pass.rs` is 3813 lines. Task 6 splits it before Task 7 extends it. `gltf_loader.rs` is 4810 lines but is not extended by this plan.

## Boundary inventory

| Name | Rust | Wire / serde | TS descriptor |
|---|---|---|---|
| Aim pitch | `aim_pitch: f32` | `aim_pitch: f32` (bitcode) | n/a (engine-derived from camera) |
| Active weapon archetype | `active_weapon_archetype: Option<String>` | `Option<String>` (bitcode) | `canonicalName` on weapon descriptor |
| Third-person model | `thirdPersonModel: String` on weapon component | n/a (descriptor-local) | `weapon.thirdPersonModel: "<path>"` |
| Viewmodel | `viewmodel: String` on weapon component | n/a (descriptor-local) | `weapon.viewmodel: "<path>"` |

## Wire format

Three additions to existing wire surfaces — no new section or message type.

**`WireMovementInput`** gains `aim_pitch: f32` after `facing_yaw`. Client-to-server on the Input channel. Bitcode layout change bumps `WIRE_VERSION`.

**`WirePlayerMovementState`** gains `aim_pitch: f32` after `capsule_eye_height`. Server-to-client on the Snapshot channel. Bitcode layout change bumps `SNAPSHOT_VERSION`.

**Entity record metadata** gains `active_weapon_archetype: Option<String>` with the same validity gate as movement-authority metadata (valid only on records carrying `PlayerMovementState`). Bitcode layout change is part of the `SNAPSHOT_VERSION` bump.

All three are covered by a single combined version bump across `WIRE_VERSION`, `SNAPSHOT_VERSION`, and the app-protocol constant. The two-gate handshake rejects mismatched peers before any payload is decoded.

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
      animation: {
        states: {
          idle: { clip: "Idle", loop: true },
          walk: { clip: "Walking", loop: true },
        },
        initial: "idle",
      },
      locomotion: { syncMode: "speedScaled" },
      attachments: {
        hand_r: "", // populated at runtime from weapon archetype
      },
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
      viewmodel: "dev/models/smg-viewmodel",
      resource: { /* ... existing ... */ },
    },
  },
});
```

## Open questions

- **Exo_red glTF tagging.** The exo_red model may lack `extras` tags for pose masks (`aimSpine`, `upperBody`, `lowerBody`), socket names (`hand_r`), and `aimBendWeight` per joint. These must be added before the model can drive the modifier stack. Determine whether to tag via Blender re-export or a content-prep script.
- **Viewmodel asset.** No viewmodel model asset exists for the reference pistol. Phase 3 requires either a dedicated viewmodel mesh or a repurposed third-person model rendered at viewmodel scale. A placeholder (the smg model rendered in camera space) can unblock implementation.
- **Locomotion animation clips.** The exo_red Mixamo export may not include walk/run clips. Verify available clips; download additional Mixamo animations if needed.
- **mesh_pass.rs split seams.** Task 6 identifies the split boundary — depth-recording extraction is the likely seam, but the implementer should evaluate whether frame-plan extraction or instance-filter extraction gives a cleaner cut at the current 3813-line shape.

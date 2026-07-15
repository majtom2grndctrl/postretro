# Pose-Modifier Stack + Aim Fidelity

## Goal
Add a post-sample pose-modifier stack to the animation runtime: an ordered, per-bone-masked list of modifiers that transforms local joint poses AFTER clip sampling/blending and BEFORE the bone palette builds, consuming per-frame inputs computed in the game-logic tick. Land the stack alone first (headless, proven by scalar aim inputs), then ship aim-pitch spine bend and the upper/lower-body split on it. This is the load-bearing Epic 21 seam: get it right and aim bend, foot IK, recoil, NPC head-look, and executions all become additive modifiers rather than bespoke pose hacks.

## Scope

### In scope
- A pose-modifier stack in `crates/model` that runs over the per-joint local-TRS buffer between blend/sample resolution and palette composition. Wgpu-free and collision-free — modifiers consume externally-supplied inputs only, never query world state.
- Per-bone masking: each modifier is scoped to a named joint set derived from glTF `extras` tags at load, mirroring the hit-zone tagging mechanism.
- Two shipped modifiers: aim-pitch spine bend (distributes a scalar pitch across a tagged spine chain) and upper/lower-body split (upper joints track aim yaw, lower joints keep the clip/velocity heading).
- Routing the single-clip sample path through a materialized local-TRS buffer so cross-joint modifiers see the whole pose (today it fuses sample+compose per joint).
- External-input plumbing: a per-entity pose-input side channel written in the fixed tick, read by the render collector, carried into the renderer's palette build.
- A behavior-preserving split of `crates/model/src/anim.rs` (~1437 lines) before it is extended.
- Headless determinism tests exercising the stack with scalar aim inputs.

### Out of scope
- Foot IK, ground probes, the locomotion descriptor, and `travelSpeed` calibration — the sibling plan `E21--foot-ik-locomotion-descriptor`, which depends on this one.
- Recoil, NPC head-look, execution/animation-lock modifiers — future modifiers on this stack; only the two above ship here.
- Any change to hit-zone authority. Hit-zone capsules keep reading the UNmodified world pose; the authoritative aim ray stays the replicated/game-logic pitch-yaw, never the bent spine. Aim bend is presentation only.
- Local first-person / remote avatar presentation and any wire change — the `E21` co-op-avatar plan.
- Bone sockets / attachments — a separate Epic 21 plan.

## Acceptance criteria
- [ ] A model with no pose-modifier tags renders a bone palette byte-identical to the pre-change path; no measurable per-frame cost when the stack is empty.
- [ ] A model whose spine joints are tagged as an aim chain, fed a scalar aim-pitch input, renders with those joints rotated so the chain tip pitches by the input angle, the bend distributed across the chain; joints outside the chain are unchanged.
- [ ] With upper-body and lower-body joint sets tagged and fed differing aim-yaw and heading inputs, upper-body joints track the aim yaw while lower-body joints keep the clip heading; joints in neither set are unchanged.
- [ ] Two modifiers whose masks overlap compose in list order — the later modifier observes the earlier one's output — verifiable from the rendered pose.
- [ ] Aim inputs computed in the fixed tick reach the rendered palette: an in-engine enemy that has acquired a target bends its torso toward that target while its legs keep the travel heading.
- [ ] Given identical scalar aim inputs, the produced palette is identical across repeated headless runs.
- [ ] Hit-zone hit capsules are unaffected by an active aim-bend modifier — a bent spine does not move the authoritative hit geometry.
- [ ] After the `anim.rs` split, every pre-existing `postretro-model` test passes unchanged and no resulting source file exceeds ~800 lines.

## Tasks

### Task 1: Split `anim.rs` into a module directory (behavior-preserving)
`crates/model/src/anim.rs` is ~1437 lines and this plan extends it; split it first along seams already present, changing no behavior. Convert to an `anim/` directory with `mod.rs` re-exporting the current public surface (`sample_clip`, `sample_clip_looped`, `sample_blended`, `sample_clip_looped_world`, `sample_blended_world`, `capture_blend`, `Loop`, `LocalTrs`, `BlendSource`) so no downstream caller changes — the renderer's `mesh_pass.rs` and `hit_zones.rs` imports must keep resolving. Suggested cuts: track-level sampling (`sample_local_trs`, `sample_local_pose`, `locate_span`, `sample_vec3_track`, `sample_quat_track`) into `anim/track.rs`; the hierarchy/palette core (`compose_world_pose`, `compose_palette`) into `anim/compose.rs`; the two-source blend machinery (`blend_local`, `resolve_blend_into`, `capture_blend`) into `anim/blend.rs`; the type/time layer (`Loop`, `LocalTrs`, `BlendSource`, `resolve_time`) into `anim/types.rs`; public samplers stay in `mod.rs`. Move the in-file `#[cfg(test)] mod tests` alongside their targets or keep as one `anim/tests.rs`. The thread-local scratch buffers (`WORLD_POSE_SCRATCH`, `BLEND_LOCAL_SCRATCH`) stay accessible to the compose/blend submodules. All existing tests must pass with no assertion edits.

### Task 2: Pose-modifier stack core in `crates/model`
Add the stack primitives to `crates/model` (wgpu-free, collision-free). Define a per-bone mask type scoping a modifier to a set of joint indices (skeleton/topo order, ≤256 joints), a `PoseModifier` value type (start with the two variants Task 3 fills; leave the set open for later foot-IK/recoil variants), and an ordered `PoseModifierStack` of (mask, modifier) entries applied in list order. Add a per-frame `PoseInputs` POD carrying the aim inputs modifiers read (aim pitch and aim yaw as radians; leave room for the sibling plan's foot-probe array). Add modified sampler entry points that materialize the per-joint `LocalTrs` buffer, run the stack over it in order (each modifier mutates only its masked joints), then compose the palette — a blended variant reusing `resolve_blend_into`, and a single-clip variant that must first materialize a `LocalTrs` buffer per joint (the current single-clip path fuses sample+compose in a closure and never builds the buffer, so this is a real change). Empty stack or absent inputs must fall through to the existing unmodified compose with no added allocation or math. Modifiers operate purely on the local-TRS buffer; a modifier needing world-space joint positions (e.g. to distribute a bend) computes them by walking the masked chain's local transforms itself — it never calls collision or wgpu. Do NOT add a modified variant to the world-pose samplers (`sample_*_world`) used by hit-zones: those must keep returning the unmodified pose so hit-zone authority is untouched.

### Task 3: Aim-pitch spine bend and upper/lower-body split modifiers
Implement the two shipped `PoseModifier` variants from Task 2, operating on the masked local-TRS joints. Aim-pitch spine bend: given a scalar aim-pitch from `PoseInputs` and a spine-chain mask ordered root→tip, apply an incremental pitch rotation per chain joint summing to the input pitch, so the chain tip faces the aim pitch and the bend spreads smoothly across the chain; joints outside the mask are untouched. Upper/lower-body split: given an upper-body mask and a lower-body mask plus aim-yaw and a heading yaw from `PoseInputs`, rotate upper-body joints so the torso tracks aim-yaw while leaving lower-body joints on their sampled (velocity-driven) heading; where the masks meet, the boundary joint blends so the twist is not a hard step. Both modifiers must be no-ops when their mask is empty. Order matters: the split runs before the pitch bend (or document the fixed order) so overlapping spine joints compose predictably; pin the order in the stack the loader builds.

### Task 4: External-input plumbing — tick writes, collector reads, renderer applies
Wire game-logic-computed aim inputs to the stack across the wgpu-free boundary. Add an optional per-frame pose-input field to `MeshComponent` (`crates/entities`) holding the entity's current `PoseInputs`; it is transient runtime state (serde-skipped), written each fixed tick and read the same frame. In `simulate_tick` (`crates/postretro/src/sim/mod.rs`), after steering/AI resolves and alongside `update_brain_animation_playback_rates`, compute each animated entity's aim inputs and store them on its `MeshComponent`: for AI entities, aim yaw/pitch toward `BrainComponent.acquired_target`'s position when a target is held, else the `agent_steering::path_state` velocity heading; heading yaw is the velocity heading. This write MUST stay in the tick because the render collector borrows the registry immutably. The collector `MeshRenderCollector::collect_inner` (`crates/postretro/src/scripting/systems/mesh_render.rs`) reads the component's `PoseInputs` and packs them, plus a reference to the model's static modifier stack (Task 5), into the per-instance render input (`MeshInstanceInput`, a copyable payload) next to the existing sample params. The renderer's mesh pass (`crates/renderer/src/render/mesh_pass.rs`), which today calls `sample_blended` / `sample_clip_looped` to build the palette, calls the modified sampler from Task 2 when the instance carries a non-empty stack and inputs, and the unmodified sampler otherwise. Enumerate and update those call sites. Name the new instance-payload fields in the sketch, not here-decided per crate.

### Task 5: Load-time joint-mask tagging from glTF `extras`
Extend the model loader (`crates/model/src/gltf_loader.rs`) to read per-joint pose-mask tags from node `extras`, mirroring `read_joint_zone` / `JointZone`: a per-node key names the mask(s) the joint belongs to (mask names `aimSpine`, `upperBody`, `lowerBody`; a joint may belong to several). Reindex the collected memberships through the same topo remap the loader already applies to `joint_zones`, producing per-mask joint-index sets parallel to `Skeleton::joints`. From those sets, build the archetype's `PoseModifierStack` at load (convention-driven for this plan: an `aimSpine` mask yields an aim-pitch-bend entry; `upperBody`+`lowerBody` masks yield an upper/lower-split entry; absent masks yield an empty stack). Surface the built stack and mask sets on `LoadedModel` next to `joint_zones` so both the renderer's model store (palette build) and the game side can hold it. Malformed/unknown mask names are ignored with a load-time warning, never a hard failure. Spine-chain ordering (root→tip) derives from the skeleton parent links across the `aimSpine` set.

### Task 6: Headless stack tests with scalar aim inputs
Prove the stack headlessly. Add `crates/model` unit tests (CPU-only, like the existing `anim` tests) that build a synthetic skeleton with masked chains, apply the stack with scalar aim inputs, and assert: empty stack equals the unmodified palette; a tagged aim chain pitches its tip by the input while off-chain joints hold; upper/lower split twists only the upper mask; two overlapping modifiers compose in list order; repeated runs with equal inputs produce identical palettes. Add one sim-level headless test in the determinism harness (`crates/postretro/src/sim/determinism_tests.rs`, following `simulate_tick_scales_walk_rate_...`) that drives `simulate_tick` for an AI entity with an acquired target and asserts the tick writes deterministic aim inputs onto the mesh component. No GPU/window in any test.

## Sequencing

**Phase 1 (sequential):** Task 1 (split `anim.rs`) → Task 2 (stack core) → the `crates/model` portion of Task 6 (headless stack unit tests). This is the whole epic's foundation: the stack lands alone and is proven by scalar aim inputs before any renderer or game-logic wiring.
**Phase 2 (concurrent after Phase 1):** Task 3 (aim modifiers), Task 5 (load-time mask tagging) — disjoint files. Task 3 depends on Task 2's variant enum; Task 5 depends on Task 2's stack type.
**Phase 3 (sequential):** Task 4 (plumbing across tick → collector → renderer) — consumes Task 3's modifiers and Task 5's per-model stack. Then the sim-level test in Task 6.

## Rough sketch
- New in `crates/model` (in the `anim/` module or a sibling `pose_mod.rs`): `JointMask` (a joint-index set over ≤`MAX_JOINTS` joints), `PoseModifier { AimPitchBend { .. }, UpperLowerSplit { .. } }` (open for later variants), `ModifierEntry { mask: JointMask, modifier: PoseModifier }`, `PoseModifierStack(Vec<ModifierEntry>)`, `PoseInputs { aim_pitch: f32, aim_yaw: f32, heading_yaw: f32 }` (POD, `Copy`; the sibling plan adds a foot-probe array). New samplers: `sample_blended_modified(a, b, weight, skeleton, stack, inputs, out)` and `sample_clip_looped_modified(clip, skeleton, time, loop_policy, stack, inputs, out)`, both inserting the stack at the current `sample_blended` seam — right after the `Vec<LocalTrs>` buffer is resolved and before `compose_palette`.
- Loader: a `PoseMaskExtras` deserialize shape and `read_pose_masks(node.extras)` beside `read_joint_zone`; `LoadedModel.pose_masks: PoseMaskSet` and `LoadedModel.pose_stack: PoseModifierStack`.
- `MeshComponent.pose_inputs: Option<PoseInputs>` (serde-skip). Writer: a new `update_pose_inputs(registry, ...)` step in `sim/mod.rs` next to `update_brain_animation_playback_rates`, reading `BrainComponent.acquired_target` and `agent_steering::path_state`.
- Renderer instance payload (`MeshInstanceInput` in `postretro-render-cpu`): a `pose_inputs: Option<PoseInputs>` field and a handle/reference to the model's `PoseModifierStack`; `mesh_pass.rs` branches to the modified sampler when both are present.

## Boundary inventory
Pose-mask tags cross the glTF-authoring boundary only (no TS/Luau/wire surface in this plan). Casing is authoring-facing camelCase, mirrored to Rust mask identifiers.

| Name | glTF node `extras` key/value | Rust |
|---|---|---|
| pose-mask membership | key `poseMask`, string or string-array value | `read_pose_masks` → `PoseMaskSet` |
| aim spine chain | `poseMask: "aimSpine"` | mask `PoseMask::AimSpine` |
| upper body | `poseMask: "upperBody"` | mask `PoseMask::UpperBody` |
| lower body | `poseMask: "lowerBody"` | mask `PoseMask::LowerBody` |

Existing precedent: hit-zone tags use node `extras` keys `hitZone` / `hitZoneRadius` (`gltf_loader.rs` `JointZoneExtras`). Follow the same per-node read + topo-remap path.

## Open questions
- Enemy aim source: this plan aims the torso at `BrainComponent.acquired_target` when held, else the velocity heading. If designers want enemies to aim only in specific logical states (e.g. Attack), that gate is a small follow-on and belongs with the locomotion descriptor in the sibling plan rather than here.
- Fixed modifier order (split before pitch bend) is pinned by the loader. If a later modifier (recoil, head-look) needs a different interleave, the stack order becomes descriptor-authored — deferred until a third modifier lands.

# Lighting — Dynamic Spot Shadow Pool

> **Status:** ready.
> **Built on:** `lighting-dynamic-flag/` (shipped — `MapLight.is_dynamic` available). `lighting-old-stack-retirement/` should ship first. Uses the `blinn_phong` shader utility introduced in `lighting-chunk-lists/` — see the utility-ownership note in Task B.
> **Concurrent with:** `lighting-lightmaps/`, `lighting-sh-amendments/`, `lighting-chunk-lists/`. See the bind-group coordination table in Task B.
> **Related:** `context/lib/rendering_pipeline.md` §4 · `context/plans/in-progress/lighting-foundation/3-direct-lighting.md` (existing dynamic direct loop, extended here).

---

## Context

Static lights bake diffuse, shadow, and specular through lightmaps and the chunk-list spec buffer. Dynamic lights — authored with `_dynamic 1` — have no bake representation. They evaluate fully at runtime via the existing per-fragment direct loop (from `lighting-foundation/3-direct-lighting.md`).

Dynamic spot lights are the primary use case: muzzle flashes, flashlights, narrative accent lights, and destructible fixtures mid-transition. They need runtime hard shadows. This plan adds a small pool of 2D perspective shadow maps for dynamic spots: one shadow map per slot, rendered once per frame per allocated slot.

Dynamic point lights are supported unshadowed — no cube maps in this iteration. Dynamic directional lights are not supported (sun bakes into the lightmap; if a dynamic directional is encountered, log a warning at load time).

---

## Goal

A pool of 8 2D depth-texture shadow map slots (retunable). Per frame: allocate visible dynamic spot lights to slots, render a depth pass per slot, sample in the fragment shader for hard shadow occlusion. Nearest-neighbor sampling throughout — hard-edged, retro-appropriate.

---

## Approach

Runtime-only plan. No compiler or PRL changes. Single workstream, but the shadow map render passes and the fragment shader sample path can be developed and tested independently against a simple test case.

---

## Task A — Shadow map pool + slot allocation

**Crate:** `postretro` · **New module:** `src/lighting/spot_shadow.rs` · **Also modifies:** `src/render/mod.rs`.

1. **Pool.** 8 slots (retunable via constant), each a 1024×1024 `Depth32Float` 2D texture, stored as a depth texture array. Allocated once at renderer init. Memory budget: 8 × 1024² × 4 bytes = **32 MiB VRAM**. If this proves tight, the first lever is dropping format to `Depth16Unorm` (halves to 16 MiB) before reducing resolution or slot count.
2. **Slot assignment.** Per frame, from the set of visible dynamic spot lights (those passing the existing influence-volume frustum cull): rank by the heuristic below and assign the top 8 to slots. Lights without a slot this frame render unshadowed (still contribute diffuse + specular through the normal direct loop).

   **Heuristic (pinned):** approximate screen-space projected area of the influence sphere:
   ```
   score = (falloff_range / max(distance(camera, light.origin), camera.near_clip))²
   ```
   Frustum-cull pre-filter guarantees the light is in front of the camera, so the clamped denominator cannot flip sign. This is the influence-sphere proxy, not the cone silhouette — cheaper to compute, ranks the same way in practice for cone half-angles under ~60°. Ties broken by stable light index so slot assignment is deterministic frame-to-frame.

   **User-tagged priority is out of scope for this plan.** A follow-up can add a `_priority` FGD property with its own plumbing plan (mirroring `lighting-dynamic-flag/`) if playtesting shows the heuristic ranks muzzle flashes or narrative accents wrong. Retro-scale dynamic spot counts (≤ ~12 visible at once) make the heuristic sufficient for v1.
3. **View matrix.** Per assigned slot, build a perspective projection from the spot light's position and aim direction with `fov = 2 * cone_angle_outer` and a depth range of `[0.05, falloff_range]` (near clip small enough not to clip geometry embedded in the fixture).
4. **Slot-index publication.** Uploaded as a `u32` per dynamic light, written into the previously-reserved pad at bytes 56–59 of the `GpuLight` record (see `postretro/src/lighting/mod.rs` — the `cone_angles_and_pad` slot's z component). Sentinel `0xFFFFFFFF` = no slot. Dynamic lights are already repacked each frame, so this costs nothing extra. `GPU_LIGHT_SIZE` stays 64 bytes; existing tests update to assert the new field.
5. **Dynamic directional (`light_sun` with `_dynamic 1`) guard.** At load time, emit a `warn!` log naming the entity and treat it as **unshadowed** (it still runs in the direct loop for diffuse + specular). This matches the point-light treatment — dynamic directionals are not dropped, just never receive a shadow slot. Cube maps and CSM are explicitly out of scope (`Out of scope` below).

### Task A acceptance gates

- Pool allocates 8 depth textures at renderer init without panic.
- With two dynamic spot lights in a test scene, both receive slots (confirmed by logging slot assignment).
- With nine dynamic spot lights in the same test scene, eight receive slots and one is unshadowed; identity of the unshadowed light matches the lowest heuristic score (confirmed by logging the full ranked list).
- Moving the camera so a previously far light becomes nearest causes slot reassignment on the next frame (confirmed by logging).
- A `light_sun` entity with `_dynamic 1` produces a `warn!` at load time and renders unshadowed but lit (diffuse + specular visible).

---

## Task B — Shadow pass + fragment shader sampling

**Crate:** `postretro` · **New shader:** `src/shaders/spot_shadow.wgsl` · **Also modifies:** `src/shaders/forward.wgsl`, `src/render/mod.rs`.

1. **Depth pass.** One render pass per allocated slot. Depth-only pipeline (`spot_shadow.wgsl`): simple vertex transform into the slot's light-space projection; no fragment output.

   **Culling per slot.** Reuse the existing BVH traversal compute pass (`rendering_pipeline.md` §7.1) with the slot's view-projection matrix in place of the camera's. The compute pass already frustum-tests each BVH leaf's AABB; it produces a per-slot indirect draw list written to its own indirect buffer. One `multi_draw_indexed_indirect` per material bucket per slot, same pattern as the forward pass. No separate scene representation needed.

   **Depth bias** (pipeline state, constant across slots):
   - `depth_bias` (constant): `2`
   - `depth_bias_slope_scale`: `1.5`
   - `depth_bias_clamp`: `0.0`
   - Cull mode: `Front` (render back-faces only; avoids self-shadow acne on lit surfaces and is standard for opaque shadow casters).

   These values are starting points lifted from common shadow-map defaults; retune at landing if acne or peter-panning is visible on the test scene.
2. **Light-space matrices.** Upload per-frame as a storage buffer of 8 `mat4x4<f32>` entries (one per slot), indexed by the slot ID published in the `GpuLight` record (Task A step 4). Unallocated slots hold identity — never sampled because the fragment-side branch keys on the `0xFFFFFFFF` sentinel.
3. **Bind group (group 2) — cross-plan layout.** This plan, `lighting-lightmaps/`, and `lighting-chunk-lists/` all extend group 2. To prevent collisions, pre-assign binding ranges here; the three plans adopt this table as they land (first-lander wires the full layout with placeholder entries for the other two, so later landers only fill in content):

   | Binding | Owner plan | Resource |
   |---------|------------|----------|
   | 0 | `lighting-lightmaps/` | Lightmap atlas texture |
   | 1 | `lighting-lightmaps/` | Lightmap sampler |
   | 2 | `lighting-chunk-lists/` | Spec-only light buffer |
   | 3 | `lighting-chunk-lists/` | Chunk grid metadata uniform |
   | 4 | `lighting-chunk-lists/` | Chunk offset table storage buffer |
   | 5 | `lighting-chunk-lists/` | Chunk light-index list storage buffer |
   | 6 | **this plan** | Spot shadow depth texture array (8 × 2D) |
   | 7 | **this plan** | Comparison sampler (nearest, `Less`) |
   | 8 | **this plan** | Light-space matrix storage buffer (8 × mat4) |

   Whichever plan lands first creates the bind group layout with all nine entries; the other two replace their placeholder bindings with real resources on landing. This avoids the re-layout churn that forced the SDF/CSM retirement coordination.
4. **Fragment sampling.** In `forward.wgsl`, for each dynamic spot light in the direct loop:
   - Read the slot index from the `GpuLight` pad field. If it equals `0xFFFFFFFF`, emit unshadowed and continue.
   - Otherwise, transform `world_position` through `light_space_matrices[slot]` into the slot's clip space; reject fragments outside `[0,1]² × [0,1]` (behind/outside the cone's projection) as unshadowed.
   - Sample the depth texture array at `(uv, slot)` with `textureSampleCompare` using the comparison sampler. Use nearest filtering — hard-edged, retro-appropriate.
   - Multiply the light's direct contribution (diffuse + Blinn-Phong specular) by the shadow factor.

   **`blinn_phong` utility ownership.** The shared utility is introduced by `lighting-chunk-lists/`. Landing order:
   - **If `lighting-chunk-lists/` lands first:** this plan imports and uses the utility unchanged.
   - **If this plan lands first:** inline a local copy in `forward.wgsl` (same signature and body as specified in `lighting-chunk-lists/`). When `lighting-chunk-lists/` lands, its PR is responsible for deleting the inline copy and replacing call sites with the shared utility — this cleanup is explicitly on the `lighting-chunk-lists/` checklist, not deferred.

### Task B acceptance gates

- A dynamic spot light in a test scene casts a visible hard-edged shadow.
- Toggling `_dynamic 0` on the same spot removes it from the runtime dynamic pool: no slot allocated, no shadow pass rendered, no entry in the dynamic light buffer. (The bake-side behavior — the light now contributes to the lightmap — is gated by `lighting-lightmaps/` landing and is tested there, not here.)
- A dynamic spot light with no slot renders unshadowed but still contributes diffuse and specular.
- Self-shadow acne and peter-panning are both absent on flat and angled surfaces under a dynamic spot at multiple angles. If either appears, retune the bias constants before landing and record the final values in the PR description.
- Back-face cull (`Cull::Front`) is applied only to the shadow pass; the forward pass's existing back-face culling is unchanged (grep-verified).

---

## Acceptance Criteria (both tasks)

1. `cargo test -p postretro` passes.
2. `cargo clippy --workspace -- -D warnings` clean.
3. No new `unsafe`.
4. Task A and Task B acceptance gates above.
5. With 8 active dynamic spot shadows on the test scene, the sum of per-slot shadow-pass GPU time (measured via `POSTRETRO_GPU_TIMING=1` with a new `spot_shadow` pass label) stays under **25% of the forward-pass GPU time** on the same frame. Relative budget so the gate is portable across hardware; the author's machine and absolute numbers go in the PR description for reference.
6. Total VRAM for the shadow pool matches the documented 32 MiB (or 16 MiB if `Depth16Unorm` is chosen at landing); recorded in the PR description.
7. No visual artifact on static geometry — the shadow pass must not incorrectly depth-test against the lightmap path. Specifically: a static lightmap-shadowed surface in the same frame as a dynamic spot shadow shows both shadow sources without one overwriting the other.

---

## Out of scope

- Dynamic point light shadows (cube maps, dual-paraboloid). Unsupported in this iteration.
- Dynamic directional light shadows (CSM replacement). Unsupported; directionals bake into the lightmap.
- PCF, VSM, or any soft-shadow technique on dynamic spots. Hard nearest-neighbor sampling by design.
- More than 8 shadow-map slots in this plan. Retune the constant if profiling demands.
- **Any user-authored priority surface** (`_priority` or similar FGD property). Heuristic-only ranking for v1; add a follow-up plan mirroring `lighting-dynamic-flag/`'s plumbing if playtesting shows the heuristic misranks important lights.
- Soft depth bias tuning beyond the starting constants in Task B step 1. Retune during landing; no further iteration planned.

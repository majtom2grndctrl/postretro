# Lighting — Dynamic Spot Shadow Pool

> **Status:** draft.
> **Depends on:** `lighting-dynamic-flag/`. `lighting-old-stack-retirement/` should ship first. Uses the `blinn_phong` shader utility introduced in `lighting-chunk-lists/` — coordinate timing or duplicate the function if this plan lands first.
> **Concurrent with:** `lighting-lightmaps/`, `lighting-sh-amendments/`, `lighting-chunk-lists/`.
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

1. **Pool.** 8 slots (retunable via constant), each a 1024×1024 `Depth32Float` 2D texture, stored as a depth texture array. Allocated once at renderer init.
2. **Slot assignment.** Per frame, from the set of visible dynamic spot lights (those passing the existing influence-volume frustum cull): rank by priority and assign slots. Default priority heuristic: screen-space projected area of the cone. User-tagged priority (`_priority` int property on the light entity, range 0–9, default 5) is a multiplier on the heuristic. Lights without a slot this frame are unshadowed. Exact heuristic formula documented at landing.
3. **View matrix.** Per assigned slot, build a perspective projection from the spot light's position and direction with `fov = 2 * cone_half_angle` and a depth range fitted to the influence sphere radius.
4. **Dynamic directional guard.** If a dynamic directional light is encountered at load time, emit a `warn!` log and exclude it from the direct loop (or emit unshadowed — implementation choice documented at landing).

### Task A acceptance gates

- Pool allocates 8 depth textures at renderer init without panic.
- With two dynamic spot lights in a test scene, both receive slots (confirmed by logging slot assignment).
- With nine dynamic spot lights in the same test scene, eight receive slots and one is unshadowed (confirmed by logging).

---

## Task B — Shadow pass + fragment shader sampling

**Crate:** `postretro` · **New shader:** `src/shaders/spot_shadow.wgsl` · **Also modifies:** `src/shaders/forward.wgsl`, `src/render/mod.rs`.

1. **Depth pass.** One render pass per allocated slot. Depth-only pipeline (`spot_shadow.wgsl`): simple vertex transform into the slot's light-space projection; no fragment output. Uses the existing geometry buffers — no separate scene representation needed.
2. **Light-space matrices.** Upload per-frame slot matrices (light-view × projection per slot) as a small uniform or storage buffer.
3. **Bind group.** Add the depth texture array and light-space matrix buffer to group 2. Coordinate with `lighting-lightmaps/` and `lighting-chunk-lists/` to avoid binding slot collisions.
4. **Fragment sampling.** In `forward.wgsl`, for each dynamic spot light in the direct loop:
   - If the light has an assigned slot, transform `world_position` into the light's clip space.
   - Sample the depth texture at the projected UV with a nearest-neighbor comparison sampler.
   - Multiply the light's direct contribution (diffuse + Blinn-Phong specular via the utility from `lighting-chunk-lists/`) by the shadow factor.
   - If no slot, emit unshadowed.

### Task B acceptance gates

- A dynamic spot light in a test scene casts a visible hard-edged shadow.
- Toggling `_dynamic` off (making the spot static) removes its contribution from the runtime pool and shifts it to the lightmap bake — shadow is now baked (confirmed by recompiling + re-running).
- A dynamic spot light with no slot renders unshadowed but still contributes diffuse and specular.

---

## Acceptance Criteria (both tasks)

1. `cargo test -p postretro` passes.
2. `cargo clippy --workspace -- -D warnings` clean.
3. No new `unsafe`.
4. Task A and Task B acceptance gates above.
5. Frame time with 8 active dynamic spot shadows does not exceed budget: total shadow passes should stay under 2 ms on dev hardware (`POSTRETRO_GPU_TIMING=1`).
6. No visual artifact on static geometry — the shadow pass must not incorrectly depth-test against the lightmap path.

---

## Out of scope

- Dynamic point light shadows (cube maps, dual-paraboloid). Unsupported in this iteration.
- Dynamic directional light shadows (CSM replacement). Unsupported; directionals bake into the lightmap.
- PCF, VSM, or any soft-shadow technique on dynamic spots. Hard nearest-neighbor sampling by design.
- More than 8 shadow-map slots in this plan. Retune the constant if profiling demands.
- Priority authoring tooling beyond the `_priority` integer — FGD documentation sufficient.

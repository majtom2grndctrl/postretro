# FX — Volumetric Smoke + Billboard Sprites

> **Status:** ready.
> **Depends on:** `lighting-old-stack-retirement/` (clears old shadow paths). `lighting-chunk-lists/` (Blinn-Phong utility + chunk list for multi-source lit sprites and fog scatter). `lighting-spot-shadows/` (shadow maps required for visible beam shafts in Task B; Task B degrades gracefully without them).
> **Concurrent with:** nothing — this plan consumes the completed lighting stack rather than contributing to it. Ship after the four concurrent lighting plans are complete.
> **Related:** `context/lib/rendering_pipeline.md` §7.3 (billboards and fog volumes called out as deferred) · `context/lib/build_pipeline.md` §Custom FGD (`env_fog_volume` entity already defined) · `context/lib/resource_management.md` §4 (sprite sheet convention).

---

## Context

The lighting stack (lightmaps, SH volumes, chunk-list specular, dynamic spot shadow maps) provides rich per-position lighting data with zero additional cost at runtime. This plan puts that data to work on two visual systems that define the "retro but embellished" aesthetic goal:

1. **Billboard smoke sprites** — camera-facing animated quads, lit by the full lighting stack (static multi-source specular, SH ambient, dynamic direct). Smoke sprites catch colored light from nearby neon signs, receive Blinn-Phong highlights from strong spots, and fill with SH-tinted ambient in shadow.

2. **Volumetric fog beams** — a low-resolution raymarched pass over `env_fog_volume` brush regions. Each sample accumulates scatter from dynamic spot lights (with shadow map occlusion for visible beam shafts and shadow wedges) and SH-tinted ambient for unlit haze. Rendered at quarter-res and composited with nearest-neighbor upscaling — the stepped pixel blocks are aesthetic, not a compromise.

Together: smoke billowing through a corridor lit by a sweeping searchlight picks up the neon colors from the chunk list, gets cut by hard-edged shadow wedges from pillars, glows with Blinn-Phong highlights on the sprite surfaces facing the beam, and fills SH-blue in the shadow. The volumetric beam itself is visible as a pixelated shaft.

---

## Goal

- **Task A:** Billboard smoke sprite rendering, lit by the full stack.
- **Task B:** Low-resolution volumetric fog/beam pass over `env_fog_volume` regions.

Both render after the opaque world geometry pass (§7.3), before post-processing. Pass order within the render stage: opaque world geometry → billboard sprite pass (Task A) → fog volume composite (Task B) → Present.

---

## Concurrent workstreams

Task A and Task B can be developed independently. Task A requires `lighting-chunk-lists/` to land first for full multi-source specular; it has a fallback (SH + nearest dynamic light) until then. Task B requires `lighting-spot-shadows/` for visible beams; it has a fallback (SH ambient scatter only, no beam shafts) until then.

```
Task A (billboard sprites): camera-facing quads + full lighting stack ─── independent
Task B (volumetric fog pass): low-res raymarch + shadow map beams ──────── independent (fallback active)
```

---

## Task A — Billboard smoke sprites

**Crate:** `postretro` · **New module:** `src/fx/smoke.rs` · **Also modifies:** `src/render/mod.rs`, `src/shaders/billboard.wgsl` *(new)*.

### Sprite system

1. **Emitter entities.** `env_smoke_emitter` point entity (FGD addition): properties `rate` (sprites/sec, default 4), `lifetime` (seconds, default 3.0), `size` (world units, default 0.5), `speed` (drift velocity, default 0.3), `collection` (sprite sheet collection name), `spec_intensity` (Blinn-Phong specular scale, default 0.3 — controls how strongly chunk-list static lights highlight the sprite surface). Emitter data resolved from BSP entity lump at level load; CPU updates per-frame in the game logic stage.

2. **Sprite representation.** Each live sprite: world position, age, size, rotation (slow random spin), opacity (fade in/out over lifetime). CPU-side ring buffer, max 512 sprites per emitter (retunable constant). Uploaded to a GPU storage buffer each frame — one entry per live sprite.

3. **Camera-facing quad generation.** Vertex shader expands each sprite entry into a camera-facing quad using the view matrix right and up vectors. No geometry shader — non-indexed draw of `6 × N` vertices. `sprite_index = vertex_index / 6u`; corner is selected by `lookup[vertex_index % 6u]` where `lookup = {0,1,2, 2,1,3}` indexes into a 4-corner table (TL, TR, BL, BR). UV maps to the correct animation frame from the sprite sheet based on `age / frame_duration`.

4. **Sprite sheet convention.** Smoke animation frames live under `textures/<collection>/smoke_00.png`, `smoke_01.png`, … following the existing sequential-frame convention in `resource_management.md` §4. Frame count derived from directory listing at load time.

5. **Render pass.** Alpha-blended additive pass immediately after the opaque world pass, before the fog volume composite. Depth write disabled; depth test enabled (`Less` — sprites occlude behind geometry but not each other). Draw calls batched by `collection`: all emitters sharing the same sprite sheet issue a single `draw(6 * total_live_sprites_for_collection)` per frame. Emitters with different collections issue separate draws. At retro-scale emitter counts (≤ ~8 per scene) this produces at most a handful of draw calls.

### Lighting a sprite

Per billboard vertex (hoisted to the vertex shader where possible, refined in fragment):

6. **SH ambient.** Sample the SH irradiance volume at the sprite world position (same trilinear path as the world shader). Provides ambient fill color — SH-blue in shadow, warm-orange near a torch, etc.

7. **Multi-source static specular.** Look up the sprite's chunk cell from world position; iterate the chunk light list from `lighting-chunk-lists/`. For each nearby static light, evaluate `blinn_phong(L, V, N, color, spec_exp, spec_int)` with `N = camera_forward` (sprite faces camera) and a low `spec_exp` (≈ 4.0 — broad, soft highlight appropriate for translucent smoke). `spec_int` is a per-emitter scalar (FGD property `spec_intensity`, default 0.3).

8. **Dynamic direct.** Iterate active dynamic lights with influence-volume early-out. Same path as the world shader's dynamic loop. Evaluate diffuse only (no specular from dynamic lights on sprites — the broad SH ambient is sufficient for the indirect contribution of dynamic lights; sharp dynamic specular on billboards reads as artifact).

9. **Composite lighting.** Final sprite color: `sprite_albedo_sample × (sh_ambient + static_specular + dynamic_diffuse) × opacity`. Additive blend — smoke contributions accumulate without darkening the scene behind them.

10. **Billboard pipeline bind group layout.** The billboard pipeline is separate from `forward.wgsl` but reuses the same bind group objects for shared resources. Groups 1 and 4 (material and lightmap) are replaced or omitted; groups 2 and 3 carry lighting data identical to the forward pass.

    | Group | Bindings | Resources |
    |-------|----------|-----------|
    | 0 | 0 | Camera uniforms |
    | 1 | 0–1 | Sprite sheet texture + sampler (no spec texture or MaterialUniform needed) |
    | 2 | 0–5 | Dynamic lights, influence volumes, spec-light buffer, chunk grid, offsets, indices |
    | 3 | 0–13 | SH volume (sampler, 9 band textures, grid uniform, animation buffers) |
    | 6 | 0 | Sprite instance storage buffer (positions, ages, sizes, rotations, opacities) |

    Group 5 (spot shadow maps) is not bound — dynamic spot shadow occlusion for sprites is out of scope; the dynamic diffuse-only path is sufficient at this fidelity level.

### Task A acceptance gates

- Smoke emitter placed in a test scene spawns camera-facing sprites that drift upward, fade in/out, and cycle animation frames.
- Sprite in a well-lit area picks up the ambient SH color of its BSP leaf (confirmed by toggling SH sampling on/off).
- Sprite near a colored neon static light shows a visible tint from the chunk list (confirmed on a test map with two differently-colored static lights in adjacent chunks).
- Sprite in a shadow cast by opaque geometry shows reduced brightness — SH probe falloff carries the occlusion.
- Max 512 sprites per emitter active simultaneously with no measurable frame time regression on dev hardware.
- **Chunk-list fallback:** with `chunk_grid.has_chunk_grid == 0` (chunk list not yet uploaded), sprites still light via SH + dynamic direct only — no panic, no black sprites. Confirmed by forcing the sentinel in a test build.

---

## Task B — Volumetric fog / beam pass

**Crate:** `postretro` · **New module:** `src/fx/fog_volume.rs` · **New shader:** `src/shaders/fog_volume.wgsl` · **Also modifies:** `src/render/mod.rs`.

### Fog data

1. **`env_fog_volume` AABB buffer.** At level load, resolve each `env_fog_volume` brush to its world-space axis-aligned bounding box and fog parameters. Upload as a compact storage buffer of up to 16 fog volume entries (retro-scale maps rarely need more): `{ min: vec3<f32>, density: f32, max: vec3<f32>, falloff: f32, color: vec3<f32>, scatter: f32 }`. Per-sample membership test in the fog shader is a simple point-in-AABB check — no BSP traversal at runtime. If a sample falls inside multiple overlapping volumes, accumulate their contributions. The existing `env_fog_volume` BSP-leaf association from `build_pipeline.md` §Custom FGD drives the AABB extraction at load time; the runtime does not walk BSP nodes.

2. **FGD additions.** Extend `env_fog_volume` with one new optional property: `scatter: f32` (fraction of light that scatters toward the camera vs. is absorbed; default 0.6). Add `fog_pixel_scale: u32` to `worldspawn` (resolution divisor for the volumetric pass; default 4, valid range 1–8; higher = coarser, more retro-looking blocks). `fog_pixel_scale` is a global render-target property — it governs the single low-res allocation for the entire fog pass and cannot sensibly vary per-volume. Placing it on `worldspawn` follows the same convention as `ambient_color` and other scene-wide render parameters.

### Volumetric pass

3. **Low-resolution target.** Allocate a `fog_pixel_scale`-downsampled RGBA16F render target (swap chain dimensions ÷ `fog_pixel_scale`). Cleared each frame. The full-resolution depth buffer from the depth pre-pass is bound as a read-only texture; the fog shader samples it with `textureLoad` at the nearest full-resolution texel corresponding to each low-res fragment's UV. This produces some edge bleed (fog leaking across silhouettes within a `fog_pixel_scale`-pixel block), which is considered aesthetically consistent with the intentional pixelated look and not a correctness issue.

4. **Ray march.** Fullscreen pass at the low-resolution target. Per fragment:
   - Reconstruct world-space ray from camera position and fragment UV + depth buffer sample (depth buffer bound as a read-only texture — full-resolution, sampled at nearest texel).
   - March the ray from the near plane to the first opaque surface (depth). Step size: `fog_step_size` world units (default 0.5 m, retunable).
   - At each step, test the sample world position against the fog AABB buffer (step 1). For each volume whose AABB contains the sample, accumulate scatter weighted by that volume's `density` and `scatter` parameters.

5. **Scatter accumulation per sample.** At each fog sample:
   - **SH ambient scatter.** Sample SH volume at sample position → ambient fog color contribution. Weighted by `density × scatter`.
   - **Dynamic spot beam.** For each allocated dynamic spot shadow map slot (from `lighting-spot-shadows/`): check if sample is inside the spot's cone (dot product against direction, compare against `cone_half_angle`). If inside, sample the shadow map — occluded samples contribute no beam scatter. Unoccluded samples add `light.color × intensity × density × scatter` along the ray. This produces visible pixelated light shafts and shadow wedges.
   - **Static light scatter.** Out of scope for this plan — see the Out of scope section. SH ambient provides the static ambient base; dynamic spot beams carry the primary visual interest. Static per-chunk scatter is deferred until profiling shows the beam-only result is insufficient.

6. **Transmittance.** Track ray transmittance: `T *= exp(-density × step_size)`. Early-exit when T < 0.01 — fully opaque fog, no need to march further.

7. **Composite.** A fullscreen blit pass composites the low-resolution fog buffer over the opaque world render: `final = scene_color + fog_scatter` (additive). Nearest-neighbor upscale from the low-res target — this is the source of the pixelated block aesthetic. Do not bilinearly filter; the steps are intentional.

8. **Bind group layout.** The fog pipeline is a separate wgpu pipeline from `forward.wgsl` but reuses the same bind group *objects* for shared resources, avoiding redundant uploads. Layout:

   | Group | Bindings | Source | Resources |
   |-------|----------|--------|-----------|
   | 0 | 0 | existing | Camera uniforms |
   | 2 | 0–5 | existing | Dynamic lights, influence volumes, spec-light buffer, chunk grid, offsets, indices |
   | 3 | 0–13 | existing | SH volume (sampler, 9 band textures, grid uniform, animation buffers) |
   | 5 | 0–2 | `lighting-spot-shadows/` | Shadow depth texture array, comparison sampler, light-space matrices |
   | **6** | **0–2** | **this plan** | Full-res depth buffer (read-only texture), fog AABB+params storage buffer, low-res scatter output texture (storage texture, write) |

   Groups 1 and 4 (material and lightmap) are not used by the fog pass and are not included in its bind group layout. Group numbers follow the authoritative cross-plan table in `lighting-spot-shadows/` Task B step 3; group 6 is the next available slot within the `max_bind_groups = 8` budget.

### Task B acceptance gates

- `env_fog_volume` brush entity placed in a test scene produces visible pixelated haze when the player enters the volume.
- A dynamic spot light aimed into a fog volume produces a visible pixelated beam shaft with a hard edge at the cone boundary.
- Geometry inside the fog volume casts a visible shadow wedge into the beam (occlusion from the shadow map confirmed by toggling the shadow sample).
- Toggling `fog_pixel_scale` (worldspawn) from 4 to 1 produces smooth fog; from 4 to 8 produces coarser, more blocky fog — confirming the resolution divisor is wired correctly.
- SH ambient scatter: fog in a blue-tinted BSP region appears blue-tinted; fog in a warm-lit region appears warm (confirmed by changing SH bake color in a test map).
- **Performance:** GPU time for the fog pass is measured with `POSTRETRO_GPU_TIMING=1` (8 active spot shadow slots, fog volume covering the camera, default step size) and recorded in the PR description. Target: under 2 ms on dev hardware. If over, reduce the default step size until it fits and document the final value — step count is the primary tuning lever before investigating shader cost.

---

## Acceptance Criteria (both tasks)

1. `cargo test --workspace` passes.
2. `cargo clippy --workspace -- -D warnings` clean.
3. No new `unsafe`.
4. Task A and Task B acceptance gates above.
5. Frame time regression on a scene with both systems active (smoke emitters + fog volume + active spot beams): total combined cost under 3 ms on dev hardware.
6. `context/lib/rendering_pipeline.md` §7.3 updated to document the billboard and volumetric passes in the frame order.
7. `context/lib/build_pipeline.md` §Custom FGD updated with `env_smoke_emitter` (all properties), the new `env_fog_volume` property (`scatter`), and the new `worldspawn` property (`fog_pixel_scale`).

---

## Out of scope

- True physically-based volumetric scattering (phase functions, multi-scattering). Hard nearest-neighbor single-scatter only.
- Volumetric shadows on world geometry from fog (god rays projected onto surfaces). Post-processing effect, separate initiative.
- Point light beam effects — only spot lights produce beam shafts (spots have a defined cone; points would require cube-map sampling, deferred to a separate plan if ever needed).
- Particle systems beyond smoke (sparks, blood, debris). Billboard sprite infrastructure introduced here is the foundation; additional emitter types are additive and do not require pipeline rework.
- GPU particle simulation (compute-driven physics). CPU ring buffer is sufficient at 512 sprites/emitter.
- Soft-edge compositing of the low-res fog buffer. Nearest-neighbor is intentional.
- **Static light scatter in fog.** Iterating the chunk light list per fog sample is expensive for thick volumes and deferred until profiling shows the SH + dynamic-beam result is insufficient. A `fog-static-scatter` cargo feature is the planned addition point if this is ever needed.

# Animated Light Weight Maps — Research Notes

Supporting material for the plan. Keep survey detail here so the spec carries only decisions.

## Quake / Quake 2 / Quake 3 lightstyles

- Lightstyles are indexed per surface. Each face stores up to 4 style slots (`styles[4]`), each pointing at a global lightstyle index and a separate lightmap layer.
- Runtime value per style: single scalar in the `"a".."z"` alphabet (discrete 0..25, scaled 0..2). Animates at ~10 Hz. Scalar modulates the style's per-texel lightmap layer; final texel is the sum across the 4 slots.
- Strengths: cheap, per-surface, composable. Brightness-only (no per-light color).
- Limits Spec 2 should improve on:
  - Fixed cap of 4 styles per *face*. Spec 2 puts the cap per *chunk* of a face, which localizes overlap density instead of forcing face subdivision.
  - Brightness scalar only. Spec 2's `AnimationDescriptor` already carries brightness + RGB color curves.
  - Discrete 10 Hz, nearest-sample. Spec 2 uses non-linear interpolation (prerequisite, plan 2) for smooth flicker/pulse/strobe.
  - One global style table; all lights sharing a style animate in lock-step. Spec 2 gives each animated light its own descriptor (phase desync, independent periods).

Source: Quake wiki lightstyle article; Q3Map2 shader manual lightstyles; id Software qrad3 source.

## Source engine (HL2) lightmaps

- VRAD bakes per-style lightmap pages. Toggleable lights use "named" light entities; up to a small fixed number of animated styles per face (same 4-slot Quake lineage).
- Supports HDR via RGBM-like exponent channel.
- No per-light contribution maps beyond the style-slot mechanism. Same fundamental shape as Quake.

Source: Valve Developer Wiki (Lightmap, Lighting); Mitchell "Shading in Valve's Source Engine" SIGGRAPH 2006.

## Unreal Lightmass

- Bakes static lightmaps, separates directional and indirect terms.
- "Stationary" lights: static diffuse pre-baked, shadow masks per-light stored in a 4-channel DistanceFieldShadowMap texture (up to 4 stationary lights overlapping any texel — if exceeded, the extras fall back to dynamic). Specular/color applied at runtime from the live light.
- This is the closest prior art to Spec 2's pattern: per-light mask maps composed at runtime, bounded by a per-texel cap, with overflow degradation.

Takeaway: Unreal's per-texel cap of 4 on stationary overlap validates a similar cap-of-4 default for Postretro's per-chunk light list (already decided in Spec 1). Unreal stores the full per-light shadow mask in a dedicated texture channel; we cannot afford that at arbitrary light count, which is why we indirect via the per-chunk list.

Source: Unreal Engine docs on Lightmass / stationary lights.

## Per-light weight maps / attribution maps

No widely-published academic "per-light attribution map" paper for real-time lightmaps. The closest is Unreal's stationary-light distance-field shadow map (above). Most academic work focuses on precomputed radiance transfer (PRT) for indirect light, not per-light direct attribution.

Practical convention across engines: lightmap atlas + small bounded per-texel light list + runtime composition. Postretro follows this with the Spec 1 chunking as the spatial accelerator.

## WebGPU / wgpu storage buffer indirect-data patterns

- `array<u32>` storage buffer for flat index lists, paired with an offset-table `array<vec2<u32>>` (offset + count). This is the pattern the existing `ChunkLightList` section uses — direct precedent, same crate, proven in production.
- Texture path: `texture_2d_array<f32>` for per-layer weight maps; but layer count scales with animated-light count, which violates the memory target. Rejected.
- Storage texture path: writable `rgba16float` atlas composed per frame in a compute pre-pass. Viable. Postretro already uses `Rgba16Float` for the irradiance atlas. Compute → atlas → sampled in forward = clean boundary.

## Cubic Hermite vs Catmull-Rom vs monotonic cubic

- Catmull-Rom is a cubic Hermite spline with auto-computed tangents (central differences between neighbors). Interpolates through every keyframe. Cheap. Shape-preserving for smooth curves but can overshoot on sharp keyframe changes (e.g. sudden strobe off → on).
- Monotonic cubic (Fritsch-Carlson) prevents overshoot but distorts shape on smooth curves — wrong tradeoff for flicker/pulse.
- Cubic Hermite with explicit tangents gives author control but adds authoring complexity (each keyframe needs an in/out tangent).

**Recommendation for plan 2's evaluator:** Catmull-Rom. Keyframe-only authoring matches the existing `Vec<f32>` brightness layout. Overshoot on strobe is acceptable for the retro aesthetic (chunky pulses are on-brand). No tangent authoring surface required.

Source: Wikipedia Catmull-Rom spline; Stanford CS248 animation curves notes; The Orange Duck "Cubic Interpolation of Quaternions" (background only).

## Bake-time cost

Per-texel per-influencing-light intersection + occlusion ray. For a chunk with K influencing animated lights and T texels:
- Inner cost: T × K × (one distance/falloff eval + one shadow ray).
- Occlusion ray cost dominates. Reuses existing `segment_clear` against the global BVH (same as static lightmap bake).
- Scoping: chunks already filter candidate lights (Spec 1 guarantees K ≤ cap = 4). Texel count bounded by chunk UV sub-region → at 4 cm/texel, a typical 2m × 2m chunk holds 2500 texels, so 10k rays per chunk worst case.
- Parallelism: per-chunk rays are independent. `rayon` `par_iter` over chunks fits the existing compiler pattern.

## Runtime despawn / toggle handling

- Scripted emitters that remove their light mid-level leave baked weights intact. Two options:
  1. Per-descriptor `active: u32` flag; shader multiplies the light's contribution by `active`. Zero cost when active, masks cleanly when inactive.
  2. Accept level-reload-only deactivation (Quake model).
- (1) is trivial; added to Spec 2. A scripted toggle sets the flag in the CPU-side descriptor buffer each frame.

## Visualization hooks

Debug shader modes used in similar engines:
- Light count heatmap (blue→red over 0..cap per texel).
- Chunk boundary overlay (checkerboard per chunk UV region).
- Single-light isolation (sample only one descriptor).

Low implementation cost, high authoring value. Recommend optional deliverable — small debug shader `world_debug_animated_lightmap.wgsl` toggled by env var. Defer if scope tightens.

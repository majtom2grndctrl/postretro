# surface-radiance-cache — research

Read at 704ed232e. Findings that inform the brief; the brief's Decisions are the record.

## Cost basis

No record splits hallway-mini's non-lightmap time by stage. Nearest figures:

| Map | Commit / date | SH Bake | Delta SH Bake | Lightmap Bake | Total | Source |
|---|---|---|---|---|---|---|
| `stress-warren-hallway-inspection-mini` | ac9015fb3, 2026-10-23 | not recorded | ~0 (no animated lights) | 370.3 s | 637.7 s | `done/lightmap-penumbra-floor/plan.md` |
| same, pre-floor | c587d8e74 | not recorded | ~0 | 178.1 s | 440 s | same |
| full hallway | 2026-10-10 | 621.5 / 608.5 s | — | 359.4 s | 1248.7 s | `done/static-specular-mask-ownership/plan.md` M7 |
| `stress-warren-mini` | 99ef79f, 2026-10-08 | 137 s | 14 s | 80 s | 271 s | `bake-ray-speed-contract.md` |
| `campaign-test` | 2026-10-07 | 7.7–9.4 s | — | 6.7 s | 23–25 s | `done/lighting-scale--cold-sh-bake-contribution-early-out/research.md` |

Hallway-mini's ~265 s outside the lightmap stage held flat across the floor change; assumed mostly SH Bake, unmeasured. Hallway-mini has only static style-0 lights, so delta timing needs `stress-warren-mini`.

Inside SH Bake, one `perf` profile (warren-mini, before c855e3e, 2026-10-07): probe re-snap per hit × light 46.1%, `segment_clear` 40.2%, `area_sample_target` 3.2%, bounce `closest_hit` 1.2%. c855e3e removed the re-snap (2.87×). Not re-profiled; shadow rays are probably still the majority.

`gi-rtx-lite/index.md` estimate, unmeasured: hallway shadow rays ~47 G → ~0.3 G, plus ~246 M gather rays, ~6 lights per hit.

## Today's estimator

- `sh_bake.rs` `RAYS_PER_PROBE` (256), uniform Fibonacci sphere (`sphere_directions`, rotated by `SAMPLING_LATTICE_OFFSET`), weight 4π/256, cosine lobe after projection (`apply_cosine_lobe_rgb`).
- Per hit, `sample_radiance_rgb`: every light in the slice → `bounce_contribution` (range, cone, n·l early-outs; `done/lighting-scale--cold-sh-bake-*`) → `light_contribution_lambert` × `soft_visibility(…, DEFAULT_AREA_SAMPLE_COUNT, segment_clear)`: 1 ray hard, 4-probe set escalating to 32 soft. Sum × `BOUNCE_ALBEDO` (0.45/π). `SKY_COLOR` black. No emissive.
- Single bounce. Indirect only; probe direct is id 35.
- Seeds: `soft_visibility_seed(probe_index, ray_index, global_light_index)`.
- `Hit` keeps point, normal, distance. `closest_hit_in_leaf` sees `BvhPrimitive.material_bucket_id` (= `face.texture_index`, set in `bvh_build::collect_primitives`). `ray_triangle_hit` is double-sided with the normal flipped toward the ray, so a back-face hit shades the side the ray came from.
- Cold `bake_sh_volume_controlled`: per probe, all static lights. Warm `bake_sh_volume_grouped_controlled` (`sh_group.rs`): 4³ groups, `reaching_lights` (falloff + `SH_REACH_CUTOFF_METERS`), `group_cache_key` folds reach cutoff, group size, reaching lights with global indices, `geometry_content_hash`, layout, validity. Warm is a deliberate dimmer-or-equal approximation (`done/incremental-bake-per-element/`).
- Deltas (`delta_sh_bake.rs`): per (animated light, affinity cell) 64-probe sub-block, `bake_probe_indirect_rgb` with `unit_radiance_light`; nothing shared across lights. `delta_sh_entry_cache_key`, `INDIRECT_DELTA_SH_STAGE_ID` v3.
- Bakes the cache cannot serve (no gather hits): ids 35, 41, 45, 47/48.

## Albedo before SH

- `TextureMips` (`texture_mips::bake_world_texture_mips`) reads only the texture-name table, `texture_root`, `prm_root`. No SH, atlas, lightmap or BVH input; per-texture content-addressed sidecars (`filename_key_for`, `bundle_hash_for`); no `StageCache` key. ~5.5 s on campaign-test, serial (`done/cell-visibility-relation/research.md`).
- Name table grows after Geometry: `environment_volumes::encode_environment_volumes_section` interns fluid-only textures just before `TextureMips`, deliberately.
- `reflection_probes/materials.rs` `SurfaceMaterials::new(names, keys, prm_root)` / `sample(texture_index, uv, footprint_texels)`: lazy per-texture `.prm` load, nearest texel at `footprint_lod`, sRGB → linear. Fallback for missing key, `NO_TEXTURE`, unreadable `.prm` or no diffuse slot: magenta/black checker (`placeholder`). Needs a neutral fallback for bounce.
- Face texture and UVs exist after Geometry: `faces[i].texture_index`, vertex `uv` in raw texels (`compute_texel_uv`; level-format `Vertex.uv` "normalized" doc is stale). `hit.rs` `SceneTracer::surface_at` shows barycentric UV interpolation.
- Key precedent: `reflection_probes/stage_key.rs` `cache_key` folds `texture_keys`.
- Grey was a tooling limit, not a look decision: `done/lighting-animated-sh/index.md` Q4.

## Constraints recorded

- `build_pipeline.md` §Compiler pipeline: pre-atlas stages key on positions only; no stage before atlas preparation emits face identity. AtlasPreparation cuts faces and rebuilds the BVH, so pre-cut face ids die there.
- §Build Cache: key = blake3(stage id ‖ version ‖ inputs); anything a cached stage reads is in its key; reflection probes key on the outputs they read, not their inputs.
- Determinism: cold bytes independent of worker count, arrival order and cache state; no output-ordering `HashMap` iteration; ordered reductions; progress and governor pauses never reach output. Cross-platform byte identity not promised (`networking.md` §Level identity).
- `--working-set-max-size` (16 GiB, per stage) gates only the lightmap stage and the SH-delta gate (3× dense bytes); neither counts a new co-resident cache. Overages are named errors.
- `development_guide.md` §1.4: bound the full lifetime, count every co-resident copy; a bake stage states its domain and why it is no larger.
- Emissive lights nothing: `resource_management.md` §4.5, §2; `done/emissive-surfaces-bloom/`; `lighting--per-term-isolation`.

## Adjacent work

| Item | Relation |
|---|---|
| `done/lighting-scale--cold-sh-bake-falloff-early-out`, `-contribution-early-out` | The cache replaces their seam (`sample_radiance_rgb`); patch direct keeps the same early-outs |
| `drafts/lighting-scale--cold-bake-reaching-light-spike` out-of-scope item 1 (bounce samples at 32) | Moot once shadow rays leave probe hits |
| `drafts/lighting-scale--sh-delta-cell-major-two-pass-bake` | Parked contingency, post-bake RAM reorder; its first slice assumes `base + Σ deltas` bytes this brief changes |
| `drafts/lighting-scale--sh-delta-receiver-reach-cull` | No work; id 41 only |
| Roadmap: embedded-emitter bias in SH probe bakes | Patch direct keeps today's four-probe path, so the bias carries into the cache; that item fixes it there afterward |
| Roadmap: SH probe classification and relocation | Consumes leak counts from the cache's gather rays later |
| Roadmap: emissive bounce | Separate owner ruling |

## Warm-locality shape

Today's warm group sums only its reaching lights. A cache that stores one summed radiance per patch keys on every light, so one light edit re-keys every group. Per-light patch layers (sparse: patches inside each light's reach) keep the group's sum restricted to its reaching set, reproduce the warm approximation exactly, and are the same structure animated deltas need as unit layers. Cold sums all static layers. Memory scales with Σ(patches in each light's reach), not patches × lights.

## Direction review (2026-10-24)

`/validate-plan`: Direction sound. Strongest rival: keep the per-hit estimator with one shadow ray per (hit, light), estimated full-hallway SH Bake ~621 s → ~200 s, still scaling with lights. Owner ordered it first as the least visible change, then colour as its own step, then this cache with multi-bounce. The one-sample form draws one emitter sample stratified across a probe's 256 rays, so its expectation equals `soft_visibility`'s; a fixed centre ray would bias penumbrae.

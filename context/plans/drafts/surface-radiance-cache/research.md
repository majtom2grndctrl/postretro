# surface-radiance-cache — research

Read at 704ed232e; estimator, albedo and pins re-read at db827b1df. Findings that inform the brief; the brief's Decisions are the record.

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
- Per hit, `sample_radiance_rgb` (`sh_bake/bounce.rs`): every light in the slice → `bounce_contribution` (range, cone, n·l early-outs; `done/lighting-scale--cold-sh-bake-*`) → `light_contribution_lambert` × visibility × the hit texture's bounce albedo. Production visibility is `BounceVisibility::OneSample`: one emitter sample per (hit, light), stratified across the probe's rays, no early-out. `FullSet` (cfg(test)) is `soft_visibility`'s probe-and-escalate set, kept as the parity reference. `SKY_COLOR` black. No emissive.
- Single bounce. Indirect only; probe direct is id 35.
- Seeds: base `soft_visibility_seed(probe_index, ray_index, global_light_index)`; deltas seed from a digest of the keyed unit light, so a reorder never changes bytes.
- `Hit` keeps point, normal, distance. `closest_hit_in_leaf` sees `BvhPrimitive.material_bucket_id` (= `face.texture_index`, set in `bvh_build::collect_primitives`). `ray_triangle_hit` is double-sided with the normal flipped toward the ray, so a back-face hit shades the side the ray came from.
- Cold `bake_sh_volume_controlled`: per probe, all static lights. Warm `bake_sh_volume_grouped_controlled` (`sh_group.rs`): 4³ groups, `reaching_lights` (falloff + `SH_REACH_CUTOFF_METERS`), `group_cache_key` folds reach cutoff, group size, reaching lights with global indices, `geometry_content_hash`, layout, validity. Warm is a deliberate dimmer-or-equal approximation (`done/incremental-bake-per-element/`).
- Deltas (`delta_sh_bake.rs`): per (animated light, affinity cell) 64-probe sub-block, `bake_probe_indirect_rgb` with `unit_radiance_light`; nothing shared across lights. `delta_sh_entry_cache_key`, `INDIRECT_DELTA_SH_STAGE_VERSION` v5 at db827b1df.
- Bakes the cache cannot serve (no gather hits): ids 35, 41, 45, 47/48.

## Albedo

Coloured bounce has landed (`done/coloured-sh-bounce/`): per-texture snapped means after one bake-wide saturation constant, grey 0.45 fallback, folded into SH group and delta keys. Findings that shaped it, kept for patch albedo:

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
| `drafts/lighting-scale--sh-delta-cell-major-two-pass-bake` | Parked contingency, post-bake RAM reorder; this brief leaves id 27 bytes alone, the animated follow-up changes them |
| `drafts/surface-radiance-cache-animated` | Follow-up (owner, 2026-10-30): id 27 through unit patch layers, after counsel reads '451 cl. 1 |
| `drafts/lighting-scale--sh-delta-receiver-reach-cull` | No work; id 41 only |
| Roadmap: embedded-emitter bias in SH probe bakes | Lands first (owner, 2026-10-30). The one-sample bounce has no early-out, so it carries no bias; the fix gives direct SH and scatter a per-light embedded-emitter rule, and patch direct reuses it |
| Roadmap: SH probe classification and relocation | Consumes leak counts from the cache's gather rays later |
| Roadmap: emissive bounce | Separate owner ruling |

## Pin table

| id | scenario | ordering | expected outcome |
|---|---|---|---|
| P1 | Probe gather ray hits a world face | Patch direct (and, cold, later bounces) finish before a probe gather reads a patch; the hit looks up a patch and traces no shadow ray | Gather traces zero shadow rays. Shadow rays come from patch direct only, and their count does not depend on probe count or rays per probe |
| P2 | Reach skip with fill > 0 | Probe gathers mark the patches they reach → bounce-one shading → later-bounce patch gathers read bounce-one radiance, including from patches no probe ray reached | Every patch a later-bounce gather reads has bounce-one radiance. The skip drops only patches no gather of any bounce reads |
| P3 | Static light removed, inserted ahead of others, or flipped to animated | Global indices of later lights move; layer keys and seeds follow content | Unchanged lights' layers hit cache and equal their cold layers byte for byte. Groups re-gather over the new summed cache |
| P4 | Two static lights with identical content | Same digest, different position | Each contributes once; a shared entry serves both only if their layers are identical by construction |
| P5 | Geometry edit outside a light's reach | Patch ids taken from pre-cut face order can shift | A layer served from cache equals the cold layer. With today's whole-map geometry hash, every layer and group re-bakes |
| P6 | Texture-mean or saturation edit | Bounce albedo goes into patch radiance before the gather | Every patch layer and group re-keys. Restoring both values hits cache (precedent `colour_key_tests.rs`) |
| P7 | First warm build after the change lands | SH group stage version bumps | Every pre-cache SH group entry misses |
| P8 | Forced-low working-set budget | The SH base gate runs before Materials and before any SH ray (`pipeline.rs` `run_after_parsing`) | Refuses with the named error before any patch is shaded or probe ray traced. Never coarsens |
| P9 | Closed room, albedo-1 faces, fill 1 | Each bounce ≈ previous bounce × albedo × enclosure | Energy per bounce does not fall. The total is bounded by bounce count × bounce one and stays finite |
| P10 | Zero static lights; emissive textures only | No layer to build | No patch layer allocated; id 34 all zero |
| P11 | Light whose emitter sphere reaches into geometry | Embedded classification on pre-cut geometry runs before patch direct, ahead of AtlasPreparation | The flagged light's patches match a no-early-out reference |
| P12 | Probe gather hit distance | The ray's hit distance is taken before patch lookup | id 34 depth moments are byte-identical to the one-sample bake |

## Warm-locality shape

Today's warm group sums only its reaching lights, an approximation forced by per-hit shadow rays. With the cache, shadow rays live in patch direct, so locality belongs there: per-light patch layers (sparse: patches inside each light's reach) confine a light edit's shadow-ray work to that light. Summing layers and re-gathering every group costs gather rays only (~246 M on the hallway, est.), so warm id 34 can equal cold. A single summed layer would re-shade every patch on any edit. Per-light unit-transport layers are also the structure the animated follow-up reads. Memory scales with Σ(patches in each light's reach), not patches × lights.

Reshape 2 (2026-10-30): the earlier "warm bakes bounce one only" kept SH-group locality but showed authors dimmer fill on warm builds than release ships; reversed under `index.md` §2 (defaults that set authors up for success).

## Direction review (2026-10-24)

`/validate-plan`: Direction sound. Strongest rival: keep the per-hit estimator with one shadow ray per (hit, light), estimated full-hallway SH Bake ~621 s → ~200 s, still scaling with lights. Owner ordered it first as the least visible change, then colour as its own step, then this cache with multi-bounce. The one-sample form draws one emitter sample stratified across a probe's 256 rays, so its expectation equals `soft_visibility`'s; a fixed centre ray would bias penumbrae.

## Detail review and reshape (2026-10-30)

`/review-brief` (premise, rows, subtract) found coloured bounce already landed, patch direct a different estimator from the one-sample baseline, a static-seed rule that conflicted with id 27, per-light bounce in id 27 against the IP guard, no SH base working-set accounting, and no row proving gather shadow rays left. Owner reshape: animated deltas split to a follow-up, later-bounce weight as a `worldspawn` key under `index.md` §2, texture-level multiplier deferred to surface lights, slope proof in place of a sizing gate.

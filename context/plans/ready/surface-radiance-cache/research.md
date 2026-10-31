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
| P1 | Probe gather ray hits a world face | Patch direct, the layer sum and later bounces finish, warm and cold alike, before a probe gather reads a patch; the hit looks up a patch and traces no shadow ray | Gather traces zero shadow rays. Shadow rays come from patch direct only, and their count does not depend on probe count or rays per probe |
| P2 | Static light removed, inserted ahead of others, flipped to animated, or entities reordered | Global indices move; layer keys follow content | Unchanged lights' layers hit and equal their cold layers byte for byte. A pure reorder leaves id 34 probe tiles and depth moments unchanged; with animated lights the slot table may move |
| P3 | Two static lights with identical content | Same digest, one shared layer entry | The sum adds the shared layer once per light; the summed digest folds multiplicity, so adding the duplicate misses the id-34 entry |
| P4 | Geometry edit outside a light's reach | Patch ids derive from the face and position on it | A layer served from cache equals the cold layer. With today's whole-map geometry hash, every layer re-shades |
| P5 | Texture-mean or saturation edit | Bounce albedo goes into patch radiance before the gather | Every layer and the id-34 entry re-key. Restoring both values hits cache (precedent `colour_key_tests.rs`) |
| P6 | First warm build after the change lands | SH stage version bumps | Every pre-change id-34 entry misses |
| P7 | Forced-low working-set budget | The SH base gate runs before Materials, before static embedded classification and before any SH ray (`pipeline.rs` `run_after_parsing`) | Refuses by name before any patch is shaded, ray traced or texture read. Never coarsens. The prediction prices every patch, so it needs no ray |
| P8 | Closed room, albedo-1 faces, fill 1 | Each bounce is about the previous bounce × albedo × enclosure | Area-weighted patch energy per bounce does not rise beyond discretisation tolerance; id 34 stays finite. Per-probe readings can rise: a probe behind a pillar reads zero at bounce one |
| P9 | Zero static lights, emissive textures only, or every static light removed in one warm edit | No layer to build; probe rays still trace for depth moments | No layer allocated; irradiance all zero; depth moments equal the one-sample bake's; warm equals cold |
| P10 | Light whose emitter sphere reaches into geometry, sdf-shadowed or not | Embedded classification on pre-cut geometry runs before patch direct, ahead of AtlasPreparation | The flagged light's patches match a no-early-out reference; the probe-point bakes' flags and warnings are unchanged |
| P11 | Probe gather hit distance | The ray's hit distance is taken before patch lookup | id 34 depth moments are byte-identical to the one-sample bake |
| P12 | Edit A, then B, then revert to A, warm | B's success replaces the map's record; the revert build's sweep can evict A-only layers before it reads | The revert hits every A layer the sweep kept and re-shades the rest; id 34 equals a cold bake of A either way |
| P13 | `_bounce_fill` and one light change in the same build | The light's layer re-shades; later bounces and the gather re-run once over the new sum at the new fill | Exactly one layer re-shades; id 34 equals a cold bake at the new fill |
| P14 | Warm build after only `--release` builds, or into an empty cache directory | `--release` neither reads nor writes the cache | Every layer and the id-34 entry miss and are written; id 34 equals the `--release` bytes |
| P15 | A layer evicted or corrupt | Missing or invalid entries miss; a layer re-shades from its keyed inputs, so its bytes repeat | The re-shaded layer equals the lost entry; the summed digest is unchanged, so the id-34 entry hits; only hit counts depend on cache state |
| P16 | `--no-penumbra-floor` toggled; or `--soft-shadow-samples` changed | A layer key folds its own flag and the floor setting; patch direct samples the default lattice, as the bounce does | Only flagged lights' layers re-shade on a floor toggle; a sample-count change re-shades none |
| P17 | Animated-light-only edit | Layers and the sum read static lights only; id 27 bakes as today | No layer re-shades; id 34 probe tiles and depth moments unchanged |
| P18 | A static light's colour or intensity changes | Layers bake colour and intensity in | Only that light's layer re-shades; id 34 equals a cold bake |
| P19 | A flagged light moved into or out of a wall, or a light inserted ahead of a flagged one | Flags come from pre-cut geometry before patch direct; each layer key folds only its own flag | Only the moved or inserted light's layer re-shades; every other layer hits |
| P20 | Validity-only brush edit, probe-spacing change, or geometry edit that leaves every layer's bytes unchanged | Layers hit; the summed digest is unchanged; the gather sees new geometry, validity or grid | The id-34 entry misses because its key covers every input the gather reads; warm equals cold |
| P21 | Animated light added, removed, or its period, phase, colour or intensity edited, warm | The summed cache is unchanged, so the id-34 entry hits | Descriptors and slot table rebuild every build from the animated set; warm id 34 bytes equal cold |
| P22 | Light added or removed whose reach holds faces no other light reaches | Patches exist on every static face, independent of the light set | Patch ids and seeds do not move; other layers hit unchanged |
| P23 | Static light whose reach covers zero patches | Its layer is empty before the sum | No allocation or shading; id 34 equals the map without it |
| P24 | Budget between the SH base, SH-delta and lightmap predictions | Both SH gates run before Materials and before static embedded classification | The lowest-priced gate the budget fails refuses by name; prediction is independent of cache mode |
| P25 | Warm build stopped during patch shading | Each layer writes to cache as it finishes, before the sum | Finished layers survive; the rerun re-shades only unfinished ones |
| P26 | SH base stage ends | The delta, direct and scatter stages follow; the delta gate reserves no cache share | Layers, sum and bounce buffers drop before the delta SH stage begins |
| P27 | Layers co-resident before the sum, or loaded from cache warm | Layers are resident before the reduction; later bounces hold at least two buffers | The prediction prices the co-resident set, the bounce buffers and the id-34 entry's encode and decode |

## Warm-locality shape

Today's warm group sums only its reaching lights, an approximation forced by per-hit shadow rays. With the cache, shadow rays live in patch direct, so locality belongs there: per-light bounce-one layers (sparse: patches inside each light's reach) confine a light edit's shadow-ray work to that light. Summing layers, later bounces over every patch and the whole-volume gather then cost gather rays only, so warm id 34 can equal cold through one bake path. Estimated warm re-run on the hallway: ~246 M probe gather rays (962,636 valid probes × 256 at 1.0 m, `done/lighting-scale--adaptive-probe-spacing/research.md`, 2026-09-18, not re-counted since the map changed) plus later-bounce patch gathers over ~1–2 M patches (`gi-rtx-lite/index.md`, est.); the warm measurement row prices both. Per-group SH caching buys nothing once any light edit moves every group. Layer memory scales with Σ(patches in each light's reach), not patches × lights; one id-34 entry is ~0.5 GB on the hallway (~512 B per valid probe).

## Direction review (2026-10-24)

`/validate-plan`: Direction sound. Strongest rival: keep the per-hit estimator with one shadow ray per (hit, light), estimated full-hallway SH Bake ~621 s → ~200 s, still scaling with lights. Owner ordered it first as the least visible change, then colour as its own step, then this cache with multi-bounce. The one-sample form draws one emitter sample stratified across a probe's 256 rays, so its expectation equals `soft_visibility`'s; a fixed centre ray would bias penumbrae.

## Detail review and reshape (2026-10-30)

`/review-brief` (premise, rows, subtract) found coloured bounce already landed, patch direct a different estimator from the one-sample baseline, a static-seed rule that conflicted with id 27, per-light bounce in id 27 against the IP guard, no SH base working-set accounting, and no row proving gather shadow rays left. Owner reshape: animated deltas split to a follow-up, later-bounce weight as a `worldspawn` key under `index.md` §2, texture-level multiplier deferred to surface lights, slope proof in place of a sizing gate.

Review round 1 after reshape 2 (2026-10-30): `/review-brief` lenses found the Path still summing warm groups over reaching lights (contradicting warm = cold), a warm-equality row that passed on today's approximation, a parity reference without the per-light embedded rule, sdf-shadowed lights outside the classifier, and unit-transport layers matching `ip-check.md` Piece 3's near-claim reading of '451 cl. 1. Resolved: one id-34 bake path cached whole, layers bake colour and intensity in, the reach skip dropped (patches follow light reach), reference and classifier take the per-light rule. The `_bounce_fill` name avoids ericw-tools' `_bouncescale`, which scales all bounced light and would mislead Quake mappers (unverified in repo).

Review round 2 (2026-10-30): a layer key copying id 35's whole flagged-set fold would tie every layer to other lights' positions, and an id-34 entry keyed on the summed digest alone would serve stale grids or descriptors. Resolved: each layer folds only its own flag; the entry covers every input the gather reads, and descriptors rebuild every build. Later bounces span every static face, so rooms beyond every light's reach still take fill. Energy rows measure area-weighted patch energy. Static layers stay, warranted by shadow-ray locality and the follow-up's reuse, with one summed pass as the fallback if counsel reads them toward '451 cl. 3.

# reflection-probes — plan of record

mode: resumable
status: approved
read at: 41f21c8ae
branch: feature/reflection-probes

## Resolved blocks

- 2026-10-14 — rows 33 and 35 restated (owner-approved): capture-path GPU rows compare against the capture's encoding (soft-knee tonemap, then sRGB), since `capture_frame_indirect` tonemaps and sRGB-encodes; linear 0.5 reads 188 ± 1. Applied to `index.md`.

## Corrections

Compiler
- Stage registry orders reporting only → execution order is hand-written in `pipeline::run_after_parsing`; id 52 encodes at `pipeline.rs:1811`, *before* TextureMips. Planning around it by inserting the stage between TextureMips (`:1823`) and ClusterDirectory (`:1859`); id 52 and the BVH are both in scope there. A new stage also touches `ORDERED_STAGES` length, `tui/tui_steps.rs` and `SUMMARY_LABELS` in `tests/compiler_cli_contract.rs`.
- `ray_traversal::BoundedRay` is an AABB prune, not a closest-hit API; `sh_bake::closest_hit` is private and returns point/normal/distance only → the stage gets its own closest-hit returning triangle offset + barycentrics, mapping to face via `geo.face_index_ranges`, interpolating `Vertex.uv` (raw texel units, not normalized), `lightmap_uv`, `lightmap_block`.
- `bake_world_texture_mips` returns no texels → albedo (slot 0) and emissive (slot 3) are read back from the `.prm` sidecars by the key map it returns; that map feeds the cache key. The mip follows the ray footprint, as the forward pass minifies, rather than mip 0, which would alias far walls at four samples per texel.
- The SH volume in scope (`packed_sh_volume`) is the stored-set L0/L1/L2 RGBA16F volume the runtime samples minus BC6H; the CPU sampler ports `build_probe_indirection_words` and `sample_sh_indirect` (normal offset, trilinear × validity × backface × Chebyshev, production probe occlusion). Ambient floor: production default 0 (`DEFAULT_AMBIENT_FLOOR`), pinned by test. Left out as the Decision says: promoted-light shadowmask subtraction (only while promoted), Surface Depth AO, normal-mapped shading normal and bumped-Lambert correction, static specular.
- Capture hits cull back faces as the world pass does; `DEFAULT_INDIRECT_SCALE` is pinned by reading the renderer source (unreachable from a dev-dependency).
- Lightmap floats are not retained past LightmapBake; only a `#[cfg(test)]` BC6H decoder exists → write a production mode-11 BC6H decoder (the encoder is mode 11 only) and decode id 22 per hit block. Bounded compile RAM; no full f32 atlas retained (see `sh-delta-compile-ram-constraints` memory).
- SH coefficients are not retained, and no CPU sampler of the SH volume at a point exists → build a CPU sampler over the dense pre-encode `packed_sh_volume` mirroring `sh_sample.wgsl` (reconstruction pieces in `level-format/src/sh_reconstruct.rs`), plus delta sub-blocks per animated light. Warm SH differs from cold, so the stage's cache key hashes the SH and delta *outputs*, not their inputs.
- `sh_bake::animation_descriptor_for` is private → use `animation_descriptor_for_light` (`pub(crate)`). `slot_for_map_light` is a section field from `AnimatedBakedLights::slot_for_source_lights`, not a sh_bake fn.
- No production CPU sdf selection; the test closure in `chunk_light_list_bake` is linear-falloff only; `SDF_SHADOW_K == SDF_SELECT_K` is a comment → write a compiler CPU `select_sdf_lights` over the chunk-light-list window, falloff-model aware, pinned against the WGSL by source and a parity constant test.
- The sdf set must come from runtime spec slots, not the lightmap filter: `bake_only` sdf lights are dropped from both the lightmap and the runtime.
- Animated sdf lights: `forward.wgsl`'s sdf loop reads static `spec_lights` colour. Task 5 confirms how the runtime animates them; the capture mirrors whatever the runtime draws, at reference, per the Reference-state Decision.
- No `<thing>_volume` peel; each classname has an explicit branch (`parse/environment_volumes.rs:30`, `parse.rs:1108`) → `reflection_volume` gets its own branch, outside `is_environment_volume_classname` so id 52 is unchanged.
- Id 52 faces carry no normal, plane or area and include side and bottom faces; volume indices count gravity and push records → the stage computes Newell normal, plane height and area per face from vertices; bindings index (volume, face) in id 52's own numbering.
- `section_registry.rs:344` asserts id 54 is unassigned → update with the new id.
- Shared mapping tests pin tile 6 only; `irradiance_tile_source_texel` asserts border 1; the direction→UV encoder is `#[cfg(test)]` → generalize the mapping test over tile sizes (6 and 128), expose the encoder to the compiler, keep border 1 (the SH atlas convention).
- `render-cpu` is a dev-dependency of level-compiler → the clamped-curve cycle mean is a compiler mirror pinned by a dev-test parity against `render_cpu::sh_compose::animated_light_scale`.
- `presentation_budget_measurement_tests.rs` compiles nothing → P16's stress-map build is a new ignored test that compiles `stress-env-volumes.map`.
- Task 4 pinned: box-match tolerance 0.25 m on all six bounds (union-find components per fluid entity per plane height); nearest-point fallback insets 1 cm toward the face's own centroid so the point never sits on the cutting wall; in-solid and outside-hull reuse the SH bake's probe-validity test (`partition::find_leaf_for_point` + `find_exterior_leaves`); inside-a-fluid tests every id-52 fluid record's brush planes.
- Placement runs on every build before the cache lookup, and the stage key hashes placement's output (each probe's source, box, point and bound faces) rather than the BSP, so drop warnings reach warm builds and a moved probe brush or changed `_capture_offset` misses exactly. Warm e24 stage: 0.02 s.
- Acceptance row 5's "an authored point in a pillar standing in the water drops" holds as worded but is unreachable for a world pillar: a pillar clips the water, so the area centroid's nearest-point fallback (P32) moves the point onto the water and the probe is kept. The test uses a pillar hanging 1.5 mm above the water (outside the 1 mm flush tolerance, inside the 2 mm lift). Reported to the owner; `_capture_offset` stays useful for choosing a better parallax point.
- P33 warns on the heights a box *binds*, per the Decision (a room box with a smaller basin box inside does not warn).
- A broad full-height pillar near the water splits a ring pool into one derived probe per side (each piece's fit sees the pillar as a wall); a slim pillar keeps one probe. Accepted for v1; an authored box covers it.
- A pool spanning a doorway now binds three probes (each room plus the doorway piece), as the doorway-rooms rule intends; the task-2 integration test was updated to assert a probe per room.
- Floating-block tops on e24 keep derived probes (points in open air; no drop applies).
- Task 5 pinned: lobe weights `max(d·axis, 0)²` over up, ±X, ±Z with the downward part split ¼ per horizontal lobe (sums to 1 by construction, trig-free, C¹; `level-format` `reflection_probe_lobe_weights`, which task 6 mirrors); texel solid angle exact octahedral density dA/|p|³ per supersample (tile sums to 4π within 1e-4); reference mean over 4096 midpoint samples of the clamped evaluation; top-K by each light's lobe-weighted captured energy, ties to the lower roster index, zero-measure lights not stored; sdf shadow rays from 2 mm off the hit via `segment_clear`. Stage version 3.
- An animated sdf light's direct is static at runtime: `pack_spec_lights` uploads colour × intensity once and the forward sdf loop ignores curve and `is_active`; only its delta SH animates. The capture mirrors that — its sdf direct is in the static term and only its delta SH enters its share — so a script switching an sdf light off leaves its direct lit in the room and the reflection alike. Runtime gap, reported to the owner.
- Runtime sdf point and directional lights draw at half brightness: `spec_buffer.rs:102` packs a zero aim with cone cosines (1, −1), so `cone_attenuation_cos` at `forward.wgsl:1281` is `smoothstep(−1, 1, 0)` = 0.5 (verified). The capture mirrors the 0.5 per the Decision ("as the runtime draws"); a source test pins that shader line so a runtime fix must update the mirror. Owner decision; proposed as a separate session.

Runtime
- `fs_main` takes only `dpdx/dpdy(in.uv)` → pixel style adds `dpdx/dpdy(in.world_position)` at the top, still uniform control flow.
- Params rows are per volume; bindings are per face (a canal volume binds several probes) → probe index rides a third vertex buffer at location 4, written once at install through the drift map's face vertex ranges. A row spare cannot carry it.
- `anim_descriptors[r]` equals roster `r` only by the compiler's identity `animation_descriptor_indices`; the loader does not enforce it, and the fluid stage has no storage budget left to bind the index table → the P10 test pins identity in the compiler, and the loader refuses a non-identity table when section 54 is present.
- `fluid_warp_reads_no_reduce_motion_accessibility_or_render_profile_input` scans the whole shader for `render_profile`/`bloom` → narrow it to the warp functions; the Look Decision makes reflection style author-owned, while the warp stays engine-owned.
- No BC-less fallback exists to mirror: `renderer_init_resources.rs` requires `TEXTURE_COMPRESSION_BC` (and BC6H filterable) at boot, so the probe array uploads BC6H unconditionally, as the SH atlas does. A malformed hand-built section is dropped whole at install (`installable_probe_section`) so no face samples a missing layer.
- Probe table (group 4 binding 2): 16 B header + 256 × 240 B probes (box, point + layer, 4 × 48 B share entries) = 61 456 B at the cap, constant-asserted in render-cpu where the layout lives. Reflectivity packs 0 for a volume with no bound face, so `draws()` reads opacity > 0 ∨ extinction > 0 ∨ reflectivity > 0.
- `fluid_surface.rs` is 627 lines after task 3 (probe code split into `fluid_reflection_probes.rs`); a further split is a follow-up candidate.
- Live-lighting cost per reflecting fragment: one tap, ~20 ALU of lobe weights, 4 table entry reads, then per present light one 48 B descriptor load and ~90 ALU (curves add 4–12 sample loads); worst case K = 4 with full curves ≈ 600 ALU + 68 storage loads. Evaluations stay per fragment (sharing them per probe needs a new pass). Task 9 measures it on a pool with four animated lights; a per-probe precompute is the follow-up lever if the 5300M shows it.
- A level with animated lights whose SH volume is absent or disabled as oversized binds group 3 b11's single zeroed dummy record, so a share entry naming roster 0 reads it as an inactive light and darkens by its share. Unguarded; needs that rare level shape.
- `FluidSceneInputs::upload` zeros the spares and dirties only on fog weight → the style flag joins both the write and the dirty check.
- `fluid_fs_main_fogs_from_the_outward_normal_and_returns_the_composite` pins the exact return tail, and `packing_lays_out_opacity_fog_and_wave_rows_with_spare_floats` asserts all three spares 0 → both update deliberately; the spare test asserts reflectivity's slot and that two spares remain 0.
- `FluidSurfaceLook` lives in `render-cpu/src/fluid_surface/slots.rs`; `capture/prepared.rs` only calls `surface_only` → `surface_only` gains reflectivity 1; the look closures (app and capture) carry "volume has a bound face".
- `render-cpu` `sh_compose::animated_light_scale`'s doc says it mirrors `animated_direct_sh_compose.wgsl`; its body matches `sh_compose.wgsl` → fix the doc; it is the CPU reference for the fluid port.
- Lighting mask: `LightTermMask::ALL` (0x17F) includes specular and depth AO, which the fluid ignores. The Decision reads "every lighting bit"; planning to the literal reading, `(mask & ALL) == ALL`, matching the Acceptance row "any one bit off … matches today's output". Effect: toggling Specular or Depth AO in the debug UI hides reflections. Owner may narrow at the skim.
- `research.md` §Budget's "9 of 16 / 7 of 12" are post-probe totals; today's fluid fragment totals are 8 sampled textures, 2 samplers, 6 uniforms, 8 storage. The Acceptance row states deltas, so it is unaffected. No test counts fluid uniforms or samplers; task 8 adds one.

Scripting
- Both render drains return the whole default when `bloom` is absent or malformed → the reflections parse sits outside those early returns in both runtimes.
- Renderer delivery is bloom-named (`apply_mod_bloom_render_profile`, `set_bloom_render_profile`) → add a reflection-style apply path called at boot (`splash_lifecycle.rs:612`) and on staged reload (`staged_manifest_lifecycle.rs:310`), restoring the default on `NoStartScript`.
- `FLUID_KEYS` is a closed `[&str; 12]` allowlist → add `surfaceReflectivity`.
- Script NaN is rejected by `conv.rs` before `validate()`, with "malformed and was skipped … non-finite number at 'surfaceReflectivity'" → the NaN case pins that message; it names the field and keeps the rest of the manifest, as the row requires.
- No per-field client-local marker exists; the digest skips `registry.fluids` wholesale and the wire carries only name + viscosity → no code change; extend `digest_ignores_fluid_fog_and_wave_fields`.
- Docs the Path's promotion list omits: `docs/scripting-reference.md` (render profile, fluid table, example), `sdk/lib/data_script.{ts,luau}` `defineFluid` hover text, the `ModManifest.render` doc string.

- Shared octahedral border rule (`octahedral::irradiance_tile_source_texel`) copied an edge border texel from the *opposite* interior edge; on the octahedral square the neighbour across an edge is the *same* edge mirrored. Owner chose to fix the shared rule before task 2 (f3a7c39e9), so the SH atlas and probe tiles share the corrected convention, which `index.md`'s "SH atlas's border convention" now names. Re-epoched: `SH_GROUP_STAGE_VERSION` 2, `DIRECT_SH_STAGE_VERSION` 5, `INDIRECT_DELTA_SH_STAGE_VERSION` 3 (the two dormant delta epochs bake uncached and stay). SH visual change, for the landing notes: vertical surfaces facing the engine −Z half-space now read their own horizontal SH direction instead of an even blend with its mirror, so they show more directional bounce contrast; +Z-facing walls, floors and ceilings are unchanged.

## Delegated answers
- **K = 4; lobes = up + four horizontal (5)**, weights from clamped cosine powers normalized to sum 1, so the ratio is continuous across lobe edges (P7). Pinned in the render-cpu ratio reference.
- **Share channel form: RGB per lobe, packed unorm10×3 in one u32.** Scalar shares misattribute a coloured light's channels. Entry = vec4 (RGB reference, roster index bits) + 5 packed shares padded to 2 vec4 = 48 B; 4 entries = 192 B per probe; 256 probes = 48 KiB < 64 KiB uniform binding. Unorm10 quantization (1/1023) sits under the BC6H tile's own error.
- **Cap = 256** = `PORTABLE_MAX_TEXTURE_ARRAY_LAYERS`: 4 MiB of BC6H tiles at the cap.
- **Probe index delivery:** vertex buffer at location 4 (above). **Style flag:** `FluidSceneInputs` spare `y`. **Reflectivity:** params-row spare `surface.z`; `surface.w` and `fog_albedo.w` stay free for SSR.
- **Lightmap source:** decode id 22 (bounded RAM) over retaining pre-encode floats.
- **Emissive parity:** direct `render-data` dependency (a layer-0 leaf; no layering rule forbids it); regenerate `crate-graph.md`.
- **Fit tolerance, ray pattern, up-facing threshold, pixel-style tilt constant:** pinned in tasks 2, 4 and 7, reported with the VRAM figure.

## AC-to-proof

Compiler
| AC | Proof | Status |
|---|---|---|
| 1 Section 54 emitted / absent / pre-section map loads | level-format round-trip + compiler fixture tests; loader test on a pre-54 PRL | achievable as stated |
| 2 Load refusals (version, out-of-range bindings, face bound twice, roster index) and reserved value loads | level-format `from_bytes` + loader validation tests | achievable as stated |
| 3 Derived probes P17, P30, P31; two heights; doorway rooms | compiler fixture tests (synthetic brushes) | achievable as stated |
| 4 Capture point rules, P29, P32, P33, `_capture_offset`, T-junction centroid | compiler unit tests | achievable as stated |
| 5 Drops P13, P15, P27; wall-side kept; pillar + offset | compiler tests + log capture | achievable as stated |
| 6 Binding: smallest box, side/sloped none, split pool, P14 | compiler unit tests | achievable as stated |
| 7 Cap P16, P21, P26; constant assertions | compiler tests; new ignored stress-map compile | achievable as stated |
| 8 `--jobs 1` vs `--jobs 4`, `--no-cache`, byte-identical id 54 | ignored CLI test after `plain_cli_is_deterministic…` | achievable as stated |
| 9 Build cache hits / misses per input | stage cache-key tests | achievable as stated |
| 10 Mover, dynamic light absent; switch present; authored brush leaves geometry/BVH/SH identical | compiler fixture tests | achievable as stated |
| 11 Lower hemisphere traced (red floor) | capture unit test | achievable as stated |
| 12 Reference state P18, P9, inactive light, bounce at production weight | capture unit tests + curve-mean parity test | achievable as stated |
| 13 SDF P19, >4 lights matches selection reference | capture tests vs CPU selection, selection pinned to WGSL | achievable as stated |
| 14 Light share fractions, references, P6, P22, P10, P20 | share-table tests + log capture | achievable as stated |
| 15 Emissive and indirect-weight parity | parity tests (render-data dep; renderer constant mirrored) | achievable as stated |
| 16 Tile mapping = SH atlas mapping, borders | shared mapping test generalized to 128 | achievable as stated |

Runtime (CPU references pinned to shader bodies)
| AC | Proof | Status |
|---|---|---|
| 17 Box projection | render-cpu reference + shader pin | achievable as stated |
| 18 Ratio P3, P6, P7, P8, P28 | render-cpu ratio reference | achievable as stated |
| 19 Live lighting P5, P4, P25 source-order; no SH read | reference + source-order test + source scan | achievable as stated |
| 20 Promotion P1; no promotion weight / crossfade / override read | reference + source scan | achievable as stated |
| 21 Fresnel values | render-cpu Fresnel reference | achievable as stated |
| 22 Underside | reference + shader pin | achievable as stated |
| 23 Composite | render-cpu composite reference (extends `fog_fragment`) | achievable as stated |
| 24 Zero-opacity P2, P23, P24 | collector / draw-set tests | achievable as stated |
| 25 Light-term mask | reference + shader pin | achievable as stated (literal reading, see Corrections) |
| 26 Pixel style cells; drift and wave tilt nothing | render-cpu style reference | achievable as stated |
| 27 Manifest style parse and frame-capture defaults | scripting tests both runtimes + capture look test | achievable as stated |
| 28 `surfaceReflectivity` validation, digest, drift scan, P12 | scripting + digest + drift scan + slots tests | achievable as stated |
| 29 Scripting example parses TS + Luau; SDK types regenerate | example test + typedef snapshots | achievable as stated |
| 30 Binding budgets | `pipeline_budget_tests.rs` + new fluid uniform/sampler count | achievable as stated |
| 31 Grep gates | source scans | achievable as stated |
| 32 DX12 FXC | `dx12_fxc_compiles_every_full_init_pipeline` (ignored, Windows) | achievable as stated |

Adapter-backed
| AC | Proof | Status |
|---|---|---|
| 33 Uniform-R probe at grazing view, capture-encoded | `fluid_surface_frame_test.rs` | achievable as restated |
| 34 Two probes + unbound face in one draw | `fluid_surface_frame_test.rs` | achievable as stated |
| 35 Unbound face 188 ± 1; lit face capture-encoded within 1/255 | `fluid_surface_frame_test.rs` | achievable as restated |
| 36 Level change P11 | `fluid_surface_frame_test.rs` | achievable as stated |

Manual (owner, in-engine)
| AC | Proof | Status |
|---|---|---|
| 37 Existing map's pool reflects after rebuild | owner, in-engine | manual |
| 38 Mirrors lit room incl. sdf corner; strafe parallax; low-eye coping | owner, in-engine | manual |
| 39 Fresnel read: straight down faint, across near-mirror, vista strong | owner, in-engine | manual |
| 40 Flicker, no lobe seam; script switch-off incl. vista; neon reflects | owner, in-engine | manual |
| 41 Pixel vs smooth follow the manifest | owner, in-engine | manual |
| 42 Canal boundary: step, never a geometry jump | owner, in-engine | manual |
| 43 Fluid-pass GPU delta (1660, 5300M); cold/warm stage time; probe VRAM on stress map | executor measures 1660 + stage + VRAM; owner runs Metal System Trace on the Mac | manual |


## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Section 54 contract: level-format types, encode/decode, version and index refusals, reserved roster value, registry id 54, loader validation incl. identity-table check; generalized octahedral mapping test (6 and 128) with the encoder exposed; production mode-11 BC6H decoder | integrating executor | — | done — `cargo test -p postretro-level-format reflection_probes` (11 + const assertions), `octahedral` (47, tiles 6 and 128), `-p postretro-level-loader --features load-prl reflection_probe` (5), `-p postretro-level-compiler --bin prl-build bc6h` (9 + 2 lightmap call sites); loader identity-table refusal untested here, pinned by P10 in task 5 |
| 2 | **First slice, compiler**: stage module (cache key over every read input), derived probe for each up-facing face (minimal fit: ray casts from the area centroid, hull clamp), closest-hit with barycentrics, capture shading from lightmap (id 22 decode) + albedo/emissive (.prm) + base SH (CPU sampler) at production indirect weight, empty share table, BC6H tile; emits id 54 for `e24-environment-test.map` | delegated (Opus), compiler crates only | 1 | done — `--bin prl-build reflection_probes` (33), `--test environment_volumes_compile` (3), CLI contract (6); e24 `--no-cache`: 12 probes, 17 bindings, east-wing probe box = the wing interior, stage 0.43 s at 5 workers, PRL byte-identical at 1/5/8 workers and warm |
| 3 | **First slice, runtime**: install probe array + share uniform (dummy group-4 resources when absent), probe-index vertex buffer at location 4, smooth-style box projection, Fresnel, union composite over fog and clear paths, underside off, mask gate, draw predicate carries "has a bound face", `surface_only` reflectivity 1; render-cpu references for projection, Fresnel, composite. **Owner checkpoint**: east-wing pool look at low eye + 1660 fluid-pass timing | delegated (Opus), render-cpu + renderer + postretro install; integrator owns shader seam | 1 | code done — render-cpu 401 (fluid 78, `reflection_tests` rows 17/21/22/23/25), row 24 collector test, renderer lib 930 (fluid 38, budgets: sampled 8→9, uniforms 6→7, samplers 2, storage 8), adapter test `a_probe_bound_face_reflects_its_capture_and_an_unbound_face_in_the_draw_reads_today` ran on GPU, postretro fluid 57, DX12 FXC passes; e24 run installs 12 probes / 17 faces, fluid pass 0.82–0.94 ms at spawn; **owner checkpoint pending** |
| 4 | Placement completeness: `reflection_volume` parse + FGD + `_capture_offset`; binding; connected-component groups; capture-point rules incl. nearest-point; drop order; cap; warnings | delegated (Opus), `reflection_probes/placement` | 2 | done — `--bin prl-build reflection` 57 + `parse::reflection` (5), `--test environment_volumes_compile` 4 (+1 ignored); row 10 byte-identity test; ignored `stress_env_volumes_builds_with_no_more_probes_than_the_cap` passed at 256 probes / 364 bindings (whole cold compile 2031 s); e24: 12 probes, nothing dropped, stage 0.45 s cold / 0.02 s warm |
| 5 | Capture completeness: animated weight maps + delta SH at reference; clamped-curve mean mirror + parity; CPU sdf selection + shadow rays (+ animated-sdf check); per-light unit accumulation → share table (K 4, 5 lobes, RGB unorm10); indirect, emissive and SDF-K parity tests; `--jobs` determinism; cache-miss matrix | delegated (Opus), `reflection_probes/capture` + `share` | 2 (parallel with 4: disjoint modules; integrator owns `mod.rs`) | done — `--bin prl-build reflection_probes` 84, level-format `reflection_probes` 16 (lobe weights), `--test reflection_probes_compile` 1 (P10), ignored `reflection_probes_are_byte_identical_at_one_and_four_jobs` ran and passed (row 8), row 9 matrix 11 misses + hits; e24 stage 0.48 s cold / 0.01 s warm, peak RSS 374 MiB; e24 has no animated or sdf lights, fixture shares checked |
| 6 | Runtime live lighting: share uniform upload, `anim_descriptors` b11 in the fluid shader, port `sh_compose.wgsl` `animated_light_scale`, lobe-blended ratio; render-cpu ratio reference; P1, P3–P8, P25, P28; source scans | delegated (Opus) | 3, 5 | done — render-cpu `fluid_surface` 86 (P3, P6, P7 ε 2e-3, P8, P28, one-ratio, share-move, P1), renderer `fluid` 45 (WGSL↔CPU body pins, sh_compose port pin, no-promotion and no-SH scans, `the_frames_one_descriptor_flush_precedes_the_fluid_pass`), adapter `a_probe_reads_darker_by_its_lights_share_when_its_descriptor_switches_off` ran on GPU, DX12 FXC ran; bindings unchanged (9/7/8/2) |
| 7 | Author surface: `surfaceReflectivity` (descriptor, validation, `FLUID_KEYS`, SDK, docs), `render.reflections.style` (both drains, independent of bloom early returns, renderer apply path, scene-input spare); pixel style per-cell tilt with world-position derivatives; narrow the render-profile scan; P2, P12, P23, P24 | delegated (Opus) | 6 (shares the fluid shader) | |
| 8 | Gates: adapter-backed tests (rows 33–36), level change P11, budget tests incl. new uniform/sampler count, grep gates, DX12 FXC run, ignored stress-map compile | integrating executor | 4, 5, 7 | |
| 9 | Manual test map (pool at floor level, neon sign, animated light, `_shadow_type sdf` light, two-room canal) in `content/dev/maps/`; measurements: 1660 GPU delta, cold/warm stage time on e24, probe VRAM on stress map; **owner-requested A/B against main**: release binary of `origin/main` built in a lean throwaway worktree (`CARGO_INCREMENTAL=0`, own target, binary copied out, target deleted at once, development_guide §Worktree builds), each binary on its own compiler's e24 PRL at the same pose with `POSTRETRO_GPU_TIMING=1`, plus the branch at reflectivity 0 (any lighting-term toggle off) to separate the probe term from the SH border fix; engine closed after each run | integrating executor; owner for Mac trace and look | 8 | |
| 10 | `/preflight`, `/review-panel` → `/fix-review-findings` loop, full gate | integrating executor | 9 | |

Hot paths: the fluid fragment stage (one tap, ≤ K descriptor evaluations, slab test; no per-frame allocation; share uniform and probe-index buffer written at install only; style and reflectivity change-driven through existing dirty paths). Build loop: the stage is cached under its own key and parallel per probe under the `--jobs` governor; its domain is the bound probes' tiles only.

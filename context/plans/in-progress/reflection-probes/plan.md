# reflection-probes — plan of record

mode: resumable
status: blocked
read at: 41f21c8ae
branch: feature/reflection-probes

## Block

Two adapter-backed Acceptance rows cannot pass as worded. `capture_frame_indirect` resolves through the window's soft-knee tonemap (`screen_effects.wgsl` `soft_knee_tonemap`, identity below a 0.98 peak) and sRGB-encodes (`renderer_capture.rs:221`), and the existing fluid GPU tests say so (`fluid_surface_frame_test.rs:222`, `:298`). Linear 0.5 reads ≈188, not 128. Proposed wording, same meaning (owner to approve or amend):

- **Row 33** (first adapter-backed row), was "…reads F·R + (1−F)·(surface over clear) within 2/255 at a fixed grazing view; at reflectivity 0 it reads today's value." → "…reads the capture's encoding (soft-knee tonemap, then sRGB) of F·R + (1−F)·(surface over clear) within 2/255 at a fixed grazing view; at reflectivity 0 it reads the encoding of today's value."
- **Row 35** (third adapter-backed row), was "An unbound, unlit fluid face at opacity 0.5 over a white clear reads 128 ± 1 red, and a lit unbound face matches today's straight-alpha value within 1/255." → "An unbound, unlit fluid face at opacity 0.5 over a white clear reads 188 ± 1 red (linear 0.5, below the tonemap knee, sRGB-encoded), and a lit unbound face reads the capture's encoding of today's straight-alpha value within 1/255."

On approval: apply the wording to `index.md`, set `status: proposed` → owner skim → `approved`.

## Corrections

Compiler
- Stage registry orders reporting only → execution order is hand-written in `pipeline::run_after_parsing`; id 52 encodes at `pipeline.rs:1811`, *before* TextureMips. Planning around it by inserting the stage between TextureMips (`:1823`) and ClusterDirectory (`:1859`); id 52 and the BVH are both in scope there. A new stage also touches `ORDERED_STAGES` length, `tui/tui_steps.rs` and `SUMMARY_LABELS` in `tests/compiler_cli_contract.rs`.
- `ray_traversal::BoundedRay` is an AABB prune, not a closest-hit API; `sh_bake::closest_hit` is private and returns point/normal/distance only → the stage gets its own closest-hit returning triangle offset + barycentrics, mapping to face via `geo.face_index_ranges`, interpolating `Vertex.uv` (raw texel units, not normalized), `lightmap_uv`, `lightmap_block`.
- `bake_world_texture_mips` returns no texels → albedo (slot 0, mip 0) and emissive (slot 3) are read back from the `.prm` sidecars by the key map it returns; that map feeds the cache key.
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

Runtime
- `fs_main` takes only `dpdx/dpdy(in.uv)` → pixel style adds `dpdx/dpdy(in.world_position)` at the top, still uniform control flow.
- Params rows are per volume; bindings are per face (a canal volume binds several probes) → probe index rides a third vertex buffer at location 4, written once at install through the drift map's face vertex ranges. A row spare cannot carry it.
- `anim_descriptors[r]` equals roster `r` only by the compiler's identity `animation_descriptor_indices`; the loader does not enforce it, and the fluid stage has no storage budget left to bind the index table → the P10 test pins identity in the compiler, and the loader refuses a non-identity table when section 54 is present.
- `fluid_warp_reads_no_reduce_motion_accessibility_or_render_profile_input` scans the whole shader for `render_profile`/`bloom` → narrow it to the warp functions; the Look Decision makes reflection style author-owned, while the warp stays engine-owned.
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
| 33 Uniform-R probe at grazing view | `fluid_surface_frame_test.rs` | **needs restatement** (see Block) |
| 34 Two probes + unbound face in one draw | `fluid_surface_frame_test.rs` | achievable as stated |
| 35 Unbound face 128 ± 1; lit face within 1/255 | `fluid_surface_frame_test.rs` | **needs restatement** (see Block) |
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
| 1 | Section 54 contract: level-format types, encode/decode, version and index refusals, reserved roster value, registry id 54, loader validation incl. identity-table check; generalized octahedral mapping test (6 and 128) with the encoder exposed; production mode-11 BC6H decoder | integrating executor | — | |
| 2 | **First slice, compiler**: stage module (cache key over every read input), derived probe for each up-facing face (minimal fit: ray casts from the area centroid, hull clamp), closest-hit with barycentrics, capture shading from lightmap (id 22 decode) + albedo/emissive (.prm) + base SH (CPU sampler) at production indirect weight, empty share table, BC6H tile; emits id 54 for `e24-environment-test.map` | delegated (Opus), compiler crates only | 1 | |
| 3 | **First slice, runtime**: install probe array + share uniform (dummy group-4 resources when absent), probe-index vertex buffer at location 4, smooth-style box projection, Fresnel, union composite over fog and clear paths, underside off, mask gate, draw predicate carries "has a bound face", `surface_only` reflectivity 1; render-cpu references for projection, Fresnel, composite. **Owner checkpoint**: east-wing pool look at low eye + 1660 fluid-pass timing | delegated (Opus), render-cpu + renderer + postretro install; integrator owns shader seam | 1 | |
| 4 | Placement completeness: `reflection_volume` parse + FGD + `_capture_offset`; binding; connected-component groups; capture-point rules incl. nearest-point; drop order; cap; warnings | delegated (Opus), `reflection_probes/placement` | 2 | |
| 5 | Capture completeness: animated weight maps + delta SH at reference; clamped-curve mean mirror + parity; CPU sdf selection + shadow rays (+ animated-sdf check); per-light unit accumulation → share table (K 4, 5 lobes, RGB unorm10); indirect, emissive and SDF-K parity tests; `--jobs` determinism; cache-miss matrix | delegated (Opus), `reflection_probes/capture` + `share` | 2 (parallel with 4: disjoint modules; integrator owns `mod.rs`) | |
| 6 | Runtime live lighting: share uniform upload, `anim_descriptors` b11 in the fluid shader, port `sh_compose.wgsl` `animated_light_scale`, lobe-blended ratio; render-cpu ratio reference; P1, P3–P8, P25, P28; source scans | delegated (Opus) | 3, 5 | |
| 7 | Author surface: `surfaceReflectivity` (descriptor, validation, `FLUID_KEYS`, SDK, docs), `render.reflections.style` (both drains, independent of bloom early returns, renderer apply path, scene-input spare); pixel style per-cell tilt with world-position derivatives; narrow the render-profile scan; P2, P12, P23, P24 | delegated (Opus) | 6 (shares the fluid shader) | |
| 8 | Gates: adapter-backed tests (rows 33–36), level change P11, budget tests incl. new uniform/sampler count, grep gates, DX12 FXC run, ignored stress-map compile | integrating executor | 4, 5, 7 | |
| 9 | Manual test map (pool at floor level, neon sign, animated light, `_shadow_type sdf` light, two-room canal) in `content/dev/maps/`; measurements: 1660 GPU delta, cold/warm stage time on e24, probe VRAM on stress map | integrating executor; owner for Mac trace and look | 8 | |
| 10 | `/preflight`, `/review-panel` → `/fix-review-findings` loop, full gate | integrating executor | 9 | |

Hot paths: the fluid fragment stage (one tap, ≤ K descriptor evaluations, slab test; no per-frame allocation; share uniform and probe-index buffer written at install only; style and reflectivity change-driven through existing dirty paths). Build loop: the stage is cached under its own key and parallel per probe under the `--jobs` governor; its domain is the bound probes' tiles only.

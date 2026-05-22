# Retire True Retro Mode

## Goal

Retire the True Retro graphics mode. Post Retro (hardware anisotropic sampler + in-shader texel-grid reconstruction) becomes the sole texture-filtering path. Strip the entire `GraphicsMode` runtime seam — the Rust enum, the frame-uniform field, the `defaultGraphicsMode` mod-manifest key, the SDK typedef rows, and the egui mode combo. This is a deliberate breaking change to the mod-facing manifest contract.

## Sequencing vs. BC5

The roadmap (around line 153) frames this plan as depending on `prm-bc5-normals` ("BC5 is the forcing function"). That sequencing is **inverted**: retire-true-retro lands **first** and does **not** depend on BC5. The Post Retro normal path keeps its current 3-channel `(rgb*2-1) -> normalize` decode unchanged; BC5 edits that single path later. Do not change normal encoding here. (This plan does not edit the roadmap — the corrected ordering is noted for the human reviewer.)

## Scope

### In scope

- Delete True-Retro-only WGSL in `forward.wgsl`: `compute_aniso_footprint`, `sample_aniso`, `sample_aniso_normal`, the `AnisoFootprint` struct, the `ANISO_*` consts, and the `TRUE_RETRO`/`POST_RETRO` mode consts. Collapse the two-arm `sample_color` / `sample_normal` dispatchers to their single Post Retro bodies. Remove the `graphics_mode` uniform field and its three `_pad` members, and the per-frame mode read in `fs_main`.
- Remove the `graphics_mode` field from the WGSL `Uniforms` layout in both `forward.wgsl` and `wireframe.wgsl`, in lockstep. Shrink `UNIFORM_SIZE` (Rust + both WGSL structs) to the new natural size.
- Delete the `GraphicsMode` enum + impl, the `FrameUniforms.graphics_mode` field and its encode in `build_uniform_data`, the `Renderer.graphics_mode` field + init + `set_graphics_mode`/`graphics_mode` accessors + the per-frame uniform write, in `render/mod.rs`.
- Remove the now-dead nearest sampler pool (`base_sampler` binding 1, `mip_count_samplers`, `create_mip_sampler`) — see Boundary inventory rationale. Keep the Post Retro hardware-aniso pool (`create_mip_aniso_sampler`, `mip_count_aniso_samplers`, group-1 binding 5).
- Scripting/SDK unwind: `parse_graphics_mode`, `ModManifestResult.default_graphics_mode`, the extraction in both the QuickJS and Luau mod-init paths and their unit tests (`runtime.rs`); the `register_enum("GraphicsMode")` block and the `defaultGraphicsMode?` field on the registered `ModManifest` plus the parity-guard expected-field list (`primitives/mod.rs`); the `GraphicsMode` `name_to_*` map rows and the literal `.d.ts`/`.d.luau` expected blocks in the drift test (`typedef.rs`); the stand-in `render::GraphicsMode` module in `gen_script_types.rs`.
- Regenerate `sdk/types/postretro.d.ts` and `sdk/types/postretro.d.luau` (drop `GraphicsMode` type + `defaultGraphicsMode?` field).
- Remove the egui Graphics Mode combo and its "Rendering" collapsing header (`debug_ui/mod.rs`), and the `GraphicsMode` import there.
- Remove the graphics-mode application in both `main.rs` boot and hot-reload sites (the `default_graphics_mode` read + `set_graphics_mode` call). The entity-descriptor drain at both sites stays.

### Out of scope

- BC5 / normal-encoding changes (separate `prm-bc5-normals` plan). The Post Retro 3-channel normal decode is unchanged.
- PRL wire format. Graphics mode was a runtime uniform only; it was never baked into the PRL. No PRL section or version change.
- Any future `GraphicsSettings` struct or replacement seam. The seam is removed outright, not preserved as a hook.
- Player-facing / persisted graphics settings, env-var selection — never existed; still absent.
- The hardware-aniso clamp value (`POST_RETRO_ANISO_CLAMP`) stays at 16; not retuned here.

## Acceptance criteria

- [ ] No symbol named `GraphicsMode`, `graphics_mode`, `defaultGraphicsMode`, `default_graphics_mode`, `parse_graphics_mode`, `TRUE_RETRO`, `POST_RETRO`, `compute_aniso_footprint`, `sample_aniso`, `sample_aniso_normal`, `AnisoFootprint`, `ANISO_*`, `create_mip_sampler`, `mip_count_samplers`, or `base_sampler` remains anywhere in `crates/postretro/src` (verifiable by repo-wide search). `TRUE_RETRO` / `POST_RETRO` here mean the `forward.wgsl` mode-discriminant consts, matched whole-word — the retained `POST_RETRO_ANISO_CLAMP` clamp const and the kept aniso-pool symbols (`create_mip_aniso_sampler`, `mip_count_aniso_samplers`) are explicitly excluded.
- [ ] `cargo build -p postretro` and `cargo build -p postretro --features dev-tools` both succeed with no dead-code or unused-import warnings introduced.
- [ ] `cargo test -p postretro` passes, including the WGSL-stride parity tests for both `forward.wgsl` and `wireframe.wgsl` (`Uniforms` stride equals the new `UNIFORM_SIZE`).
- [ ] A mod whose `setupMod()` returns `defaultGraphicsMode` (any value) loads with that key silently ignored — it is no longer a recognized manifest field and does not error. (Breaking change: the key is removed from the contract; mods relying on it no longer switch modes.)
- [ ] The SDK typedef drift test passes against regenerated `sdk/types/postretro.d.ts` and `.d.luau`; neither file mentions `GraphicsMode` or `defaultGraphicsMode`.
- [ ] The world forward pass renders Post Retro filtering on `content/dev/maps/campaign-test.prl`: hardware aniso at grazing angles, crisp reconstructed texels up close — pixel-identical to the prior Post Retro mode (the removal changes no Post Retro math).
- [ ] The egui diagnostics panel (dev-tools build) has no Graphics Mode control and no "Rendering" header; the Lighting systems controls are unaffected.

## Tasks

### Task 1: Shrink the shared uniform layout

Pin the new `UNIFORM_SIZE`. The current packed layout ends at `indirect_scale` (offset 92..96); `graphics_mode` sits at 96 with 12 bytes of trailing pad to reach 112. Removing `graphics_mode` and the three `_pad` members leaves a natural size of **96** bytes, already 16-byte aligned — no replacement padding needed. Set the Rust `UNIFORM_SIZE` const to 96, drop the `graphics_mode` field from `FrameUniforms` and its encode (`bytes[96..100]`) in `build_uniform_data`, and update the size/encoding comment block above `UNIFORM_SIZE`. In `forward.wgsl` and `wireframe.wgsl`, delete the `graphics_mode: u32` field, the `_pad0/_pad1/_pad2` members, and the padding comments from `struct Uniforms` so both WGSL structs round to 96. Update the `graphics_mode_uniform_encoding` and `uniform_data_has_correct_size` tests (the former is deleted with the enum; the latter drops the `graphics_mode` initializer). The two stride-parity tests (`forward_wgsl_struct_strides_match_cpu_layout`, the wireframe equivalent) need no edit beyond the const change — they assert equality against `UNIFORM_SIZE`.

### Task 2: Strip True Retro from forward.wgsl

Delete `compute_aniso_footprint`, `sample_aniso`, `sample_aniso_normal`, the `AnisoFootprint` struct, the `ANISO_TAP_COUNT`/`ANISO_THRESHOLD`/`ANISO_TINY_EPS` consts, and the `TRUE_RETRO`/`POST_RETRO` consts and their header comments. Collapse `sample_color` to its Post Retro body: take `tex`, `uv`, `ddx`, `ddy`; call `sample_post_retro(tex, aniso_sampler, uv, ddx, ddy)`. Collapse `sample_normal` likewise: sample once via `sample_post_retro`, then `normalize(n.rgb * 2.0 - 1.0)` (the existing Post Retro 3-channel decode — unchanged). Keep `sample_post_retro` intact, but note its `seam_width` floor currently reuses `ANISO_TINY_EPS`; replace that reference with an inline `1.0e-6` literal (or a local const) since the `ANISO_*` consts are gone. In `fs_main`, remove the `aniso_fp` computation and the `gfx_mode` read; compute `ddx = dpdx(in.uv)` / `ddy = dpdy(in.uv)` once and pass them to the collapsed `sample_color`/`sample_normal` at all three call sites (base color, normal, specular `.r`). Verify `sample_post_retro` and both collapsed dispatchers stay in uniform control flow (they call `textureSampleGrad` with explicit derivatives — already safe).

### Task 3: Remove the GraphicsMode enum and renderer seam

Delete the `GraphicsMode` enum + impl block (`render/mod.rs`). Delete the `Renderer.graphics_mode` field, its initializer (`GraphicsMode::DEFAULT`), the `set_graphics_mode`/`graphics_mode` accessors, and the per-frame `graphics_mode: self.graphics_mode` assignment into `FrameUniforms`. (`FrameUniforms.graphics_mode` itself is removed in Task 1.) Remove the test `graphics_mode_uniform_encoding` and the `graphics_mode: GraphicsMode::PostRetro` initializer in the wireframe-stride test's `FrameUniforms` literal (around line 3772).

### Task 4: Remove the dead nearest sampler pool

The nearest pool (`base_sampler` at group-1 binding 1, `mip_count_samplers`, `create_mip_sampler`) is consumed only by the deleted True Retro arms. Once Task 2 lands it is fully dead. Remove: the `create_mip_sampler` fn, the `mip_count_samplers` field + its two seed inserts (construction and `install_textures` growth loop) + its two `.get(...)` lookups, the `sampler` parameter on `build_material_bind_group`, the group-1 BGL entry for binding 1, the binding-1 `BindGroupEntry` in `build_material_bind_group`, and the `base_sampler` declaration in `forward.wgsl`. The aniso sampler must now bind at the slot the shader expects — decide whether to renumber the aniso sampler to binding 1 or leave it at binding 5 with binding 1 vacated; **pin: leave the aniso sampler at binding 5** to minimize churn (binding numbers need not be contiguous; the BGL simply omits 1). Update the group-1 BGL comment (currently describes 1=base_sampler nearest, 5=aniso) to reflect the single sampler. Update the `mip_count_samplers` and `mip_count_aniso_samplers` doc comments that cross-reference each other.

### Task 5: Unwind the scripting/SDK manifest seam

In `runtime.rs`: remove `parse_graphics_mode`, the `ModManifestResult.default_graphics_mode` field, the `use crate::render::GraphicsMode;` import, the `defaultGraphicsMode` extraction blocks in both `run_mod_init_quickjs` and `run_mod_init_luau` (the `default_graphics_mode` local and its assignment into the returned struct), and the six `mod_init_*_default_graphics_mode_*` unit tests (quickjs + luau × absent/trueRetro/postRetro/invalid — note quickjs has 4 and luau has 4). In `primitives/mod.rs`: remove the `register_enum("GraphicsMode")` block, the `.field("defaultGraphicsMode?", ...)` on the registered `ModManifest`, the `default_graphics_mode: None` field in the parity-guard's `_shape_anchor`, and `"defaultGraphicsMode"` from the `expected_fields` list. In `typedef.rs`: remove the `"GraphicsMode" => ...` rows from both `name_to_*` maps and the `GraphicsMode`/`defaultGraphicsMode` lines from both literal expected `.d.ts` and `.d.luau` blocks in the drift test. In `gen_script_types.rs`: remove the stand-in `mod render { ... GraphicsMode ... }` and its explanatory comment — once `runtime.rs` no longer imports `crate::render::GraphicsMode`, the stand-in is unreferenced.

### Task 6: Regenerate SDK types and remove the egui combo + boot wiring

Run `cargo run -p postretro --bin gen-script-types` to regenerate `sdk/types/postretro.d.ts` and `sdk/types/postretro.d.luau` (must match the Task 5 literal-block edits — drift test green). In `debug_ui/mod.rs`: remove the `use super::GraphicsMode;` import and the entire "Rendering" `CollapsingHeader` block (it contains only the Graphics Mode combo). In `main.rs`: at both the boot site and the hot-reload site, remove the `let graphics_mode = manifest.default_graphics_mode;` local, the `if let (Some(mode), Some(renderer)) = (...) { renderer.set_graphics_mode(mode); }` block, and the now-stale comments referencing the mod-chosen filtering mode. The entity-descriptor drain (`upsert_entity_type` loop) and surrounding lifecycle stay intact at both sites.

## Sequencing

**Phase 1 (sequential):** Task 1 — shrinks the shared uniform; every later WGSL/Rust edit assumes the new `UNIFORM_SIZE`.
**Phase 2 (concurrent):** Task 2 (forward.wgsl bodies), Task 3 (enum + renderer seam), Task 5 (scripting/SDK) — independent files once the uniform shape is fixed.
**Phase 3 (sequential):** Task 4 — removing binding 1 / the nearest pool depends on Task 2 having deleted the only consumer (the True Retro arms).
**Phase 4 (sequential):** Task 6 — regen + egui + boot wiring; the typedef regen depends on Task 5's registry edits, and the egui removal depends on Task 3's accessor removal.

## Boundary inventory

Rows being **removed** from the cross-boundary surface. After this plan none of these names exist.

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Mode enum | `GraphicsMode { TrueRetro, PostRetro }` (render module) — **removed** | n/a | `type GraphicsMode` — **removed** | same — **removed** | n/a |
| Manifest default | `ModManifestResult.default_graphics_mode: Option<GraphicsMode>` — **removed** | optional `defaultGraphicsMode` key — **removed (breaking)** | `defaultGraphicsMode?: GraphicsMode` — **removed** | `defaultGraphicsMode: GraphicsMode?` — **removed** | n/a |
| Frame uniform field | `FrameUniforms.graphics_mode: u32` + WGSL `Uniforms.graphics_mode` — **removed** | uniform buffer (offset 96, with 12B pad) — **removed; `UNIFORM_SIZE` 112 → 96** | n/a | n/a | n/a |
| Nearest sampler pool | `base_sampler` (group-1 binding 1), `mip_count_samplers`, `create_mip_sampler` — **removed** | BGL group-1 binding 1 — **removed** | n/a | n/a | n/a |

The `defaultGraphicsMode` manifest key is a contract removal. Per `index.md` the primitive/manifest surface is an API contract; dropping the key (rather than deprecating it) is the deliberate, breaking decision recorded here. Mods that set it will no longer switch modes — the key is silently ignored (unknown manifest fields are not errors; only missing required fields like `name` error).

## Open questions

- **Aniso sampler binding number.** Pinned to leave the aniso sampler at group-1 binding 5 with binding 1 vacated (non-contiguous bindings are valid; minimizes diff). An implementer who prefers contiguity may renumber to binding 1 — purely cosmetic, both shader and BGL must agree. Flagged, not blocking.
- **`UNIFORM_SIZE` 96 vs. larger.** Pinned to 96 (the natural aligned size after removal). No future field is reserved here; if a later plan needs a uniform slot it grows the struct then. Confirm 96 is 16-byte aligned (it is: 96 = 6×16).

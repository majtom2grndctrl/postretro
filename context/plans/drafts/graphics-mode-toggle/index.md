# Graphics Mode Toggle — True Retro / Post Retro

## Goal

Two named texture-filtering modes, switchable at runtime. **True Retro** keeps the nearest-sampler + manual shader-aniso look — hard pixel edges, the existing `shader-anisotropic-filtering` plan. **Post Retro** uses a hardware-anisotropic sampler plus an in-shader texel-grid reconstruction step: crisp texels up close (antialiased seams instead of hard edges), hardware aniso kills grazing-angle shimmer at distance. Post Retro suits the larger arenas now on the table, where high-aspect grazing footprints dominate. Mods choose the startup default via the mod manifest; absent → Post Retro. A dev toggle flips modes live for A/B comparison.

## Prerequisites

- `baked-texture-mips` (landed). Both modes read the baked mip pyramid; hardware aniso requires a mip chain.
- `shader-anisotropic-filtering` (in `ready/`) — **must land first.** It is the entire True Retro branch. This plan replaces its compile-time `ENABLE_MANUAL_ANISO` const with the runtime mode test; without it the True Retro acceptance criterion is untestable and the const would be touched twice. This plan does not define manual aniso itself.

## Scope

### In scope

- `GraphicsMode { TrueRetro, PostRetro }` enum in the render module beside `LightingIsolation`. Extensible umbrella; governs texture filtering only for now.
- Mode carried to the shader as a `u32` field on `FrameUniforms`; `Renderer::set_graphics_mode` setter and a `Renderer` field defaulting to Post Retro. Mirrors the `lighting_isolation` precedent end to end.
- Post Retro sampling path in `forward.wgsl`: per-slot (diffuse, specular, normal) texel-grid reconstruction feeding `textureSampleGrad` through a linear+anisotropic sampler.
- A second sampler: linear min/mag/mip + `anisotropy_clamp` (const, start 16), per-mip-count LOD clamp, parallel to the existing nearest pool. Bound at group 1 binding 5 alongside the nearest sampler at binding 1, so both are resident and the shader chooses per mode. BGL extended.
- Mod-manifest default: optional `defaultGraphicsMode` on `setupMod()`'s return value, applied to the renderer after mod-init; absent → the renderer's Post Retro construction default. SDK typedef + parity-guard test updated.
- egui diagnostics combo box to switch modes live (dev-tools), matching the Lighting Isolation control.

### Out of scope

- Player-facing / persisted graphics setting. No settings-persistence subsystem exists; deferred.
- Launch-time env-var selection.
- Other rendering knobs under the mode umbrella (post-processing, etc.). The enum is structured to grow; nothing else hangs off it yet.
- EWA, RIP-maps, TAA — same rejections as `shader-anisotropic-filtering`.
- Sprites, billboards, skybox, fog, shadow passes. World forward pass only.
- BC5 normal handling — separate `prm-bc5-normals` plan. See normal-slot open question.

## Acceptance criteria

- [ ] egui diagnostics panel has a Graphics Mode control; switching it changes the look live on a running map with no reload.
- [ ] A mod whose `setupMod()` omits `defaultGraphicsMode` boots Post Retro; returning `"postRetro"` boots Post Retro; returning `"trueRetro"` boots True Retro.
- [ ] An invalid `defaultGraphicsMode` string fails mod-init with a clear error, consistent with the existing missing-`name` validation.
- [ ] Post Retro: a grazing-angle tiled floor on `content/dev/maps/campaign-test.prl` shows no shimmer in motion. Up close, texels are crisp with antialiased edges — visibly sharper than a plain linear-sampler build with reconstruction disabled.
- [ ] True Retro: pixel-identical to the `shader-anisotropic-filtering` plan's output, both up close (hard edges) and at grazing angle.
- [ ] Both modes sample the baked mip pyramid; zero runtime mip work.
- [ ] `gen-script-types` output includes `defaultGraphicsMode?` on `ModManifest` plus the `GraphicsMode` type; the typedef drift test passes.
- [ ] Forward-pass GPU cost recorded for both modes at 1080p on the target GPU (`POSTRETRO_GPU_TIMING`) in the PR.

## Tasks

### Task 1: GraphicsMode enum + uniform plumbing

Define `GraphicsMode { TrueRetro, PostRetro }` in the render module beside `LightingIsolation`, with `ALL_VARIANTS`, `label()`, a `u32` uniform encoding (`TrueRetro = 0`, `PostRetro = 1`), and a `DEFAULT = PostRetro` constant. Add a `graphics_mode: u32` field to the Rust `FrameUniforms` and the matching WGSL `Uniforms` struct (preserve 16-byte alignment — state the constraint, let the implementer place the field). Add a `graphics_mode` field on `Renderer` initialized to `GraphicsMode::DEFAULT`, write it into the per-frame uniform, and add `Renderer::set_graphics_mode` / `Renderer::graphics_mode`. End-to-end mirror of the `lighting_isolation` path.

### Task 2: Linear+anisotropic sampler binding

Add a linear min/mag/mip sampler with `anisotropy_clamp` (const `POST_RETRO_ANISO_CLAMP`, start 16) and the same per-mip-count LOD clamp as the nearest pool — a parallel `HashMap<u32, Sampler>`. Extend the group-1 BGL with binding 5 (`Filtering` sampler, `FRAGMENT` visibility) and bind the matching-mip-count aniso sampler there in every material bind group, alongside the nearest sampler at binding 1. Both samplers stay resident so the shader selects per mode at no rebind cost.

### Task 3: Post Retro sampling path in forward.wgsl

Add `@group(1) @binding(5) var aniso_sampler: sampler;`. Compute `ddx = dpdx(in.uv)`, `ddy = dpdy(in.uv)` once in `fs_main`. Branch on `uniforms.graphics_mode`:

- **Post Retro:** per slot, texel-grid reconstruction (warp UV toward the nearest texel center, antialias the seam over one screen pixel via `fwidth`) then `textureSampleGrad(tex, aniso_sampler, uv_recon, ddx, ddy)`. Pass the *original* UV derivatives — they drive mip selection and the hardware aniso footprint; the warp only shifts the sample point. Normal slot decodes (`*2 - 1`) and renormalizes after its single reconstructed aniso sample.
- **True Retro:** the manual-aniso path from `shader-anisotropic-filtering`, gated by `graphics_mode == TrueRetro` in place of that plan's compile-time `ENABLE_MANUAL_ANISO` const.

Both branches use `textureSampleGrad` (explicit derivatives), keeping sampling in uniform control flow (the branch is on a uniform-buffer value).

### Task 4: Mod-manifest defaultGraphicsMode

Add `default_graphics_mode: Option<GraphicsMode>` to `ModManifestResult`. In both `run_mod_init_quickjs` and `run_mod_init_luau`, extract the optional `defaultGraphicsMode` string and map `"trueRetro"` / `"postRetro"` → enum; unknown value returns the same `ScriptError` shape as the missing-`name` path. Register the `GraphicsMode` type and the `defaultGraphicsMode?` field in the typedef registry (`scripting/primitives/mod.rs`) and update the ModManifest parity-guard test. After `run_mod_init` succeeds — both the boot path and the start-script hot-reload path — call `renderer.set_graphics_mode` when the manifest carries a mode; otherwise leave the Post Retro construction default.

### Task 5: egui mode toggle

Add a `graphics_mode` field to `DiagnosticsState`, seeded from `renderer.graphics_mode()`. Add a Graphics Mode combo box to `draw_diagnostics_panel` over `GraphicsMode::ALL_VARIANTS`, calling `renderer.set_graphics_mode` on change — same shape as the Lighting Isolation combo.

### Task 6: Perf measurement + aniso-clamp decision

With `POSTRETRO_GPU_TIMING=1`, record forward-pass cost on the target GPU at 1080p for both modes on campaign-test at grazing angles. Drop `POST_RETRO_ANISO_CLAMP` (16 → 8) if the aniso unit cost is unacceptable. Record numbers in the PR.

## Sequencing

**Phase 1 (sequential):** Task 1 — the enum, uniform field, and setter that everything else references.
**Phase 2 (concurrent):** Task 2, Task 4, Task 5 — sampler/BGL, manifest plumbing, debug UI; independent once the enum and setter exist.
**Phase 3 (sequential):** Task 3 — `forward.wgsl` consumes Task 1's uniform and Task 2's aniso-sampler binding.
**Phase 4 (sequential):** Task 6 — measure once the Post Retro path runs.

## Rough sketch

```wgsl
// Proposed design — Post Retro per-slot sample (texel-grid reconstruction + hardware aniso)
fn sample_post_retro(tex: texture_2d<f32>, samp: sampler, uv: vec2<f32>,
                     ddx: vec2<f32>, ddy: vec2<f32>) -> vec4<f32> {
    let dims = vec2<f32>(textureDimensions(tex, 0));
    let uv_tex = uv * dims;
    let seam = floor(uv_tex + 0.5);
    let aa = clamp((uv_tex - seam) / fwidth(uv_tex), vec2(-0.5), vec2(0.5));
    let uv_recon = (seam + aa) / dims;          // warp toward texel center, AA the seam
    return textureSampleGrad(tex, samp, uv_recon, ddx, ddy); // original derivatives drive mip + aniso
}
```

Reconstruction is per-slot because slots may differ in resolution (`dims` from each texture). At minification the `fwidth` window exceeds a texel and the warp degrades to plain linear — exactly where hardware aniso takes over. Up close it gives crisp texels with one-pixel antialiased seams. The two techniques partition by footprint regime; the handoff is automatic.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Mode enum | `GraphicsMode { TrueRetro, PostRetro }` (render module) | manual extract (not serde) | `type GraphicsMode = "trueRetro" \| "postRetro"` | same | n/a |
| Mode string values | `TrueRetro` / `PostRetro` | `"trueRetro"` / `"postRetro"` | same | same | n/a |
| Manifest default | `ModManifestResult.default_graphics_mode: Option<GraphicsMode>` | optional `defaultGraphicsMode` key | `defaultGraphicsMode?: GraphicsMode` | `defaultGraphicsMode: GraphicsMode?` | n/a |
| Shader uniform | `FrameUniforms.graphics_mode: u32` (`TrueRetro=0`, `PostRetro=1`) | uniform buffer | n/a | n/a | n/a |

## Open questions

- **Normal-map averaging under hardware aniso.** Hardware aniso averages *encoded* normal texels along the footprint and decodes once — biasing toward flat `(0.5, 0.5, 1.0)`, the artifact the `shader-anisotropic-filtering` plan avoids by decoding per tap. True Retro keeps that per-tap decode; Post Retro accepts the bias. 2-channel normals (`prm-bc5-normals`: store XY, reconstruct Z) make linear averaging closer to correct and largely retire this. Acceptable for an aesthetic mode now; revisit with BC5.
- **Reconstruction edge feel.** The one-pixel antialiased seam is the deliberate Post Retro difference from True Retro's hard edges. If it reads as too soft, narrow the `fwidth` window (sub-pixel) — costs nothing, sharpens the seam. Decide by eye in the A/B.
- **Aniso clamp value.** 16 is the wgpu ceiling and the quality default. Task 6 may set it to 8 if the target GPU's aniso cost is too high; "much smaller than Crysis" arenas top out around 16:1 aspect, so 16 is unlikely to be wasted.
- **GraphicsMode string casing.** Values pinned camelCase (`"trueRetro"`/`"postRetro"`) to match the script-facing key idiom. Switch to a `#[serde(rename_all)]`-style scheme only if manifest parsing moves to serde.

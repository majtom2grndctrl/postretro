# PRM BC5 Normals

## Goal

Encode normal-map slots in `.prm` files as BC5 (two-channel block compression) instead of Rgba8Unorm. Halves VRAM and disk for normals with no visible aesthetic cost: normal maps are low-frequency relative to pixel-art diffuse, and BC5's per-block RG quantisation stays well below the threshold where specular shading shows banding. Diffuse stays uncompressed — BC1/BC7 visibly degrade hard-edged pixel art and are out of scope.

Depends on the baked-texture-mips plan, which has already landed. This plan also depends on the `retire-true-retro` plan landing first (`context/plans/drafts/retire-true-retro/`): the RG-decode shader change targets the single post-retirement normal path, so True Retro must be gone before this plan applies. This plan extends the `.prm` (PostRetro Material) sidecar format defined there with a new per-slot `format_tag` value. See `context/plans/done/baked-texture-mips/index.md` for `.prm` location and layout, and confirm the existing `format_tag` set against `crates/level-format/src/prm.rs` (the done plan is frozen and may be stale).

## Scope

### In scope

- New `.prm` `format_tag` value `3 = Bc5RgUnorm`. Permitted only on the normal slot. Diffuse and specular slots reject `format_tag = 3` at compile time with a clear error.
- BC5 encoder in `postretro-level-compiler`. Encodes the normal slot's Rgba8Unorm linear-filtered chain into BC5 blocks per mip. Tangent-space encoding: store `(n.x, n.y)` in BC5's R, G channels; reconstruct `n.z = sqrt(max(0, 1 - x*x - y*y))` in the shader.
- Per-mip format fall-back at the block-size floor. The normal slot's BC5 chain stops at the last level whose width and height are both ≥ 4 px. Smaller levels are absent; the sampler clamps to the smallest BC5 level. The slot retains a single `format_tag`; no per-level format byte.
- Runtime GPU upload path detects `format_tag = 3` on the normal slot, requests `wgpu::TextureFormat::Bc5RgUnorm` for the texture, uploads BC5 payload bytes directly per level. Sampler unchanged.
- Adapter feature request: `wgpu::Features::TEXTURE_COMPRESSION_BC` added to the requested feature set. Engine refuses to start on adapters without it — clean error, no fallback path.
- Shader change in `forward.wgsl`: normal-slot sample returns RG; reconstruct Z and renormalise. Diffuse and specular sampling untouched.
- `STAGE_VERSION` bump for the `.prm` texture-baker cache stage. Existing `.prm` files regenerate on first build after the bump.

### Out of scope

- BC1, BC3, BC7 on diffuse. Aesthetic conflict with hard-edged pixel art.
- ETC2 / ASTC. Mobile-tier; target hardware is GTX 10-series and newer desktop.
- BC5 on specular. R8Unorm is already one byte per texel — compression gain is marginal and BC4 is a separate decision.
- Streaming or progressive mip residency.
- Per-texture or per-material override to disable BC5. One filter, one pipeline.
- Recompressing existing `.prm` files in-place when `STAGE_VERSION` bumps. Cache regenerates naturally on next build.
- Fallback path for adapters without `TEXTURE_COMPRESSION_BC`. BC5 is a hard requirement; no Rgba8 fallback shipped.
- Signed BC5 (`Bc5RgSnorm`). Unsigned with the standard `* 2 - 1` shader decode is sufficient for tangent-space normals and matches the existing Rgba8Unorm normal encoding.

## Acceptance criteria

- [ ] Round-trip test: synthetic tangent-space normal map encoded to BC5, decoded, Z-reconstructed. Every output normal is unit-length within 1/127 and within 2° of the input direction.
- [ ] Disk: normal `.prm` payloads for the campaign-test scene are ≤ 30% of the pre-BC5 baseline (BC5 is 4 bpp vs 32 bpp for Rgba8Unorm; the tail-out truncation removes the sub-4×4 levels entirely, so the BC5 portion is ~25% and the small header overhead keeps the aggregate under 30%).
- [ ] VRAM: normal texture footprint on the same scene is ≤ 30% of the pre-BC5 baseline by the same reasoning.
- [ ] The normal slot's written `level_count` equals the number of source mip levels whose width and height are both ≥ 4 px (sub-4×4 levels absent); a distant normal-mapped surface sampling the smallest BC5 mip renders with no sampler error under the shared `lod_max_clamp`.
- [ ] `cargo run -p postretro -- content/dev/maps/campaign-test.prl` renders with no rendering errors. Specular highlights on normal-mapped surfaces at grazing angles show no visible quantisation banding compared to the pre-BC5 build (A/B screenshot pair in the PR).
- [ ] Engine started on an adapter without `TEXTURE_COMPRESSION_BC` exits with a clear error naming the missing feature. Verified by temporarily masking the feature from the requested set.
- [ ] `.prm` cache stage `STAGE_VERSION` is incremented from `1` to `2`. A second `cargo run -p postretro-level-compiler` with no source changes touches no files (cache hit on the new version).
- [ ] Compiler rejects a `format_tag = 3` on the diffuse or specular slot with a clear error naming the slot and the file.

## Tasks

### Task 1: BC5 encoder dependency and wrapper

Pick the encoder crate. Investigate `intel_tex_2` (Intel ISPC Texture Compressor bindings, MIT licensed, ships ISPC binaries) and `texpresso` (pure-Rust BC1–BC3 — does not currently ship BC5, confirm during the task) and `basis_universal` (broader scope, heavier). Recommend the most permissively-licensed, actively-maintained crate that ships BC5 RG encoding. If none fits, write a small in-tree BC5 block encoder (BC5 = two independent BC4 blocks; BC4 is a min/max endpoint pair plus 3-bit-per-texel selectors over a 4×4 block — straightforward, ~150 LoC).

Expose a single helper in `postretro-level-compiler` that takes a `&[u8]` Rgba8Unorm linear normal-map level (width, height, row-major, tight) and returns BC5 bytes. Width and height must be ≥ 4 and multiples of 4; caller pads or skips per the per-mip rule (Task 3). Include a unit test on a fixed synthetic normal map asserting the round-trip tolerance from acceptance criterion #1. The test reconstructs `n.z` in Rust mirroring the shader formula (`sqrt(max(0, 1 - x*x - y*y))`), since the shader path is not exercisable from a compiler unit test.

### Task 2: `.prm` format-tag extension

Add `Bc5RgUnorm = 3` to the `.prm` per-slot format enum `PrmFormat` (in the shared `postretro-level-format` crate, `crates/level-format/src/prm.rs`; existing variants are `Rgba8UnormSrgb = 0` diffuse, `Rgba8Unorm = 1` normal, `R8Unorm = 2` specular), and extend `PrmFormat::from_tag` to map `3`. The enum change lands in the shared crate, consumed by both compiler and runtime. Update the `.prm` writer to permit `format_tag = 3` only on the normal slot — diffuse and specular writes fail with an error naming the offending slot and the source/`.prm` file. The wgpu mapping stays in the renderer: `prm_format_to_wgpu` (`crates/postretro/src/render/loaded_texture.rs`) gains the `PrmFormat::Bc5RgUnorm => wgpu::TextureFormat::Bc5RgUnorm` arm (added in Task 4). `level-format` stays wgpu-free. Bump the `.prm` texture-baker `STAGE_VERSION` (`u8`, `crates/level-format/src/prm.rs`) from `1` to `2`. See baked-texture-mips for the `.prm` byte layout and slot ordering; no new sections.

### Task 3: Mip tail-out at the BC5 block-size floor

BC5 requires 4×4 block alignment. The encoder stops the normal slot's BC5 chain at the last level whose width and height are both ≥ 4 px. Smaller levels are not written. The `.prm` normal slot's `level_count` reflects the truncated chain. Levels ≥ 4 px but not a multiple of 4 (possible for non-power-of-two sources, since the chain halves to 1×1) are padded up to the next multiple of 4 by replicating edge texels into the trailing partial block, matching the mip downsampler's clamp-to-edge. Only levels with width or height < 4 are dropped. The runtime's existing per-texture `lod_max_clamp` (set to `level_count - 1`) clamps sampling to the shortest chain present — no new sampler machinery required.

Option A (per-level format byte) was rejected: wgpu/WebGPU textures carry one `TextureFormat` for the entire mip chain, so mixed BC5/Rgba8 levels in a single texture are not a valid GPU primitive.

### Task 4: Runtime upload and adapter feature

Add `wgpu::Features::TEXTURE_COMPRESSION_BC` to the requested features in the renderer's adapter request. On request failure, surface a clear startup error naming the missing feature. Extend the runtime upload path: when a normal-slot `.prm` level reports `format_tag = 3`, allocate the texture as `Bc5RgUnorm`, upload BC5 bytes per level with the correct block-aligned `bytes_per_row` (block-row stride = ceil(w/4) * 16). Diffuse and specular paths unchanged.

### Task 5: Shader Z-reconstruction

In `forward.wgsl`, the normal-slot sample path changes from `vec4` → `(rgb * 2 - 1)` → `normalize` to:

```wgsl
let rg = textureSample(normal_tex, normal_samp, uv).rg * 2.0 - 1.0;
let z  = sqrt(max(0.0, 1.0 - dot(rg, rg)));
let n  = normalize(vec3<f32>(rg, z));
```

(`// Proposed design` — remove once landed.) Renormalise unconditionally: BC5 endpoint quantisation plus bilinear filtering leaves sampled normals off unit length, and acceptance criterion #1 requires unit length within 1/127. Confirm against acceptance criterion #3. This plan assumes the `retire-true-retro` plan has already landed, collapsing the two current arms of `sample_normal` (which today dispatches to `sample_post_retro` and the True Retro `sample_aniso_normal`) into a single surviving normal-decode function; the RG decode edits that one function. Confirm its name against the post-retirement shader (expected `sample_post_retro`) before editing. Also update the existing `forward.wgsl` comment declaring BC5 out of scope for the 3-channel path.

## Sequencing

**Phase 1 (sequential):** Task 1 — encoder availability is resolved here, including the in-tree-encoder fallback; Tasks 3–4 only consume the resulting helper.
**Phase 2 (concurrent):** Task 2, Task 3 — `.prm` format and per-level fall-back are independent of encoder internals.
**Phase 3 (concurrent):** Task 4, Task 5 — runtime upload and shader Z-reconstruction land together; either alone produces broken visuals.

## Rough sketch

- BC5 block layout (per 4×4 block, 16 bytes): two BC4 blocks back-to-back. Block 0 = R channel, block 1 = G channel. Each BC4 block is `[min: u8, max: u8, selectors: 48 bits]`.
- Encoder strategy: per block, find min/max of the channel's 16 texels; build the 8-entry endpoint palette (interpolated between min and max); for each texel pick the closest palette entry. Standard reference: Microsoft BC4/BC5 spec. No need for cluster fit or refinement passes — normal maps are smooth enough that the trivial endpoint search produces results within the acceptance tolerance.
- BC5's two-channel storage maps cleanly to tangent-space `(n.x, n.y)`: both channels are normalised independently, which is what BC5 does naturally.
- Cache key for the texture-baker stage: PNG content blake3 + `STAGE_VERSION`. Bumping the stage version is the only migration mechanism — existing `.prm` files outside the cache are not touched, and the cache regenerates them on next build.
- The `.prm` `STAGE_VERSION` bump (1 → 2) only forces `.prm` sidecar cache regeneration; it is distinct from and does not change the PRL container `version`, which is unchanged. BC5 remains additive at the PRL level — only the `.prm` sidecar cache regenerates.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Format tag | `PrmFormat::Bc5RgUnorm` | `u8 = 3` (in `.prm` per-slot header) | n/a | n/a | n/a |
| Adapter feature | `wgpu::Features::TEXTURE_COMPRESSION_BC` | n/a | n/a | n/a | n/a |
| GPU format | `wgpu::TextureFormat::Bc5RgUnorm` | n/a | n/a | n/a | n/a |

No new PRL sections. No FGD or scripting surface changes. No wire-format change to the per-slot `.prm` layout: the normal slot keeps its single per-slot `format_tag`; only its `level_count` and payload shrink.

## Open questions

- **Per-level format vs. tail-out (Task 3).** Resolved: tail-out (option B).
- **Encoder crate choice.** `intel_tex_2` is the likely pick if it currently exposes BC5 RG and its ISPC binary distribution is acceptable for the build pipeline. Confirm during Task 1; fall back to in-tree encoder if not.
- **Specular shading sensitivity.** BC5's per-block endpoint quantisation can produce banding when the normal varies smoothly across a block boundary and a specular highlight grazes that band. Worst case is mirror-smooth specular on a gently curved surface; pixel-art content rarely hits this. Visible bands in the campaign-test A/B fail acceptance criterion #4; the fix is encoder refinement (cluster-fit endpoints), not a softened gate.
- **Snorm vs. unorm.** This plan picks `Bc5RgUnorm` with `* 2 - 1` shader decode to match the existing Rgba8Unorm normal-map convention. `Bc5RgSnorm` would skip the decode but breaks symmetry with future non-BC5 normal slots. Revisit only if shader decode shows in a profile.

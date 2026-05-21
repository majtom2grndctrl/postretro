# PRM BC5 Normals

## Goal

Encode normal-map slots in `.prm` files as BC5 (two-channel block compression) instead of Rgba8Unorm. Halves VRAM and disk for normals with no visible aesthetic cost: normal maps are low-frequency relative to pixel-art diffuse, and BC5's per-block RG quantisation stays well below the threshold where specular shading shows banding. Diffuse stays uncompressed — BC1/BC7 visibly degrade hard-edged pixel art and are out of scope.

Depends on the baked-texture-mips plan landing first. This plan extends the `.prm` (PostRetro Material) sidecar format defined there with a new per-slot `format_tag` value. See `context/plans/drafts/baked-texture-mips/index.md` for `.prm` location, layout, and the existing `format_tag` set.

## Scope

### In scope

- New `.prm` `format_tag` value `3 = Bc5RgUnorm`. Permitted only on the normal slot. Diffuse and specular slots reject `format_tag = 3` at compile time with a clear error.
- BC5 encoder in `postretro-level-compiler`. Encodes the normal slot's Rgba8Unorm linear-filtered chain into BC5 blocks per mip. Tangent-space encoding: store `(n.x, n.y)` in BC5's R, G channels; reconstruct `n.z = sqrt(max(0, 1 - x*x - y*y))` in the shader.
- Per-mip format fall-back at the block-size floor. Mips whose width or height is below the BC5 block size (4 px) cannot be BC5-encoded; those levels stay Rgba8Unorm. The `.prm` carries the format per level (see *Per-level format*).
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
- [ ] Disk size for the campaign-test scene's normal `.prm` payloads is ≤ 55% of the pre-BC5 baseline. VRAM measurement on the same scene shows the same ratio for normal textures.
- [ ] `cargo run -p postretro -- content/dev/maps/campaign-test.prl` renders with no rendering errors. Specular highlights on normal-mapped surfaces at grazing angles show no visible quantisation banding compared to the pre-BC5 build (A/B screenshot pair in the PR).
- [ ] Engine started on an adapter without `TEXTURE_COMPRESSION_BC` exits with a clear error naming the missing feature. Verified by temporarily masking the feature from the requested set.
- [ ] `.prm` cache stage `STAGE_VERSION` is incremented. A second `cargo run -p postretro-level-compiler` with no source changes touches no files (cache hit on the new version).
- [ ] Compiler rejects a `format_tag = 3` on the diffuse or specular slot with a clear error naming the slot and the file.

## Tasks

### Task 1: BC5 encoder dependency and wrapper

Pick the encoder crate. Investigate `intel_tex_2` (Intel ISPC Texture Compressor bindings, MIT licensed, ships ISPC binaries) and `texpresso` (pure-Rust BC1–BC3 — does not currently ship BC5, confirm during the task) and `basis_universal` (broader scope, heavier). Recommend the most permissively-licensed, actively-maintained crate that ships BC5 RG encoding. If none fits, write a small in-tree BC5 block encoder (BC5 = two independent BC4 blocks; BC4 is a min/max endpoint pair plus 3-bit-per-texel selectors over a 4×4 block — straightforward, ~150 LoC).

Expose a single helper in `postretro-level-compiler` that takes a `&[u8]` Rgba8Unorm linear normal-map level (width, height, row-major, tight) and returns BC5 bytes. Width and height must be ≥ 4 and multiples of 4; caller pads or skips per the per-mip rule (Task 3). Include a unit test on a fixed synthetic normal map asserting the round-trip tolerance from acceptance criterion #1.

### Task 2: `.prm` format-tag extension

Add `format_tag = 3 = Bc5RgUnorm` to the `.prm` per-slot format enum. Update the `.prm` writer to permit `format_tag = 3` only on the normal slot — diffuse and specular writes fail. Update the reader to map `3` to `wgpu::TextureFormat::Bc5RgUnorm`. Bump the `.prm` texture-baker `STAGE_VERSION`. See baked-texture-mips for the `.prm` byte layout and slot ordering; no new sections.

### Task 3: Per-level format and mip fall-back

Each level in a normal-slot mip chain carries its own format tag. BC5 requires 4×4 block alignment, so levels whose width or height drops below 4 px stay Rgba8Unorm. Two paths to choose from in this draft:

- **A: Per-level format byte.** Each level header carries a one-byte format tag. Encoder writes BC5 for levels ≥ 4×4, Rgba8Unorm for smaller levels. Loader picks the wgpu format per level. Tradeoff: GPU textures with mixed-format mip chains are not directly supported — the runtime would need to upload smaller levels into a separately-allocated Rgba8 texture or skip them.
- **B: Tail-out early.** Stop the BC5 chain at the last level ≥ 4×4. Smaller levels are simply absent from the BC5 texture; sampler `lod_max_clamp` is set to that last level. Visual impact: smallest mips (4×4 and below) are unavailable; sampler clamps to the smallest BC5 level. Simpler runtime, one format per texture.

**Recommend B.** Mips at 2×2 and 1×1 contribute negligibly to grazing-angle shading and the lod-clamp is already a per-texture knob from the baked-texture-mips plan. Pick during draft review.

### Task 4: Runtime upload and adapter feature

Add `wgpu::Features::TEXTURE_COMPRESSION_BC` to the requested features in the renderer's adapter request. On request failure, surface a clear startup error naming the missing feature. Extend the runtime upload path: when a normal-slot `.prm` level reports `format_tag = 3`, allocate the texture as `Bc5RgUnorm`, upload BC5 bytes per level with the correct block-aligned `bytes_per_row` (block-row stride = ceil(w/4) * 16). Diffuse and specular paths unchanged.

### Task 5: Shader Z-reconstruction

In `forward.wgsl`, the normal-slot sample path changes from `vec4` → `(rgb * 2 - 1)` → `normalize` to:

```wgsl
let rg = textureSample(normal_tex, normal_samp, uv).rg * 2.0 - 1.0;
let z  = sqrt(max(0.0, 1.0 - dot(rg, rg)));
let n  = vec3<f32>(rg, z);
```

(`// Proposed design` — remove once landed.) Renormalise only if filtering tolerance demands it (BC5 hardware sampling already produces near-unit-length results after the sqrt). Confirm against acceptance criterion #3. The shader-aniso plan's `sample_aniso` helper, when applied to the normal slot, must use the same RG-then-reconstruct path; coordinate at integration time.

## Sequencing

**Phase 1 (sequential):** Task 1 — encoder availability decides whether Task 3 needs an in-tree implementation.
**Phase 2 (concurrent):** Task 2, Task 3 — `.prm` format and per-level fall-back are independent of encoder internals.
**Phase 3 (concurrent):** Task 4, Task 5 — runtime upload and shader Z-reconstruction land together; either alone produces broken visuals.

## Rough sketch

- BC5 block layout (per 4×4 block, 16 bytes): two BC4 blocks back-to-back. Block 0 = R channel, block 1 = G channel. Each BC4 block is `[min: u8, max: u8, selectors: 48 bits]`.
- Encoder strategy: per block, find min/max of the channel's 16 texels; build the 8-entry endpoint palette (interpolated between min and max); for each texel pick the closest palette entry. Standard reference: Microsoft BC4/BC5 spec. No need for cluster fit or refinement passes — normal maps are smooth enough that the trivial endpoint search produces results within the acceptance tolerance.
- BC5's two-channel storage maps cleanly to tangent-space `(n.x, n.y)`: both channels are normalised independently, which is what BC5 does naturally.
- Cache key for the texture-baker stage: PNG content blake3 + `STAGE_VERSION`. Bumping the stage version is the only migration mechanism — existing `.prm` files outside the cache are not touched, and the cache regenerates them on next build.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Format tag | `PrmFormatTag::Bc5RgUnorm` | `u8 = 3` (in `.prm` per-slot header) | n/a | n/a | n/a |
| Adapter feature | `wgpu::Features::TEXTURE_COMPRESSION_BC` | n/a | n/a | n/a | n/a |
| GPU format | `wgpu::TextureFormat::Bc5RgUnorm` | n/a | n/a | n/a | n/a |

No new PRL sections. No FGD or scripting surface changes. `.prm` per-level format byte is internal to the format defined by the baked-texture-mips plan.

## Open questions

- **Per-level format vs. tail-out (Task 3).** Recommendation is tail-out (option B). Confirm during draft review or before promoting to `ready/`.
- **Encoder crate choice.** `intel_tex_2` is the likely pick if it currently exposes BC5 RG and its ISPC binary distribution is acceptable for the build pipeline. Confirm during Task 1; fall back to in-tree encoder if not.
- **Specular shading sensitivity.** BC5's per-block endpoint quantisation can produce banding when the normal varies smoothly across a block boundary and a specular highlight grazes that band. Worst case is mirror-smooth specular on a gently curved surface; pixel-art content rarely hits this, but flag for follow-up if the campaign-test A/B reveals visible bands.
- **Snorm vs. unorm.** This plan picks `Bc5RgUnorm` with `* 2 - 1` shader decode to match the existing Rgba8Unorm normal-map convention. `Bc5RgSnorm` would skip the decode but breaks symmetry with the uncompressed-fallback levels (option A) or with future non-BC5 normal slots. Revisit only if shader decode shows in a profile.

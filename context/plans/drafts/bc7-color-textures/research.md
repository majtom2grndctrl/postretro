# bc7-color-textures — research

Read at 784af6536. Derivation behind the brief; findings that inform but do not decide.

## Encoder options

There is no credible pure-Rust BC7 encoder on crates.io (checked 2026-10-11).

| Candidate | Implementation | Disqualifier |
|---|---|---|
| `intel_tex_2` | ISPC kernels behind FFI, runtime ISA dispatch (SSE/AVX/NEON) | `unsafe` FFI. Its float kernels very likely produce different bytes on different CPUs (inferred, not measured). Panics on unsupported architectures. |
| `ctt` + `ctt-*` | Wraps bc7enc_rdo/bc7e, Compressonator, etcpak (FFI, prebuilt static libraries) | FFI. Same float and SIMD determinism concern. Heavy. |
| `block_compression` | WGSL compute port of ISPC kernels | GPU only. Needs an adapter at bake time and results vary by driver. |
| `image_dds` | Wraps `intel_tex_2` | Same as `intel_tex_2`. |
| `texpresso`, `tbc` | Pure Rust | No BC7. |
| `basis-universal` | C++ bindings | A transcoder, not a direct BC7 encoder. |

In-tree precedent: `bc5.rs` and `bc6h.rs` are dependency-free, integer-only and about 550 lines each, which makes them platform-deterministic by construction. An in-tree encoder covering modes 1, 5 and 6 is an estimated 800–1500 lines (not measured).

## Determinism and cache

- `build_pipeline.md` Determinism invariant: BC6H lightmap output is exempt (it is lossy, so correctness means round-trip within tolerance). The BC5 shadowmask is explicitly not exempt. An in-tree integer encoder keeps BC7 non-exempt.
- `bundle_hash_for` (texture_mips cache) hashes the presence mask plus tagged source PNG bytes, so it carries no bake-transform identity. Changing what bytes mean requires a `STAGE_VERSION` bump. A cache hit never reruns the encoder, so cross-machine byte drift would show up only as different `.prm` bytes, never as a cache miss. That is why determinism matters even though the cache stays correct.
- `STAGE_VERSION` is 3, while the magic's fourth byte is `\x02`. They are no longer strictly in lockstep. Adding a tag leaves the layout unchanged, so the magic stays.

## Producers and consumers

- One chain function, `build_diffuse_chain_impl`, does sRGB LUT → linear f32 → Mitchell downsample → sRGB re-encode, carrying straight alpha. Its callers:
  - world slot 0 and slot 3 (`bake_world_texture_mips`)
  - sprite and flipbook frames (`bake_sprite_collection`, layer-major)
  - model diffuse (`bake_diffuse_texture`, reached from the compiler and from `postretro-tool bake-model-textures`)
- How each consumer uses alpha:
  - World forward and models: `blend: None`, so alpha is ignored.
  - Billboards: additive One/One blend. Output is `(lit + emissive) * a`, so alpha is a coverage weight and BC7 alpha error is visible there.
- Samplers:
  - World, movers and static models: linear with 16× anisotropy.
  - Skinned models: nearest magnification.
  - Baked sprites: linear, clamp to edge.
  - Every material path samples through `sample_post_retro`, which snaps the UV to the texel grid with an anti-aliased seam. A magnified texel renders crisp, so a per-texel BC7 colour error shows as a sharp wrong-coloured texel, not a blur. The A/B has to be done close up.
- No CPU code decodes color-slot texels. Every reader slices or uploads by size through `mip_level_bytes` / `upload_bytes_per_pixel`.
- Placeholders stay `Rgba8UnormSrgb`. Bind group layouts are filterable float and never keyed on format, so a BC7 view binds unchanged.

## Representability

- wgpu-core 30 (`TextureDimensionError::NotMultipleOfBlockWidth/Height`) rejects a block-compressed texture whose base size is not a multiple of the block size. Only mip 0 is constrained.
- `build_normal_bc5_chain_impl` pads each level up to a multiple of 4, but the renderer creates the texture at the logical mip 0 size. A 6×6 normal map therefore bakes BC5 and then fails texture creation. That is the latent defect the shared rule cures.
- `sample_normal` reconstructs z from `.rg` only, so an `Rgba8Unorm` fallback normal shades the same as BC5.

## Content shape (dev, 2026-10-11)

Source PNG dimensions, excluding `.prl-cache`:

| Size | Count |
|---|---|
| 1024² (mostly `textures/50-free-textures`) | 166 |
| 64² | 125 |
| 32² | 64 |
| 128² | 24 |
| 2048² | 6 |

Nearly all of the byte win sits in the 1024² sets. The aesthetic risk sits in the 32–64 px pixel art.

## Footprint arithmetic

A 1024² RGBA8 chain with full mips is about 5.6 MB. The same chain in BC7 (8 bpp, tail-out at 4×4) is about 1.4 MB. That gives BC7 ≈ 25% of RGBA8 for block-aligned power-of-two textures. The ≤ 30% acceptance row leaves headroom for small and non-aligned textures that stay RGBA8.

## Decision notes

- **Who wins.** Dev content's baked materials total about 279 MB (2026-10-11), dominated by the 1024² photo sets. The engine's own low-res pixel art saves little, and the gate exists to keep it exact. The win goes to mods and content that ship hi-res textures.
- **No shader change.** Hardware sRGB decode yields the same color the shader samples today, as `plans/done/emissive-surfaces-bloom` §Wire format anticipated. Upload maps format through one shared match.
- **Old readers.** An unknown format tag returns `UnsupportedFormatTag` and the slot degrades to a placeholder. The `STAGE_VERSION` bump rejects old sidecars first anyway.
- **Determinism and the gate.** A machine-dependent encoder would make the BC7-or-RGBA8 choice itself vary between machines, not just the bytes. That is why the in-tree encoder is load-bearing for the gate.
- **Later encoder changes.** Any change that alters bytes bumps `STAGE_VERSION` again, because the key carries no encoder identity.
- **Sprite warm builds.** `bake_sprite_collection` builds diffuse and BC5 normal chains before its cache check. World and model paths check first. With BC7 the sprite order would re-encode every warm build.
- **Sprite normal gate.** `plan_baked_sprite_array` accepts only `Bc5RgUnorm` normals, and a test pins the rejection of `Rgba8Unorm`. `billboard.wgsl` reads the normal from `.rg`, so admitting the RGBA8 fallback is shader-safe.

## Gate metric

An absolute worst-block metric is biased against the win:
- A 1024² texture has hundreds of times more blocks than a small one, so more chances for one bad block.
- Noisy photo blocks (grass, bark, concrete) are the "many unrelated colours" case that modes 1, 5 and 6 reproduce worst. Yet noise hides their error.
- The same error in a 3-colour pixel-art block is plainly visible.

Candidates, all as integer scores in sRGB-encoded space with alpha weighted:
- absolute max per-channel error, or block SSE
- error normalised by the block's own contrast or variance
- a deterministic distinct-colour class per block: a strict bound for low-palette blocks, a looser one for noisy blocks

The first slice reports the photo-set pass share under each candidate at a pixel-art-safe threshold before picking.

## Rivals not taken

| Rival | Why not |
|---|---|
| FFI encoders (`intel_tex_2`, `ctt`) | See Encoder options: they break the SDK build or determinism |
| Authored per-texture compression setting (Unreal, Godot) | A modder-facing one-way door with no consumer yet. The chokepoint keeps it additive |
| BC1 for opaque world diffuse | Half of BC7's footprint again, but 565 endpoints and 4 colours per block fail far more blocks, so the gate would reject much more. Sprites need alpha, so BC7 ships anyway. Could later become a second format behind the chokepoint, tried first on opaque slots |
| Minimum-size cutoff (no BC7 below, say, 256²) | The 128–256² weapon art and a mod's 512 px pixel art still need a gate, so the gate subsumes it. Small textures are cheap to encode |
| Runtime top-mip drop | Complementary, not rival: a low-VRAM player option that costs crispness. A separate brief |

## Pin table

Edges and orderings the Acceptance rows cite.

| Id | Scenario | Ordering / timing | Expected outcome |
|---|---|---|---|
| P1 | Flipbook: 3 layers clean, 1 over the fidelity threshold | Bake | The whole slot stays `Rgba8UnormSrgb` for every layer (format is per slot, not per layer). Layer count and layer-major order are unchanged. Listed once with reason "fidelity" |
| P2 | Color texture aligned at mip 0, unaligned below (12×12, 36×20) | Bake | Chain truncated at the block floor. Each level edge-padded to a multiple of 4 and sized by ceil block count. Padding texels add no error and never decide the gate |
| P3 | One sidecar: BC7 diffuse + RGBA8 emissive, the reverse, or BC7 diffuse beside RGBA8 normal and R8 specular | Read, then upload plan | Each slot reads and plans under its own format and level count. The shared LOD clamp is the deepest slot's level count, and no slot is planned for levels it lacks |
| P4 | Model diffuse PNG byte-identical to a world diffuse PNG | Tool path and compiler path, either order | The sidecar at the shared key is byte-identical whichever path writes first. A pre-bump sidecar at that key is rebaked, never kept as a valid richer bundle |
| P5 | Same inputs | Cold build, then warm build | Identical byte summary: bytes per format, uncompressed list, reasons. The reason needs no persistence: after the bump, an uncompressed color slot with unaligned base dims is "size" and one with aligned base dims is "fidelity" |
| P6 | Bundle with no color slot | Bake | No BC7 encode runs |

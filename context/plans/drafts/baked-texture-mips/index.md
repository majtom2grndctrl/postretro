# Baked Texture Mips

## Goal

Move texture mip-chain generation out of the renderer and into `prl-build`. Mip pyramids are baked once at compile time in linear color space, stored in the PRL, and uploaded directly at runtime. Zero CPU work at level load, gamma-correct mips that don't darken midtones, and mips become asset data rather than a renderer side-effect.

## Scope

### In scope

- New PRL section carrying per-texture, per-mip pixel data for diffuse, specular, and normal slots.
- Compiler-side mip generation in `postretro-level-compiler`. Mitchell-Netravali separable filter (B = 1/3, C = 1/3) on every slot. **Filtering happens in linear space**: sRGB diffuse decoded to linear before filtering and re-encoded on output; R8 specular treated as linear; Rgba8 normal filtered linearly then renormalised per texel.
- Runtime `upload_texture_data` accepts a pre-baked mip chain and uploads each level directly.
- Sampler `lod_max_clamp` tightened to actual mip count per texture (currently 24.0, over-provisioned).
- PRL format version bump (3 → 4); old files rejected by the loader (existing behaviour for version mismatch).

### Out of scope

- Block-compressed formats (BCn / ETC2). Pixel data stays Rgba8 / R8.
- Anisotropic mip generation (separate vertical / horizontal LOD chains, RIP-maps).
- Per-texture filter override surface (suffix, sidecar TOML, CLI flag). One filter, one pipeline, no authoring knobs until an artist complains.
- Streaming, partial residency, runtime cache of baked mips.
- Hot-reload of textures while the engine is running.
- Compatibility shims for PRL v3 — pre-release, format bump is hard.
- Runtime mip generation as a fallback. The renderer no longer carries the downsample code path; if a PRL omits the section, load fails the same way any missing required section does.
- Shader-side anisotropic filtering for grazing-angle surfaces. Separate plan; depends on this one landing first.

## Acceptance criteria

- [ ] `prl-build` writes a `TextureMips` section for every texture name referenced by the map, covering the diffuse and (when source PNGs exist) specular and normal slots.
- [ ] Each mip chain runs from level 0 (source resolution) down to a 1×1 final level, with `floor(log2(max(w, h))) + 1` levels total.
- [ ] PRL header `version` is 4. Loading a v3 file fails with `UnsupportedVersion`.
- [ ] `cargo run -p postretro -- content/dev/maps/campaign-test.prl` renders the campaign-test map with no rendering errors; midtones may appear lighter than the current build because of the gamma-correct filtering change, which is the intended improvement. `upload_texture_data` performs no CPU downsample (verifiable by removing the downsample code path from the renderer crate without breaking the build).
- [ ] Unit test confirms gamma-correct Mitchell-Netravali output for a fixed sRGB input matches a golden reference (computed in linear space) within ±1 LSB per channel.
- [ ] Unit test on a 50/50 black/white sRGB checker confirms mid mips average to sRGB midgrey (~0.73 in byte space), not the linear midpoint (~0.5). This is the gamma-correctness regression guard.
- [ ] Normal-map mips remain unit-length within 1/127 after filtering (test on a synthetic input).
- [ ] Loading a PRL with no source PNGs on disk renders correctly — pixel data comes entirely from the PRL.
- [ ] Sampler `lod_max_clamp` for each texture equals `mip_count - 1`, not the global 24.0.

## Tasks

### Task 1: PRL TextureMips section in `postretro-level-format`

Add `SectionId::TextureMips = 32` and `texture_mips.rs` with `TextureMipsSection` (`to_bytes` / `from_bytes`, round-trip test). Layout described under *Wire format*. Bump `CURRENT_VERSION` to 4 and update the `UnsupportedVersion` test fixture. Update `context/lib/build_pipeline.md`'s section table at promotion time (not during the task).

### Task 2: Compiler-side mip baker in `postretro-level-compiler`

Add `texture_mips.rs` next to `texture_validation.rs`. Exposes one entry point that takes the deduplicated texture-name list (the same list `TextureNamesSection` is built from) and the texture root path; returns a `TextureMipsSection`. Reads each `<name>.png`, `<name>_s.png`, `<name>_n.png` from the texture root using the same case-insensitive lookup the runtime uses today (factor `build_name_to_path_map` into a shared helper — see *Plumbing*).

Generates the chain with slot-correct gamma handling:

- **Diffuse (Rgba8UnormSrgb):** decode each byte to linear `f32` via a 256-entry sRGB→linear LUT, run separable Mitchell-Netravali in linear `f32`, encode back to sRGB on output. Alpha treated as linear throughout (premultiplied alpha is not used).
- **Specular (R8Unorm):** filter directly in linear `f32`. Already linear on disk.
- **Normal (Rgba8Unorm):** filter linearly in `f32`, then per output texel decode `n = sample.rgb * 2 - 1`, normalise, re-encode. Fall back to `(0, 0, 1)` if `||n|| < 1e-4` (degenerate from filtering).

Missing source = no entry for that slot, same semantics as the runtime today. Wire into `pack.rs` after `TextureNames` is built. Hook into the build cache (`cache.rs`) keyed on PNG content hash + `STAGE_VERSION`.

### Task 3: Runtime upload path in `crates/postretro/src/render/mod.rs`

Change `upload_texture_data`'s signature to take pre-baked levels — a slice of `(level_w, level_h, &[u8])` — instead of a single mip-0 buffer. Delete `downsample_2x`, the `mitchell_netravali` weight code, and `mip_level_count_for`; the caller now supplies the level count. Update the three real call paths (diffuse, specular, normal in `gpu_textures` build) plus the placeholders (`black_specular_texture`, `neutral_normal_texture`, `generate_placeholder` checkerboard, `Placeholder Texture Diffuse`) — placeholders carry a single 1×1 level. Replace `crates/postretro/src/texture.rs`'s `LoadedTexture` with a struct that owns the mip chain decoded from the PRL `TextureMips` section; `load_textures` becomes "index the section by name", not "scan a directory for PNGs."

Tighten the sampler: set `lod_max_clamp` per texture (or globally to `max_mip_count - 1`) rather than the current 24.0.

## Sequencing

**Phase 1 (sequential):** Task 1 — defines the section type Task 2 produces and Task 3 consumes.
**Phase 2 (concurrent):** Task 2, Task 3 — both depend on Task 1 only; they meet at the section bytes. Task 3 can be implemented and unit-tested against a hand-crafted v4 PRL fixture, but the campaign-test acceptance check requires Task 2 done.

## Rough sketch

- `TextureMipsSection` mirrors `TextureNamesSection`'s ordering invariant: entry `i` describes the texture at `TextureNamesSection.names[i]`. Each entry carries three optional sub-chains (diffuse, specular, normal). A bitmask on the entry header indicates which slots are present.
- Filter implementation: precompute 1D Mitchell-Netravali weights once per (src_len, dst_len) pair; apply horizontally into a scratch `f32` buffer, then vertically. Edge condition: clamp-to-edge sampling.
- sRGB encode/decode: 256-entry decode LUT, IEC 61966-2-1 piecewise curve for encode (linear `< 0.0031308` segment, gamma branch above). Polynomial approximation acceptable if the regression test passes.
- Stage versioning: `texture_mips::STAGE_VERSION = 1`. Bump on any change to filter math or section layout.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Section enum | `SectionId::TextureMips` | `u32 = 32` | n/a | n/a | n/a |
| Section struct | `TextureMipsSection` | section body bytes | n/a | n/a | n/a |
| Slot bitmask | `MipSlot::{Diffuse, Specular, Normal}` | `u8` bits 0/1/2 | n/a | n/a | n/a |

## Wire format

Little-endian throughout. Length-prefix integers are `u32`. Entry order matches `TextureNamesSection.names` exactly, one entry per name.

Section body:

```
u32  entry_count                            -- equals TextureNamesSection.names.len()
per entry:
  u8   slot_mask                            -- bit 0 diffuse, bit 1 specular, bit 2 normal
  u8   reserved[3]                          -- padding; must be zero
  per present slot (in slot order diffuse → specular → normal):
    u16  width                              -- mip 0 width, >= 1
    u16  height                             -- mip 0 height, >= 1
    u8   level_count                        -- floor(log2(max(w, h))) + 1
    u8   format_tag                         -- 0 = Rgba8UnormSrgb, 1 = Rgba8Unorm, 2 = R8Unorm
    u8   reserved[2]
    u32  payload_bytes                      -- total bytes for all levels concatenated
    [u8; payload_bytes]                     -- levels packed back-to-back, level 0 first,
                                            -- each level a tight bytes_per_pixel * w * h
                                            -- block (no row alignment padding)
```

Empty entry encoding: `slot_mask = 0`, no sub-blocks.

## Plumbing

- The compiler's PNG lookup currently lives only in the runtime (`crates/postretro/src/texture.rs::build_name_to_path_map`). Lift it into `postretro-level-compiler`; the runtime no longer scans PNGs after Task 3.
- `pack.rs` must receive the texture-root path. It already receives the parsed map; add a path argument alongside the existing texture-validation pass that reads PNG dimensions. The CLI in `main.rs` already knows the texture root.
- The build cache (`cache.rs`) needs a new stage entry `"texture_mips"`. Inputs hash: each referenced PNG's content blake3 + `STAGE_VERSION`.
- Runtime loader: `crates/postretro/src/level_load.rs` (or equivalent — confirm during implementation) currently calls `load_textures` after reading the PRL. After Task 3 this becomes a section read; the texture-root path argument it threads can be removed from that call site once nothing else needs it.

## Open questions

- **PRL bloat / texture duplication across maps.** Each map's PRL now carries every texture it references. A 512² Rgba8 mip chain is ~1 MB per texture; a map referencing 80 textures adds ~80 MB. Reasonable for ship maps, painful for the iteration loop. Two mitigations possible later: shared texture pack files keyed by content hash, or per-texture sidecar `.pmip` files cached in `.prl-cache/`. Out of scope for v1; price before merging.
- **Normal-map filtering for fine geometric detail.** Mitchell + renormalise loses high-frequency variance (specular under-aliasing on glossy surfaces at distance). LEAN / Toksvig variance maps fix it but need a fourth channel or a separate roughness slot. Out of scope for v1; flag for a follow-up plan if artists complain.

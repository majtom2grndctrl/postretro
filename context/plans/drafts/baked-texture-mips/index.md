# Baked Texture Mips

## Goal

Move texture mip-chain generation out of the renderer and into `prl-build`. Mip pyramids are baked once at compile time, stored in the PRL, and uploaded directly at runtime. Zero CPU work at level load, higher-quality filtering than the current 2×2 box, and mips become asset data rather than a renderer side-effect.

## Scope

### In scope
- New PRL section carrying per-texture, per-mip pixel data for diffuse, specular, and normal slots.
- Compiler-side mip generation in `postretro-level-compiler`. Mitchell-Netravali separable filter (B = 1/3, C = 1/3) on the diffuse slot, with channel-correct handling: sRGB diffuse decoded to linear before filtering, encoded back to sRGB on output; specular (R8 linear) filtered linearly; normal-map slot filtered linearly then renormalised per texel.
- Runtime `upload_texture_data` accepts an externally provided mip chain and uploads each level directly.
- PRL format version bump (3 → 4); old files rejected by the loader (existing behaviour for version mismatch).
- Per-texture filter override via texture-name suffix or a small TOML manifest — see Open questions for the chosen mechanism.

### Out of scope
- Block-compressed formats (BCn / ETC2). Pixel data stays Rgba8 / R8 as today.
- Anisotropic mip generation (separate vertical / horizontal LOD chains).
- Streaming, partial residency, or runtime cache of baked mips.
- Hot-reload of textures while the engine is running.
- Compatibility shims for PRL v3 — pre-release, format bump is hard.
- Runtime mip generation as a fallback. The renderer no longer carries the downsample code path; if a PRL omits the section, load fails the same way any missing required section does.

## Acceptance criteria

- [ ] `prl-build` writes a `TextureMips` section for every texture name referenced by the map, covering the diffuse and (when source PNGs exist) specular and normal slots.
- [ ] Each mip chain in the section runs from level 0 (source resolution) down to a 1×1 final level, with `floor(log2(max(w, h))) + 1` levels total.
- [ ] PRL header `version` is 4. Loading a v3 file fails with `UnsupportedVersion`.
- [ ] `cargo run -p postretro -- content/dev/maps/campaign-test.prl` renders the campaign-test map with no visible mip-related change versus the current build, and `upload_texture_data` performs no CPU downsample (verifiable by removing `downsample_2x` from the renderer crate without breaking the build).
- [ ] A standalone unit test in the compiler confirms the Mitchell-Netravali output for a fixed input matches a golden reference within ±1 LSB per channel.
- [ ] Normal-map mips remain unit-length to within 1/127 after filtering (test on a synthetic input).
- [ ] sRGB diffuse mips show no darkening vs. the current renderer-side box filter on a 50/50 black/white checker (gamma-correct filtering test).
- [ ] Loading a PRL with no source PNGs on disk renders correctly — pixel data comes entirely from the PRL.

## Tasks

### Task 1: PRL TextureMips section in `postretro-level-format`
Add `SectionId::TextureMips = 32` and `texture_mips.rs` with `TextureMipsSection` (`to_bytes` / `from_bytes`, round-trip test). Layout described under *Wire format*. Bump `CURRENT_VERSION` to 4 and update the `UnsupportedVersion` test fixture. Update `context/lib/build_pipeline.md`'s section table at promotion time (not during the task).

### Task 2: Compiler-side mip baker in `postretro-level-compiler`
Add `texture_mips.rs` next to `texture_validation.rs`. Exposes one entry point that takes the deduplicated texture-name list (the same list `TextureNamesSection` is built from), the texture root path, and the per-texture filter overrides; returns a `TextureMipsSection`. Reads each `<name>.png`, `<name>_s.png`, `<name>_n.png` from the texture root using the same case-insensitive lookup the runtime uses today (factor `build_name_to_path_map` into a shared helper or duplicate it — call out under *Plumbing*). Generates the chain with the slot-correct filter (Mitchell-Netravali on sRGB diffuse with linear-space arithmetic; linear box-equivalent Mitchell on R8 specular; linear Mitchell + renormalise on Rgba8 normal). Missing source = no entry for that slot, same semantics as the runtime today. Wire into `pack.rs` after `TextureNames` is built. Hook into the build cache (`cache.rs`) keyed on PNG content hash + filter choice + `STAGE_VERSION`.

### Task 3: Runtime upload path in `crates/postretro/src/render/mod.rs`
Change `upload_texture_data`'s signature to take pre-baked levels — a slice of `(level_w, level_h, &[u8])` — instead of a single mip-0 buffer. Delete `downsample_2x` and `mip_level_count_for`; the caller now supplies the level count. Update the three real call paths (diffuse, specular, normal in `gpu_textures` build) plus the placeholders (`black_specular_texture`, `neutral_normal_texture`, `generate_placeholder` checkerboard, `Placeholder Texture Diffuse`) — placeholders carry a single 1×1 level. Replace `crates/postretro/src/texture.rs`'s `LoadedTexture` with a struct that owns the mip chain decoded from the PRL `TextureMips` section; `load_textures` becomes "index the section by name", not "scan a directory for PNGs."

### Task 4: Filter override surface
Implement the chosen override mechanism (see *Open questions*). Wire the override map through `prl-build` CLI / map worldspawn KVPs / sidecar TOML — once decided — into the Task 2 entry point.

## Sequencing

**Phase 1 (sequential):** Task 1 — defines the section type Task 2 produces and Task 3 consumes.
**Phase 2 (concurrent):** Task 2, Task 3 — both depend on Task 1 only; they meet at the section bytes.
**Phase 3 (sequential):** Task 4 — needs Task 2's baker entry point to plumb overrides into.

## Rough sketch

- `TextureMipsSection` mirrors `TextureNamesSection`'s ordering invariant: entry `i` describes the texture at `TextureNamesSection.names[i]`. Each entry carries three optional sub-chains (diffuse, specular, normal). A bitmask on the entry header indicates which slots are present.
- Filter implementation: precompute 1D Mitchell-Netravali weights once per (src_len, dst_len) pair; apply horizontally into a scratch buffer, then vertically. Edge condition: clamp-to-edge sampling, matching the current box filter's behaviour on odd dimensions.
- sRGB diffuse: decode each byte to linear via lookup table (256-entry `f32`), filter in `f32`, encode back via the standard sRGB curve (cheap polynomial or table). Premultiplied alpha is not used today; treat alpha as a linear channel.
- Normal-map renormalisation: after filtering, for each texel decode `n = sample.rgb * 2 - 1`, normalise, re-encode. If `||n|| < 1e-4` (degenerate from filtering), fall back to `(0, 0, 1)`.
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

Empty entry encoding: `slot_mask = 0`, no sub-blocks. Mirrors how `TextureNamesSection` handles a zero-length name (zero-length payload, no sentinel record).

## Plumbing

- The compiler's PNG lookup currently lives only in the runtime (`crates/postretro/src/texture.rs::build_name_to_path_map`). The baker needs the same lookup. Pick one: lift it into `postretro-level-compiler` and have the runtime use the section instead (preferred, since the runtime no longer scans PNGs once Task 3 lands), or duplicate it temporarily. The plan assumes the lift.
- `pack.rs` must receive the texture-root path. It already receives the parsed map; add a path argument alongside the existing texture-validation pass that reads PNG dimensions. The CLI in `main.rs` already knows the texture root.
- The build cache (`cache.rs`) needs a new stage entry `"texture_mips"`. Inputs hash: each referenced PNG's content blake3 + the filter override table.
- Runtime loader: `crates/postretro/src/level_load.rs` (or equivalent — confirm during implementation) currently calls `load_textures` after reading the PRL. After Task 3 this becomes a section read; the texture-root path argument it threads can be removed from that call site once nothing else needs it.

## Open questions

- **Filter override surface.** Three candidates: (a) suffix on the texture name (e.g. `…_pixelart` → nearest, `…_sharp` → Lanczos); (b) sidecar TOML at `content/<mod>/textures/filters.toml` keyed by collection/name glob; (c) a single global filter set via `prl-build --mip-filter` CLI flag with no per-texture override. (b) is most ergonomic for pixel-art mods; (c) is the smallest landing surface. Decide before Task 4.
- **Normal-map filtering for fine geometric detail.** Mitchell + renormalise is the standard cheap option but loses high-frequency variance (causes specular under-aliasing on glossy surfaces at distance). LEAN / Toksvig variance maps fix it but require a fourth channel or a separate roughness slot. Out of scope for v1; flag for a follow-up plan if the artists complain.
- **PRL bloat / texture duplication across maps.** Each map's PRL now carries every texture it references. A 512² Rgba8 mip chain is ~1 MB per texture; a map referencing 80 textures adds ~80 MB. Reasonable for ship maps, painful for the iteration loop. Two mitigations possible later: shared texture pack files keyed by content hash, or per-texture sidecar `.pmip` files cached in `.prl-cache/`. Out of scope for v1, but worth pricing before merging.
- **Renderer-supplied placeholders (checkerboard, neutral normal, black specular).** These are generated by the renderer today, not loaded from PRL. Keep that path — the placeholders are 1×1 or 64×64 and the cost is negligible — or route them through the baker for uniformity? Recommend keep, but call it out so Task 3 doesn't accidentally remove the renderer-side generators.
- **The original task brief described the runtime filter as Mitchell-Netravali; it is actually a 2×2 box filter (`downsample_2x` in `render/mod.rs`).** This plan treats the Mitchell-Netravali quality upgrade as part of the move. If we want pure "lift and shift" first, split Task 2 into a box-filter port and a separate filter-upgrade plan.

# Baked Texture Mips

## Goal

Move texture mip-chain generation out of the renderer and into `prl-build`. Mip pyramids bake once at compile time in linear color space, land in per-texture sidecar `.prm` files under `.prl-cache/tex/`, and upload directly at runtime. Zero CPU filtering at level load, gamma-correct mips that don't darken midtones, mips become content-addressed asset data shared across every map that references the texture.

## Scope

### In scope

- New `.prm` (PostRetro Material) sidecar file format. One file per texture name, content-addressed by `blake3(PNG content)`, stored under `content/<mod>/.prl-cache/tex/<hex>.prm`. Bundles diffuse + specular + normal slots; each slot optional.
- Compiler-side mip generation in `postretro-level-compiler`. Mitchell-Netravali separable filter (B = C = 1/3) on every slot. Filtering happens in linear space: sRGB diffuse decoded to linear via a 256-entry LUT before filtering and re-encoded on output; R8 specular treated as linear; Rgba8 normal filtered linearly then renormalised per output texel.
- New small PRL section mapping each `TextureNamesSection` entry to its `.prm` cache key (32-byte blake3). Lets the runtime locate sidecars without re-reading PNGs.
- Runtime loads `.prm` at level load, uploads each level directly. Renderer no longer downsamples on CPU.
- Sampler `lod_max_clamp` tightened to `mip_count - 1` per texture (was global 24.0).
- `.prl-cache/` is per-mod, gitignored, regenerated on demand. The compiler writes to it; the runtime reads from it.

### Out of scope (non-goals)

- BC5 or any block compression. The `.prm` `format_tag` byte is extensible from day one (value 3 reserved for BC5), but the encoder lands in a follow-up plan in the same milestone.
- Shipping pack-file consolidation of `.prl-cache/tex/` into a single archive.
- Hot-reload of textures while the engine runs.
- App-side plugins for TrenchBroom / Aseprite / Photoshop. Authoring stays as plain PNGs.
- Streaming, partial residency, runtime cache of decoded mips.
- Per-texture filter override surface (suffix, sidecar TOML, CLI flag).
- Anisotropic mip generation (RIP-maps, separate H/V chains).
- Manual `prl-bake-textures` step. Conversion is implicit during `prl-build`.
- PRL v3 backwards compatibility. Pre-release; version bump is hard.
- Shader-side anisotropic filtering for grazing-angle surfaces. Separate plan; depends on this one.

## Acceptance criteria

- [ ] `prl-build` writes one `.prm` file per referenced texture into `content/<mod>/.prl-cache/tex/`, named `<blake3-hex>.prm`. Repeated builds with unchanged PNGs are no-ops (cache hit on filename).
- [ ] PRL carries a `TextureCacheKeys` section: one 32-byte blake3 per entry in `TextureNamesSection`, same ordering. PRL header `version` is 4. Loading a v3 file fails with `UnsupportedVersion`.
- [ ] Each present mip chain runs from level 0 (source resolution) down to a 1×1 final level, with `floor(log2(max(w, h))) + 1` levels total.
- [ ] `cargo run -p postretro -- content/dev/maps/campaign-test.prl` renders the campaign-test map with no rendering errors. Midtones may appear lighter than the current build due to gamma-correct filtering — intended.
- [ ] `upload_texture_data` performs no CPU downsample. Verifiable by deleting the downsample code path from the renderer crate without breaking the build.
- [ ] Unit test: gamma-correct Mitchell-Netravali output for a fixed sRGB input matches a golden reference (computed in linear space) within ±1 LSB per channel.
- [ ] Unit test: a 50/50 black/white sRGB checker filters to sRGB midgrey (~0.73 in byte space) at mid mip levels, **not** the linear midpoint (~0.5). Gamma-correctness regression guard.
- [ ] Unit test: normal-map mips remain unit-length within 1/127 after filtering (synthetic input).
- [ ] Loading a PRL whose source PNGs are absent from disk renders correctly — pixel data comes entirely from `.prm` sidecars. The runtime never opens a PNG.
- [ ] Sampler `lod_max_clamp` for each texture equals `mip_count - 1`, not 24.0.

## Tasks

Sized to land as separate commits.

### Task 1: `TextureCacheKeys` section in `postretro-level-format`

Add `SectionId::TextureCacheKeys = 32`, a `TextureCacheKeysSection` type with `to_bytes` / `from_bytes` and a round-trip test. Layout: `u32 count` + `[u8; 32] * count`. `count` equals `TextureNamesSection.names.len()`; entry `i` is the blake3 of the PNG content for texture `i`. Bump `CURRENT_VERSION` to 4 and update the `UnsupportedVersion` test fixture.

Update `context/lib/build_pipeline.md`'s section table at plan promotion time, not during the task.

### Task 2: `.prm` writer in `postretro-level-compiler`

Add `texture_mips.rs` next to `texture_validation.rs` plus a small `prm.rs` (or fold both into one module) that owns the `.prm` wire format. Entry point takes the deduplicated texture-name list, the texture root path, and the cache root (`<mod>/.prl-cache/tex/`). For each name:

1. Resolve `<name>.png`, `<name>_s.png`, `<name>_n.png` via case-insensitive lookup (factor `build_name_to_path_map` out of the runtime into a shared helper — see *Plumbing*).
2. Compute `key = blake3(diffuse_png_bytes)` if diffuse exists; otherwise hash the first present slot's PNG. Key identifies the bundle.
3. If `<cache>/<hex>.prm` already exists and parses, skip rebake.
4. Otherwise build mip chains per slot in linear `f32`, encode per *Wire format* below, write atomically (tempfile + rename).
5. Emit `(name → key)` so Task 4 can populate `TextureCacheKeysSection`.

Gamma handling per slot:

- **Diffuse (`Rgba8UnormSrgb`, tag 0):** decode each byte to linear `f32` via 256-entry sRGB→linear LUT, run separable Mitchell-Netravali in linear `f32`, encode back to sRGB on output. Alpha treated as linear throughout (premultiplied alpha not used).
- **Specular (`R8Unorm`, tag 2):** filter directly in linear `f32`. Already linear on disk.
- **Normal (`Rgba8Unorm`, tag 1):** filter linearly in `f32`, then per output texel decode `n = sample.rgb * 2 - 1`, normalise, re-encode. Fall back to `(0, 0, 1)` if `||n|| < 1e-4` (degenerate from filtering).

Edge condition: clamp-to-edge sampling. Precompute 1D Mitchell weights once per `(src_len, dst_len)` pair; horizontal pass into a scratch `f32` buffer, then vertical.

Wire into `pack.rs` after `TextureNames` is built. The compiler's build cache (`cache.rs`) participates via `STAGE_VERSION` (`texture_mips::STAGE_VERSION = 1`); the on-disk `.prm` files themselves act as the cache, keyed by content hash. Same content hash ⇒ same filename ⇒ cache hit without any inputs-hash bookkeeping.

### Task 3: `.prm` reader and runtime upload path

Touches `crates/postretro/src/texture.rs` and `crates/postretro/src/render/mod.rs`.

- Parse `.prm` per *Wire format*. Reader returns a struct owning per-slot `(format_tag, width, height, level_count, payload)`.
- Change `upload_texture_data`'s signature: take a slice of `(level_w, level_h, &[u8])` instead of a single mip-0 buffer. Delete `downsample_2x`, the `mitchell_netravali` weight code, and `mip_level_count_for`; the caller supplies levels.
- `LoadedTexture` becomes a thin wrapper over the parsed `.prm` slot data plus GPU handles.
- `load_textures` rewrites: for each entry in `TextureNamesSection`, look up the blake3 from `TextureCacheKeysSection`, open `<mod>/.prl-cache/tex/<hex>.prm`, decode, upload each level per slot. The texture-root PNG scan disappears from the runtime.
- Update placeholders (`black_specular_texture`, `neutral_normal_texture`, `generate_placeholder` checkerboard, `Placeholder Texture Diffuse`) to construct a single-level synthetic mip chain through the new path.
- Set sampler `lod_max_clamp` to `mip_count - 1` per texture.

Task 3 can be unit-tested against a hand-crafted `.prm` fixture + v4 PRL fixture, but the campaign-test acceptance check requires Task 2 done.

### Task 4: Plumbing and cleanup

- Lift PNG name-lookup helper (`build_name_to_path_map`) from `crates/postretro/src/texture.rs` into `postretro-level-compiler`. The runtime no longer scans the textures directory.
- Thread the texture root and cache root into `pack.rs`. The CLI in `main.rs` already knows both.
- Wire Task 2's `(name → key)` output into Task 1's section in `pack.rs`.
- Add `content/*/.prl-cache/` to `.gitignore` (one entry covers every mod).
- Remove any now-dead PNG-scanning and downsample code paths from the renderer crate.

## Sequencing

**Phase 1 (sequential):** Task 1. Defines the section Task 4 populates and Task 3 consumes.

**Phase 2 (concurrent):** Task 2 and Task 3. Both depend on Task 1 only; they meet at the `.prm` wire format spec'd in this document.

**Phase 3 (sequential):** Task 4. Wires the producer to the consumer, drops dead code.

## Rough sketch

- Cache key choice: hashing PNG content (not `(PNG content, STAGE_VERSION)`) keeps the filename stable across stage-version bumps but lets stale `.prm` files survive a filter change. `STAGE_VERSION` lives inside the `.prm` header (see *Wire format*) so the reader can detect a mismatch and the writer can overwrite. On `STAGE_VERSION` bump, builds rewrite every `.prm`; mismatched files are overwritten in place.
- `.prm` files are content-addressed by the **diffuse** PNG when present (or the first available slot otherwise). A texture name with the same diffuse PNG but a different normal map produces a different blake3 only if we hash all source PNGs together — pick the simpler rule: hash just the bytes used to derive the cache filename, then store the full triple inside. If the normal map changes while diffuse doesn't, the build cache (`cache.rs` `STAGE_VERSION` + per-name inputs hash) catches it and the writer re-emits the `.prm`. The on-disk filename does not need to be unique per (diffuse, specular, normal) tuple — it just needs to be unique per **texture name** for any given build state. Document this clearly in the writer.
- Per-mod cache layout (`content/<mod>/.prl-cache/tex/`) avoids cross-mod collisions when two mods ship different PNGs under the same texture name.
- sRGB encode: IEC 61966-2-1 piecewise curve (linear `< 0.0031308` segment, gamma branch above). Polynomial approximation acceptable if the regression test passes.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| PRL section enum | `SectionId::TextureCacheKeys` | `u32 = 32` | n/a | n/a | n/a |
| PRL section struct | `TextureCacheKeysSection` | `u32 count` + `[u8; 32] * count` | n/a | n/a | n/a |
| Sidecar file | `PrmFile` (compiler + runtime) | `.prm` body (see below) | n/a | n/a | n/a |
| Slot bitmask | `PrmSlot::{Diffuse, Specular, Normal}` | `u8` bits 0/1/2 | n/a | n/a | n/a |
| Format tag | `PrmFormat::{Rgba8UnormSrgb, Rgba8Unorm, R8Unorm}` | `u8` 0/1/2 (3 reserved BC5) | n/a | n/a | n/a |

## Wire format

Little-endian throughout. All integers unsigned.

### `.prm` file body

```
-- header (12 bytes)
[u8; 4]  magic                = b"PRM\x01"     -- ASCII "PRM" + version byte
u8       version              = 1              -- bumped on any incompatible layout change
u8       stage_version                         -- mirrors texture_mips::STAGE_VERSION at write time
u8       slot_mask                             -- bit 0 diffuse, bit 1 specular, bit 2 normal
u8       reserved             = 0
u32      total_body_bytes                     -- bytes after this header, for quick validation

-- per present slot, in fixed order diffuse → specular → normal:
u8       format_tag                            -- 0 Rgba8UnormSrgb, 1 Rgba8Unorm, 2 R8Unorm
u8       reserved             = 0
u16      width                                 -- mip 0 width, ≥ 1
u16      height                                -- mip 0 height, ≥ 1
u8       level_count                           -- floor(log2(max(w, h))) + 1
u8       reserved             = 0
u32      payload_bytes                         -- total bytes for all levels concatenated
[u8; payload_bytes]                            -- levels packed back-to-back, level 0 first
                                               -- each level tight bytes_per_pixel * w_n * h_n
                                               -- (no row alignment padding)
```

Reader validation:

- Reject if `magic[0..3] != "PRM"` or `magic[3] != version`.
- Reject if `stage_version` doesn't match the engine's compiled-in `STAGE_VERSION`. Writer overwrites on mismatch.
- Reject if `slot_mask` has bits above bit 2 set.
- For each present slot: `level_count == floor(log2(max(width, height))) + 1`, `payload_bytes` equals the sum of `bytes_per_pixel(format_tag) * w_n * h_n` across levels with `w_n = max(1, width >> n)`, `h_n = max(1, height >> n)`.
- `bytes_per_pixel`: tag 0 → 4, tag 1 → 4, tag 2 → 1.

Empty file (no slots) encoding: `slot_mask = 0`, `total_body_bytes = 0`, no per-slot blocks. Permitted but never written by Task 2 (a texture with no source PNGs in any slot doesn't produce a `.prm` — the loader treats it the same as a missing texture today).

### `TextureCacheKeysSection` body

```
u32      count                                -- equals TextureNamesSection.names.len()
[u8; 32] keys[count]                          -- blake3 of the PNG bundle for each name
```

Ordering invariant: `keys[i]` corresponds to `names[i]`. A texture with no PNG at all (placeholder-only) writes a zero key (32 zero bytes); the runtime treats that as "no sidecar, use placeholder."

## Plumbing

- PNG name-lookup helper currently in `crates/postretro/src/texture.rs::build_name_to_path_map` moves to `postretro-level-compiler` (shared module). Runtime drops its copy.
- `pack.rs` gains a `texture_root` and `cache_root` argument. The CLI in `main.rs` already knows both.
- Compiler's build cache (`cache.rs`) gains a stage entry `"texture_mips"` whose inputs hash is `blake3(all referenced PNG content blake3s, sorted) || STAGE_VERSION`. The cache entry's payload is the `(name → key)` map. The `.prm` files themselves are not stored in `cache.rs` — they live as content-addressed files under `.prl-cache/tex/`.
- Runtime loader (`crates/postretro/src/level_load.rs` or equivalent) replaces the post-PRL `load_textures(texture_root, …)` call with a `load_textures(prl, mod_cache_root)` call that reads `.prm` files. The texture-root path argument can be removed from that call site.
- `.gitignore`: add `content/*/.prl-cache/`.

## Open questions

- **Cache-key scope.** Hashing only the diffuse PNG keeps filenames stable and short but means a normal-map-only edit produces the same filename — the writer must always overwrite when `STAGE_VERSION` or any input changes. Alternative: hash `(diffuse, specular, normal)` bytes together so filename uniquely identifies the bundle. Trades cache-locality for stricter content-addressing. Recommend keeping the simpler diffuse-only rule and relying on `cache.rs` for invalidation; revisit if a workflow proves it wrong.
- **Mod cache scoping under overlay loads.** When a map references a texture that a mod overrides, the `.prm` lives under the overriding mod's cache. Runtime needs to know which mod a given texture name resolves to at PRL load time. Current mod-overlay logic already does this for PNGs; confirm the same resolver returns the right `<mod>` directory when the lookup is for the cache path.
- **Cache directory deletion as a recovery step.** Users hitting corrupted `.prm` files should be told to `rm -rf content/<mod>/.prl-cache/` and rebuild. Document at promotion time in `build_pipeline.md`.
- **Normal-map filtering for fine geometric detail.** Mitchell + renormalise loses high-frequency variance (specular under-aliasing on glossy surfaces at distance). LEAN / Toksvig variance maps fix it but need a fourth channel or a separate roughness slot. Flag for a follow-up plan if artists complain.

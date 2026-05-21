# Baked Texture Mips

## Goal

Move texture mip-chain generation out of the renderer and into `prl-build`. Mip pyramids bake once at compile time in linear color space, land in per-texture sidecar `.prm` files under `content/<mod>/.prl-cache/tex/`, and upload directly at runtime. Zero CPU filtering at level load, gamma-correct mips that don't darken midtones, mips become content-addressed asset data shared across every map that references the texture.

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
- Emissive-mask (`_e.png`) baking. `slot_mask` bit 3 is reserved at the byte level; the writer never sets it and the reader rejects it (`ReservedSlotBitsSet`) until a follow-up plan implements emissive.

## Acceptance criteria

- [ ] `prl-build` writes one `.prm` file per referenced texture into `content/<mod>/.prl-cache/tex/`, named `<blake3-hex>.prm`. Repeated builds with unchanged source PNGs across all slots are no-ops (writer hits the existing `.prm` and reads its `bundle_hash`; matching bundle hash skips re-bake).
- [ ] PRL carries a `TextureCacheKeys` section: one 32-byte blake3 per entry in `TextureNamesSection`, same ordering. PRL header `version` is 4. Loading a v3 file fails with `UnsupportedVersion`.
- [ ] Each present mip chain runs from level 0 (source resolution) down to a 1×1 final level, with `floor(log2(max(w, h))) + 1` levels total.
- [ ] `cargo run -p postretro -- content/dev/maps/campaign-test.prl` renders the campaign-test map with no rendering errors. Midtones may appear lighter than the current build due to gamma-correct filtering — intended.
- [ ] `upload_texture_data` performs no CPU downsample. Verifiable by deleting the downsample code path from the renderer crate without breaking the build.
- [ ] Unit test: gamma-correct Mitchell-Netravali output for a fixed sRGB input matches a golden reference (computed in linear space) within ±1 LSB per channel.
- [ ] Unit test: a 50/50 black/white sRGB checker filters to sRGB-encoded ~187/255 (linear 0.5) at mid mip levels, **not** the naive byte midpoint ~128/255. Gamma-correctness regression guard; tolerance ±1 byte per channel.
- [ ] Unit test: normal-map mips remain unit-length within 1/127 after filtering (synthetic input).
- [ ] Loading a PRL whose source PNGs are absent from disk renders correctly — pixel data comes entirely from `.prm` sidecars. The runtime never opens a PNG.
- [ ] Sampler `lod_max_clamp` for each texture equals `mip_count - 1`, not 24.0.
- [ ] A corrupt `.prm` file (truncated, bad magic, wrong stage_version) yields a `warn!` log entry and falls back to placeholders; the affected level still loads successfully.

## Tasks

Sized to land as separate commits.

### Task 1: `TextureCacheKeys` section + `.prm` wire format in `postretro-level-format`

Add `SectionId::TextureCacheKeys = 32`, a `TextureCacheKeysSection` type with `to_bytes` / `from_bytes` and a round-trip test. Layout: `u32 count` + `[u8; 32] * count`. `count` equals `TextureNamesSection.names.len()`; entry `i` is the blake3 of the PNG content for texture `i`. Bump `CURRENT_VERSION` to 4. Existing `CURRENT_VERSION` assertions in level-format tests derive from the constant and bump transparently; the only hand-written test is a new v3-reject test asserting `UnsupportedVersion { version: 3 }`. Extend the hand-written `SectionId::from_u32` match arm to cover the new variant.

Add a `prm` module to `postretro-level-format` owning the `.prm` wire format: `PrmFile`, `PrmSlot`, `PrmFormat`, `PrmReadError`, and a `STAGE_VERSION` constant. Both the compiler-side writer (Task 2) and runtime-side reader (Task 3) import these types from level-format — the parser is not reinvented in the runtime crate. `PrmFile::to_bytes` / `from_bytes` follow the hand-rolled `Vec<u8>` / byte-slice convention used by other level-format sections (see `texture_names.rs`). No `postcard` dependency added.

Update `context/lib/build_pipeline.md`'s section table at plan promotion time, not during the task.

### Task 2: `.prm` writer in `postretro-level-compiler`

Add `texture_mips.rs` next to `texture_validation.rs`. The wire-format types live in `postretro-level-format::prm` (Task 1); the writer imports them. Entry point takes the deduplicated texture-name list, the texture root path, and the cache root (`<mod>/.prl-cache/tex/`). For each name:

1. Resolve `<name>.png`, `<name>_s.png`, `<name>_n.png` via case-insensitive lookup (factor `build_name_to_path_map` out of the runtime into a shared helper — see *Plumbing*).
2. Compute `key = blake3(diffuse_png_bytes)` if diffuse exists. Otherwise `key = blake3([bit_index_byte] || first_present_png_bytes)`, where `bit_index_byte` is `0x01` for specular-only or `0x02` for normal-only. The tag byte prevents collisions between specular-only and normal-only textures that share PNG bytes. Slot probe order: diffuse → specular → normal. Hash inputs are raw on-disk PNG file bytes — no decode/re-encode normalization. The blake3 reflects the file, not the decoded pixels.
3. If `<cache>/<hex>.prm` exists, parses successfully, and its `bundle_hash` field matches the recomputed bundle hash for the current source PNGs, skip rebake. Otherwise overwrite.
4. Otherwise build mip chains per slot in linear `f32`, encode per *Wire format* below, write atomically — tempfile in the same `.prl-cache/tex/` directory as the final file, then `std::fs::rename` for cross-platform atomic replacement.
5. Return `(name → key)` from the baker — do NOT touch `pack.rs` directly. Task 4 wires this into `TextureCacheKeysSection`.

Gamma handling per slot:

- **Diffuse (`Rgba8UnormSrgb`, tag 0):** decode each byte to linear `f32` via 256-entry sRGB→linear LUT, run separable Mitchell-Netravali in linear `f32`, encode back to sRGB on output. RGB: filter in linear f32, clamp to [0, 1], sRGB-encode to bytes. Alpha: filter in linear f32, clamp to [0, 1], then `((x * 255.0).round() as u8)` — alpha is never sRGB-encoded. Premultiplied alpha not used. Specular clamps to [0, 1] before R8 quantization the same way.
- **Specular (`R8Unorm`, tag 2):** filter directly in linear `f32`. Already linear on disk.
- **Normal (`Rgba8Unorm`, tag 1):** filter each RGB component linearly in `f32` (no input clamp). Per output texel: `n = sample.rgb * 2 - 1`. If `||n|| < 1e-4`, substitute `(0, 0, 1)`; otherwise normalize to unit length. Re-encode as `((n * 0.5 + 0.5) * 255.0).round() as u8` per component. Alpha (if present) is filtered linearly and quantized like specular.

Kernel evaluated in destination texel space with scale = 2.0 (each mip is a 2× downsample of the prior). Per-destination-texel weights renormalize to sum exactly 1.0. Clamp-to-edge: out-of-bounds taps replicate the nearest source sample. Precompute 1D Mitchell weights once per `(src_len, dst_len)` pair; horizontal pass into a scratch `f32` buffer, then vertical.

The baker is invoked from `main.rs` after `texture_validation::validate_sibling_color_spaces` (main.rs:163) and before `pack_and_write_portals` (main.rs:515), at the point the deduplicated texture-name list is materialized for the pack call. The returned `(name → key)` map is threaded to the pack call.

### Task 3: `.prm` reader and runtime upload path

Renames `crates/postretro/src/texture.rs` to `ui_texture.rs` (UI-only after the refactor), adds `crates/postretro/src/render/loaded_texture.rs` (new, hosts `LoadedTexture` + world-material placeholders + the `.prm` upload path so wgpu handles stay inside the renderer module), and touches `crates/postretro/src/render/mod.rs`.

Type split:

| Type | Filter | Mips | Source | Use |
|---|---|---|---|---|
| `UiTexture` (new, `crates/postretro/src/ui_texture.rs`) | Nearest min/mag | none (1 level) | PNG direct | splash, HUD, any 2D blit |
| `LoadedTexture` (refactored, `crates/postretro/src/render/loaded_texture.rs`) | Nearest min/mag, Linear mipmap | full chain | `.prm` sidecar | world materials |

`UiTexture` carries `{ data, width, height }`. `splash::load_splash` (`render/splash.rs:22`), `splash::upload_splash_texture` (`render/splash.rs:51`), and `install_splash_from_loaded` all return / accept `UiTexture`. The two `install_splash_from_loaded` call sites in `crates/postretro/src/main.rs` (lines 1220 and 1289) thread `UiTexture` through. `LoadedTexture` becomes a thin wrapper over the parsed `.prm` slot data + GPU handles (per the bullet below). The magenta-checker placeholder remains in the `LoadedTexture` family (it's a world-material fallback, not UI) with `mip_count = 1`.

- Import the reader from `postretro-level-format::prm`. Decoded result is per-slot `(format_tag, width, height, level_count, payload)`.
- Change `upload_texture_data` to accept `levels: &[(u32, u32, &[u8])]` in place of the current `(width, height, data)` parameters; the existing `format: wgpu::TextureFormat` parameter stays. Caller supplies one tuple per mip level (level width, level height, byte payload) plus the slot's format tag mapped to `wgpu::TextureFormat`. Delete `downsample_2x`, the `mitchell_netravali` weight code, and `mip_level_count_for`; the caller supplies levels.
- wgpu's `Queue::write_texture` accepts tightly-packed source rows; the `.prm` payload layout (no row alignment padding) uploads directly without staging.
- `LoadedTexture` becomes a thin wrapper over the parsed `.prm` slot data plus GPU handles, living in `crates/postretro/src/render/loaded_texture.rs`. The move puts wgpu types fully inside the renderer module, restoring `context/lib/index.md` §2's "Renderer owns GPU" invariant which the prior CPU-side `LoadedTexture` straddled. The `is_placeholder` field is preserved (load-bearing for sibling-probe skipping in the pre-rename `texture.rs:184,224`; the probe logic follows `LoadedTexture` into the new file).
- `load_textures` rewrites: for each entry in `TextureNamesSection`, look up the blake3 from `TextureCacheKeysSection`, open `<mod>/.prl-cache/tex/<hex>.prm`, decode, upload each level per slot. The texture-root PNG scan disappears from the runtime. (`texture_names` is `Vec<String>`; the runtime's current `Vec<Option<String>>` wrap at `startup/worker.rs:80-84` drops away.)
- Update placeholders (`black_specular_texture`, `neutral_normal_texture`, `generate_placeholder` checkerboard, `Placeholder Texture Diffuse`) to use `mip_count = 1` (`lod_max_clamp = 0.0`). The 64×64 checkerboard will look blockier at distance than the current mipped placeholder — intended, makes missing-texture cases more obvious in modder workflows.
- Replace the single global `base_sampler` with a `HashMap<u32, wgpu::Sampler>` (`mip_count_samplers`) on the renderer. Each material bind group selects the sampler whose key matches its `LoadedTexture.mip_count`. Sampler descriptor is unchanged except `lod_max_clamp = (mip_count - 1) as f32`. Existing call sites at `render/mod.rs:1289`, `1337`, `2213`, `2263` switch from `&base_sampler` to a lookup. Population is eager: after `load_textures` returns, the renderer collects the distinct `mip_count` set across the loaded textures and materializes one sampler per value before any material bind group is built. Lifetime is engine-lifetime — the pool persists across level reloads, accumulating only new `mip_count` entries; the set is bounded by the largest texture ever observed.
- Placeholders use the same sampler descriptor as world materials (Nearest min/mag, Linear mipmap_filter) and pick up the `mip_count_samplers[&1]` entry. No separate placeholder-only sampler.
- Delete the PNG-fixture-driven tests from the pre-rename `crates/postretro/src/texture.rs`: the `build_name_map_*` tests (which follow `build_name_to_path_map` into the compiler), `load_textures_loads_matching_pngs`, `load_textures_case_insensitive_match`, `load_textures_missing_produces_checkerboard`, `load_textures_none_entry_produces_checkerboard`, the `None` slot in `load_textures_preserves_index_order`, and the seven sibling-probe tests at lines 528–718. Roughly 22 of 27 tests in the file go. The five `checkerboard_*` tests at lines 290–328 move with the placeholder logic to `crates/postretro/src/render/loaded_texture.rs`. Replacement: `.prm`-fixture tests built via a `#[cfg(test)]` helper module in `postretro-level-format::prm` that emits in-memory `.prm` byte blobs.

Task 3 can be unit-tested against a hand-crafted `.prm` fixture + v4 PRL fixture, but the campaign-test acceptance check requires Task 2 done.

### Task 4: Plumbing and cleanup

- Lift PNG name-lookup helper (`build_name_to_path_map`) from the renamed `crates/postretro/src/ui_texture.rs` into `postretro-level-compiler`. The runtime no longer scans the textures directory.
- Thread the texture root and cache root into `pack.rs`. The CLI in `main.rs` already knows both.
- Wire Task 2's `(name → key)` output into Task 1's section in `pack.rs`.
- Remove any now-dead PNG-scanning and downsample code paths from the renderer crate.

## Sequencing

**Phase 1 (sequential):** Task 1. Defines the section Task 4 populates and Task 3 consumes.

**Phase 2 (concurrent):** Task 2 and Task 3. Both depend on Task 1 only; they meet at the `.prm` wire format spec'd in this document.

**Phase 3 (sequential):** Task 4. Wires the producer to the consumer, drops dead code.

## Rough sketch

- Cache key choice: hashing PNG content (not `(PNG content, STAGE_VERSION)`) keeps the filename stable across stage-version bumps but lets stale `.prm` files survive a filter change. `STAGE_VERSION` lives inside the `.prm` header (see *Wire format*) so the reader can detect a mismatch and the writer can overwrite. On `STAGE_VERSION` bump, builds rewrite every `.prm`; mismatched files are overwritten in place.
- `.prm` files are content-addressed by the **diffuse** PNG when present (or the first available slot otherwise). A texture name with the same diffuse PNG but a different normal map produces the same filename — that is intended. If any source PNG changes, the writer detects it on the next build by re-hashing source PNGs and overwriting the existing `.prm` when the recomputed `bundle_hash` diverges from what the `.prm` header records. The on-disk filename does not need to be unique per (diffuse, specular, normal) tuple — it just needs to be unique per **texture name** for any given build state. Document this clearly in the writer.
- Per-mod cache layout (`content/<mod>/.prl-cache/tex/`) avoids cross-mod collisions when two mods ship different PNGs under the same texture name.
- sRGB encode: the golden reference uses the exact IEC 61966-2-1 piecewise curve (linear `< 0.0031308` segment, gamma branch above). The encoder may use a polynomial approximation provided every output stays within ±1 LSB of the exact formula (validated by the gamma-correctness regression test).

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| PRL section enum | `SectionId::TextureCacheKeys` | `u32 = 32` | n/a | n/a | n/a |
| PRL section struct | `TextureCacheKeysSection` | `u32 count` + `[u8; 32] * count` | n/a | n/a | n/a |
| Sidecar file | `PrmFile` (compiler + runtime) | `.prm` body (see below) | n/a | n/a | n/a |
| Slot bitmask | `PrmSlots::{Diffuse, Specular, Normal}` | `u8` bits 0/1/2 (bit 3 reserved at byte level for future emissive) | n/a | n/a | n/a |
| Format tag | `PrmFormat::{Rgba8UnormSrgb, Rgba8Unorm, R8Unorm}` | `u8` 0/1/2 (3 reserved BC5) | n/a | n/a | n/a |
| UI texture | `UiTexture` (`crates/postretro/src/ui_texture.rs`) | n/a (runtime only) | n/a | n/a | n/a |

## Wire format

Little-endian throughout. All integers unsigned. Byte arrays (`[u8; N]`) are stored verbatim; endianness applies only to integer fields.

### `.prm` file body

```
-- header (43 bytes)
[u8; 4]  magic                = b"PRM\x01"     -- ASCII "PRM" + version byte; version lives only here
u8       stage_version                         -- equals postretro-level-format::prm::STAGE_VERSION (u8) at write time
u8       slot_mask                             -- bit 0 diffuse, bit 1 specular, bit 2 normal, bit 3 emissive (reserved)
u8       reserved             = 0
[u8; 32] bundle_hash                           -- blake3 over `slot_mask` byte followed by `(bit_index_byte, source_png_file_bytes)`
                                               -- for every present slot in order diffuse (bit 0) → specular (bit 1) → normal (bit 2).
                                               -- `bit_index_byte` is one of `0x00`, `0x01`, `0x02`. Including `slot_mask`
                                               -- makes slot deletion an unambiguous fingerprint change. Writer compares this
                                               -- against the recomputed bundle hash to detect input changes when the
                                               -- diffuse-only filename hash hasn't moved.
u32      total_body_bytes                     -- sum across all present slots of (12-byte per-slot header
                                               -- + that slot's payload_bytes). Excludes this file header.

-- per present slot, in fixed order diffuse → specular → normal.
-- Per-slot block header is 12 bytes (1 + 1 + 2 + 2 + 1 + 1 + 4), followed by `payload_bytes` of level data.
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

- Reject if `magic[0..3] != b"PRM"` with `PrmReadError::BadMagic { found: magic }`. The version byte `magic[3]` must be exactly `0x01`; any other value yields `PrmReadError::UnsupportedVersion { version: magic[3] }`.
- Reject if `stage_version != postretro-level-format::prm::STAGE_VERSION` with `PrmReadError::StageVersionMismatch { expected, found }`. `STAGE_VERSION` is a `u8`; a future bump beyond 255 requires a `.prm` magic-byte version bump. Writer overwrites on mismatch.
- Reject if `slot_mask` has bits 3–7 set, or if `slot_mask == 0`, with `PrmReadError::ReservedSlotBitsSet { mask }`. Bit 3 (emissive) is reserved at the byte level only — the writer never sets it and the reader rejects it until a follow-up plan implements emissive.
- For each present slot: `level_count == floor(log2(max(width, height))) + 1`, `payload_bytes` equals the sum of `bytes_per_pixel(format_tag) * w_n * h_n` across levels with `w_n = max(1, width >> n)`, `h_n = max(1, height >> n)`.
- Reject if `total_body_bytes` != Σ `(12 + payload_bytes_i)` across present slots, or if the remaining file size after the 43-byte header doesn't equal `total_body_bytes`, with `PrmReadError::TotalBodyBytesMismatch { expected, found }`.
- `bytes_per_pixel`: tag 0 → 4, tag 1 → 4, tag 2 → 1.

`PrmReadError` variants: `BadMagic { found: [u8; 4] }`, `UnsupportedVersion { version: u8 }`, `StageVersionMismatch { expected: u8, found: u8 }`, `ReservedSlotBitsSet { mask: u8 }`, `LevelCountMismatch { slot: u8, expected: u8, found: u8 }`, `PayloadBytesMismatch { slot: u8, expected: u32, found: u32 }`, `TotalBodyBytesMismatch { expected: u32, found: u32 }`, `Truncated`, `Io(std::io::Error)`.

On any reject (bad magic, `stage_version` mismatch, `slot_mask` invalid, level/payload arithmetic mismatch), the runtime logs a `warn!` naming the texture and the failure reason, substitutes per-slot placeholders (`black_specular_texture`, `neutral_normal_texture`, magenta checker for diffuse), and continues level load. Level load never fails on a `.prm` parse error.

File-level errors (bad magic, version mismatch, stage_version mismatch, total_body_bytes mismatch, truncated header) fail the entire `.prm`; the runtime substitutes per-slot placeholders for all three slots and continues. Per-slot errors (level_count or payload_bytes arithmetic mismatch on one slot) fall back to that slot's placeholder; other slots that parsed cleanly are used.

### `TextureCacheKeysSection` body

```
u32      count                                -- equals TextureNamesSection.names.len()
[u8; 32] keys[count]                          -- blake3 of the PNG bundle for each name
```

Ordering invariant: `keys[i]` corresponds to `names[i]`. A texture with no PNG at all (placeholder-only) writes a zero key (32 zero bytes). On `keys[i] == [0u8; 32]`, the runtime skips the `.prm` lookup entirely and substitutes per-slot placeholders without logging a warning.

## Plumbing

- PNG name-lookup helper currently in `crates/postretro/src/texture.rs::build_name_to_path_map` moves to `postretro-level-compiler` (shared module). Runtime drops its copy. The now-UI-only file is renamed to `ui_texture.rs`; `LoadedTexture` and the world-material placeholder logic relocate to the new `crates/postretro/src/render/loaded_texture.rs`.
- A new helper `resolve_cache_root(map_path: &Path) -> PathBuf` returns `resolve_texture_root(map_path).parent().unwrap().join(".prl-cache").join("tex")`; it lives next to `resolve_texture_root` in `main.rs`. `pack.rs` gains a `texture_root` and `cache_root` argument; the compiler threads `resolve_cache_root`'s return value in. The existing `--cache-dir` / `--no-cache` CLI flags govern only the workspace-rooted `cache.rs` directory; they do NOT affect `.prm` output, which always lives next to the textures it caches.
- The compiler's `cache.rs` mechanism is unused here — the on-disk `.prm` file (content-addressed filename) IS the cache. Existence + parse-success at `<cache>/<hex>.prm` is the cache hit signal; the `bundle_hash` field in the header is the input-fingerprint check for the all-slots-unchanged path.
- Crate split for the `.prm` wire format: `postretro-level-format::prm::{PrmFile, PrmSlot, PrmFormat, PrmReadError}` is the surface area shared by writer (level-compiler) and reader (postretro runtime). `postretro-level-compiler` is binary-only (no `[lib]` target), so the level-format crate is the only viable shared home. `postretro-level-format::prm::STAGE_VERSION` (`u8`) is the canonical constant; `texture_mips` imports it directly. No re-export.
- The global `base_sampler` field at `render/mod.rs:521` is removed; the `mip_count_samplers` pool replaces it.
- Runtime loader (`crates/postretro/src/startup/worker.rs::run_worker`): replace `texture_root = content_root.join("textures")` at lines 73–88 with `mod_cache_root = content_root.join(".prl-cache").join("tex")`. Pass `&world.texture_names` (`Vec<String>`) and `mod_cache_root` to the new `load_textures` signature (now `crates/postretro/src/render/loaded_texture.rs::load_textures`). Drop the `Vec<Option<String>>` wrap at lines 80–84; the new `TextureNamesSection` makes it unnecessary.
- `.gitignore`: already covers `.prl-cache/` (matches at any depth); no change needed. Confirm at promotion.

## Open questions

- **Cache directory deletion as a recovery step.** Users hitting corrupted `.prm` files should be told to `rm -rf content/<mod>/.prl-cache/` and rebuild. Document at promotion time in `build_pipeline.md`.
- **Normal-map filtering for fine geometric detail.** Mitchell + renormalise loses high-frequency variance (specular under-aliasing on glossy surfaces at distance). LEAN / Toksvig variance maps fix it but need a fourth channel or a separate roughness slot. Flag for a follow-up plan if artists complain.

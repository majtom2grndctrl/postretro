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
- Emissive-mask (`_e.png`) baking. `slot_mask` bit 3 is reserved; the writer and reader leave it unset until a follow-up plan implements emissive.

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

Add `SectionId::TextureCacheKeys = 32`, a `TextureCacheKeysSection` type with `to_bytes` / `from_bytes` and a round-trip test. Layout: `u32 count` + `[u8; 32] * count`. `count` equals `TextureNamesSection.names.len()`; entry `i` is the blake3 of the PNG content for texture `i`. Bump `CURRENT_VERSION` to 4 and update every test that constructs the header or asserts on `CURRENT_VERSION`, and add a v3 reject test that asserts `UnsupportedVersion { version: 3 }`. Extend the hand-written `SectionId::from_u32` match arm to cover the new variant.

Add a `prm` module to `postretro-level-format` owning the `.prm` wire format: `PrmFile`, `PrmSlot`, `PrmFormat`, `PrmReadError`, and a `STAGE_VERSION` constant. Both the compiler-side writer (Task 2) and runtime-side reader (Task 3) import these types from level-format — the parser is not reinvented in the runtime crate.

Update `context/lib/build_pipeline.md`'s section table at plan promotion time, not during the task.

### Task 2: `.prm` writer in `postretro-level-compiler`

Add `texture_mips.rs` next to `texture_validation.rs`. The wire-format types live in `postretro-level-format::prm` (Task 1); the writer imports them. Entry point takes the deduplicated texture-name list, the texture root path, and the cache root (`<mod>/.prl-cache/tex/`). For each name:

1. Resolve `<name>.png`, `<name>_s.png`, `<name>_n.png` via case-insensitive lookup (factor `build_name_to_path_map` out of the runtime into a shared helper — see *Plumbing*).
2. Compute `key = blake3(diffuse_png_bytes)` if diffuse exists; otherwise hash the first present slot's PNG; slot probe order is fixed at diffuse → specular → normal. Key identifies the bundle.
3. If `<cache>/<hex>.prm` already exists and parses, skip rebake.
4. Otherwise build mip chains per slot in linear `f32`, encode per *Wire format* below, write atomically (tempfile + rename).
5. Return `(name → key)` from the baker — do NOT touch `pack.rs` directly. Task 4 wires this into `TextureCacheKeysSection`.

Gamma handling per slot:

- **Diffuse (`Rgba8UnormSrgb`, tag 0):** decode each byte to linear `f32` via 256-entry sRGB→linear LUT, run separable Mitchell-Netravali in linear `f32`, encode back to sRGB on output. Alpha treated as linear throughout (premultiplied alpha not used). After filtering, clamp each linear sample to `[0.0, 1.0]` before sRGB encode. Specular clamp likewise to `[0.0, 1.0]` before R8 quantization.
- **Specular (`R8Unorm`, tag 2):** filter directly in linear `f32`. Already linear on disk.
- **Normal (`Rgba8Unorm`, tag 1):** filter linearly in `f32`, then per output texel decode `n = sample.rgb * 2 - 1`, normalise, re-encode. Fall back to byte `(127, 127, 255)` (the engine's neutral-normal placeholder encoding) if `||n|| < 1e-4` after filtering.

Edge condition: clamp-to-edge sampling. Precompute 1D Mitchell weights once per `(src_len, dst_len)` pair; horizontal pass into a scratch `f32` buffer, then vertical.

The baker is invoked from `main.rs` between `texture_validation::validate_sibling_color_spaces` (main.rs:163) and `pack_and_write_portals` (main.rs:515). The returned `(name → key)` map is threaded to the pack call.

### Task 3: `.prm` reader and runtime upload path

Touches `crates/postretro/src/texture.rs` and `crates/postretro/src/render/mod.rs`.

Type split:

| Type | Filter | Mips | Source | Use |
|---|---|---|---|---|
| `UiTexture` (new in `crates/postretro/src/texture.rs`) | Nearest min/mag | none (1 level) | PNG direct | splash, HUD, any 2D blit |
| `LoadedTexture` (refactored) | Nearest min/mag, Linear mipmap | full chain | `.prm` sidecar | world materials |

`UiTexture` carries `{ data, width, height }`. `install_splash_from_loaded` and `splash::upload_splash_texture` (`render/splash.rs:51`) switch to `UiTexture`. `LoadedTexture` becomes a thin wrapper over the parsed `.prm` slot data + GPU handles (per the bullet below). The magenta-checker placeholder remains in the `LoadedTexture` family (it's a world-material fallback, not UI) with `mip_count = 1`.

- Import the reader from `postretro-level-format::prm`. Decoded result is per-slot `(format_tag, width, height, level_count, payload)`.
- Change `upload_texture_data` to accept `levels: &[(u32, u32, &[u8])]` and `format: wgpu::TextureFormat`. Caller supplies one tuple per mip level (level width, level height, byte payload) plus the slot's format tag mapped to `wgpu::TextureFormat`. Delete `downsample_2x`, the `mitchell_netravali` weight code, and `mip_level_count_for`; the caller supplies levels.
- wgpu's `Queue::write_texture` accepts tightly-packed source rows; the `.prm` payload layout (no row alignment padding) uploads directly without staging.
- `LoadedTexture` becomes a thin wrapper over the parsed `.prm` slot data plus GPU handles. The current `is_placeholder` field is preserved (load-bearing for sibling-probe skipping at `texture.rs:184,224`).
- `load_textures` rewrites: for each entry in `TextureNamesSection`, look up the blake3 from `TextureCacheKeysSection`, open `<mod>/.prl-cache/tex/<hex>.prm`, decode, upload each level per slot. The texture-root PNG scan disappears from the runtime. (`texture_names` is `Vec<String>`; the runtime's current `Vec<Option<String>>` wrap at `startup/worker.rs:80-84` drops away.)
- Update placeholders (`black_specular_texture`, `neutral_normal_texture`, `generate_placeholder` checkerboard, `Placeholder Texture Diffuse`) to use `mip_count = 1` (`lod_max_clamp = 0.0`). The 64×64 checkerboard will look blockier at distance than the current mipped placeholder — intended, makes missing-texture cases more obvious in modder workflows.
- Replace the single global `base_sampler` with a `HashMap<u32, wgpu::Sampler>` (`mip_count_samplers`) on the renderer. Each material bind group selects the sampler whose key matches its `LoadedTexture.mip_count`. Sampler descriptor is unchanged except `lod_max_clamp = (mip_count - 1) as f32`. Existing call sites at `render/mod.rs:1289`, `1337`, `2213`, `2263` switch from `&base_sampler` to a lookup.
- Existing `texture.rs` test suite (~19 tests against PNG fixtures) is replaced with `.prm`-fixture tests. A small `prm-test-fixtures` helper builds in-memory `.prm` byte blobs.

Task 3 can be unit-tested against a hand-crafted `.prm` fixture + v4 PRL fixture, but the campaign-test acceptance check requires Task 2 done.

### Task 4: Plumbing and cleanup

- Lift PNG name-lookup helper (`build_name_to_path_map`) from `crates/postretro/src/texture.rs` into `postretro-level-compiler`. The runtime no longer scans the textures directory.
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
- sRGB encode: IEC 61966-2-1 piecewise curve (linear `< 0.0031308` segment, gamma branch above). Polynomial approximation acceptable if the regression test passes.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| PRL section enum | `SectionId::TextureCacheKeys` | `u32 = 32` | n/a | n/a | n/a |
| PRL section struct | `TextureCacheKeysSection` | `u32 count` + `[u8; 32] * count` | n/a | n/a | n/a |
| Sidecar file | `PrmFile` (compiler + runtime) | `.prm` body (see below) | n/a | n/a | n/a |
| Slot bitmask | `PrmSlots::{Diffuse, Specular, Normal, Emissive}` (Emissive reserved) | `u8` bits 0/1/2/3 | n/a | n/a | n/a |
| Format tag | `PrmFormat::{Rgba8UnormSrgb, Rgba8Unorm, R8Unorm}` | `u8` 0/1/2 (3 reserved BC5) | n/a | n/a | n/a |
| UI texture | `UiTexture` (`crates/postretro/src/texture.rs`) | n/a (runtime only) | n/a | n/a | n/a |

## Wire format

Little-endian throughout. All integers unsigned.

### `.prm` file body

```
-- header (43 bytes)
[u8; 4]  magic                = b"PRM\x01"     -- ASCII "PRM" + version byte; version lives only here
u8       stage_version                         -- u8 truncation of PrmFile STAGE_VERSION at write time
u8       slot_mask                             -- bit 0 diffuse, bit 1 specular, bit 2 normal, bit 3 emissive (reserved)
u8       reserved             = 0
[u8; 32] bundle_hash                           -- blake3 over concat of (slot_bit_index_u8, source_png_bytes)
                                               -- for every present slot in fixed order diffuse → specular → normal.
                                               -- Writer compares to detect input changes when the diffuse-only
                                               -- filename hash hasn't moved.
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

- Reject if `magic[0..3] != "PRM"`. Version lives in `magic[3]` only — no separate version byte to cross-check.
- Reject if `stage_version` doesn't match `postretro-level-format::prm::STAGE_VERSION` truncated to `u8` (the canonical home; `texture_mips::STAGE_VERSION` is a re-export `u32` for consistency with other cache stages). Bump beyond 255 requires a `.prm` `version` bump (i.e. a new magic byte). Writer overwrites on mismatch.
- Reject if `slot_mask` has bits 4–7 set. Bit 3 (emissive) parses but the corresponding slot block is ignored by the current reader.
- For each present slot: `level_count == floor(log2(max(width, height))) + 1`, `payload_bytes` equals the sum of `bytes_per_pixel(format_tag) * w_n * h_n` across levels with `w_n = max(1, width >> n)`, `h_n = max(1, height >> n)`.
- `bytes_per_pixel`: tag 0 → 4, tag 1 → 4, tag 2 → 1.

On any reject (bad magic, `stage_version` mismatch, `slot_mask` invalid, level/payload arithmetic mismatch), the runtime logs a `warn!` naming the texture and the failure reason, substitutes per-slot placeholders (`black_specular_texture`, `neutral_normal_texture`, magenta checker for diffuse), and continues level load. Level load never fails on a `.prm` parse error.

Empty file (no slots) encoding: `slot_mask = 0`, `total_body_bytes = 0`, no per-slot blocks. Permitted but never written by Task 2 (a texture with no source PNGs in any slot doesn't produce a `.prm` — the loader treats it the same as a missing texture today).

### `TextureCacheKeysSection` body

```
u32      count                                -- equals TextureNamesSection.names.len()
[u8; 32] keys[count]                          -- blake3 of the PNG bundle for each name
```

Ordering invariant: `keys[i]` corresponds to `names[i]`. A texture with no PNG at all (placeholder-only) writes a zero key (32 zero bytes); the runtime treats that as "no sidecar, use placeholder."

## Plumbing

- PNG name-lookup helper currently in `crates/postretro/src/texture.rs::build_name_to_path_map` moves to `postretro-level-compiler` (shared module). Runtime drops its copy.
- `pack.rs` gains a `texture_root` and `cache_root` argument. Compiler computes `cache_root = resolve_texture_root(map_path).parent().join('.prl-cache/tex/')`. The existing `--cache-dir` / `--no-cache` CLI flags govern only the workspace-rooted `cache.rs` directory; they do NOT affect `.prm` output, which always lives next to the textures it caches.
- The compiler's `cache.rs` mechanism is unused here — the on-disk `.prm` file (content-addressed filename) IS the cache. Existence + parse-success at `<cache>/<hex>.prm` is the cache hit signal; the `bundle_hash` field in the header is the input-fingerprint check for the all-slots-unchanged path.
- Crate split for the `.prm` wire format: `postretro-level-format::prm::{PrmFile, PrmSlot, PrmFormat, PrmReadError}` is the surface area shared by writer (level-compiler) and reader (postretro runtime). `postretro-level-compiler` is binary-only (no `[lib]` target), so the level-format crate is the only viable shared home. `postretro-level-format::prm::STAGE_VERSION` is the canonical constant; `texture_mips::STAGE_VERSION` re-exports it (compiler depends on level-format, so the constant flows one-way).
- The mod-overlay resolver that locates `<name>.png` returns the same `<mod>` directory for the `.prm` lookup at `<mod>/.prl-cache/tex/<hex>.prm`. No new resolver code; runtime calls the existing resolver with the texture name and asks for the parent `<mod>` directory.
- The global `base_sampler` field at `render/mod.rs:521` is removed; the `mip_count_samplers` pool replaces it.
- Runtime loader (`crates/postretro/src/startup/worker.rs::run_worker`) replaces the post-PRL `load_textures(texture_root, …)` call with a `load_textures(prl, mod_cache_root)` call that reads `.prm` files. The texture-root path argument can be removed from that call site.
- `.gitignore`: already covers `.prl-cache/` (matches at any depth); no change needed. Confirm at promotion.

## Open questions

- **Cache directory deletion as a recovery step.** Users hitting corrupted `.prm` files should be told to `rm -rf content/<mod>/.prl-cache/` and rebuild. Document at promotion time in `build_pipeline.md`.
- **Normal-map filtering for fine geometric detail.** Mitchell + renormalise loses high-frequency variance (specular under-aliasing on glossy surfaces at distance). LEAN / Toksvig variance maps fix it but need a fourth channel or a separate roughness slot. Flag for a follow-up plan if artists complain.

# Lightmap Resolution

## Goal

Lift the lightmap atlas resolution ceiling so baked lighting can resolve detail the current cap forbids, and make the higher resolution affordable with BC6H compression at rest. Today every non-trivial level saturates the 4096² atlas at the default 0.04 m/texel and silently coarsens on overflow — the cap is the active, routine limit on lighting fidelity (and the root cause of blocky baked soft-shadow contact edges, which a runtime filter can only mask).

## Scope

### In scope

- Raise the bake atlas cap from 4096 to 8192, with the runtime explicitly requiring and validating that device support.
- Per-map density opt-in (`_lightmap_density` worldspawn KVP) so only fidelity-critical maps pay the higher cost; default stays 0.04 m/texel.
- BC6H compression at rest for the irradiance atlas: deterministic bake-time encode, stored under the already-versioned `irradiance_format` tag, uploaded as a hardware-decoded, hardware-filterable BC6H texture. ~8× smaller on disk and in VRAM.
- A single `STAGE_VERSION` bump so previously-coarsened maps re-bake instead of serving stale cache.
- Quality + cost validation on the AMD Radeon Pro 5500M floor.

### Out of scope

- **Runtime bicubic lightmap filtering** — resolution attacks the cause; the cheap linear sampler is sufficient at finer density. POC parked on branch `proto/bicubic-lightmap`.
- **Multi-page / texture-array atlas** — defer until a single level genuinely needs >8192² of charts. Nothing is close.
- **Raising the requested device limit above 8192 (toward 16384)** — a future option if 8192 proves insufficient on the densest maps; not this plan.
- **Compressing the direction atlas** — it stays `Rgba8Unorm` octahedral on the nearest sampler (octahedral lerp ≠ slerp; small at 67 MB → 268 MB worst case). Revisit only if it becomes a budget problem.
- **LOD** — separate future roadmap milestone.

## Acceptance criteria

### Automated (test-gated)

- [ ] A chart set that overflows 4096² at a given density packs at ≤8192² without coarsening; one that exceeds 8192² still returns `AtlasOverflow` and the retry coarsens.
- [ ] Atlas dimensions stay power-of-two and ≤ the cap; both width and height remain 4-aligned (BC6H block requirement).
- [ ] Raising the cap bumps the lightmap `STAGE_VERSION`; the existing version-bump test passes against the new value (previously-coarsened maps re-key).
- [ ] A worldspawn `_lightmap_density` value sets the bake density and re-keys the cache; absent, the bake uses 0.04; `--lightmap-density` overrides the KVP; non-finite/≤0 warns and falls back to default.
- [ ] BC6H bake is byte-identical across two `--no-cache` processes on the same input (determinism invariant holds with the encoder in the path).
- [ ] BC6H round-trip error on a synthetic HDR irradiance gradient is within a fixed tolerance (per-channel max relative error threshold, asserted in a compiler test).
- [ ] The lightmap PRL section round-trips `irradiance_format = BC6H`: `to_bytes`→`from_bytes` reproduces dimensions and the block-sized blob; `from_bytes` still accepts `irradiance_format = 0` (uncompressed).
- [ ] Loading a BC6H lightmap section builds a `Bc6hRgbUfloat` texture bound as `Float { filterable: true }`; loading an uncompressed section still builds `Rgba16Float`. Both sample through the linear sampler.
- [ ] A lightmap atlas larger than the granted `max_texture_dimension_2d` degrades to flat ambient with a logged `[Renderer]` error — no panic.

### Manual-visual (NOT machine-verified)

- [ ] BC6H irradiance shows no perceptible banding versus uncompressed on a representative lit level (HDR lighting, smooth gradients).
- [ ] A level re-baked at 0.02 m/texel shows visibly less blocky soft-shadow contact edges than at 0.04, with the far penumbra unchanged.

### Measured (reported, not gated)

- [ ] BC6H reduces the irradiance blob ~8× on disk and resident VRAM; report actual sizes for occlusion-test and campaign-test at 4096² and 8192².
- [ ] Forward-pass time on the 5500M is unchanged by BC6H (hardware-decoded; identical tap count) — confirmed, not assumed.

## Tasks

### Task 1: Raise the cap + make the device requirement explicit

Raise `MAX_ATLAS_DIMENSION` (`lightmap_bake.rs`) from 4096 to 8192. The bake is a CLI with no GPU device, so the cap is a constant chosen to match guaranteed device support, not a `device.limits()` read. Make that requirement explicit on the runtime side: add `max_texture_dimension_2d: 8192` to the requested `wgpu::Limits` in `render/mod.rs` (clamped to `adapter.limits()`), and add an adapter pre-check that bails with a named `[Renderer]` error if the adapter grants less than 8192 — mirroring the sibling pre-checks (`TEXTURE_COMPRESSION_BC`, `atlas_format_filterable`). Add a runtime guard that validates a loaded lightmap atlas fits the granted `device.limits().max_texture_dimension_2d` and degrades to flat ambient with a logged error if not (mirror the disable-on-overflow posture of `sh_volume.rs`). Bump the lightmap `STAGE_VERSION` (currently 6 → 7): the cap is not part of `input_hash`, so without the bump a map that previously overflowed-and-coarsened would serve stale coarse cache against an unchanged input. This single bump covers every bake-output change in this plan.

### Task 2: Per-map density opt-in

Add a `_lightmap_density` worldspawn KVP (float, meters) read by the compiler and routed into `LightmapConfig.lightmap_density`. Default stays `DEFAULT_TEXEL_DENSITY_METERS` (0.04) when absent. The existing `--lightmap-density` CLI flag overrides the KVP. Since `LightmapConfig` already feeds `input_hash`, an authored density change re-keys the cache for free. Validate non-finite/≤0 with a parse warning that falls back to the default (match the CLI flag's validation posture). See Boundary inventory.

### Task 3: BC6H irradiance compression at rest

Add `IRRADIANCE_FORMAT_BC6H = 1` next to `IRRADIANCE_FORMAT_RGBA16F = 0` in the level-format lightmap section; widen the `from_bytes` validator to accept it and size the irradiance blob by block math (`ceil(w/4)·ceil(h/4)·16`) instead of `w·h·8`. In the bake, encode the irradiance atlas to `Bc6hRgbUfloat` with a deterministic BC6H encoder and emit it under the BC6H tag; keep the uncompressed path selectable for debugging (a `LightmapConfig` bool, folded into `input_hash`). The direction atlas is unchanged. At runtime, branch the irradiance texture creation on the format tag: `Bc6hRgbUfloat` (block-compressed upload via `create_texture_with_data`) vs `Rgba16Float`, both bound `Float { filterable: true }` and sampled through the existing linear sampler. No shader change — the fetch already reads `.rgb`, and BC6H is RGB-only (drop the unused alpha). `TEXTURE_COMPRESSION_BC` is already a required, granted feature (used for BC5 normals); no new feature request — at most a defensive BC6H format-features check in the `atlas_format_filterable` style.

### Task 4: Quality + cost validation

Add the automated tests above (determinism, BC6H round-trip tolerance, section round-trip, atlas-sizing, over-limit degrade). Re-bake occlusion-test and campaign-test at 4096²/8192² and at 0.04/0.02 density; record irradiance disk + VRAM sizes (expect ~8× BC6H reduction). On the 5500M: confirm no forward-pass regression (BC6H hardware-decoded), and visually confirm no BC6H banding and reduced contact blockiness at finer density.

## Sequencing

**Phase 1 (sequential):** Task 1 — owns the `STAGE_VERSION` bump and the cap; everything else builds on it.
**Phase 2 (concurrent):** Task 2 + Task 3 — independent surfaces (compiler frontend KVP vs bake-encode + format + runtime upload); both rely on Task 1's bump, neither bumps again.
**Phase 3 (sequential):** Task 4 — consumes Task 3's BC6H output and Task 1's cap.

## Rough sketch

- **Cap:** `MAX_ATLAS_DIMENSION` (`crates/level-compiler/src/lightmap_bake.rs:62`) 4096 → 8192. `shelf_pack` (`:546-603`) already clamps and power-of-two-rounds both axes; no packing change. Coarsen retry (`main.rs:338-383`) needs no change — it triggers at the new ceiling.
- **Explicit device requirement:** add `max_texture_dimension_2d` to `required_limits` (`render/mod.rs:969-975`) and a pre-check beside `:980-1026`. Runtime atlas-fits-device guard mirrors `sh_volume.rs:283-300` (log + disable → flat ambient).
- **STAGE_VERSION:** `lightmap_bake.rs:54` 6 → 7; add a changelog entry in the bump-convention comment. The existing bump-enforcing test re-keys.
- **Format tag:** `IRRADIANCE_FORMAT_RGBA16F` (`crates/level-format/src/lightmap.rs:68`) already documents BC6H as the intended extension. Add `IRRADIANCE_FORMAT_BC6H = 1`; widen the `!= 0` rejection in `from_bytes`. Header layout (28 bytes) unchanged.
- **Encoder:** a deterministic BC6H encoder (e.g. `intel_tex_2` ISPC bindings — confirm byte-determinism across runs as an AC). Mirror the BC5 block-size accounting at `loaded_texture.rs:140` (`ceil(w/4)·ceil(h/4)·16`).
- **Runtime format branch:** `upload_irradiance_texture` (`lighting/lightmap.rs:227-251`) selects `Bc6hRgbUfloat` vs `Rgba16Float` by tag. BGL `Float { filterable: true }` (binding 0) unchanged. wgpu 29: `Bc6hRgbUfloat` is filterable with no extra feature; hardware-decoded before filtering.

## Boundary inventory

| Name | Rust | Wire / serde | Luau | FGD KVP |
|---|---|---|---|---|
| lightmap density | `LightmapConfig.lightmap_density: f32` (meters) | baked into `LightmapSection.texel_density` (f32 LE) | n/a | `_lightmap_density` (worldspawn, float meters) |
| irradiance format BC6H | `IRRADIANCE_FORMAT_BC6H: u32 = 1` | `irradiance_format` u32 LE = `1` in section id 22 header | n/a | n/a |

## Wire format

Lightmap section (id 22) header stays 28 bytes, little-endian, unchanged in shape. The existing `irradiance_format: u32` field gains value `1 = Bc6hRgbUfloat` (alongside `0 = Rgba16Float`). When `irradiance_format = 1`, the irradiance blob is BC6H 4×4 block bytes — `ceil(width/4)·ceil(height/4)·16` — instead of `width·height·8`; `from_bytes` sizes the blob by format. `direction_format`, the direction blob (`width·height·4`), the `texel_density` field, and the optional mode trailer are unchanged. Mirrors the current section exactly except for the new format value and the format-dependent irradiance blob length. Empty/absent lightmap encoding is unchanged.

## Open questions

- **One plan or split.** Cap-raise + density (Tasks 1–2) is small; BC6H (Task 3) is the substantial, dependency-bearing piece. Kept together because compression is what makes the raised cap affordable to actually use — but Task 3 could split into its own spec if the encoder dependency or determinism work proves large.
- **Encoder choice + determinism.** `intel_tex_2` (ISPC BC6H) is the likely pick; the plan hard-requires byte-identical output across runs (AC). If the chosen encoder isn't deterministic, this blocks Task 3 and needs an alternative (a pure-Rust BC6H encoder, or pinning encoder settings).
- **BC6H default on vs opt-in.** Proposed: BC6H is the default irradiance storage (8× win, HDR-appropriate, smooth low-frequency data compresses cleanly), uncompressed retained behind the tag for debugging. Revisit if quality validation surfaces banding on any representative level.
- **Density opt-in surface.** Proposed `_lightmap_density` worldspawn KVP as the durable per-map authoring surface (CLI overrides). Alternative: CLI-only, deferring the KVP — simpler but the build-command-carried density is easy to forget.

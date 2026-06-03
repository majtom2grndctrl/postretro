# Lightmap Resolution

## Goal

Lift the lightmap atlas resolution ceiling so baked lighting can resolve detail the current cap forbids, and make the higher resolution affordable with BC6H compression at rest. Today every non-trivial level saturates the 4096² atlas at the default 0.04 m/texel and silently coarsens on overflow — the cap is the active, routine limit on lighting fidelity (and the root cause of blocky baked soft-shadow contact edges, which a runtime filter can only mask).

## Scope

### In scope

- Raise the bake atlas cap from 4096 to 8192, with the runtime explicitly requiring and validating that device support.
- Per-map density opt-in (`_lightmap_density` worldspawn KVP) so only fidelity-critical maps pay the higher cost; default stays 0.04 m/texel.
- BC6H compression at rest for the irradiance atlas: bake-time encode, stored under the already-versioned `irradiance_format` tag, uploaded as a hardware-decoded, hardware-filterable BC6H texture. ~8× smaller on disk and in VRAM.
- A single `STAGE_VERSION` bump so previously-coarsened maps re-bake instead of serving stale cache.
- Quality + cost validation on the AMD Radeon Pro 5500M floor.

### Out of scope

- **Runtime bicubic lightmap filtering** — resolution attacks the cause; the cheap linear sampler is sufficient at finer density. POC parked on branch `proto/bicubic-lightmap`.
- **Multi-page / texture-array atlas** — defer until a single level genuinely needs >8192² of charts. Nothing is close.
- **Raising the requested device limit above 8192 (toward 16384)** — a future option if 8192 proves insufficient on the densest maps; not this plan.
- **Compressing the direction atlas** — it stays `Rgba8Unorm` octahedral on the nearest sampler (octahedral lerp ≠ slerp; 67 MB at 4096² → 268 MB at 8192²). Revisit only if it becomes a budget problem.
- **LOD** — separate future roadmap milestone.

## Acceptance criteria

### Automated (test-gated)

- [ ] A chart set that overflows 4096² at a given density packs at ≤8192² without coarsening; one that exceeds 8192² still returns `AtlasOverflow` and the retry coarsens.
- [ ] Atlas dimensions stay power-of-two and ≤ the cap; both width and height remain 4-aligned (BC6H block requirement; satisfied for free since dimensions are always power-of-two ≥ 4, so `ceil(w/4)` is exact).
- [ ] Raising the cap bumps the lightmap `STAGE_VERSION`; the existing version-bump test passes against the new value (previously-coarsened maps re-key).
- [ ] A worldspawn `_lightmap_density` value sets the bake density and re-keys the cache; absent, the bake uses 0.04; `--lightmap-density` overrides the KVP; non-finite/≤0 warns and falls back to default.
- [ ] Two `--no-cache` bakes of the same input each decode within tolerance — byte-equality not required (lossy encode; output bytes are an implementation detail). The cache keys on inputs, so encoder non-determinism never mis-keys it.
- [ ] BC6H round-trip error on a synthetic HDR irradiance gradient is within tolerance. The per-channel relative-error threshold is calibrated against the chosen encoder so the owner-verified no-banding gate holds — set tight enough that the manual check passes, then freeze as the regression guard. Both the determinism AC above and this round-trip AC gate against the same frozen threshold.
- [ ] The lightmap PRL section round-trips `irradiance_format = BC6H`: `to_bytes`→`from_bytes` reproduces dimensions and the block-sized blob; `from_bytes` still accepts `irradiance_format = 0` (uncompressed).
- [ ] A lightmap atlas larger than the granted `max_texture_dimension_2d` degrades to the neutral placeholder (the no-usable-section fallback) with a logged `[Renderer]` error — no panic. The guard is a pure dimension-vs-limit comparison; unit-testable by injecting a fake limit — no real oversize allocation needed.

### Manual / owner-verified (NOT machine-verified)

- [ ] Loading a BC6H lightmap section builds a `Bc6hRgbUfloat` texture bound as `Float { filterable: true }`; loading an uncompressed section still builds `Rgba16Float`. Both sample through the linear sampler. (Requires a real device — not CI-testable here.)
- [ ] BC6H irradiance shows no perceptible banding versus uncompressed on `campaign-test` (HDR lighting, smooth gradients).
- [ ] A level re-baked at 0.02 m/texel shows visibly less blocky soft-shadow contact edges than at 0.04, with the far penumbra unchanged.

### Measured (reported, not gated)

- [ ] BC6H reduces the irradiance blob ~8× on disk and resident VRAM; report actual sizes for occlusion-test and campaign-test at 4096² and 8192².
- [ ] No forward-pass regression beyond measurement noise on the 5500M (hardware BC6H decode is not guaranteed bit-for-bit identical bandwidth to `Rgba16Float` on RDNA1/Metal) — confirmed, not assumed.

## Tasks

### Task 1: Raise the cap + make the device requirement explicit

Raise `MAX_ATLAS_DIMENSION` (`lightmap_bake.rs`) from 4096 to 8192. The bake is a CLI with no GPU device, so the cap is a constant chosen to match guaranteed device support, not a `device.limits()` read. Make that requirement explicit on the runtime side: add an adapter pre-check that bails with a named `[Renderer]` error if the adapter grants `max_texture_dimension_2d` less than 8192 — mirroring the `atlas_format_filterable` pre-check (which carries the `[Renderer]` prefix the AC requires; the older `TEXTURE_COMPRESSION_BC` bail predates that convention and omits the prefix, so copy `atlas_format_filterable`, not the BC check). Then set `max_texture_dimension_2d: 8192` in the requested `wgpu::Limits` in `render/mod.rs`; the pre-check guarantees this is satisfiable, so no clamp is needed. (The current `required_limits` does not set this field and wgpu's default is already 8192; this formalizes the requirement — the pre-check is the load-bearing part.) Add a runtime guard that validates a loaded lightmap atlas fits the granted `device.limits().max_texture_dimension_2d` and degrades to the existing neutral placeholder with a logged error if not: add a `.filter()` mirroring `render/sh_volume.rs:283-300` that drops an oversize section to `usable = None`, so it falls through to the existing `upload_placeholder_irradiance` path (the same branch a no-usable-section map already takes). "Flat ambient" in this plan means that existing placeholder, not a new value. The two guards are distinct paths: the init adapter pre-check fail-fasts to guarantee ≥8192 device support, while the runtime per-atlas guard is defensive for a baked atlas that exceeds the granted limit (e.g. future or corrupt content) and degrades to flat ambient rather than panicking. Bump the lightmap `STAGE_VERSION` (currently 6 → 7): the cap is not part of `input_hash`, so without the bump a map that previously overflowed-and-coarsened would serve stale coarse cache against an unchanged input. This single bump covers every bake-output change in this plan.

### Task 2: Per-map density opt-in

Add a `_lightmap_density` worldspawn KVP (float, meters per texel — matching `DEFAULT_TEXEL_DENSITY_METERS`) read by the compiler and routed into `LightmapConfig.lightmap_density`. Default stays `DEFAULT_TEXEL_DENSITY_METERS` (0.04) when absent. The existing `--lightmap-density` CLI flag overrides the KVP. Since `LightmapConfig` already feeds `input_hash`, an authored density change re-keys the cache for free. Also register `_lightmap_density` in the `worldspawn` FGD definition so it is exposed in the editor (the compiler reads worldspawn KVPs regardless, but editor authoring needs the FGD entry). Validate non-finite/≤0 the way the loader handles invalid KVPs (`build_pipeline.md` §Built-in Classname Routing: warn, fall back to the documented default, continue), applied here at compile time in the compiler's worldspawn read: warn naming the key (worldspawn has no meaningful per-entity origin to name) and fall back to default. The `--lightmap-density` CLI flag keeps its existing hard-reject posture. See Boundary inventory.

### Task 3: BC6H irradiance compression at rest

Add `IRRADIANCE_FORMAT_BC6H = 1` next to `IRRADIANCE_FORMAT_RGBA16F = 0` in the level-format lightmap section; widen the `from_bytes` validator to accept the new `irradiance_format` value (it reads the blob by the stored `irr_len`, not by recomputing block math); the bake producer writes `ceil(w/4)·ceil(h/4)·16` into `irr_len` instead of `w·h·8`. In the bake, encode the irradiance atlas to `Bc6hRgbUfloat` with the in-tree BC6H encoder (mirroring `bc5.rs`; see Open questions) and emit it under the BC6H tag; keep the uncompressed path selectable for debugging (a `LightmapConfig` bool, folded into `input_hash`). The direction atlas is unchanged. At runtime, branch the irradiance texture creation on the format tag: `Bc6hRgbUfloat` (block-compressed upload via `create_texture_with_data`) vs `Rgba16Float`, both bound `Float { filterable: true }` and sampled through the existing linear sampler. No shader change — the fetch already reads `.rgb`, and BC6H is RGB-only (drop the unused alpha in the BC6H encode only; the uncompressed-debug path stays byte-identical to today — `Rgba16Float`, `w·h·8`, alpha retained — so the format tag is the only divergence). `TEXTURE_COMPRESSION_BC` is already a required, granted feature (used for BC5 normals); no new feature request — a BC6H format-features check that fail-fasts at init with a named `[Renderer]` error (since BC6H is the default irradiance storage), matching the `atlas_format_filterable`/`Rgba16Float` sibling checks — not a silent log.

### Task 4a: Automated validation

Add the automated tests: determinism (two `--no-cache` bakes decode within the frozen tolerance), BC6H round-trip tolerance (synthetic HDR gradient, compiler test), section round-trip (`irradiance_format = BC6H` and `= 0`), atlas-sizing (power-of-two, 4-aligned, ≤cap), and over-limit degrade (inject fake limit, assert flat-ambient + logged error, no real oversize allocation).

### Task 4b: Owner hardware / visual sign-off (5500M)

Re-bake occlusion-test and campaign-test at 4096²/8192² and at 0.04/0.02 density. Record irradiance disk + VRAM sizes (expect ~8× BC6H reduction). Confirm no forward-pass regression beyond measurement noise. Confirm no BC6H banding on campaign-test and reduced contact blockiness at finer density.

## Sequencing

**Phase 1 (sequential):** Task 1 — owns the `STAGE_VERSION` bump and the cap; everything else builds on it.
**Phase 2 (concurrent):** Task 2 + Task 3 — independent surfaces (compiler frontend KVP vs bake-encode + format + runtime upload); both rely on Task 1's bump, neither bumps again.
**Phase 3 (sequential):** Task 4a (automated tests), then Task 4b (owner hardware/visual sign-off) — both consume Task 3's BC6H output and Task 1's cap.

## Rough sketch

- **Cap:** `MAX_ATLAS_DIMENSION` (`crates/level-compiler/src/lightmap_bake.rs:62`) 4096 → 8192. `shelf_pack` (`:546-603`) already clamps and power-of-two-rounds both axes; no packing change. Coarsen retry (`main.rs:338-383`) needs no change — it triggers at the new ceiling.
- **Explicit device requirement:** add `max_texture_dimension_2d` to `required_limits` (`render/mod.rs:969-975`) and a pre-check beside `:980-1026`. Runtime atlas-fits-device guard mirrors `render/sh_volume.rs:283-300` (log + drop section to the existing neutral placeholder).
- **STAGE_VERSION:** `lightmap_bake.rs:54` 6 → 7; add a changelog entry in the bump-convention comment. The existing bump-enforcing test re-keys.
- **Format tag:** `IRRADIANCE_FORMAT_RGBA16F` (`crates/level-format/src/lightmap.rs:68`) already documents BC6H as the intended extension. Add `IRRADIANCE_FORMAT_BC6H = 1`; widen the `!= 0` rejection in `from_bytes`. Header layout (28 bytes) unchanged.
- **Encoder:** in-tree pure-Rust BC6H, mirroring `bc5.rs` (single non-partitioned mode; `intel_tex_2` ISPC as a temporary unblock — see Open questions). Byte-determinism not required (lossy; gated on round-trip tolerance). Mirror the BC5 block-size accounting at `loaded_texture.rs:140` (`ceil(w/4)·ceil(h/4)·16`).
- **Runtime format branch:** `upload_irradiance_texture` (`lighting/lightmap.rs:227-251`) selects `Bc6hRgbUfloat` vs `Rgba16Float` by tag. BGL `Float { filterable: true }` (binding 0) unchanged — the BGL entry is sample-type only, so both `Rgba16Float` and `Bc6hRgbUfloat` bind to the same BGL and pipeline; the per-tag branch needs no second BGL/pipeline variant. wgpu 29: `Bc6hRgbUfloat` is filterable with no extra feature; hardware-decoded before filtering.

## Boundary inventory

| Name | Rust | Wire / serde | Luau | FGD KVP |
|---|---|---|---|---|
| lightmap density | `LightmapConfig.lightmap_density: f32` (meters per texel) | baked into `LightmapSection.texel_density` (f32 LE) | n/a | `_lightmap_density` (worldspawn, float, meters per texel) |
| irradiance format BC6H | `IRRADIANCE_FORMAT_BC6H: u32 = 1` | `irradiance_format` u32 LE = `1` in section id 22 header | n/a | n/a |

## Wire format

Lightmap section (id 22) header stays 28 bytes, little-endian, unchanged in shape. The existing `irradiance_format: u32` field gains value `1 = Bc6hRgbUfloat` (alongside `0 = Rgba16Float`). The header carries explicit `irr_len`/`dir_len` byte-count fields; `from_bytes` reads the blob by those stored lengths (unchanged mechanism). When `irradiance_format = 1`, the bake producer writes `ceil(width/4)·ceil(height/4)·16` into `irr_len` instead of `width·height·8`. `direction_format`, the direction blob (`width·height·4`), the `texel_density` field, and the optional mode trailer are unchanged. Mirrors the current section exactly except for the new format value and the format-dependent irradiance blob length. Empty/absent lightmap encoding is unchanged.

## Open questions

- **One plan or split.** Cap-raise + density (Tasks 1–2) is small; BC6H (Task 3) is the substantial, dependency-bearing piece. Kept together because compression is what makes the raised cap affordable to actually use — but Task 3 could split into its own spec if the encoder dependency or determinism work proves large.
- **Encoder choice.** Decided: in-tree pure-Rust BC6H encoder, mirroring `bc5.rs`. Irradiance is smooth low-frequency HDR data, so a single non-partitioned mode (two high-precision RGB endpoints, 16 interpolated steps) reproduces it without banding — the same min/max-endpoint bet BC5 already wins on normal maps, and it keeps the compiler dependency-free per the lean northstar. `intel_tex_2` (ISPC BC6H) is an acceptable temporary unblock if the bit-packing proves slow to land. The encoder sits behind a clean seam — it just produces block bytes; nothing downstream (format tag, runtime upload, sizing) cares how — so swapping implementations is a reversible door, no rest-of-plan changes.
- **BC6H default on vs opt-in.** Proposed: BC6H is the default irradiance storage (8× win, HDR-appropriate, smooth low-frequency data compresses cleanly), uncompressed retained behind the tag for debugging. Revisit if quality validation surfaces banding on any representative level.
- **Density opt-in surface.** Proposed `_lightmap_density` worldspawn KVP as the durable per-map authoring surface (CLI overrides). Alternative: CLI-only, deferring the KVP — simpler but the build-command-carried density is easy to forget.

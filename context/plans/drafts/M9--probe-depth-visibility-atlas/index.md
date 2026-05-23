# Probe Depth/Visibility Atlas (bake)

## Goal

Bake per-probe depth moments (mean ray distance and mean squared distance) into the SH irradiance volume, ray-cast through the same global BVH the SH baker already traverses. The runtime's depth-aware Chebyshev interpolant (spec #3, separate) consumes these moments to weight each probe by visibility, killing the indirect-light leak through walls that survives the spec #1 weight fixes. This spec produces only the baked data and the data format — no runtime sampling change.

Milestone 9 spec #2. Chain: #1 (probe weight correctness, shipped) → **#2 (this) → #3 (Chebyshev runtime interpolant)**. Spec #1's measurement gate records the residual through-wall smear after the free weight fixes; that residual is what this atlas exists to remove. If spec #1 recorded zero residual on the gate map, this bake is still built (the leak is map-dependent), but the depth term is the mechanism that addresses leak structurally rather than heuristically.

## Key decisions

| # | Decision | Rationale |
|---|---|---|
| PRL section strategy | **Extend the existing ShVolume section (id 20).** Grow the per-probe record with two depth-moment f16 values; advance `probe_stride`; bump `SH_VOLUME_VERSION`. **Not** a sibling section. | `PROBE_STRIDE`'s own docstring names "future per-probe base data (e.g. DDGI distance fields)" as the intended stride-growth path — this is pre-built scaffolding. The loader already skips `probe_stride - PROBE_STRIDE` trailing bytes per probe, so the stride grows without breaking the read path. Depth moments share the base SH grid 1:1 (`grid_origin`/`cell_size`/`grid_dimensions`); a sibling section would duplicate that grid header and risk drift. One section, one grid = chunk-friendly (a future brick split partitions one probe array, not two sections). The SH stage cache serializes the whole section, so the moments are cached for free under the existing key once the stage version bumps. |
| Moment representation | **Two values per probe: mean distance `E[d]` and mean squared distance `E[d²]`, stored f16.** | These are the two inputs Chebyshev's one-tailed inequality needs (`variance = E[d²] − E[d]²`). Storing the two moments — not a precomputed variance — lets spec #3 choose its bias/clamp without a re-bake. f16 matches the delta-SH precedent (`DeltaShProbe` stores SH as f16) and keeps the stride growth small. |
| Distance frame | **Linear world-space distance in meters, measured from the probe origin along each sampled ray.** | The probe grid is world-space (`grid_origin`/`cell_size` in meters). A linear metric lets spec #3 reconstruct `dist(probe, fragment)` from the same grid coordinates with no extra transform. No octahedral/per-direction encoding — a single isotropic moment pair per probe is the minimum DDGI-style depth signal and the cheapest chunk-friendly format. |
| Ray reuse | **Accumulate moments in the existing 256-ray sphere loop, reusing each ray's `closest_hit` distance.** | The SH baker already casts `RAYS_PER_PROBE = 256` Fibonacci-lattice rays per probe and computes a hit distance per ray. Moments are a near-free byproduct: no new rays, no second BVH pass. Sky misses contribute a far sentinel distance (no early hit = "far away," which is the correct visibility signal). |
| Invalid probes | **Invalid probes (in-wall / exterior) write zeroed moments**, matching how they zero their SH coefficients today. | Spec #1 already drops invalid corners via band-0 alpha before any depth weighting; the depth term never sees an invalid probe. Zeroing keeps the record uniform and the bake deterministic. |
| Far sentinel | **A sky-miss ray contributes `4 × length(cell_size)` — 4× the full 3D cell diagonal — to both moments.** | Cell-relative, so it reads as "fully open" at the probe-spacing scale the Chebyshev interpolant (spec #3) operates in, and tracks spec #5's coarser open-area spacing automatically; trivially f16-representable. Spec #3 may revisit the exact multiple. |
| Cache | **Bump `sh_bake::STAGE_VERSION`; reuse the existing `sh_volume` cache stage and key.** | The moments are baked inside `bake_sh_volume` and serialized inside `ShVolumeSection`, so the existing cache stage covers them. The version bump invalidates stale entries on the next build, per the documented stage-version rule. |

## Scope

### In scope

- Compute, per valid probe, `E[d]` and `E[d²]` over the 256 sphere rays already cast by the SH baker, using each ray's `closest_hit` distance (sky miss → far sentinel). Invalid probes write zeroed moments.
- Extend `ShProbe` and the ShVolume on-disk record with the two f16 moments; advance `probe_stride`; bump `SH_VOLUME_VERSION`; update `to_bytes`/`from_bytes` round-trip and the section's tests.
- Bump `sh_bake::STAGE_VERSION` so the build cache rebakes; keep the existing `sh_volume` cache stage and key composition unchanged otherwise.
- Keep the bake deterministic: order-stable moment accumulation in the existing `into_par_iter` fan-out, byte-identical output for identical inputs.
- Log a coarse depth-moment aggregate (mean/max `E[d]` over valid probes, plus valid/invalid counts) in the SH bake stats.

### Out of scope (non-goals)

- The runtime Chebyshev / DDGI visibility-weighted interpolant — spec #3. No shader change, no new GPU texture, no runtime upload change here.
- Per-direction (octahedral) depth maps. A single isotropic moment pair per probe is the chosen format; directional depth is a deferred refinement, not a half-built field.
- Brick/streaming split of the probe grid. Streaming is deferred (handoff "Streaming: why deferred"); this spec only keeps the format chunk-friendly so the split needs no interpolant rewrite.
- Any change to the spec #1 weight blend, validity plumbing, or band-0 alpha.
- A sibling PRL section, a new SectionId, or any pack-side section-list change.
- Directional fog (spec #4) and the memory-budget checkpoint (spec #5).

## Acceptance criteria

- [ ] After a clean build of a map with static lighting, the ShVolume section carries two depth-moment values per probe, and a fresh `ShVolumeSection::from_bytes` followed by `to_bytes` round-trips byte-identically.
- [ ] For a probe in open space, the baked mean distance is large (rays mostly miss or hit far geometry); for a probe in a tight corner near walls, the baked mean distance is small. The two are distinguishable in the baked data, demonstrating the moments encode local occlusion.
- [ ] A probe flagged invalid (in-wall or exterior leaf) writes zeroed depth moments, matching its zeroed SH coefficients.
- [ ] Squared-distance moment is consistent with the mean: `E[d²] >= E[d]²` (variance non-negative) for every valid probe. The check compares the pre-rounding f32 moments, since two independently-rounded f16 values can violate a naive `>=`.
- [ ] Re-running the bake on identical inputs produces byte-identical ShVolume section bytes (the build cache's determinism contract still holds with moments included).
- [ ] Bumping `sh_bake::STAGE_VERSION` invalidates the prior `sh_volume` cache entry: the first build after the change is a cache miss and rebakes; the second is a hit.
- [ ] An old `.prl` (pre-bump `SH_VOLUME_VERSION`) is rejected at load with the existing version-mismatch error rather than silently misread.
- [ ] A reader that knows only the minimum `PROBE_STRIDE` (an unaware consumer) still reads SH coefficients and validity correctly from a new-format file — the stride-skip path tolerates the larger record.
- [ ] The SH bake stats log line reports a coarse depth-moment aggregate (mean and max `E[d]` over valid probes, plus valid/invalid probe counts).

## Tasks

### Task 1: Extend the ShVolume format with depth moments

In `crates/level-format/src/sh_volume.rs`: add the two f16 depth moments to `ShProbe` (a mean-distance and a squared-distance value), advance `PROBE_STRIDE` to cover them on a 4-byte boundary, and bump `SH_VOLUME_VERSION`. Update `to_bytes` to write the moments after `validity`/padding within the per-probe record, and `from_bytes` to read them; the existing `probe_stride`-skip at the end of the per-probe read loop already tolerates any trailing bytes, so the only read change is pulling the two moments from their fixed offset within the record before the skip. Extend the round-trip, stride, and version-mismatch tests to cover the moments — including a test that a fixture with `version == 3` is rejected by `from_bytes` (anchors the old-`.prl` rejection AC). Keep the moments inside the per-probe record (not a trailing block) so the grid header and z-major/y/x probe order are unchanged. See Wire format below for the exact constraints.

Plumbing: `PROBE_STRIDE` and `SH_VOLUME_VERSION` are public constants in this module; the runtime loader (`render/sh_volume.rs`) and the baker (`sh_bake.rs`) both reference `PROBE_STRIDE` — the loader only via the section's own read path (it does not hand-roll the stride), so growing the constant flows through without a runtime edit. `ShProbe::default()` must zero the new fields.

### Task 2: Bake the depth moments in the SH baker

In `crates/level-compiler/src/sh_bake.rs`: extend the per-probe ray loop so each probe accumulates `Σd` and `Σd²` alongside the SH projection, then divides by `RAYS_PER_PROBE` to produce `E[d]` and `E[d²]`. `bake_probe_indirect_rgb`'s ray loop calls `sample_radiance_rgb`, which casts the ray via `closest_hit` but currently DISCARDS the hit distance (returns only `Vec3`). To accumulate moments, `sample_radiance_rgb` must return the per-ray distance to its sole caller; the moment accumulation then lives in a SH-path-only variant beside `bake_probe_indirect_rgb` (see plumbing). A ray that misses all geometry (`closest_hit` returns `None`) contributes the far sentinel `4 × length(cell_size)` (4× the full 3D cell diagonal) — large enough to read as "fully open" under Chebyshev at the probe-spacing scale, and f16-representable. The sentinel contributes to BOTH moments: `sentinel` to `Σd` and `sentinel²` to `Σd²`. All `RAYS_PER_PROBE` rays always accumulate (sky-miss rays included via the sentinel), so dividing by the constant `RAYS_PER_PROBE` is exact. Write the two moments into each valid probe's `ShProbe`; invalid probes keep the zeroed default (the early `validity == 0` return path already produces `ShProbe::default()`). Bump `sh_bake::STAGE_VERSION`. Extend `log_stats` to report the moment aggregate. The accumulation must stay order-stable per probe (sequential sum over the fixed direction list), preserving the determinism guard test.

Plumbing: the moment accumulation lives inside the existing `into_par_iter().map()` over probes — each probe computes its own moments locally, so the parallel fan-out stays order-preserving (one probe → one `ShProbe`, no cross-probe reduction). No new field in `ShBakeCtx`, `ShInputs`, or `ShConfig`: the moments derive entirely from geometry the bake already reads, and the cache key already covers geometry via `ShInputs`. The far sentinel is `4 * length(cell_size)`, computed from the grid header's `cell_size` (passed into the probe bake) rather than a fixed module constant, so it scales with probe spacing. Topology: change `sample_radiance_rgb` to return its `closest_hit` distance to its sole caller; add a SH-path-only moment-accumulating wrapper beside `bake_probe_indirect_rgb` that sums `Σd`/`Σd²` from those distances. Leave `bake_probe_indirect_rgb` itself and its `delta_sh_bake.rs:141` caller (the per-light delta baker, which does not carry depth moments) untouched.

### Task 3: Determinism and version regression coverage

Extend tests so the format change is caught by the existing guards. Requirements:

- The byte-identical-on-repeat test (`sh_volume_bake_produces_byte_identical_output_on_repeated_runs`) **must** exercise probes with varied hit distances so the moment-accumulation path is genuinely covered. A fixture with a single floor triangle produces uniform, near-degenerate distances and can pass without ever exercising the moments — false confidence this project's byte-identical-determinism culture rejects. If the existing fixture is degenerate, add geometry (e.g. a second triangle at a different depth, or a wall) that produces a range of `closest_hit` distances across the probe set.
- Add a `STAGE_VERSION`-bump cache-miss-then-hit test in `main.rs`'s cache tests.
- Add a section-level test that a known *valid, non-degenerate* probe satisfies `E[d²] >= E[d]²`, comparing the pre-rounding f32 moments (an invalid probe's zeroed moments pass vacuously and an f16-rounded comparison can flake). Anchors the variance-non-negativity AC.

## Sequencing

**Phase 1 (sequential):** Task 1 — format extension; the baker writes into the new fields, so the format must exist first.
**Phase 2 (sequential):** Task 2 — bake; consumes Task 1's extended `ShProbe` and stride.
**Phase 3 (sequential):** Task 3 — regression coverage; asserts the behavior Task 1 and Task 2 produce.

## Rough sketch

- Format: `ShProbe` in `crates/level-format/src/sh_volume.rs` gains two f16 fields (mean distance, mean squared distance). `PROBE_STRIDE` grows from 112 to 116. `SH_VOLUME_VERSION` 3 → 4. The two moments serialize within the per-probe record after the validity byte; `from_bytes` reads them at their fixed in-record offset before advancing by `probe_stride`.
- Baker: `sample_radiance_rgb` returns its `closest_hit` distance (sole caller); a SH-path-only wrapper beside `bake_probe_indirect_rgb` in `crates/level-compiler/src/sh_bake.rs` returns the SH coeffs plus `(sum_d, sum_d2)`; `bake_sh_volume` divides by `RAYS_PER_PROBE` and stamps the moments into each `ShProbe`. The shared `bake_probe_indirect_rgb` and its `delta_sh_bake.rs` caller are untouched. Sky miss → sentinel `4 * length(cell_size)`. `STAGE_VERSION` 1 → 2.
- Cache: no code change beyond the `STAGE_VERSION` bump — `main.rs` (~397) keys on `sh_bake::STAGE_VERSION`, so the bump alone invalidates and the existing `c.put(&sh_key, &section.to_bytes())` stores the moment-bearing section.
- Pack: unchanged. `pack.rs` (~362, ~425) calls `sh_volume.to_bytes()` and tags it `SectionId::ShVolume`; the section grows internally with no section-list edit.
- Runtime: untouched in this spec. `render/sh_volume.rs` reads the section via its own loader and uploads SH bands; the moments sit in the per-probe record waiting for spec #3 to repack them into a depth texture alongside the band textures.

## Wire format

The depth moments extend the existing **ShVolume section (id 20)**, not a new section. The section's existing header and probe-order conventions are unchanged; only the per-probe record grows.

- **Endianness:** little-endian (matches the section's existing `to_le_bytes` throughout).
- **Section version:** `SH_VOLUME_VERSION` advances 3 → 4. Old files (version ≤ 3) are rejected by the existing version-check in `from_bytes`. This is the format's stated purpose for the version field. `SH_VOLUME_VERSION` is the ShVolume section-internal version, independent of the PRL container header `version` (also 4); bumping it does not touch the header version.
- **Per-probe record growth:** the moments are appended **inside** the per-probe record, after the existing `validity` u8 and its padding, as **two f16 values** (mean distance, then mean squared distance), with trailing padding to land `probe_stride` on a 4-byte boundary. Order: `[27 × f32 sh_coefficients][u8 validity][f16 E_d][f16 E_d2][padding to stride]`. Pinned offsets: `sh_coefficients` bytes 0–107, `validity` byte 108 (unchanged — spec #1's band-0-alpha packer is unaffected), `E_d` (f16) bytes 109–110, `E_d2` (f16) bytes 111–112, padding bytes 113–115, total `PROBE_STRIDE` = 116.
- **`probe_stride`:** the header's `probe_stride` field grows to the new record size and is written from the updated `PROBE_STRIDE` constant. The loader's existing `probe_stride < PROBE_STRIDE` reject and `probe_stride`-skip both continue to hold: a future stride growth (spec #3 or a brick split) needs no read-loop change. The read loop advances by the file header's `probe_stride` field (not the reader's compiled-in `PROBE_STRIDE` constant), which is why an unaware reader compiled with a smaller constant still parses a larger record correctly.
- **f16 encoding:** via `lightmap::f32_to_f16_bits`, matching `DeltaShProbe`'s f16 SH storage. A far sentinel distance must round-trip representably in f16 (well under f16 max).
- **Empty / invalid probes:** invalid probes write `0x0000` for both moments (zeroed, like their SH coefficients). An empty grid (`grid_dimensions == [0,0,0]`) emits zero probe records — unchanged.
- **Grid header:** `grid_origin`, `cell_size`, `grid_dimensions`, `animated_light_count` are unchanged. The moments are addressed by the same z-major/y/x probe index as the SH bands — this is the chunk-friendly property: any later brick split partitions the single probe array and the moments travel with their probe.
- **Mirrors this layout:** the per-probe-record growth mirrors `PROBE_STRIDE`'s documented forward-compat design directly (the docstring cites "DDGI distance fields"). No other section's layout is touched.

## Open questions

None outstanding. Resolved during review: the far sentinel is pinned to `4 * length(cell_size)` (see Key decisions / Task 2; spec #3 may revisit the multiple), and `PROBE_STRIDE` is pinned to 116 (see Wire format).

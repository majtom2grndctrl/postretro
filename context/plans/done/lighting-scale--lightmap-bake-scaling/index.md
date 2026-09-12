# Lightmap Bake Scaling — Coarser Direction Atlas + Per-Surface Density

## Goal

Cut static-lightmap runtime **direction-atlas** storage — VRAM and the load-time
footprint — for interior-heavy and larger maps. The direction atlas is uncompressed
`Rgba8Unorm` and structurally ~80% of lightmap bytes (BC6H irradiance is ~1
B/texel-equivalent against RGBA8 direction at 4, over shared dims and layer count →
`4/(4+1)`; measured baseline in `research.md`). Two direction levers shrink it: bake it
coarser per axis, and drop its two unused channels (`Rgba8`→`Rg8`). Both reshape the
direction **encode** after the atlas packer runs, so neither changes the shared atlas
`layer_count` or the bake's peak memory — they are runtime VRAM/load wins only.

A third lever, a **per-surface density override**, lets authors place lightmap detail
where it matters — playing-field surfaces fine, decorative or distant geometry coarse —
and is the only lever here that reduces bake-side chart pressure (fewer texels upstream
of the packer, so fewer/smaller layers).

These three levers stand on their own — the direction atlas is the dominant uncompressed
lightmap term on any map, and per-surface density is author-facing detail control — and
they also serve the fine-density (0.04 m/texel) effort for large maps. Measurement
(`research.md`) settles what blocks 0.04 there: not the atlas caps (stress-warren-class
maps pack within both the 8192² per-layer and 256-layer caps — the array packer opens
layers rather than overflowing), and not peak bake RAM (the full fixture peaks ~7.8 GB at
0.04, comfortably resident). The binding barrier is **bake throughput** — a full 0.04 bake
runs ~70 minutes — which Task 3 attacks directly by cutting decorative-surface texel
counts. This plan does not by itself deliver uniform 0.04 baking of a large map: that is
primarily a throughput problem (peak RAM becomes binding only on maps larger than
stress-warren, where the sibling `lighting-scale--lightmap-bake-incremental-flush` bounds
it — see Sequencing). The irradiance BC6H path is untouched.

## Scope

### In scope

- Bake the direction atlas at a lower resolution than irradiance, exploiting the
  format's already-decoupled `dir_width`/`dir_height`. Default half-res per axis
  (~4× fewer direction texels), factor selectable, factor 1 = current behavior.
- Drop the two constant/unused direction channels (`Rgba8Unorm` → `Rg8Unorm`) after
  verifying the shader samples only R/G for the static atlas (~2× further). New
  direction-format tag, no section version bump.
- Per-surface density override authored via a brush-entity scale region carrying a
  `_lightmap_scale` KVP; resolved per chart in chart planning, layered over the
  global density.
- Fold `_lightmap_scale` (and the direction-scale factor) into the bake cache key so
  changing either re-bakes rather than serving a stale atlas.

### Out of scope / non-goals

- **The irradiance BC6H path.** No change to the irradiance format or encoder. Tasks 1–2
  (the direction levers) leave irradiance dims untouched; Task 3 shrinks charts, so it
  reduces irradiance and direction texels together — the two atlases share one chart/UV
  geometry, so irradiance dims fall wherever a region coarsens. That drop is expected and
  is counted in the total-lightmap-bytes acceptance criterion.
- **Compile-time peak bake RAM.** ~7.8 GB at 0.04 on the full stress-warren fixture
  (comfortably resident, not the 0.04 barrier); binding only on maps larger than
  stress-warren, where the sibling `lighting-scale--lightmap-bake-incremental-flush`
  bounds it (see Sequencing for the coupling).
- **The storage-buffer / SH-delta footprint problem.** Separate spec
  (`lighting-scale--sh-delta-footprint-instrumentation`); unrelated GPU budget.
- **Block-compressing the direction atlas.** Octahedral lerp ≠ slerp — BC block
  compression corrupts decoded directions (the standing design note). Coarsening
  and channel-drop are the only direction levers here.
- **The animated direction atlas.** It keeps `Rgba8Unorm` — its `.a` is a live
  coverage flag the forward pass reads. Only the static atlas changes.
- **Old-`.prl` migration.** All fixtures re-bake from source.
- **Per-face granularity below a region.** The `.map` format exposes no per-face
  scalar channel; region-brush granularity is the authoring unit (see Task 3).

## Tasks

### Task 1: Coarser direction atlas

Bake the direction atlas at `dir_dims = irr_dims / DIRECTION_TEXEL_SCALE` per axis
(default `2`; `1` reproduces current output). The `LightmapSection` format already
carries independent `dir_width`/`dir_height`/`dir_texel_density`; today the encoder
sets them equal to irradiance. Reduce the composited direction (`Vec3` unit vectors)
and coverage buffers by the factor **inside the direction-encode step**, per layer
over each layer's own `atlas_width × atlas_height` plane — a `factor×factor` block
must never straddle a layer boundary (the same per-layer discipline `dilate` follows),
and the reduced direction keeps `layer_count` equal to irradiance (see pin P6). Within each block
sum the unit vectors in a fixed row-major order (the encode is serial, so the sum
order is stable; see pin P7) and renormalize (a slerp-reasonable dominant-direction reduction,
unlike octahedral lerp), OR coverage across the block. When the summed direction is
degenerate — `length_squared` below the composite's `1e-8` guard, a covered block whose
vectors cancel — fall back to the neutral up-vector the uncovered encode path emits,
never `normalize`-of-zero; the block stays covered but its direction is the
Lambert-valid fallback (see pin P5). Then octahedral-encode the reduced buffer and
write the reduced dims + density (`dir_texel_density = texel_density × DIRECTION_TEXEL_SCALE`). Do the reduction after the byte-identity composite seam: both the warm per-light
composite and the cold monolithic bake reach the same full-res `CompositedAtlas`, and
the reduction consumes its `direction`/`coverage` and produces reduced bytes only, never
resizing the `CompositedAtlas` fields. Both paths therefore encode identically off
full-res buffers, so the seam is unaffected (see pin P8). The factor must be a power of
two so it divides the pow2 atlas dims cleanly; expose it via a CLI flag defaulting to
`2` (mirror the hand-rolled `uncompressed_irradiance` flag wiring in `main.rs`) and fold
it into the **warm section cache key**
(`lightmap_layer::section_input_hash`), not only into `LightmapConfig`. The factor
must also be no larger than `MIN_ATLAS_DIMENSION` (64) so the smallest real atlas
still reduces to a ≥1 direction dimension — reject or clamp a larger factor rather
than emitting a zero-width direction atlas (see pin P10). `MIN_ATLAS_DIMENSION` is private
to `lightmap_bake.rs`; export it or use the literal 64 for the CLI-time clamp. The direction factor
changes only `encode_section`'s direction output, so it moves no per-light layer
hash, no chart dims, and no placement — nothing else in `section_input_hash` shifts.
Without this fold, a re-bake at a new factor hits the memoized section and serves the
stale-factor `.prl` (see pin P1). `LightmapConfig`'s serde derivation does not reach
the warm section key — that key is hand-folded — so adding the field there alone does
not re-key the warm path. Per-vertex lightmap UVs are unchanged —
they normalize to `[0,1]` and sample the coarser atlas at the correct texel through
the existing nearest sampler, so no geometry or runtime change is needed beyond the
atlas dimensions the loader already reads from the section header.

### Task 2: Drop unused direction channels (Rg8)

Verify first, then act: the static direction encoder writes `[qx, qy, 128, 255]` —
blue and alpha are constant — and the forward pass decodes the static atlas from
`.r`/`.g` only (recovering z as `1 − |x| − |y|`), never reading `.a`. If that holds
against current source, add a second direction-format tag (`…_OCT_RG8`) and an
`Rg8Unorm` encode path that writes only the two meaningful bytes per texel; the
parser already rejects unknown direction-format tags, so this needs no on-disk section
version bump. The static direction atlas always encodes `Rg8` once this lands — the
dropped channels are provably constant, so there is no toggle and no retained `Rgba8`
path. The on-disk section version stays unchanged (the `dir_format` tag registry handles
forward-compat), but bump `lightmap_layer::LIGHTMAP_SECTION_VERSION` — the compiler-internal
section-memo version passed as the `stage_version` of the `"lightmap_section"` `CacheKey`
in `pipeline.rs` — so stale `Rgba8`-era cached sections regenerate on the next bake instead
of serving cached bytes. That constant scopes exactly to `encode_section` / `to_bytes`
changes and is the sibling of `section_input_hash` in the section key, not a value folded
inside it. It is distinct from the on-disk `level-format` `lightmap::LIGHTMAP_SECTION_VERSION`
(id 22, still 2) and from `LAYER_FORMAT_VERSION` — the only version `section_input_hash`
actually folds, which covers the per-light layer payload; bumping that one would needlessly
re-bake every unchanged layer. The `Rg8` change touches only `encode_section`, so the
per-light layers are unchanged and only the section memo needs re-keying (see pin P12). Retain the tag for the runtime: add a stored `direction_format` field to
`LightmapSection` mirroring `irradiance_format` (today `dir_format` is written to the
header but validated-then-discarded on read, so nothing survives for the upload to branch
on). The runtime direction-texture upload then branches on the section's direction-format
tag to create the matching texture format; the BGL sample type —
`Float { filterable: false }` on the static direction binding in
`lighting/lightmap.rs::bind_group_layout_entries`, read through the nearest sampler
because linear interpolation of octahedral directions does not commute with slerp —
and the shared octahedral decode are unchanged (an `Rg8Unorm` sample yields
`(r, g, 0, 1)`, and the static path reads only `r`/`g`). Leave the
animated direction atlas as `Rgba8Unorm` — its `.a` is a coverage flag. This lever
stacks on Task 1 (coarser + channel-drop ≈ 8× off the direction half).

### Task 3: Per-surface lightmap density

Add a brush-entity scale region (a `@SolidClass` mirroring the existing region
brush entities) carrying a positive-float `_lightmap_scale` KVP. Parse it to a
world-space AABB + bounding planes exactly as the existing region brush entities
resolve, so world brushes stay world geometry (authors draw a scale box around a
region rather than converting surfaces to entities). Thread the parsed scale-region set into chart planning: add it to the parsed map data as a
new field mirroring `fog_volumes`, and give `prepare_atlas`/`plan_charts` it as an argument
(today they carry only the scalar `texel_density`). Both bake paths must receive the same
region set — `prepare_atlas` is called from the warm path and from the cold
`bake_lightmap_controlled`/`bake_lightmap` chain — or the warm-vs-cold byte-identity gate
(pin P8, AC below) breaks; passing an empty set from either path ships a silent divergence (see pin P13). In chart planning,
resolve each chart's effective density from the regions its origin falls inside, per the
precedence table below — a region's `_lightmap_scale` below 1 yields fewer chart texels (coarser), above 1 more (finer); density is m/texel, so effective density = `global_density ÷ _lightmap_scale`. Precedence: a scale region overrides the global default for the surfaces it covers. A
chart belongs to a region when its origin — `Chart.origin`, the face's first vertex `p0`,
not a centroid — lies inside the region AABB; a face straddling a region edge is decided
by that corner (see pin P9). Overlapping regions resolve by **last-defined wins**:

| Chart origin | Effective density |
|---|---|
| inside no scale region | global density (CLI/worldspawn/default) |
| inside exactly one region `R` | `global_density ÷ R._lightmap_scale` |
| inside two or more regions | `global_density ÷ _lightmap_scale` of the last region in parse entity-iteration order |

"Last" is the region's position in the parse entity-iteration order (`geo_map.entities`
order — the same order region brush entities accumulate, mirroring `fog_volume`); the
per-chart resolve iterates regions in that fixed order and the last containing region
wins. Because that order is a pure function of the source `.map`, re-baking picks the same
winner and the determinism gate holds (see pins P3, P4). Add the entity
to the FGD with the KVP and its default. Fold the resolved per-chart scale set into
the lightmap cache key so a region edit re-bakes. `atlas_layout_fingerprint` today
folds atlas dims and per-placement `x`/`y`/`layer` but **not** per-chart
`width_texels`/`height_texels` — safe pre-change because chart dims were a pure
function of geometry (`geometry_slice_hash`) and the single global density
(`lightmap_density`). A region scale is a third input that moves chart dims through
neither fold, so a chart that shrinks yet keeps its placement and atlas dims (a lone
chart on its own layer, floored to a 64² atlas at `(0,0)`) aliases to an unchanged key
and the warm path serves the stale-resolution section (see pin P2). Fold each chart's
resolved dims (or the per-chart scale) into `atlas_layout_fingerprint` — and thus into
`layer_input_hash`/`section_input_hash`: two region-definition orders that yield the
same per-chart dims must key the same, and any order that changes a chart's dims must
re-key (see pin P11). This changes chart sizing, map
parsing, and the lightmap cache key; it does not touch layer assignment, so the leaf-cohesion invariant
(all of a leaf's charts on one atlas layer) is unaffected — coarser charts simply
pack smaller.

**Why author-painted regions, not a measured classifier.** The coarsening target here is
author/design intent — decorative or distant geometry off the playing field — which is
gameplay-space knowledge, not a signal a measured error/contrast classifier can observe
in the lightmap. This differs from the SH track's deprecated manual loss-predictors:
`lighting-scale--variable-base-probe-density` retired surface-distance predictors for a
measured classifier and kept manual regions for protection only, because surface-distance
was a manual proxy for a measurable quantity. "The player never approaches this surface"
is not such a proxy, so a classifier cannot substitute for the author's region.

## Sequencing

**Phase 1 (concurrent):** Task 1 (coarser direction — direction-encode path) and
Task 3 (per-surface density — chart planning + parse). Distinct functions, but both
extend the warm cache-key path in `lightmap_layer.rs` — Task 1 folds the direction
factor into `section_input_hash`, Task 3 folds per-chart dims into
`atlas_layout_fingerprint` (which feeds `section_input_hash` transitively). The edits
land in different functions, but concurrent agents must reconcile both cache-key folds
in `lightmap_layer.rs` at integration.
**Phase 2 (sequential):** Task 2 (Rg8) — consumes Task 1's reshaped direction-encode
path and shares the runtime direction-texture format branch.

**Cross-plan (fine-density epic).** This plan is the foundation and lands before its
epic-partner `lighting-scale--lightmap-bake-incremental-flush`, which wraps a per-layer
bake-encode-drop loop around this plan's direction-encode reshaping in the same
`lightmap_bake.rs` path to bound peak bake RAM. Both plans' sequencing agrees on this
order; landing this plan first means incremental-flush's per-layer encode simply calls
the already-reshaped direction encoder instead of chasing a moving target.

## Acceptance criteria

- [ ] The coarsening lever alone drops the direction blob ~4×: on a lit fixture map,
  baking at direction factor 2 versus factor 1 (format held constant) drops the direction
  blob byte count ~4× (exact, since the dims divide cleanly), with the irradiance blob
  byte count unchanged.
- [ ] The shipped default (direction factor 2, `Rg8`) drops the direction blob ≈8×
  versus the pre-change bake (factor 1, `Rgba8`) — the coarsening ~4× and the channel-drop
  ~2× compounded.
- [ ] Bumped-Lambert highlight direction on a normal-mapped fixture surface shows no
  visible regression in a before/after A/B; low-frequency direction is preserved. (Manual
  visual gate — the low-frequency-preservation intent is the reviewable claim; the A/B is
  not a runnable test.)
- [ ] A map with a coarse scale region bakes fewer total lightmap bytes than the same
  map without the region; surfaces inside a region are coarser (fewer texels), and
  surfaces outside every region keep their sampled lightmap values (compared via
  per-vertex UV lookup, not blob bytes — global atlas repacking may shift an outside
  chart's placement and byte offset even when its lighting is unchanged).
- [ ] Per-surface precedence is observable: a surface inside a scale region bakes at
  effective density `global_density ÷ region_scale` (so `region_scale` below 1 is
  coarser); a surface outside every region uses the global density unchanged.
- [ ] A `.prl` carrying the new direction-format tag loads (a unit round-trip of the
  `Rg8` section through `to_bytes`/`from_bytes`) and renders direction correctly (the
  render check is a runtime/visual gate); a `.prl` with an unknown direction-format tag is
  rejected at load with a clear error (unit).
- [ ] Re-baking the same map twice yields byte-identical `.prl` output (determinism
  gate holds), including with a scale region present and with a non-default direction
  scale. The orderings this must survive are pinned in *Pinned behaviors*:
  overlapping-region precedence (P3, P4), the degenerate direction block (P5), the
  per-layer reduction and fixed sum order (P6, P7), and warm-vs-cold byte-identity (P8).
- [ ] A warm-cache re-bake after changing `_lightmap_scale` on a region, or after
  changing the direction-scale factor, reflects the new value rather than serving a stale
  cached atlas — both are folded into the lightmap cache key (see pins P1, P2, P11). The
  unconditional `Rgba8`→`Rg8` switch is not a fold but a compiler-internal cache-memo
  version bump: the first bake after the change regenerates any stale pre-change `Rgba8`
  cached section rather than serving it (see pin P12).
- [ ] The byte-identity gate between the warm per-light composite and the cold
  monolithic bake still passes — including with a scale region present, where the warm
  pipeline path and the cold `bake_lightmap_controlled` path feed the same region set
  into their respective `prepare_atlas` calls (see pins P8, P13).
- [ ] A normal (non-verbose) bake gains no new per-item log spam: any new per-surface
  or per-region footprint breakdown appears only under `-v`/`--verbose`, and the
  single-line atlas summary stays one line. (Review/grep gate — a negative-existence
  check, not a runnable test.)

## Pinned behaviors

Scenarios the bake must satisfy by construction, each testable. Domain ∈ {cache-key,
precedence, determinism, ordering, seam, edge}; Verify is the check kind (all `unit`).

| id | scenario | ordering | expected outcome | domain | verify |
|---|---|---|---|---|---|
| P1 | Warm bake at factor 2, then re-bake at factor 1, nothing else changed | encode_section reduces at factor 2 → section memoized under `section_input_hash`; factor→1; re-bake recomputes the same hash unless the factor is folded | section cache MISS, section re-encoded at factor 1; direction blob differs | cache-key | unit |
| P2 | Lone chart on its own leaf, `_lightmap_scale=0.5` shrinks 40×40 → 20×20, both floor to a 64² atlas at (0,0,0) | region added; chart dims halve; atlas dim and placement unchanged | warm cache MISS; coarser section baked, never a stale hit | cache-key | unit |
| P3 | Chart origin inside overlapping regions R_a (0.5) and R_b (0.25) | resolve iterates regions in parse-accumulation order; last containing region wins | density = `global ÷` later-defined `region_scale`; same across bakes | precedence | unit |
| P4 | Re-bake same `.map` with overlapping scale regions | entity iteration order fixed by source; winner recomputed | byte-identical `.prl` | determinism | unit |
| P5 | Covered `2×2` block, two `+X` and two `-X` unit vectors, sum ≈ 0 | reduce block → renormalize | fall back to the neutral up-vector, never `normalize`-of-zero/NaN; deterministic bytes | determinism | unit |
| P6 | Multi-layer atlas, a `factor×factor` block at a layer boundary | reduction over the layer-major buffer | block never spans two layers; reduced per layer over its own `w×h` plane; direction `layer_count` == irradiance | ordering | unit |
| P7 | Any covered block reduction | sum unit vectors within the block | fixed row-major serial sum order → byte-identical across runs | determinism | unit |
| P8 | Warm composite vs cold monolithic bake, non-default factor | both reach the full-res `CompositedAtlas`; reduction only inside `encode_section` | `CompositedAtlas` buffers stay full-res on both paths; seam equality holds; both encode reduced bytes identically | seam | unit |
| P9 | Face straddling a region edge, `p0` inside but centroid outside | membership test uses `Chart.origin` = `p0` | whole chart takes the region density iff `p0` is inside; deterministic | precedence | unit |
| P10 | CLI factor 128 on a map that bakes to a 64² atlas | `64 / 128` | factor rejected or clamped; never a zero-width direction atlas | edge | unit |
| P11 | Two region-definition orders: (a) same resolved per-chart dims, (b) different resolved dims | fold resolved dims into the cache key | (a) keys identical, no spurious re-bake; (b) keys differ, correct re-bake | cache-key | unit |
| P12 | Warm cache built before the `Rg8` change (`Rgba8` direction sections), then a re-bake after it lands, same factor/scale/density | the change edits only `encode_section`'s direction output — no `layer_input_hash`, `texel_density`, or `uncompressed_irradiance` moves, so `section_input_hash`'s non-version inputs are unchanged; only a bumped format version reaching the section `CacheKey` (the `lightmap_section` stage version) shifts the key | section cache MISS on the first post-change bake; stale `Rgba8` section regenerated as `Rg8`, never served | cache-key | unit |
| P13 | Warm pipeline path and cold `bake_lightmap_controlled` (`--no-cache`) each call `prepare_atlas`, a scale region present | one path gets the parsed region set, the other an empty set → `plan_charts` coarsens charts on one path only; dims/placements/atlas dims diverge upstream of the `CompositedAtlas` | both `prepare_atlas` call sites receive the identical region set; an empty/divergent set on either path is a defect — warm-vs-cold byte-identity (P8) fails by construction | seam | unit |

## Rough sketch

Format — `crates/level-format/src/lightmap.rs`: `LightmapSection` carries
`dir_width`/`dir_height`/`dir_texel_density` but no direction-format field. `dir_format`
is a header u32, hardcoded to `DIRECTION_FORMAT_OCT_RGBA8` on write and
validated-then-discarded on read. Add a `direction_format` field mirroring the existing
`irradiance_format`: `to_bytes` writes it, `from_bytes` stores it, and the runtime upload
branches on it. `DIRECTION_FORMAT_OCT_RGBA8 = 0`; the parser rejects unknown tags (the
tag exists so new encodings skip a version bump). `encode_direction_oct` returns
`[qx, qy, 128, 255]`.

Bake — `crates/level-compiler/src/lightmap_bake.rs`: `encode_section` sets the dir
dims equal to irr and calls `encode_direction_rgba8(&direction, &coverage)`; the
`CompositedAtlas { direction: Vec<Vec3>, coverage: Vec<bool> }` is the byte-identity
seam (both bake paths reach it, then encode). `plan_charts` computes
`width_texels = ceil(u_extent / density) + padding` — the per-chart density hook for
Task 3. `Chart.leaf_index` and the multi-bin packer keep leaf cohesion; Task 3 does
not touch either. `DEFAULT_TEXEL_DENSITY_METERS = 0.04`.

Density resolve — `crates/level-compiler/src/main.rs::resolve_lightmap_density`
(CLI > worldspawn `_lightmap_density` > default). Region brush entities parse to
world AABB + planes in `crates/level-compiler/src/parse.rs` (the existing
`fog_volume` resolve is the pattern to mirror); `FaceMeta` carries `leaf_index` +
`texture_index`.

Runtime — `crates/renderer/src/lighting/lightmap.rs` (direction-texture upload + BGL
binding 1) branches texture format on the section's direction-format tag;
`crates/renderer/src/shaders/forward.wgsl::decode_lightmap_direction` reads `.r`/`.g`
and the static direction sample never reads `.a`. The animated direction atlas
(forward binding 5) stays `Rgba8Unorm` for its `.a` coverage flag.

Logging discipline: any footprint summary is a one-line `log::info` (like the
per-section `pack.rs` lines and the existing lightmap summary); per-surface /
per-region breakdowns go behind the `--verbose` gate that already wraps
`lightmap_bake::log_stats`. Non-verbose bakes stay quiet.

## Boundary inventory

| Name | Rust | Wire / serde | FGD KVP |
|---|---|---|---|
| Per-surface scale | per-chart effective density in chart planning | n/a (affects chart dims, not a new section field) | `_lightmap_scale` (float, default 1.0) on a scale-region `@SolidClass` |
| Direction format tag | new `DIRECTION_FORMAT_OCT_RG8` const + `direction_format` section field (mirrors `irradiance_format`) | `dir_format` header u32 (new accepted value; no version bump) | n/a |
| Direction scale factor | `DIRECTION_TEXEL_SCALE` (pow2, default 2) | reflected in the header's `dir_width`/`dir_height`/`dir_texel_density` | optional CLI flag |

## Wire format

No new section and no version bump. Within the existing `LightmapSection` (id 22,
version 2) header: `dir_width`/`dir_height` now carry the reduced direction
dimensions (irr dims / scale), `dir_texel_density` carries the reduced density, and
`dir_format` gains one accepted value for the `Rg8` octahedral layout (direction blob
becomes `dir_width × dir_height × 2` bytes per layer, layer-major, versus `× 4`).
Unknown `dir_format` values keep rejecting as before.

Adding an encoding via the `dir_format` tag registry without a section version bump is a
witting divergence from the pattern the sibling SH-volume format changes set — each bumped
its own section's version rather than registering a tag: `octahedral-irradiance-atlas`
bumped `sh_volume::SH_VOLUME_VERSION` and `delta_sh_volumes::DELTA_SH_VOLUMES_VERSION`, and
`lighting-scale--variable-base-probe-density` bumped id 34 to v10 and id 35 to v3. Neither
touched the id-22 `LightmapSection`, so this is a cross-section precedent, not a prior
decision on this section. Here the `LightmapSection` parser already rejects unknown
`dir_format` tags — a designed-in affordance, per the `DIRECTION_FORMAT_OCT_RGBA8` doc
comment in `level-format/src/lightmap.rs` — so a new accepted tag is forward-safe without a
version gate, and all fixtures re-bake from source (no old-`.prl` migration).

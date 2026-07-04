# Static-Light Shadowmask Cache Addendum

Addendum to `context/plans/in-progress/static-light-shadowmask-world-receipt/`.

## Goal

Add build-stage cache support for `ShadowmaskAtlas` so warm `prl-build`
rebuilds do not rerun selected-light soft-visibility rays when inputs are
unchanged. Preserve current uncached output bytes and keep the addendum scoped
to compiler-side cache plumbing.

## Scope

### In scope

- Reuse existing per-light `"lightmap_layer"` cache entries as the raw
  visibility source for selected static entity-shadow lights.
- Add a whole-section `"shadowmask_atlas"` memo entry for encoded
  `ShadowmaskAtlasSection` bytes.
- Keep `--release` and `--no-cache` on the current recompute path.
- Add focused compiler tests for cache hit/miss, invalidation, corrupt-entry
  recovery, and no-cache bypass.
- Update the in-progress shadowmask plan or build-pipeline context only if the
  implementation lands before that plan is promoted or shipped.

### Out of scope

- Renderer changes.
- PRL wire-format changes.
- Animated static lights.
- A duplicate per-light raw-visibility cache.
- Broad refactors of `crates/level-compiler/src/main.rs` or
  `crates/level-compiler/src/lightmap_layer.rs`.

## Acceptance criteria

- [ ] A warm cached rebuild with unchanged inputs can serve
  `ShadowmaskAtlasSection` from a `"shadowmask_atlas"` entry and skips selected
  layer reads, selected layer bakes, channel assignment, quantization, and
  section encoding.
- [ ] On a `"shadowmask_atlas"` miss, selected-light layers are loaded from or
  written to existing `"lightmap_layer"` entries. Missing selected layers are
  baked once and stored under the existing `lightmap_layer::LAYER_FORMAT_VERSION`
  key.
- [ ] The cached warm output bytes for `ShadowmaskAtlasSection` match the
  uncached `shadowmask_bake::bake_shadowmask_atlas` output byte-for-byte for the
  same inputs.
- [ ] `--release` and `--no-cache` bypass all `"shadowmask_atlas"` reads/writes
  and produce the same section bytes as the current recompute path.
- [ ] Changing `EntityShadowLightsSection.light_indices` membership changes
  the `"shadowmask_atlas"` key; a focused helper test also pins the key as
  order-sensitive even though valid emitted sections are ascending today.
- [ ] Changing `--soft-shadow-samples` changes the selected layer input hashes
  and therefore changes the `"shadowmask_atlas"` key.
- [ ] Changing atlas dimensions, atlas layer placement, or selected-light atlas
  layout changes the `"shadowmask_atlas"` key.
- [ ] Channel assignment/drop policy, visibility quantization, and
  `ShadowmaskAtlasSection::to_bytes` serialization changes are covered by a
  `SHADOWMASK_ATLAS_STAGE_VERSION` bump or equivalent stage epoch.
- [ ] Corrupt `"shadowmask_atlas"` entries are treated as misses: the compiler
  logs a warning, rebuilds the section, and overwrites the cache entry.
- [ ] Tests verify the no-cache path writes no `"shadowmask_atlas"` entry and
  does not read existing entries.

## Tasks

### Task 1: Cacheable shadowmask stage

In `crates/level-compiler/src/shadowmask_bake.rs`, add a cache-facing API that
builds the same `ShadowmaskAtlasSection` bytes as `bake_shadowmask_atlas` while
accepting preloaded `LightmapLayer` values. Keep the existing uncached
`bake_shadowmask_atlas` path for `--release` and `--no-cache`. Add
`SHADOWMASK_ATLAS_STAGE_ID` with value `"shadowmask_atlas"` and
`SHADOWMASK_ATLAS_STAGE_VERSION`. Add a helper that computes the whole-section
input hash from `lightmap_layer::LAYER_FORMAT_VERSION`, selected
`EntityShadowLightsSection.light_indices` in order, selected
`lightmap_layer::layer_input_hash` values in the same order, atlas width,
height, and layer count. The stage version covers channel assignment/drop
policy, raw visibility quantization to `Rgba8Unorm`, and
`ShadowmaskAtlasSection::to_bytes` payload semantics.

### Task 2: Main compiler wiring and tests

In `crates/level-compiler/src/main.rs`, route the Shadowmask atlas stage through
`StageCache` only when `stage_cache` is present. On a whole-section hit, decode
`ShadowmaskAtlasSection::from_bytes` and return it. On a miss or corrupt entry,
resolve each selected `AlphaLightsNs` entry to its `MapLight`, compute that
light's `lightmap_layer::layer_input_hash` with the same `SharedAtlas`,
`final_lightmap_density`, `bvh_primitives`, `geo_result`, and
`args.soft_shadow_samples`, then load the existing `"lightmap_layer"` entry via
`lightmap_layer::LAYER_FORMAT_VERSION`. Bake and store only missing layers with
`lightmap_layer::bake_light_layer`. Build the section through the cache-facing
helpers in `shadowmask_bake.rs` and store the encoded `"shadowmask_atlas"`
entry. If `stage_cache` is `None`, call the existing
`shadowmask_bake::bake_shadowmask_atlas` path exactly as today. Co-locate tests
with `shadowmask_bake.rs` or the existing level-compiler cache tests, using
small synthetic layers and `StageCache` temp directories instead of cold map
compiles. Prove a whole-section hit skips selected layer work by seeding only a
valid `"shadowmask_atlas"` entry, omitting `"lightmap_layer"` entries, and
asserting the cached section is returned without rebake fallback.

## Sequencing

**Phase 1 (sequential):** Task 1 — defines the cache key and layer-fed section builder.
**Phase 2 (sequential):** Task 2 — consumes Task 1 helpers and wires cache policy into the compiler.

## Rough Sketch

Current Task 2 implementation bakes masks through
`shadowmask_bake::bake_shadowmask_atlas`. That function resolves
`EntityShadowLightsSection.light_indices` through `AlphaLightsNs`, then calls
`lightmap_layer::bake_light_layer` for every selected light. This reruns
selected-light soft-visibility rays even when the same light already has a
valid `"lightmap_layer"` cache entry.

The addendum should keep `lightmap_layer::LayerTexel.raw_visibility` as the
single per-light raw mask source. Do not add a second per-light cache for raw
visibility. The existing layer key already covers the light, influence-bounded
geometry slice, lightmap density, soft-shadow sample count, and atlas layout.

The whole-section key is a memo layer above selected light layers. It must be
order-sensitive because `ShadowmaskAtlasSection.channels` is indexed by
selection index, not by `AlphaLights` index. Include atlas dimensions and layer
count directly even though the layer hashes already include layout; this keeps
the section key self-describing and protects empty/degenerate selection paths.

Handle section cache corruption the same way as the existing lightmap-section
cache: decode failure is a warning plus a rebuild. Handle layer cache
corruption the same way as the existing lightmap-layer cache: `LightmapLayer`
decode failure is a miss and rebake for that selected light.

`main.rs` and `lightmap_layer.rs` are already large. Keep edits localized. If a
helper extraction is needed, place shadowmask-specific cache-key and
section-build logic in `shadowmask_bake.rs` rather than expanding unrelated
compiler orchestration.

## Boundary Inventory

| Name | Rust | Stage cache | PRL / wire | FGD KVP |
|---|---|---|---|---|
| Shadowmask atlas cache | `ShadowmaskAtlasSection` | `"shadowmask_atlas"` | Existing SectionId 42 bytes | n/a |
| Selected layer source | `LightmapLayer` | Existing `"lightmap_layer"` | Not shipped | n/a |

## Cache Format

No PRL wire format changes.

`"shadowmask_atlas"` payload is exactly
`ShadowmaskAtlasSection::to_bytes()`. `StageCache` wraps it in the standard
cache entry envelope: payload length and payload hash, filename derived from
`CacheKey::new("shadowmask_atlas", SHADOWMASK_ATLAS_STAGE_VERSION, input_hash)`.

`input_hash` is a fixed-order byte hash:

- `lightmap_layer::LAYER_FORMAT_VERSION` as little-endian `u32`.
- Selected-light count as little-endian `u32`.
- Each selected `AlphaLights` index from
  `EntityShadowLightsSection.light_indices`, in the supplied selection order.
  Current emitted sections are ascending, but the helper remains
  order-sensitive because `ShadowmaskAtlasSection.channels` is indexed by
  selection position.
- Each selected `lightmap_layer::layer_input_hash`, in the same order.
- Atlas width, height, and layer count as little-endian `u32`.

`SHADOWMASK_ATLAS_STAGE_VERSION` must bump when section bytes can change without
changing those inputs: channel coloring/drop order, dropped-light tie-breaks,
raw visibility quantization, empty-section behavior, or
`ShadowmaskAtlasSection::to_bytes` semantics.

## Open Questions

None.

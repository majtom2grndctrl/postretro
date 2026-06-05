---
name: Warm Lightmap Section Cache
description: Make a warm (cache-hit) lightmap build cheap. The per-light layer cache already hits on unchanged maps, but the warm path still spends ~20s decoding 9×478 MB layer blobs and re-running composite/dilate/BC6H every build. Add a second-level cache of the composited LightmapSection so a no-edit rebuild does one ~83 MB section decode and skips the layer reads, composite, and encode entirely. Compiler-internal; runtime and PRL format untouched.
type: plan
---

# Warm Lightmap Section Cache

> Builds on the shipped `incremental-bake-per-element/` per-light layer cache. That
> plan made the *bake* skip on a cache hit; this plan makes the *warm hit itself*
> cheap. Same `StageCache`/`CacheKey` substrate, same warm/cold contract.

## Goal

Cut the warm (full-cache-hit) lightmap stage from ~20s to ~1s on `campaign-test`.
The per-light layer cache already hits — nothing re-bakes — but on every warm
build the stage still reads and postcard-decodes nine ~478 MB layer blobs
(≈4.2 GB, ~3.2s I/O + ~11.6s decode) and re-runs composite + dilate + BC6H
encode (~5s), all to reproduce a section it produced byte-for-byte last build.
Memoize the composited `Lightmap` section so an unchanged map skips that work.

## Background — why the warm hit is slow

The lightmap layer is **dense by design**: each light's layer enumerates every
covered atlas texel (`LayerTexel` = 40 bytes) so the compositor can reproduce
coverage and the dark-texel fallback normal exactly (the byte-identity gate). At
a 4096² atlas that is ~478 MB per light, nine lights, postcard-encoded. A warm
build pays the full decode + composite + encode even though every layer hit.
Sub-stage timing on a 100%-warm `campaign-test` build:

| Sub-stage | Warm cost |
|---|---|
| prepare atlas | 0.0s |
| layer loop (9× disk read + postcard decode) | ~14.8s |
| composite | 2.7s |
| dilate | 1.1s |
| BC6H encode | 1.2s |

The decode + I/O of the cached layers is the bulk. None of it is a re-bake.

## Approach — three candidates, one chosen

| | Approach | No-edit win | Edit-path win | Risk to byte-identity gate | Cost |
|---|---|---|---|---|---|
| **(b) ✓** | Cache the composited `LightmapSection`, keyed on the ordered set of layer keys | **full** (~20s → ~1s) | none | none — memoizes already-proven output | one new stage id |
| (c) | Zero-copy `bytemuck` layer codec instead of postcard | partial (decode only) | yes | low | `LAYER_FORMAT_VERSION` bump → one cold rebuild |
| (a) | Value-sparse layers + shared coverage/normal side-table | partial | yes | **high** — rewrites the proven compositor coverage/fallback logic | most invasive |

**Chosen: (b) as the core, (c) as a complementary follow-up.** (b) alone solves
the reported symptom (the no-edit rebuild) and is the safest — it caches the
output of a pipeline whose correctness the existing determinism gate already
proves, so it cannot perturb byte-identity. (c) stacks on top to make the
*single-light-edit* recompose path fast (that path still reads the layer blobs).
(a) is deferred: it attacks layer size but rewrites the compositor's coverage and
dark-texel-fallback branches — the exact logic the byte-identity gate guards — for
a win (c) gets more cheaply. See Out of scope.

## Scope

### In scope

- **Composited-section cache (b).** A new `"lightmap_section"` stage id on the
  existing `StageCache`. Payload is the encoded `LightmapSection` (post-BC6H,
  ~83 MB) via its existing `to_bytes`/`from_bytes`. Key folds the ordered
  per-light layer keys (so any light/geometry/atlas change invalidates it),
  `texel_density`, the `uncompressed_irradiance` flag, and a section-cache
  version constant.
- **Warm-path short-circuit.** On a section-cache hit, skip the per-light layer
  loop, composite, dilate, and encode entirely; decode the cached section and
  use it. The shared atlas prep still runs (it is ~0s and downstream
  animated-light passes need its `charts`/`placements`/dims).
- **Zero-copy layer codec (c).** Replace the postcard `LightmapLayer` round-trip
  with a fixed-layout `bytemuck` POD encoding so the edit-path recompose decodes
  by bulk cast/copy, not field-by-field parse. Bumps `LAYER_FORMAT_VERSION`.
- Corruption handling for the section entry mirrors `StageCache::get` and the
  layer codec: a length/hash failure or a `from_bytes` decode failure is a miss
  (warn, recompose), so the build always succeeds.

### Out of scope

- **Value-sparse layers + shared side-table (approach a).** Considered; deferred.
  It rewrites the compositor's coverage/fallback-normal reconstruction (the
  byte-identity-gated logic) for a benefit (c) achieves without touching it.
  Revisit only if disk footprint — not warm time — becomes the pain.
- **Cold / `--release` / `--no-cache` path.** Untouched. The exact monolithic
  bake neither reads nor writes any cache, including the section cache.
- **PRL format and runtime.** No new shipped section. The section cache reuses
  the existing `Lightmap` (id 22) `to_bytes` format as an opaque cache blob.
- **SH, animated weight-map, SDF stages.** Unchanged.
- **Atlas repacking or layer-grain changes.** The per-light grain and atlas
  layout stay as the shipped plan left them.

## Acceptance criteria

- [ ] Building `campaign-test` twice with no change: the second build logs a
      `lightmap_section` cache hit, reads no per-light layer blob, runs no
      composite/dilate/encode, and emits a `.prl` byte-identical to the first.
- [ ] The warm lightmap stage on that second build completes in ≲2s (down from
      ~20s); wall-clock for both builds is recorded.
- [ ] Editing one light: the `lightmap_section` entry misses and recomposes; the
      unchanged lights' layers still hit (only the edited light's layer re-bakes);
      the resulting `.prl` is byte-identical to a cold `--no-cache` build of the
      same edited map (the byte-identity gate still holds end to end).
- [ ] A corrupt or missing `lightmap_section` entry is detected, discarded with a
      warning, and recomposed from the layers; the build succeeds.
- [ ] `--no-cache` / `--release` neither read nor write the `lightmap_section`
      entry and produce output identical to the monolithic bake.
- [ ] `--cache-dir <PATH>` places the `lightmap_section` entry under the override.
- [ ] After the codec change (c), all `lightmap_layer` entries from a prior
      `LAYER_FORMAT_VERSION` are treated as misses (one cold rebuild), and the
      existing `composite_matches_monolithic_atlas_bit_for_bit` and layer
      round-trip tests pass against the new codec.

## Tasks

### Task 1: Composited-section cache (approach b)

Add a `"lightmap_section"` stage id with its own `u32` version constant
(alongside `LAYER_FORMAT_VERSION` in `lightmap_layer.rs`). In the warm branch of
`main.rs` (the `if let Some(ref cache) = stage_cache` lightmap block, ~`:340`):
after preparing the shared atlas and computing each light's `layer_input_hash`,
assemble the section key, then `cache.get` it. On hit, decode the cached
`LightmapSection` and build `LightmapBakeOutput` from it plus the already-prepared
`charts`/`placements`/dims. On miss, run the existing per-light get/bake/put loop,
composite, dilate, encode, then `cache.put` the encoded section's `to_bytes`.

The section key must change whenever any input that determines the section bytes
changes. Fold, under a fixed layout: the ordered per-light layer keys (each
already covers that light's params, influence-bounded geometry slice, density,
sample count, and atlas layout — and includes `LAYER_FORMAT_VERSION`), the
`texel_density` passed to `encode_section`, and the `uncompressed_irradiance`
flag (it selects BC6H vs RGBA16F output). Exact byte layout is the implementer's
choice; the constraint is total coverage of section-determining inputs.

### Task 2: Zero-copy layer codec (approach c)

Make `LightmapLayer::{to_bytes,from_bytes}` a fixed-layout `bytemuck` POD
encoding (small header: `atlas_width`, `atlas_height`, texel count; then the
`[LayerTexel]` block) instead of postcard. `LayerTexel` is all `u32`/`f32`, so it
is `Pod + Zeroable` under `#[repr(C)]`. Decode must be a bulk cast or single
copy, not a per-struct parse; account for `Vec<u8>` alignment (use a
bulk-copy-into-`Vec<LayerTexel>` if a borrow-cast would violate alignment — still
far cheaper than postcard's 12M-struct decode). Bump `LAYER_FORMAT_VERSION` (the
format change invalidates all cached layers — one cold rebuild). The codec must
still round-trip exactly so the composite stays byte-identical to the monolithic
bake.

### Task 3: Tests + timing

Section-cache tests mirroring the layer suite in `lightmap_layer.rs`: round-trip
skip (build twice → section hit, no layer read), single-light edit (section miss +
recompose, unchanged layers hit), corruption recovery (garbage section entry →
miss → recompose), `--no-cache` bypass, `--cache-dir` redirect. Keep the existing
`composite_matches_monolithic_atlas_bit_for_bit` and layer round-trip tests green
against the new codec. Record warm-vs-warm wall-clock on `campaign-test` (the
~20s → ≲2s evidence) and confirm a single-light-edit warm rebuild is byte-identical
to a cold `--no-cache` build of the same edited map.

## Sequencing

**Phase 1 (sequential):** Task 1 — the section cache; delivers the headline win.
**Phase 2 (sequential):** Task 2 — the codec change accelerates the edit-path
recompose Task 1 falls back to; independent of Task 1's logic but bumps the layer
format, so land it after Task 1 to avoid two cache-invalidating churns at once.
**Phase 3 (sequential):** Task 3 — tests and timing validate both.

## Rough sketch

Warm-path control flow in `main.rs` (lightmap block, replacing the straight
layer-loop → composite → encode):

```
prepare shared atlas (cheap, always)            // charts/placements/dims for downstream
compute layer_input_hash per light              // cheap; no blob reads
section_key = CacheKey::new("lightmap_section", LIGHTMAP_SECTION_VERSION,
                           fold(ordered layer keys, density, uncompressed_flag))
match cache.get(section_key).and_then(LightmapSection::from_bytes ok):
  Some(section) -> HIT:  use section directly        // skip layer reads + composite + dilate + encode
  None          -> MISS: existing per-light get/bake/put loop
                         -> composite -> dilate -> encode_section
                         -> cache.put(section_key, section.to_bytes())
```

Key types/fns (grounded): `LightmapSection::{to_bytes -> Vec<u8>, from_bytes ->
crate::Result<Self>}` (`level-format/src/lightmap.rs:172,201`);
`CompositedAtlas::encode_section(density, uncompressed) -> LightmapSection`
(`lightmap_bake.rs:172`); `cache::CacheKey::new` / `StageCache::{get,put}`
(`cache.rs:27,62,114`); `LAYER_FORMAT_VERSION` and the layer loop
(`lightmap_layer.rs:23`, `main.rs:383-431`).

**Layer cache blob (Task 2)** is compiler-internal, not a PRL section: fixed
little-endian header (`atlas_width: u32`, `atlas_height: u32`, `count: u32`)
followed by `count` × `LayerTexel` POD records. Not versioned in-blob — the
`"lightmap_layer"` stage version gates it, as today.

## Open questions

- **Section version vs. layer version coupling.** Folding the per-light layer
  keys into the section key already chains in `LAYER_FORMAT_VERSION`, so a layer
  bump auto-invalidates sections. The separate `LIGHTMAP_SECTION_VERSION` then
  only needs bumping when the section *encode* or `to_bytes` format changes.
  Confirm this is the intended split during Task 1, or fold both versions
  explicitly for belt-and-suspenders.
- **Section blob size on bigger atlases.** ~83 MB at 4096² with BC6H; an
  uncompressed (`--debug`/RGBA16F) section is larger. The flat-file `StageCache`
  handles it (it already stores 478 MB layer blobs), but note peak memory on the
  decode for very large atlases.
- **Whether to keep (c) at all** if Task 1 measurements show the single-light-edit
  recompose is already acceptable once the no-edit case is cached. Decide after
  Task 1's timing.

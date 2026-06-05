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
a 4096² atlas that is ~478 MB per light, nine lights, postcard-encoded. ~478 MB
is both the in-memory dense size and roughly the on-disk postcard size (40-byte
POD records compress to near-nothing), so postcard's cost is decode CPU, not
blob size — which is why Task 2 (zero-copy) targets decode time, not footprint.
A warm build pays the full decode + composite + encode even though every layer
hit.
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
  `layer_input_hash` values for the `layer_lights` set in its exact filtered
  order (global static order, `ShadowType::Sdf` dropped) plus
  `LAYER_FORMAT_VERSION` explicitly (the per-light `CacheKey` digest is
  private), `texel_density`, `uncompressed_irradiance`, and a section-cache
  version constant. Any light/geometry/atlas change or layer-format bump
  invalidates the section.
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
- **`--cache-dir` / `--no-cache` by construction.** The section entry uses the
  same `stage_cache` instance as the layers, so both flags apply to it without
  separate path handling.

### Out of scope

- **Value-sparse layers + shared side-table (approach a).** Considered; deferred.
  It rewrites the compositor's coverage/fallback-normal reconstruction (the
  byte-identity-gated logic) for a benefit (c) achieves without touching it.
  Revisit only if disk footprint — not warm time — becomes the pain.
- **Cold / `--release` / `--no-cache` path.** Untouched. The exact monolithic
  bake neither reads nor writes any cache, including the section cache.
- **PRL format and runtime.** No new shipped section. `LightmapSection` is the
  in-memory type whose `to_bytes` IS the shipped `Lightmap` (id 22) section
  serialization. The cache stores that same bytes as an opaque blob — no new
  format, no second serializer; `LightmapSection::to_bytes` is called once on
  the miss path and the result is stored verbatim.
- **SH, animated weight-map, SDF stages.** Unchanged.
- **Atlas repacking or layer-grain changes.** The per-light grain and atlas
  layout stay as the shipped plan left them.

## Acceptance criteria

- [ ] Building `campaign-test` twice with no change: the second build logs a
      `lightmap_section` cache hit, reads no per-light layer blob, and runs no
      composite/dilate/encode. Smoke-test: the emitted `.prl` is byte-identical to
      the first (expected — the second build replays the bytes the first wrote; the
      real correctness gate is AC#3 below).
- [ ] The warm lightmap stage on that second build completes in ≲1s. The hit path
      is an ~83 MB read + blake3 + memcpy with no BC6H decode, so a stray
      re-encode (~1.2s) must not be able to hide under the budget. Record three
      wall-clock numbers: cache-disabled warm baseline (old path), first no-change
      build (miss + put), second no-change build (hit). If the hit-stage time is
      not dominated by the section read, suspect a path that still composites or
      encodes.
- [ ] **Primary correctness criterion.** Editing one light: the `lightmap_section`
      entry misses and recomposes; the unchanged lights' layers still hit (only the
      edited light's layer re-bakes); the resulting `.prl` is byte-identical to a
      cold `--no-cache` build of the same edited map (composite path vs. monolithic
      path — genuinely different code, so this is the real byte-identity gate); and
      rebuilding again with no further change is a warm section-cache hit (the
      recomposed section was `put` and is immediately reusable).
- [ ] A corrupt or missing `lightmap_section` entry is detected, discarded with a
      warning, and recomposed from the layers; the build succeeds.
- [ ] `--no-cache` / `--release` neither read nor write the `lightmap_section`
      entry and produce output identical to the monolithic bake.
- [ ] `--cache-dir <PATH>` places the `lightmap_section` entry under the override.
- [ ] After the codec change (c), a warm rebuild logs all `lightmap_layer` entries
      as misses; the cache dir gains new-keyed files while the old-keyed files
      remain untouched (orphaned, never read). The existing
      `composite_matches_monolithic_atlas_bit_for_bit` and layer round-trip tests
      pass against the new codec.
- [ ] Adding a light, removing a light, or reordering the light set each forces a
      `lightmap_section` miss on the next build. Verifiable: make each change, rebuild,
      confirm `RUST_LOG=info` logs a `lightmap_section` miss (not a hit).
- [ ] Changing `--soft-shadow-samples` forces a `lightmap_section` miss. The
      sample count folds into every `layer_input_hash` (as `area_sample_count`),
      hence into the section key. Verifiable: rebuild with a different value,
      observe a `lightmap_section` miss in `RUST_LOG=info` output.

## Tasks

### Task 1: Composited-section cache (approach b)

Add a `"lightmap_section"` stage id with its own `u32` version constant
(alongside `LAYER_FORMAT_VERSION` in `lightmap_layer.rs`). In the warm branch of
`main.rs` (the `if let Some(ref cache) = stage_cache` lightmap block, ~`:340`):
after preparing the shared atlas and computing each light's `layer_input_hash`,
assemble the section key, then `cache.get` it. Both branches produce the same
`LightmapBakeOutput { section, charts, placements, atlas_width, atlas_height }`
(confirmed at `main.rs:432`): `charts`, `placements`, `atlas_width`, and
`atlas_height` always come from the already-prepared `prepared` atlas regardless
of branch; only `section` differs. On hit, decode the cached `LightmapSection`
as `section`. On miss, run the existing per-light get/bake/put loop, composite,
dilate, encode to produce `section`, then `cache.put` the encoded section's
`to_bytes`.

The section key must change whenever any input that determines the section bytes
changes. Fold, under a fixed layout: the ordered `layer_input_hash` `[u8;32]`
values for the `layer_lights` set in its exact filtered order (global static
order, `ShadowType::Sdf` dropped — matching the warm branch's `layer_lights`
construction at `main.rs:380`); `LAYER_FORMAT_VERSION` folded explicitly (the
per-light `CacheKey.digest` is private and cannot be read — folding the input
hashes plus the version constant gives the same invalidation coupling the full
keys would give); `texel_density` passed to `encode_section`; and
`uncompressed_irradiance` (it selects BC6H vs RGBA16F output). The fold must
be unambiguous about light-set boundaries: fixed-width 32-byte `layer_input_hash`
records make a plain ordered concatenation injective, but fold the light count
too so add/remove cannot alias a reorder. Exact byte layout is the implementer's
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

Prerequisites (confirmed against source): `bytemuck` is not currently a
dependency of `crates/level-compiler` — add it. `LayerTexel` has no `#[repr(C)]`
today — add it before deriving `Pod`. The `serde::Serialize/Deserialize` derives
on `LayerTexel` can be dropped once postcard is removed (nothing else serializes
`LayerTexel` — `layer_input_hash` postcards `MapLight`, not `LayerTexel`).
`LightmapLayer`'s own `serde::Serialize/Deserialize` derives also become dead at
that point and should be dropped alongside `LayerTexel`'s.

### Task 3: Tests + timing

Section-cache tests mirroring the layer suite in `lightmap_layer.rs`: round-trip
skip (build twice → section hit, no layer read), single-light edit (section miss +
recompose, unchanged layers hit), corruption recovery (garbage section entry →
miss → recompose), `--no-cache` bypass, `--cache-dir` redirect, add/remove/reorder-light
invalidation (each forces a section miss), and a `--soft-shadow-samples` change
forcing a section miss. Keep the existing `composite_matches_monolithic_atlas_bit_for_bit`
and layer round-trip tests green against the new codec. Record warm-vs-warm
wall-clock on `campaign-test` (the ~20s → ≲1s evidence) and confirm a
single-light-edit warm rebuild is byte-identical to a cold `--no-cache` build of
the same edited map.

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
                           fold(ordered layer_input_hash values + LAYER_FORMAT_VERSION,
                                texel_density, uncompressed_irradiance))
match cache.get(section_key).and_then(LightmapSection::from_bytes ok):
  Some(section) -> HIT:  use section directly        // skip layer reads + composite + dilate + encode
  None          -> MISS: existing per-light get/bake/put loop
                         -> composite -> dilate -> encode_section
                         -> cache.put(section_key, section.to_bytes())
```

Key types/fns (grounded): `LightmapSection::{to_bytes -> Vec<u8>, from_bytes ->
crate::Result<Self>}` (`level-format/src/lightmap.rs:172,201`);
`CompositedAtlas::encode_section(texel_density, uncompressed_irradiance) -> LightmapSection`
(`lightmap_bake.rs:172`); `cache::CacheKey::new` / `StageCache::{get,put}`
(`cache.rs:27,62,114`); `LAYER_FORMAT_VERSION` and the layer loop
(`lightmap_layer.rs:23`, `main.rs:383-431`).

**Layer cache blob (Task 2)** is compiler-internal, not a PRL section: fixed
native-endian header (`atlas_width: u32`, `atlas_height: u32`, `count: u32`)
followed by `count` × `LayerTexel` POD records — native-endian throughout
(header + POD records); the cache is dev-local and makes no cross-arch
portability guarantee, matching the existing layer cache. Not versioned in-blob
— the `"lightmap_layer"` stage version gates it, as today.

## Open questions

- **Section version vs. layer version coupling.** Resolved. The section key
  folds the `layer_input_hash` values plus `LAYER_FORMAT_VERSION` explicitly
  (the per-light `CacheKey` digest is private). A layer-format bump therefore
  auto-invalidates all section entries. `LIGHTMAP_SECTION_VERSION` bumps only
  when the section encode or `LightmapSection::to_bytes` format changes — the
  two version constants cover disjoint concerns.
- **Section blob size on bigger atlases.** ~83 MB at 4096² with BC6H; an
  uncompressed (`--debug`/RGBA16F) section is larger. The flat-file `StageCache`
  handles it (it already stores 478 MB layer blobs), but note peak memory on the
  decode for very large atlases.
- **Whether to keep (c) at all** if Task 1 measurements show the single-light-edit
  recompose is already acceptable once the no-edit case is cached. Decide after
  Task 1's timing.

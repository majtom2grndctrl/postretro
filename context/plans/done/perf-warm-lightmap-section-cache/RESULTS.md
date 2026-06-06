# Warm Lightmap Section Cache — Results (Task 3)

Timing and byte-identity evidence for the second-level `lightmap_section` cache.
All numbers are the compiler's own per-stage "Lightmap Bake" row (release build,
`RUST_LOG=info`). Measured 2026-06-06.

## Warm-vs-warm timing (`content/dev/maps/campaign-test.map`)

30 map lights / 13 static-baked lights; `.prl` lightmap section ~83 MB.

| Build | Cache flag | Lightmap Bake stage | Notes |
|-------|-----------|---------------------|-------|
| Cache-disabled baseline | `--no-cache` | **40.41 s** | Monolithic cold path (ship source of truth) |
| First no-change (section **miss** + put) | `--cache-dir <fresh>` | 113.91 s | Per-light layer bake + composite + dilate + encode + blob writes |
| Second no-change (section **hit**) | `--cache-dir <warm>` | **0.15 s** | One section decode; target was ≲1 s |

Warm hit speedup vs cache-disabled baseline: **40.41 s → 0.15 s** (~270×).

### Hit-path log evidence (second build)

The warm hit build logged exactly:

```
[cache] lightmap_section hit
```

and **zero** `lightmap_layer` lines — confirming the hit path reads no per-light
layer blob and runs no composite / dilate / encode. The 0.15 s is dominated by
the section read (~83 MB read + blake3 validation + memcpy decode); no stray
re-encode (~1.2 s) hides in it (the stage is well under the 1 s read budget, and
the plan's "must not composite or encode" sanity check holds — no encode-time
appears in the stage number).

The two no-change warm builds (`warm1.prl`, section miss; `warm2.prl`, section
hit) are **byte-identical** (`cmp` clean), so serving from the section cache does
not perturb output.

> Note: the first-warm (miss) build is *slower* than the cache-disabled cold
> build because the warm path bakes each light's layer individually, composites,
> and writes layer + section blobs — that one-time cost is the price of the warm
> hit. The win is the second build and every subsequent no-edit rebuild.

## Edit-path byte-identity (AC#3, primary correctness gate)

CLI runs on `content/dev/maps/soft_shadow_test.map` (small fixture; 6 static
lights; faster than campaign-test). The warm recompose path (per-light layer
composite) is genuinely different code from the cold `--no-cache` monolithic
bake, so byte-identity proves the composite reproduces the monolithic atlas.

| Check | Result |
|-------|--------|
| No-edit: warm recompose (section miss → 6 layer bakes → composite → encode) vs cold `--no-cache` | **byte-identical** (`cmp` clean) |
| No-edit: second build is a section **hit**, output vs warm-miss output | **byte-identical**; logged `lightmap_section hit` |
| Single-light edit (one `_color` changed): warm rebuild vs cold `--no-cache` of the same edited map | **byte-identical** (`cmp` clean) |
| Edit invalidation coupling (edited warm build) | `lightmap_section miss` + **1** `lightmap_layer miss` (edited light) + **5** `lightmap_layer hit` (unchanged lights) |
| Sanity: edited output differs from unedited output | confirmed differs (the edit took effect) |

The single-light edit re-bakes only the edited light's layer; the other five
hit, the section recomposes, and the resulting `.prl` matches the cold monolithic
build bit-for-bit. Edited-map lightmap stage: 2.40 s warm (5/6 layers cached) vs
11.70 s cold.

## Unit-test evidence

The section-cache behaviors are also pinned by seven unit tests in
`crates/level-compiler/src/lightmap_layer.rs` `mod tests` (mirroring the layer
suite). See `section_cache_round_trip_skips_recompose`,
`single_light_edit_changes_section_key_but_not_unedited_layer_key`,
`corrupt_section_entry_is_discarded_and_recomposed`,
`cache_dir_override_places_section_entry_under_override`,
`no_cache_path_writes_no_section_entry`,
`section_key_changes_on_add_remove_and_reorder`, and
`section_key_changes_when_soft_shadow_samples_change`. All green.

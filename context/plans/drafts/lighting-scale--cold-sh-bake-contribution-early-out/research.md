# Research — SH bounce probe-snap reuse and exact-zero guard

Measurements behind the brief. Read at `c855e3e`; 4-core / 15 GiB cloud container; release
`prl-build`; ship config (`--release --no-tui`, map defaults).

## Where the bake time went (before)

Cold ship bakes, per-stage times from the compiler's own Build Summary:

| stage | `campaign-test` | `stress-warren-mini` |
|---|---|---|
| SH Bake | 18.93 s | **721.32 s** |
| Delta SH Bake | 5.17 s | 82.92 s |
| Lightmap Bake | 6.91 s | 185.26 s |
| AnimWeightMaps | 2.49 s | 16.44 s |
| Total | 179.49 s¹ | 1051.53 s |

¹ Includes 124.8 s of DataScript and 10.1 s of TexValidation that were first-run costs on a
fresh checkout (`find_scripts_build` rebuilt `scripts-build` via cargo). Both are ~0 s on the
next run; ignore them.

The SH bounce bake dominates once a map is larger than the small fixtures.

## SH stage profile (before)

`perf record` on a symbolized release build (`strip = none`, line tables), 60 s of the
`stress-warren-mini` SH stage, DWARF call graphs. Inclusive share of the stage:

| work | share |
|---|---|
| `SoftProbes::new` (probe-index re-snap per hit × light) | **46.1 %** |
| `segment_clear` (shadow rays; BVH `intersects_aabb` 29.8 % self) | 40.2 % |
| `area_sample_target` | 3.2 % |
| `closest_hit` (the bounce ray itself) | 1.2 % |

`sincosf` alone was 8.5 % self, most of it under the re-snap. The re-snap is a pure function
of the light and the sample count — for point/spot lights, of the count alone.

## After

Same machine, same commands. Output SHA-256 identical to the `main` build:

| map | `.prl` SHA-256 (before = after) |
|---|---|
| `campaign-test` (cold) | `e9a2ae6992180aaf3dff90448e205d2646d1eab8ea929b7534c053adf3787d29` |
| `stress-warren-mini` (cold) | `e43943319b7efa7ee961dd035f71502e767d8e9d54865f8dba28ce811db5bbce` |

| stage | `campaign-test` before → after | `stress-warren-mini` before → after |
|---|---|---|
| SH Bake | 18.93 → 9.43 s | **721.32 → 251.60 s (2.87×)** |
| Delta SH Bake | 5.17 → 2.15 s | 82.92 → 22.15 s |
| Direct SH Bake | 0.42 → 0.34 s | 4.12 → 2.32 s |
| Direct SH Delta Bake | 0.11 → 0.09 s | 6.03 → 4.56 s |
| Billboard Direct Scatter | 0.35 → 0.24 s | 3.86 → 2.14 s |
| AnimWeightMaps | 2.49 → 1.40 s | 16.44 → 7.76 s |
| Lightmap Bake | 6.91 → 6.66 s | 185.26 → 187.26 s (already hoists per chart) |
| Total | ~44.6² → 24.91 s | **1051.53 → 502.83 s (−52 %)** |

² Before total less the first-run DataScript/TexValidation costs.

The SH stage fell further (−65 %) than the re-snap's 46 % share: the exact-zero guard also
skips the shadow rays — and their probe work — for lights aimed away or behind the surface.
The two were not timed separately.

Single-run wall-clock on a shared container; treat second-level differences in small stages
as noise.

## Warm builds and the cache

`campaign-test`, default (warm) mode. Three builds, one SHA-256
(`3c78f081bdf86f00de89f18a3e3d8086553ed11a9b0eaed5f2f87ab8e7e2debf`): `main` into a fresh
cache, this change into a fresh cache, and this change reading `main`'s cache. Cached entries
are byte-compatible across the change, so no stage epoch moves. Warm fresh-cache total:
39.80 → 24.36 s (SH 18.35 → 8.30 s, delta SH 5.58 → 2.20 s).

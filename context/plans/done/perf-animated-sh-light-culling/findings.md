# Task 5 Findings — Animated SH Light Culling

Experiment run of `campaign-test` end-to-end on macOS/Metal, adapter **AMD Radeon
Pro 5300M**. Baseline = checked-in v1 (dense) `content/dev/maps/campaign-test.prl`;
new artifact = `/tmp/out.prl` (v2 sparse, affinity-cell CSR).

This is the experiment's findings note. The quality/perf numbers are measured
findings feeding the ship/optimize decision, not contracts. The correctness ACs
are gates and all pass.

## AC results

| AC | Result | Measured |
|---|---|---|
| #1 Bake succeeds; sec 27 smaller | **PASS** | DeltaShVolumes section: **88.36 MiB → 37.54 MiB** (−50.82 MiB / **−57.5%**). Whole `.prl`: 644.1 → 591.9 MiB. |
| #2 Out-of-region drop | **PASS** | 10 of 21 lights drop. Lights 0/3/4: **75,264 emitted vs 166,375 full-AABB** (−55%); light 5: 80,640 vs 166,375. (Small lights can round up at the cell boundary, e.g. light 2: 11,520 vs 6,859 — expected coarse-grid behavior; AC needs only one drop.) |
| #3 Clean boot, no validation errors | **PASS** | `first_level_frame=19.4ms`, base grid 74×23×114 + 21 delta lights, zero wgpu `validation` lines. |
| #4 Footprint vs 128 MiB target | **PASS (recorded)** | delta_subblocks 37.49 MiB + affinity_offsets 13,228 B + affinity_lights 43,872 B = **total 37.54 MiB (39,366,412 B)** — **3.6× under** the 134,217,728 B floor. GPU footprint == on-disk (f16 stays f16 through to the shader). |
| #5 Loop bound = affinity-cell list length | **PASS** | `sh_compose.wgsl`: `start = affinity_offsets[cell]; end = affinity_offsets[cell+1]; for i in start..end`. `delta_light_count` removed; that uniform slot is now `delta_scale: f32`. |
| #6 Version reject path | **PASS** | Loading the v1 baseline with the new engine logs `delta sh volumes section version 1, expected 2 — recompile the .prl with the current prl-build` and stays in splash (no crash). |
| #7 Spec-floor load (134_217_728) | **PASS** | Temporarily set `REQUIRED_STORAGE_BUFFER_BINDING_SIZE` to the spec floor, rebuilt, ran: loads to `first_level_frame=19.0ms`, no limit error. Edit reverted; `git diff` clean. |
| #8 Compose dispatch GPU time | **DEFERRED (env)** | Adapter lacks `TIMESTAMP_QUERY`; also the SH compose pass is not currently an instrumented timing pair. Needs a TIMESTAMP_QUERY adapter + adding the pass to the timing pairs. |
| #9 Worst-case affinity-cell list length | **RECORDED** | **10** (a per-workgroup shared-loop bound, read once per workgroup, not per-thread). Mean 5.66 over 1,939 non-empty cells; 10,968 CSR entries across 3,306 cells. |
| #10 Within-cell occupancy | **RECORDED** | **11.3%** (79,564 / 701,952 sub-block probes non-zero). Dense sub-blocks spend ~89% on zero probes — the storage driver flagged in Open questions. |
| #11 AFFINITY_FACTOR rationale comment | **PASS** | `affinity_grid.rs:28-35` ties `4` to `@workgroup_size(4,4,4)` / 64 threads, read-once-per-workgroup. |
| No visual regression (manual A/B) | **HANDED TO USER** | See below — requires a human eyeball A/B; not machine-verifiable. |

## Headline numbers

- **Footprint total: 37.54 MiB** — 3.6× under the 128 MiB floor. The raised 512 MiB
  wgpu limit is now non-load-bearing (left requested as headroom, per scope).
- **.prl DeltaShVolumes section: −50.82 MiB / −57.5%.**
- **Compose dispatch time: unmeasurable in this environment** (no TIMESTAMP_QUERY
  adapter; pass not instrumented). The qualitative win is the per-probe loop bound
  dropping from `delta_light_count` (21, all lights, with an AABB test each) to the
  per-cell list length (mean 5.66, worst 10, no AABB test).
- Within-cell occupancy **11.3%**; worst-case cell list length **10**.
- Bake: total 96.24 s; Delta SH phase 11.84 s (the dominant cost is the unrelated
  AnimWeightMaps phase at 70.83 s).

## Recommendation

**Ship the sparse form as-is. Do NOT build the occupancy-bitmap fork now.**

Footprint is 3.6× under the floor — the plan's primary goal (single bind group under
the 128 MiB spec floor) is met with margin. Occupancy is low (11.3%), so the bitmap
fork from Open questions *could* shrink the payload to ~4 MiB, but it adds a per-cell
prefix-sum and variable per-cell indexing that the current point-read design
deliberately avoids — for no current need. Keep it as a latent optimization for a
future larger map that approaches the floor. No light cap needed (worst-case list
length is 10, shared per workgroup).

The `delta_scale` dev-tools slider (0..1; `debug_ui/mod.rs`) is sufficient to judge
the quality/frame-time tradeoff by eye. Promotion to a user-facing quality setting
correctly defers to the global quality-settings spec after the UI system lands.

## Reuse note (for the follow-up plan)

The out-of-scope **1,962,545-tile animated-lightmap install** dispatch (separate code
path; tracked separately) fires at runtime with `exceeds
max_compute_workgroups_per_dimension (65535)` — same dense-everything root cause. The
affinity-cell decomposition here (`affinity_grid.rs::decompose_affinity` → per-light
portal-reachable cell lists + CSR) is a reusable building block for pruning/partitioning
that dispatch. Flagged for the follow-up plan.

## Handed to the human

1. **Manual visual A/B (no-regression AC).** Use the `delta_scale` slider (0 = base-only,
   1 = full delta) in the SH debug UI on `campaign-test.prl`, on the **normal render path
   with DDGI probe depth-visibility weighting active** (not a DDGI-bypass/visibility-off
   debug mode). The one genuine sampling change is the removal of trilinear interpolation:
   v2 sub-blocks are base-probe-coincident 1:1 point reads, where v1 trilinearly blended 8
   delta probes at per-light AABB origins. f16 storage and `unpack2x16float` are bit-exact,
   so watch **animated-lit gradients for banding or position shift**, not brightness parity.
2. **Compose GPU timing.** Needs a TIMESTAMP_QUERY-capable adapter and adding the SH compose
   pass to the GPU timing pairs.

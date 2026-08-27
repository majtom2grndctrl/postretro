# Task 1 validation — arena-warren thin slice

Date: 2026-08-27
Fixture: `content/dev/maps/stress-warren-showcase.map`
Status: **do not promote default-on from this slice**. The concrete output and
capture pass, but the literal raw-section I5 assertion finds L2↔L0 faces in
ids 27 and 45. See [Seams and I5](#seams-and-i5).

This is deliberately a Task-1-only record. Coarsening remains default-off; no
format, renderer binding, cap-default, opt-out, or protection implementation
was changed.

## Method and retained artifacts

The map uses the checked-in showcase source, at its documented practical
measurement density (`--sh-probe-spacing 10.0 --lightmap-density 0.25`). Both
bakes supplied `--sh-delta-max-size 64MiB`, disabled the compiler cache, and
requested `--sh-analyze`.

| Artifact | SHA-256 | Purpose |
| --- | --- | --- |
| `/private/tmp/postretro-lighting-scale-sh-adaptive-coarsening-v2/stress-warren-showcase.uniform-l0.prl` | `5b4dcce2ca63ddae287ac5ad1454ba247539e265c18ea80df113f7c050b1bad6` | Retained default-off, uniform-L0 pre-activation baseline for Task 2's byte-identical opt-out check. |
| `/private/tmp/postretro-lighting-scale-sh-adaptive-coarsening-v2/stress-warren-showcase.coarsened.prl` | `6b09660edea72c3b4863fd061bc467cefe0bedbf920dbd3396f625e9c3a61ff3` | Explicit `--sh-coarsen` comparison bake, loaded and composed. |
| `/private/tmp/postretro-lighting-scale-sh-adaptive-coarsening-v2/stress-warren-showcase.uniform-l0.capture.png` | `a5df81547faccf0a541d8271219d4569d8e144ba251352501d250b4fe0f2c097` | Retained whole-frame renderer-readback proxy for the id-35 compose-atlas golden. |
| `/private/tmp/postretro-lighting-scale-sh-adaptive-coarsening-v2/stress-warren-showcase.coarsened.capture.png` | `a5df81547faccf0a541d8271219d4569d8e144ba251352501d250b4fe0f2c097` | Coarsened comparison proxy. |

The generated binary/PNG artifacts intentionally remain outside Git because
they total roughly 127 MiB. The corresponding `*.sh-analysis.json` files and
the temporary PRL inspectors are retained alongside them in the same directory.

Baseline invocation (default-off: no `--sh-coarsen`):

```text
cargo run --release -p postretro-level-compiler -- \
  content/dev/maps/stress-warren-showcase.map \
  -o /private/tmp/postretro-lighting-scale-sh-adaptive-coarsening-v2/stress-warren-showcase.uniform-l0.prl \
  --sh-probe-spacing 10.0 --lightmap-density 0.25 \
  --sh-delta-max-size 64MiB --sh-analyze \
  --sh-analyze-out /private/tmp/postretro-lighting-scale-sh-adaptive-coarsening-v2/stress-warren-showcase.uniform-l0.sh-analysis.json \
  --no-cache
```

The comparison substituted only `--sh-coarsen` and its output filenames. Both
commands exited successfully. The bakes reported the fixture's existing
geometry/decal and sub-texel-penumbra warnings; they did not fail the bake.

## P5 cap baseline and wire/load facts

The uncoarsened aggregate is **1,033,056 bytes** (0.985 MiB), or **1.54%** of
the 64 MiB (67,108,864-byte) P5 cap. The requested 64 MiB cap accepted the
baseline; this is a measured pass, not a coarsen-to-fit fallback.

The loaded container is PRL v4. The delta-section wire/payload versions remain
the contracted id-27/id-41/id-45 values **5/3/3**, with no layout change.

| Section | Baseline `delta_subblocks` | Coarsened | Ratio | Emitted levels, baseline → on |
| --- | ---: | ---: | ---: | --- |
| id 27 indirect | 130,176 B | 5,472 B | 4.20% | 25/0/0 (L0/L1/L2) → 5/0/20 |
| id 41 direct | 772,704 B | 772,704 B | 100.00% | 25/0/0 → 25/0/0 |
| id 45 animated direct | 130,176 B | 5,472 B | 4.20% | 25/0/0 → 5/0/20 |
| **Aggregate** | **1,033,056 B** | **783,648 B** | **75.86%** | — |

These are the raw, post-compaction `delta_subblocks` payloads from the **loaded
PRLs**. The level histograms are therefore emitted post-smoothing state, not
the separate `sh_analyze` reclassification histogram.

## Runtime data-volume projection and resident bytes

For a frame in which all relevant CSR records are selected, the id-41
read-equivalence projection is its uploaded `delta_subblocks` size:
**772,704 B/frame before and after (1.000×)**. This fixture gives no direct
id-41 traffic reduction because id 41 emitted L0 in all 25 cells. The id-27
and id-45 diagnostics each decline from 130,176 B to 5,472 B (0.0420×); the
all-delta projection declines from 1,033,056 B to 783,648 B (0.7586×).

Resident bytes were calculated from the exact renderer upload layout: payload,
three u32s per cell for mask-low/mask-high/widened-level metadata, CSR offsets,
CSR light IDs, and (where applicable) descriptor IDs. No minimum-buffer padding
was needed for this nonempty fixture.

| Section | Baseline resident | Coarsened resident | Ratio |
| --- | ---: | ---: | ---: |
| id 27 | 130,680 B | 5,976 B | 0.0457× |
| id 41 | 773,612 B | 773,612 B | 1.0000× |
| id 45 | 130,680 B | 5,976 B | 0.0457× |
| **All delta resources** | **1,034,972 B** | **785,564 B** | **0.7590×** |

This records the requested id-27/id-45 diagnostics without inventing a
savings threshold. The dominant id-41 metric is unchanged, so this fixture
does not establish a default-on runtime bandwidth win.

## Error gate and composed output

The settled limits used here are `rel_p95_max = 0.10`, `rel_max_max = 0.25`,
and `darkness_frac = 0.02`; no new threshold was introduced. A direct retained
L0-versus-emitted-PRL decode of every coarsened id-27/id-45 valid RGB tile found
zero reconstruction error: maximum per-brick relative p95 **0.0** and relative
max **0.0**. (The payloads' nonzero f16 values are alpha; their RGB delta is
zero.) Id 41 remains L0, so the combined emitted composed data is exact for
this fixture. This passes the pinned error gate.

There is no pre-existing direct compose-atlas readback golden. Per
`rendering_pipeline.md` §7.8, `cargo run -p xtask -- capture …` was used as the
explicitly permitted whole-frame proxy: it loads the PRL and calls the
renderer-owned offscreen `capture_frame_indirect` path, which runs the direct
compose before the scene-color readback. The two 1280×720 RGBA8 capture files
are byte-identical (0 nonzero pixel/channel differences). This supports I1's
value check for this map, and no source changed the dense id-35-dimension
`Rgba16Float` D2Array or its existing group-3/group-4 binding-15 and shared
sampler binding-2 contract.

Visual limitation: the temporary PRL path makes the capture content-root
resolve its PRM cache under `/baked/materials`; 16 texture cache entries were
therefore placeholders (magenta checkerboard). The renderer composition did
execute and the two frames are identical, but that image is not a meaningful
human seam-quality inspection of final materials. No discontinuity was visible
in the proxy, but a material-complete manual visual pass remains required before
promotion.

## Seams and I5

`sh_analyze` records the supporting (non-gating) seam diagnostic as 31 pairs,
`residual_max = 0`, `residual_mean = 0`, `cross_level_pairs = 0`. It is not the
emitted-level assertion: the analyzer calculates its own candidate levels.

The required raw emitted-PRL face scan is stricter and reports:

- id 41: 0 L2↔L0 violations.
- ids 27 and 45: **five** L2↔L0 pairs each, at `x = 3 → 4` for each `z = 0..4`
  (the grid is 5×1×5). The L2 endpoint is a participating brick; the L0 endpoint
  has no valid probes and is deliberately non-participating in the current
  `classify_levels` smoothing pass (`p10_zero_valid_is_l0_and_non_participating`).

Thus the current code's participating-brick convention explains the result,
but it does not satisfy the Task-1 / I5 wording that **every** face-adjacent
cell in every present section differs by at most one. This is a validation
failure to resolve before a default-on recommendation, not a reason to change
the mechanism in this task.

## Compose dispatch time

I attempted the built-in runtime timestamp path with:

```text
POSTRETRO_GPU_TIMING=1 cargo run -p xtask -- run --features dev-tools -- \
  /private/tmp/postretro-lighting-scale-sh-adaptive-coarsening-v2/stress-warren-showcase.uniform-l0.prl
```

The selected adapter logs that it lacks `TIMESTAMP_QUERY` and/or
`TIMESTAMP_QUERY_INSIDE_ENCODERS`, so the renderer ran without GPU timing.
There is consequently no valid compose-dispatch duration or ratio to report;
none has been inferred from CPU time. The same adapter limitation applies to
the coarsened comparison. The windowed run did load the baseline PRL, though
the temporary map path also put its scripted content and textures outside the
normal `content/dev` root.

## Focused coverage run

The final validation command set is intentionally limited to the compiler
module fixtures relevant to the ordering and no-silent-retry contracts, plus a
workspace check:

```text
cargo check
CARGO_PROFILE_TEST_SPLIT_DEBUGINFO=off cargo test -p postretro-level-compiler --bin prl-build sh_coarsen
CARGO_PROFILE_TEST_SPLIT_DEBUGINFO=off cargo test -p postretro-level-compiler --bin prl-build coarsened_payload_still_hard_fails_the_cap_without_retry
```

In particular, the `sh_coarsen` fixture group covers P1's protection-before-
smoothing ordering, the x-fastest face-neighbor traversal, composed-magnitude
provider wiring, and the deliberate zero-valid non-participation that the raw
fixture result exposed. No bare `cargo test -p postretro-level-compiler` is
used. All three commands above passed (`cargo check`, 22 `sh_coarsen` tests,
and the one cap no-retry test).

I also attempted the renderer's development-only storage-footprint fixture with
`--features dev-tools`:

```text
CARGO_PROFILE_TEST_SPLIT_DEBUGINFO=off cargo test -p postretro-renderer --lib \
  --features dev-tools direct_promotion_logs_id_41_bound_storage_once
```

It cannot currently compile due to an unrelated test assertion in
`crates/renderer/src/render/debug_ui/mod.rs`: `DiagnosticsTab::ALL` now has 7
entries while the asserted literal has 6. This task does not change that
debug-UI surface, so the resident-byte figures above remain the direct
resource-layout calculation rather than a dev-tools log capture.

## Task-1 decision inputs

- P5 uniform baseline/cap: pass.
- Default-off uniform baseline retained for I4 / later byte-identical opt-out:
  pass (comparison deferred to Task 2).
- Load and compose, payload wire contract, and whole-frame I1 proxy: pass.
- Emitted per-brick error gate: pass (0.0/0.0).
- Dominant id-41 traffic reduction: absent on this fixture (1.000×).
- GPU compose time: unavailable on the selected adapter; needs a timestamp-capable
  measurement environment.
- Literal all-cell I5 scan: fail for ids 27 and 45 because five non-participating
  zero-valid L0 cells neighbor L2 cells.

No savings-pass magnitude has been introduced. Given the I5 mismatch and the
missing timestamp-capable timing evidence, this validation slice should feed a
**no-promote** recommendation until the contract is reconciled and remeasured.

## Supplemental fixture: `occlusion-test.map`

This supplemental Task-1 comparison uses the same explicit before/after
controls as the showcase: a default-off bake (no `--sh-coarsen`) followed by
an otherwise identical explicit `--sh-coarsen` bake. Both use
`--sh-probe-spacing 10.0 --lightmap-density 0.25 --sh-delta-max-size 64MiB
--sh-analyze --no-cache --no-tui`, and both completed successfully despite the
fixture's pre-existing light-default, coplanar-face, and watertightness
diagnostics.

```text
cargo run --release -p postretro-level-compiler -- \
  content/dev/maps/occlusion-test.map \
  -o /private/tmp/postretro-lighting-scale-sh-adaptive-coarsening-v2/occlusion-test.uniform-l0.prl \
  --sh-probe-spacing 10.0 --lightmap-density 0.25 \
  --sh-delta-max-size 64MiB --sh-analyze \
  --sh-analyze-out /private/tmp/postretro-lighting-scale-sh-adaptive-coarsening-v2/occlusion-test.uniform-l0.sh-analysis.json \
  --no-cache --no-tui

cargo run --release -p postretro-level-compiler -- \
  content/dev/maps/occlusion-test.map \
  -o /private/tmp/postretro-lighting-scale-sh-adaptive-coarsening-v2/occlusion-test.coarsened.prl \
  --sh-probe-spacing 10.0 --lightmap-density 0.25 \
  --sh-delta-max-size 64MiB --sh-analyze \
  --sh-analyze-out /private/tmp/postretro-lighting-scale-sh-adaptive-coarsening-v2/occlusion-test.coarsened.sh-analysis.json \
  --sh-coarsen --no-cache --no-tui
```

### Retained artifacts, wire facts, and P5-sized baseline

| Artifact | SHA-256 | Bytes |
| --- | --- | ---: |
| `occlusion-test.uniform-l0.prl` | `6bd669b3b128d151cb5ec57d4bbdae1144df9ddf9fb8bb92799643bb5eac1086` | 4,319,809 |
| `occlusion-test.coarsened.prl` | `1c268fbfba5bf1b9a9dd361f8d2461e6f3ca1692b6547a4e669fe7b636a48e4a` | 4,304,833 |
| `occlusion-test.uniform-l0.capture.png` | `a4acae550e6725c3d2d1d911c302f49b972a5c100eb14063bc12bb5b36424143` | 194,265 |
| `occlusion-test.coarsened.capture.png` | `a4acae550e6725c3d2d1d911c302f49b972a5c100eb14063bc12bb5b36424143` | 194,265 |

All are retained under
`/private/tmp/postretro-lighting-scale-sh-adaptive-coarsening-v2/`, outside
Git. The containers are PRL v4 and the loaded id-27/id-41/id-45 payload
versions remain **5/3/3**; no layout or binding contract changed.

The uniform aggregate `delta_subblocks` payload is **23,040 B** (0.022 MiB),
or 0.0343% of the 64 MiB cap; the specified cap accepted it without any
coarsen-to-fit retry. The coarsened aggregate is **8,064 B** (0.3500×).

| Section | Uniform payload | Coarsened payload | Ratio | Emitted post-smoothing levels, uniform → on |
| --- | ---: | ---: | ---: | --- |
| id 27 indirect | 6,912 B | 576 B | 0.0833× | 4/0/0 → 0/0/4 (L0/L1/L2) |
| id 41 direct | 9,216 B | 576 B | 0.0625× | 4/0/0 → 0/0/4 |
| id 45 animated direct | 6,912 B | 6,912 B | 1.0000× | 4/0/0 → 1/2/1 |
| **Aggregate** | **23,040 B** | **8,064 B** | **0.3500×** | — |

These histograms and payloads are read from the loaded PRLs, so they are the
emitted post-smoothing state rather than `sh_analyze` reclassification.

### Runtime projections, error, and seams

The all-selected id-41 per-frame projection declines from **9,216 B** to
**576 B** (0.0625×). The id-27 diagnostic projection likewise declines from
6,912 B to 576 B (0.0833×); id 45 remains 6,912 B (1.0000×). The renderer
upload-layout resident calculation (payload + three u32 cell metadata values +
CSR offsets/light ids + descriptors where present) is:

| Section | Uniform resident | Coarsened resident | Ratio |
| --- | ---: | ---: | ---: |
| id 27 | 7,004 B | 668 B | 0.0954× |
| id 41 | 9,292 B | 652 B | 0.0702× |
| id 45 | 7,004 B | 7,004 B | 1.0000× |
| **All delta resources** | **23,300 B** | **8,324 B** | **0.3573×** |

The retained-L0 versus emitted-PRL decoder uses the shared L1 trilinear rule
(valid corners, weights `local/3`, renormalized) and L2 valid-probe mean. It
sums all present ids and CSR entries before comparing each interior RGB texel
against the analyzer's composed magnitude. The worst combined emitted cell is
`rel_p95 = 0.05928` and `rel_max = 0.07977`, passing the pinned `≤0.10` and
`≤0.25` gates. Id 27 supplies those maxima; id 41 alone is 0.00052/0.00460,
and id 45's L1/L2 entries carry zero RGB delta in this fixture.

The raw all-cell x-fastest face-adjacency scan reports **zero** level-difference
violations in every present section: id 27 (all L2), id 41 (all L2), and id 45
(L0/L1/L2 = 1/2/1). This map therefore **passes literal I5**, including cells
with no participating delta. Its supporting `sh_analyze` diagnostic is 4
pairs, 2 cross-level pairs, `residual_max = 0.11788003`, and
`residual_mean = 0.009306223`; this remains a diagnostic, not an added seam
threshold. The captured proxy was manually inspected and shows no apparent
seam, but it is near-black with placeholder checkerboard materials because the
temporary PRL path cannot resolve the normal PRM cache; it is not a
material-complete visual sign-off.

### I1 proxy, load/compose, and timing

Both retained PRLs were loaded and composed through renderer-owned whole-frame
capture, as allowed by `rendering_pipeline.md` §7.8. The capture scene uses the
map's transformed player spawn `[-23.1648, 1.4224, -15.0368]`, yaw/pitch 0°, a
100° FOV, and a 1280×720 target. The two RGBA8 files have the same SHA-256 and
a direct diff has 0 nonzero channels/pixels. This is a passing whole-frame I1
proxy for the unchanged dense id-35 compose contract, not a new direct-atlas
readback golden.

```text
cargo run -p xtask -- capture /private/tmp/postretro-lighting-scale-sh-adaptive-coarsening-v2/occlusion-test.uniform-l0.capture.json
cargo run -p xtask -- capture /private/tmp/postretro-lighting-scale-sh-adaptive-coarsening-v2/occlusion-test.coarsened.capture.json

POSTRETRO_GPU_TIMING=1 cargo run -p xtask -- run --features dev-tools -- \
  /private/tmp/postretro-lighting-scale-sh-adaptive-coarsening-v2/occlusion-test.uniform-l0.prl
```

The windowed timing attempt loaded the baseline, but the selected adapter logs
that it lacks `TIMESTAMP_QUERY` and/or `TIMESTAMP_QUERY_INSIDE_ENCODERS`; no
compose-dispatch duration or ratio exists for either occlusion variant. The
same capability limitation is already recorded for the showcase, so timing
availability remains a no-result rather than an inferred CPU proxy.

### Requested kinematic fixture availability

No source file named `kinematic-movers.map` is present on this integration
checkout. A repository-wide case-insensitive filename scan returned no paths:

```text
rg --files -0 . | tr '\\0' '\\n' | awk 'tolower($0) ~ /(^|\\/)kinematic-movers\\.map$/ { print }'
```

`content/dev/maps/kinematic-platform.map` exists but was **not** substituted,
because it is a different fixture and no approval was given. Consequently
there are no baseline/on artifacts or map-specific metrics for the requested
kinematic-movers source. Occlusion's literal-I5 pass does not change the
Task-1 **no-promote** finding: the retained showcase still fails literal
all-cell I5 in ids 27/45 and both fixtures lack timestamp-capable timing
evidence.

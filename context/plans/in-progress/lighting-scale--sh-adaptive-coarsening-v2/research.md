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
they total roughly 150 MiB. The corresponding `*.sh-analysis.json` files and
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

## Correction and supplemental fixture: `kinematic-platform.map`

The user subsequently confirmed that
`content/dev/maps/kinematic-platform.map`—not the absent
`kinematic-movers.map`—is the intended kinematic fixture. The prior filename
scan is retained as a record of the original spelling; this fixture was not
substituted until that confirmation. It uses the same default-off/on controls
as the preceding two fixtures:

```text
cargo run --release -p postretro-level-compiler -- \
  content/dev/maps/kinematic-platform.map \
  -o /private/tmp/postretro-lighting-scale-sh-adaptive-coarsening-v2/kinematic-platform.uniform-l0.prl \
  --sh-probe-spacing 10.0 --lightmap-density 0.25 \
  --sh-delta-max-size 64MiB --sh-analyze \
  --sh-analyze-out /private/tmp/postretro-lighting-scale-sh-adaptive-coarsening-v2/kinematic-platform.uniform-l0.sh-analysis.json \
  --no-cache --no-tui

cargo run --release -p postretro-level-compiler -- \
  content/dev/maps/kinematic-platform.map \
  -o /private/tmp/postretro-lighting-scale-sh-adaptive-coarsening-v2/kinematic-platform.coarsened.prl \
  --sh-probe-spacing 10.0 --lightmap-density 0.25 \
  --sh-delta-max-size 64MiB --sh-analyze \
  --sh-analyze-out /private/tmp/postretro-lighting-scale-sh-adaptive-coarsening-v2/kinematic-platform.coarsened.sh-analysis.json \
  --sh-coarsen --no-cache --no-tui
```

Both completed successfully with the fixture's seven pre-existing
missing-`style` light warnings. Ids 27 and 45 are absent (the compiler skipped
the indirect and animated-direct delta stages); id 41 is the only present
delta section.

### Retained artifacts and emitted PRL facts

| Artifact | SHA-256 | Bytes |
| --- | --- | ---: |
| `kinematic-platform.uniform-l0.prl` | `bf5a5451442dffd116952f9228f71573e7157a3e9144a30972cda95898c93cd3` | 4,909,069 |
| `kinematic-platform.coarsened.prl` | `bf5a5451442dffd116952f9228f71573e7157a3e9144a30972cda95898c93cd3` | 4,909,069 |
| `kinematic-platform.uniform-l0.capture.png` | `803b6602cc34e2b120fde910bd3063c376b408527c3c2901f25844d73ed5b311` | 237,010 |
| `kinematic-platform.coarsened.capture.png` | `803b6602cc34e2b120fde910bd3063c376b408527c3c2901f25844d73ed5b311` | 237,010 |

The equal PRL hashes are the retained default-off baseline and explicit-on
comparison result, not a copied artifact. The raw loaded containers are PRL
v4; the present id-41 payload remains version 3 (the unchanged contract is
id-27/id-41/id-45 = 5/3/3 when each section is present).

The uniform aggregate `delta_subblocks` payload is **133,632 B** (0.1274 MiB,
0.1991% of the 64 MiB cap); the explicit-coarsen aggregate is the same. The
cap accepted both with no coarsen-to-fit retry.

| Section | Uniform payload | Coarsened payload | Ratio | Emitted post-smoothing levels, uniform → on |
| --- | ---: | ---: | ---: | --- |
| id 27 indirect | absent | absent | — | — |
| id 41 direct | 133,632 B | 133,632 B | 1.0000× | 6/0/0 → 6/0/0 (L0/L1/L2) |
| id 45 animated direct | absent | absent | — | — |
| **Aggregate** | **133,632 B** | **133,632 B** | **1.0000×** | — |

The emitted PRL is therefore a valid P15-style N=0-coarsenable result, rather
than evidence of an activation failure. All reported levels come from the
loaded PRL, not the analyzer's independent reclassification.

### Runtime projections, reconstruction, and literal I5

The all-selected id-41 per-frame projection is **133,632 B/frame** before and
after (1.0000×). Ids 27/45 have no diagnostic traffic because they are absent.
The exact renderer upload-layout resident calculation is likewise unchanged:
id 41 / all delta resources are **133,796 B → 133,796 B** (1.0000×), comprising
the payload plus six × three-u32 cell metadata values, seven CSR offsets, and
16 CSR light ids.

No emitted cell is coarsened, so the retained-L0 versus emitted decode has zero
coarsened cells and reports `rel_p95 = 0.0`, `rel_max = 0.0`; it passes the
pinned error limits vacuously. This is not a savings claim. The raw all-cell,
x-fastest face scan finds zero violations in present id 41 (1×2×3 affinity
grid); ids 27/45 are absent. Thus this fixture **passes literal I5 for every
present section**. The supporting analyzer diagnostic is 7 pairs, 3
cross-level pairs, `residual_max = 0.0327905`, and
`residual_mean = 0.0024592555`; its candidate cross-level count does not alter
the emitted-PRL result and is not a seam gate.

### Load/compose proxy and timing

Both PRLs were loaded and composed through the renderer-owned whole-frame
capture permitted by `rendering_pipeline.md` §7.8. The capture uses transformed
player spawn `[-6.5024, 1.2192, -3.2512]`, yaw/pitch 0°, 100° FOV, and 1280×720
RGBA8 output. The two capture hashes match and their direct pixel diff is zero.
This is a passing I1 proxy for the unchanged dense id-35 output/binding
contract, but it is necessarily exact because the PRLs are byte-identical.
Manual inspection found no discontinuity; missing PRM sidecars yield magenta
checkerboard placeholders, so it is not material-complete visual sign-off.

```text
cargo run -p xtask -- capture /private/tmp/postretro-lighting-scale-sh-adaptive-coarsening-v2/kinematic-platform.uniform-l0.capture.json
cargo run -p xtask -- capture /private/tmp/postretro-lighting-scale-sh-adaptive-coarsening-v2/kinematic-platform.coarsened.capture.json

POSTRETRO_GPU_TIMING=1 cargo run -p xtask -- run --features dev-tools -- \
  /private/tmp/postretro-lighting-scale-sh-adaptive-coarsening-v2/kinematic-platform.uniform-l0.prl
```

The windowed baseline load reaches the renderer, but the selected adapter again
lacks `TIMESTAMP_QUERY` and/or `TIMESTAMP_QUERY_INSIDE_ENCODERS`; no compose
duration or ratio is available. This correct fixture adds neither a dominant
id-41 saving nor timestamp evidence, and therefore does not change the
Task-1 **no-promote** result already set by the showcase's literal I5 failure
and missing timing evidence.

## Supplemental fixture: `campaign-test.map`

This is an additional Task-1 evidence fixture, not a replacement for the
Stress-Warren-pinned gate. The retained default-off bake and the explicit
`--sh-coarsen` comparison used the same fixed controls as the other fixtures:

```text
cargo run --release -p postretro-level-compiler -- \
  content/dev/maps/campaign-test.map \
  -o /private/tmp/postretro-lighting-scale-sh-adaptive-coarsening-v2/campaign-test.uniform-l0.prl \
  --sh-probe-spacing 10.0 --lightmap-density 0.25 \
  --sh-delta-max-size 64MiB --sh-analyze \
  --sh-analyze-out /private/tmp/postretro-lighting-scale-sh-adaptive-coarsening-v2/campaign-test.uniform-l0.sh-analysis.json \
  --no-cache --no-tui

cargo run --release -p postretro-level-compiler -- \
  content/dev/maps/campaign-test.map \
  -o /private/tmp/postretro-lighting-scale-sh-adaptive-coarsening-v2/campaign-test.coarsened.prl \
  --sh-probe-spacing 10.0 --lightmap-density 0.25 \
  --sh-delta-max-size 64MiB --sh-analyze \
  --sh-analyze-out /private/tmp/postretro-lighting-scale-sh-adaptive-coarsening-v2/campaign-test.coarsened.sh-analysis.json \
  --sh-coarsen --no-cache --no-tui
```

Both complete successfully, with the checked-in map's existing light-default
and watertightness warnings. The retained artifacts are outside Git under
`/private/tmp/postretro-lighting-scale-sh-adaptive-coarsening-v2/`:

| Artifact | SHA-256 | Bytes |
| --- | --- | ---: |
| `campaign-test.uniform-l0.prl` | `1428c937067d265b77c3aea88048825689c5ce5ffb44072aa4ddf5d90076c4ed` | 6,187,409 |
| `campaign-test.coarsened.prl` | `f819723cd81e6b9c05efd1bc7c53d6b96524bdbfb3530807b94de4f33d6ff076` | 6,174,737 |
| `campaign-test.uniform-l0.capture.png` | `06f321cf8f852fa679662380419f7d04920bdc1f2aba7a561b013c75bd373668` | 322,138 |
| `campaign-test.coarsened.capture.png` | `06f321cf8f852fa679662380419f7d04920bdc1f2aba7a561b013c75bd373668` | 322,138 |

The loaded containers are PRL v4 and preserve the id-27/id-41/id-45 payload
versions **5/3/3**. The default-off aggregate is **39,744 B** (0.0379 MiB,
0.0592% of 64 MiB), so it passes the P5-sized cap check without any retry;
the explicit-on aggregate is 27,072 B (0.6812×).

| Section | Uniform payload | Coarsened payload | Ratio | Emitted post-smoothing levels, uniform → on |
| --- | ---: | ---: | ---: | --- |
| id 27 indirect | 13,824 B | 1,152 B | 0.0833× | 12/0/0 → 7/0/5 (L0/L1/L2) |
| id 41 direct | 12,096 B | 12,096 B | 1.0000× | 12/0/0 → 12/0/0 |
| id 45 animated direct | 13,824 B | 13,824 B | 1.0000× | 12/0/0 → 12/0/0 |
| **Aggregate** | **39,744 B** | **27,072 B** | **0.6812×** | — |

The table and all following scans read the loaded PRLs, not analyzer candidate
levels. The all-selected id-41 frame-volume projection remains **12,096
B/frame** (1.0000×); id27 declines 13,824 → 1,152 B, and id45 is unchanged.
The exact renderer upload-layout resident calculation is:

| Section | Uniform resident | Coarsened resident | Ratio |
| --- | ---: | ---: | ---: |
| id 27 | 14,052 B | 1,380 B | 0.0982× |
| id 41 | 12,312 B | 12,312 B | 1.0000× |
| id 45 | 14,052 B | 14,052 B | 1.0000× |
| **All delta resources** | **40,416 B** | **27,744 B** | **0.6865×** |

The retained-L0 versus emitted-PRL decode sums every present id and CSR entry
before its interior-RGB comparison. The combined worst emitted cell is
`rel_p95 = 0.06316` and `rel_max = 0.07921`, within the pinned 0.10/0.25
limits (absolute p95/max 0.01888/0.05149). Thus the relative reconstruction
error gate independently passes.

The raw x-fastest all-cell face scan is not a pass: ids 41 and 45 have zero
violations, while id 27 has **five** L2↔L0 pairs on its 3×1×4 grid:
`(1,0,0)→(2,0,0)`, `(1,0,1)→(2,0,1)`,
`(1,0,1)→(1,0,2)`, `(0,0,2)→(1,0,2)`, and
`(0,0,2)→(0,0,3)`. Campaign therefore **fails literal I5** despite the
passing error gate. The supporting analyzer diagnostic is 5 pairs, 3
cross-level pairs, `residual_max = 0`, and `residual_mean = 0`; it is not the
emitted-level assertion and introduces no separate threshold.

### I1 whole-frame proxy and timing

Both PRLs loaded and ran the renderer-owned whole-frame capture permitted by
`rendering_pipeline.md` §7.8. The scene uses transformed player spawn
`[-65.8368, 1.8288, -45.9232]`, yaw/pitch 0°, a 100° FOV, and 1280×720 RGBA8.
The proxy captures are hash- and pixel-identical (zero nonzero channels and
pixels), so it passes this I1 proxy for the unchanged dense id-35 output and
binding contract. It is not a direct compose-atlas readback golden. Manual
inspection saw no apparent seam, but the temporary PRL root gives placeholder
materials (and a dark capture), so it is not a material-complete visual
sign-off.

```text
cargo run -p xtask -- capture /private/tmp/postretro-lighting-scale-sh-adaptive-coarsening-v2/campaign-test.uniform-l0.capture.json
cargo run -p xtask -- capture /private/tmp/postretro-lighting-scale-sh-adaptive-coarsening-v2/campaign-test.coarsened.capture.json

POSTRETRO_GPU_TIMING=1 cargo run -p xtask -- run --features dev-tools -- \
  /private/tmp/postretro-lighting-scale-sh-adaptive-coarsening-v2/campaign-test.uniform-l0.prl
```

The windowed baseline reached the renderer but the selected adapter lacks
`TIMESTAMP_QUERY` and/or `TIMESTAMP_QUERY_INSIDE_ENCODERS`; no compose
dispatch time or ratio is available for either variant.

### Campaign gate summary and pinned-fixture limits

Campaign independently clears the 64 MiB baseline/cap check, wire-version
contract, PRL load/compose, whole-frame I1 proxy, and relative reconstruction
error gate. It does **not** independently clear all relevant functional gates:
literal I5 fails in id 27, id41 traffic has no reduction, the material-complete
manual visual check is unavailable, and there is no timestamp-capable compose
time. In addition, this map cannot replace the plan's specific
`stress-warren-showcase.map` criterion; that pinned fixture still has its own
five raw L2↔L0 violations in both ids 27/45 and still lacks a timestamp-capable
measurement. Campaign is corroborating evidence for the existing Task-1
**no-promote** result, not an independent clearance of it.

## Corrected Stress Warren density/error follow-up (2026-08-27)

This supersedes the earlier 10 m Stress Warren diagnostic as gate evidence.
It is a measurement-only follow-up: source and plan files are unchanged.  Each
row is a default-off uniform-L0 bake paired with explicit `--sh-coarsen`, using
`--lightmap-density 0.25 --sh-delta-max-size 2GiB --sh-analyze --no-cache
--no-tui`; the 2 GiB value prevents silent I3 coarsen-to-fit while measuring
the raw P5 baseline.  Exact output and analysis paths use
`/private/tmp/postretro-lighting-scale-sh-adaptive-coarsening-v2/stress-warren-showcase.spacing-<density>.<variant>.(prl|sh-analysis.json)`.
All outputs are PRL v4 and retain id-27/id-41/id-45 payload versions **5/3/3**.

### Artifacts, volume, resident data, and emitted levels

| Spacing | Uniform SHA-256 | Coarsened SHA-256 | Aggregate delta B, off -> on (ratio) | id-41 B/frame, off -> on (ratio) | Resident delta B, off -> on |
| --- | --- | --- | ---: | ---: | ---: |
| 1.25 m | `18a4dbeb8eb06c6f9d202186e1cc2f92163252302b37c3b438aff87a87d2138d` | `672dbfda897fd41816291109350a0de68388ca61430b80e2031531a65f48402b` | 155,055,168 -> 17,095,968 (0.1103x) | 71,071,488 -> 14,249,088 (0.2005x) | 155,469,088 -> 17,509,888 |
| 1.0 m (ship) | `145fe9b6414c93d50521f7a5c1f22df15f7c13e15983ef6191b57b02d600653c` | `61d4ddb1f65fe5bf62ff044f05ce57aa539bacdb507b32ecf63adc605bec660a` | 303,822,432 -> 30,484,224 (0.1003x) | 145,267,200 -> 25,985,952 (0.1789x) | 304,577,640 -> 31,239,432 |
| 0.75 m | `2a88d2f5d5a70e179ba17059283b6b83079f25a7592b02c990fccdb16d0d1bfa` | `ecd5ca20def5e3024b11e0a21f15eba5a259cede043396ce282bf0e80fe550af` | 651,254,976 -> 50,258,304 (0.0772x) | 306,455,616 -> 40,527,360 (0.1322x) | 652,967,128 -> 51,970,456 |

The direct id-41 bytes are the selected-section per-frame projection; no
savings pass threshold is invented.  The uniform payloads are respectively
2.31x, 4.53x, and 9.70x the 64 MiB P5 limit: none passes that raw cap.

Histograms below are loaded emitted post-smoothing PRL levels (`L0/L1/L2`),
not analyzer candidates.  Corrected I5 scans exclude zero-valid sentinels.

| Spacing | id 27 uniform -> on | id 41 uniform -> on | id 45 uniform -> on | Participating I5 violations (27/41/45) | Raw all-cell sentinel diagnostic (27/41/45) |
| --- | --- | --- | --- | --- | --- |
| 1.25 m | 7296/0/0 -> 362/1/6933 | 7296/0/0 -> 1039/1907/4350 | 7296/0/0 -> 379/66/6851 | 0/0/0 | 949/623/940 |
| 1.0 m | 13440/0/0 -> 1293/2/12145 | 13440/0/0 -> 2538/2739/8163 | 13440/0/0 -> 1312/69/12059 | 0/0/0 | 2985/2066/2977 |
| 0.75 m | 30528/0/0 -> 3808/1/26719 | 30528/0/0 -> 5686/4152/20690 | 30528/0/0 -> 3828/77/26623 | 0/0/0 | 6252/4823/6240 |

All three densities pass literal corrected I5.  The raw all-cell pairs are
sentinel-boundary diagnostics only, not replacement invariant results.

### Composed-error disambiguation

The retained-L0/emitted-PRL decoder reconstructs present id 27/41/45 CSR
entries, combines the composed RGB interiors, uses emitted L2 means and the
shader zero fallback for sparse-L1 corners.  Its floor is
`max(0.02 * map_p95, 1e-6)`; a failure exceeds raw `rel_p95=.10` or
`rel_max=.25`.  Complete 1.25 m records for all 1,636 failing bricks are
retained outside Git as
`stress-warren-showcase.spacing-1.25.error-disambiguation.full.json`, SHA-256
`d0b2aab81ca988f1cd0d25a9ed26c10b91ce5d8c50be0760fc15bd60d6eb8f71`.

| Spacing | map p95 / floor | Raw / floored failures | Bypassed / bright-gated | Worst bright abs p95/max; rel p95/max | Worst bypass abs p95/max; rel p95/max | Dominant |
| --- | ---: | ---: | ---: | --- | --- | --- |
| 1.25 m | 2.1947632 / 0.043895264 | 1636 / 1635 | 10 / 1626 | .2548523/1.1226196; .2514301/.9516246 | .0164795/.0494385; .4422604/.4384303 | id 41 |
| 1.0 m | 2.1497803 / 0.042995606 | 2007 / 2006 | 14 / 1993 | .0878906/1.0166016; .0694110/.7419153 | .0229034/.0503235; .7906242/.6638486 | id 41 |
| 0.75 m | 2.0895996 / 0.041791992 | 2648 / 2648 | 24 / 2624 | .5349121/1.7324219; .2884793/.8181714 | .0862088/.2859701; 2.1983571/.6040233 | id 41 |

At 1.25 m, bright-gated cell 5213 is dominated by id-41 (absolute
.2400716/1.0537109; relative .2368479/.8932119).  Bypassed cell 2129 also has
id-41 absolute .0164795/.0494385, but is only 10 of 1,636 failures.  Flooring
removes one failure at 1.25 m and 1.0 m, and none at 0.75 m.  Thus a pure
un-floored metric artifact is rejected; bypass behavior is secondary, while
bright-gated id-41 composed/control error dominates.  Every representative
density fails the literal `.10/.25` composed-error gate.

Compiler totals (uniform/coarsened) were 767.97/1049.67 s at 1.25 m,
1559.46/1767.03 s at 1.0 m, and 2547.06/2188.03 s at 0.75 m.  These are bake
times, not compose dispatch time.  The selected adapter lacks timestamp
capability, so GPU compose timing remains unavailable.  No new I1 capture was
made in this density/error-only follow-up; the prior whole-frame proxy does not
replace the missing content-rooted manual visual check.

Result: payload and id-41 traffic improve and corrected I5 remains clean, but
the uniform P5 cap and composed-error gates fail at all densities.  This is
Task-1 **no-promote** evidence, not a source-change request.

## Id-41-only 1.0 m validation (2026-08-28)

This supersedes the three-section safety conclusion above. The shipped scope is
now id-41 direct coarsening only. Ids 27 and 45 emit uniform L0 and their
adaptive paths are deferred until script and animation amplitudes have a
bounded runtime contract.

Each fixture used a retained uniform-L0 baseline and a fresh `--sh-coarsen`
comparison at `--sh-probe-spacing 1.0 --lightmap-density 0.25 --no-cache
--no-tui`. The comparison bakes used `--sh-delta-max-size 2GiB` solely to
measure the existing payloads; it is not a cap-policy pass.

| Fixture | Uniform delta payload | Id-41-only delta payload | Id-41 payload, off -> on | Id-41 retained | Aggregate retained |
| --- | ---: | ---: | ---: | ---: | ---: |
| Stress Warren safety | 303,822,432 B | 186,099,840 B | 145,267,200 -> 27,544,608 B | 18.96% | 61.25% |
| Campaign primary win | 8,686,944 B | 6,284,448 B | 4,592,736 -> 2,190,240 B | 47.69% | 72.34% |
| Kinematic corroboration | 5,419,296 B | 4,735,296 B | 5,419,296 -> 4,735,296 B | 87.38% | 87.38% |

The id-41 value is the direct-delta read-volume projection. It is also the
dominant resident-byte change: Campaign direct resident bytes fall from
4,647,156 to 2,244,660; Kinematic falls from 5,458,168 to 4,774,168.
Campaign is the primary intentional-lighting win. Kinematic independently
corroborates a smaller positive direct reduction and contains id 41 only.

### Safety and emitted quality

The id-41 runtime envelope repaired every sampled emitted-quality failure:

| Fixture | Envelope failures, before -> after | L0 restores / smoothing refinements | Emitted `.10/.25` failures | Participating I5 violations |
| --- | ---: | ---: | ---: | ---: |
| Stress Warren | 38 -> 0 | 38 / 51 | 0 | 0 in ids 27, 41, and 45 |
| Campaign | 12 -> 0 | 12 / 13 | 0 | 0 in ids 27, 41, and 45 |
| Kinematic | 1 -> 0 | 1 / 0 | 0 | 0 in id 41 |

The emitted PRL face scan excludes zero-valid L0 sentinels. Its participating
result is I5. Raw all-cell L2-to-sentinel-L0 counts remain diagnostics, not
seams. Ids 27 and 45 are all L0 in these comparisons and retain their baseline
payload exactly: Stress 82,235,232 B / 76,320,000 B, and Campaign 2,464,128 B
/ 1,630,080 B. Kinematic emits neither mutable section.

### Remaining promotion blockers

The selected adapter lacks timestamp-query support. Compose dispatch time and
the net-runtime-savings sign are therefore not evaluable; do not infer them
from bake or CPU time. The temporary PRL path also uses placeholder materials,
so a content-rooted manual seam review remains required.

P5 is unresolved. Stress Warren's uniform baseline is 303,822,432 B
(289.75 MiB), over the proposed unconditional 64 MiB cap. The representative
uniform baselines are below that cap: Campaign 8,686,944 B (8.29 MiB) and
Kinematic 5,419,296 B (5.17 MiB). Applying the planned 64 MiB default
unchanged would fail loud on the Stress safety fixture even though id-41-only
coarsening passes its quality and I5 gates. Resolve that cap-policy conflict
before promotion; do not silently coarsen harder or treat the safety fixture
as a cap pass.

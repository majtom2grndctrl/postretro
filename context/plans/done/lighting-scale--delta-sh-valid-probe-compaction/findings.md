# Task 5 Findings — Delta-SH Valid-Probe Compaction

**Status:** complete with documented measurement limits. `campaign-test.map`
and the reduced `stress-warren-mini.map` fixture compiled with corrected
`--sh-analyze` accounting that agrees with compiler compaction logs. The full
`stress-warren-showcase.map` still has no completed PRL, byte, VRAM, or GPU
measurement. Do not treat that missing anchor result as zero or as a pass.

## Conditions and retained artifacts

- Measured campaign source: `45c6db65` (`Analyze compact SH delta payloads`),
  on top of `bff1fe77` (the compaction implementation). The later stress retry
  used `7baae10f` (cache header v2).
- Compiler builds passed: 8.78 s for the campaign source and 7.09 s for the
  cache-v2 retry source (`cargo build -p postretro-level-compiler`).
- Earlier temporary campaign/stress outputs were removed during the recovery
  purge and laptop restart. Their recorded figures below remain trace evidence,
  not currently retained artifacts.
- Current reduced-stress artifacts are `content/dev/maps/stress-warren-mini.prl`
  (265 MiB; SHA-256
  `ab0ad2052eefd7ab20cb237024cbbadaee274bc58d1db7cdd805bc9bb7ab344d`)
  and `content/dev/maps/stress-warren-mini.sh-analysis.json` (SHA-256
  `5745280a630a5cd668ba3f448c5017a53656190b41c118f25caa744cf4ea4295`).
- The measured PRL is
  `campaign-test.prl` (154,419,827 B; SHA-256
  `63700f342680f6892ba3a2559c7afc160dee175d52d7d4cd0b86fa125931e874`).
  The `RUST_LOG=info` replay emitted byte-accounting logs and wrote an identical
  PRL with the same SHA-256. Its corresponding analyzer JSON is
  `campaign-test-info.sh-analysis.json` (the first run's identical analysis is
  `campaign-test.sh-analysis.json`).
- Both successful campaign builds used the plan's `--sh-probe-spacing 2` m
  condition, default `0.04` m lightmap density, warm/approximate indirect SH,
  and `-j 4` to limit compiler concurrency after the stress build was killed.
  This is a development measurement, not a `--release`/shipping bake.

Commands:

```text
# First successful artifact, compiler total 67.92 s / wall 68.20 s.
time target/debug/prl-build content/dev/maps/campaign-test.map \
  -o /private/tmp/postretro-delta-sh-measure/campaign-test.prl \
  --no-tui -j 4 --sh-probe-spacing 2 --sh-analyze \
  --sh-analyze-out /private/tmp/postretro-delta-sh-measure/campaign-test.sh-analysis.json

# Accounting trace, compiler total 54.35 s / wall 54.54 s.
time env RUST_LOG=info target/debug/prl-build content/dev/maps/campaign-test.map \
  -o /private/tmp/postretro-delta-sh-measure/campaign-test-info.prl \
  --no-tui -j 4 --sh-probe-spacing 2 --sh-analyze \
  --sh-analyze-out /private/tmp/postretro-delta-sh-measure/campaign-test-info.sh-analysis.json
```

The second run is a trace/reproducibility run, not a cold-build benchmark. It
pruned 464 cache entries (5.25 GiB) before starting and still reported a mix of
cache hits and misses.

## Map results

| Map | Result | Output / duration | Measurement status |
|---|---|---|---|
| `campaign-test.map` | Passed | `campaign-test.prl`, 154,419,827 B; first build 68.20 s wall | Measured below. Compiler warned that indirect SH was approximate, and issued 61 non-fatal source/watertightness warnings. |
| `stress-warren-mini.map` | Passed | `stress-warren-mini.prl`, 265 MiB; compiler total 1,377.40 s | Reduced fixture from `origin/main`; measured below. It is playable but not interchangeable with the full showcase anchor. |
| `stress-warren-showcase.map` (initial) | Failed: exit 137 / SIGKILL | No PRL or analyzer JSON; output directory was empty after failure. Wall 16:49.72; 5,343.55 s user, 299.82 s system. | Not measurable. Killed in lightmap bake at 43%; latest ETA 1,295.2 s. Repeated cache warning: `cache payload exceeds u32 length prefix`. |
| `stress-warren-showcase.map` (cache v2 retry) | Failed: exit 137 / SIGKILL | No PRL or analyzer JSON. Wall 10:25.36; 1,212.79 s user, 169.14 s system. | Not measurable. Killed in lightmap bake at 13%; latest ETA 3,751.1 s. Cache staged at least 46 GiB before `No space left on device`; the observed signal does not establish a specific host-kill cause. |

The failed stress command was:

```text
time target/debug/prl-build content/dev/maps/stress-warren-showcase.map \
  -o /private/tmp/postretro-delta-sh-measure/stress-warren-showcase.prl \
  --no-tui --sh-probe-spacing 2 --sh-analyze \
  --sh-analyze-out /private/tmp/postretro-delta-sh-measure/stress-warren-showcase.sh-analysis.json
```

The required cache-v2 retry command was:

```text
time target/debug/prl-build content/dev/maps/stress-warren-showcase.map \
  -o /private/tmp/postretro-delta-sh-measure/stress-warren-showcase-v2.prl \
  --no-tui -j 4 --sh-probe-spacing 2 \
  --cache-dir /private/tmp/postretro-delta-sh-measure/cache-v2 \
  --cache-max-size 8GiB --sh-analyze \
  --sh-analyze-out /private/tmp/postretro-delta-sh-measure/stress-warren-showcase-v2.sh-analysis.json
```

The retry's cache effects were material: the dedicated `cache-v2` directory
reached **46 GiB**, despite the 8 GiB start-of-build cache budget, and the
filesystem had 15 GiB free at inspection (97% full). It was later explicitly
purged during recovery. The retry no longer reported the
old `u32 length prefix` staging failure; instead its final observed cache warning
was `No space left on device (os error 28)` while staging an entry. Since no PRL
was packed, no analyzer JSON was written and none of the payload/section ratios
can be measured for stress.

## Reduced Stress-Warren payload accounting

The successful reduced fixture used `--lightmap-density 0.08`,
`--sh-probe-spacing 1.75`, `--no-cache`, and `--sh-analyze`. It is a
resource-bounded development fixture, not a replacement for the full-showcase
anchor condition.

```text
RUST_LOG=info cargo run -p postretro-level-compiler -- \
  --lightmap-density 0.08 --sh-probe-spacing 1.75 --sh-analyze \
  --sh-analyze-out content/dev/maps/stress-warren-mini.sh-analysis.json \
  --no-cache content/dev/maps/stress-warren-mini.map
```

| Section | Dense baseline B | Compact payload B | Ratio / reduction | PRL section B | CSR entries |
|---|---:|---:|---:|---:|---:|
| id 27 indirect delta | 24,035,328 | 12,074,688 | 0.502373 / 49.76% | 12,100,694 | 1,304 |
| id 41 direct delta | 32,440,320 | 16,472,448 | 0.507777 / 49.22% | 16,500,250 | 1,760 |
| id 45 animated direct delta | 21,233,664 | 10,423,008 | 0.490872 / 50.91% | 10,448,406 | 1,152 |
| **Delta total** | **77,709,312** | **38,970,144** | **0.501486 / 49.85%** | **39,049,350** | **4,216** |

The compiler's three valid-probe-compaction lines exactly match the analyzer's
compact payloads. The packed sections independently report 39,049,350 B, with
79,206 B of descriptor, mask, CSR, and other metadata. Each reports 54,226
valid affinity-local probes. The grid is 72x11x128 = 101,376 probes, of which
54,226 are valid (53.4900%).

The compact delta total uses 14.517% of the 256 MiB cap, leaving 229,465,312 B;
the cap reports zero overage. Build time was 1,377.40 s: lightmap bake 111.61 s,
SH bake 1,059.53 s, and ShadowmaskAtlas 173.79 s. The analyzer has all three
delta sections and the base direct section. The shared ShadowmaskAtlas is
100,663,340 B (1024x1024x24, 25 selected lights); Lightmap is 125,829,168 B on
the same atlas layout. The packer reports 45,477,410 B SH, 232,227,324 B
non-SH, and 277,704,734 B total footprint.

## Campaign payload and section accounting

Definitions:

- **Dense baseline** is the compiler's post-exact-zero-drop, pre-valid-probe-
  compaction dense-64 payload. This is the analyzer's `uniform` line.
- **Compact payload** is the emitted variable-stride delta payload and is the
  analyzer's `(a)compacted` line.
- **PRL section bytes** include compact payload plus descriptor, masks, CSR,
  and other section metadata. They are expected to exceed compact payload bytes.
- Ratios are `compact payload / dense baseline`.

| Section | Dense baseline B | Compact payload B | Ratio / reduction | PRL section B | CSR entries |
|---|---:|---:|---:|---:|---:|
| id 27 indirect delta | 1,105,920 | 437,472 | 0.395573 / 60.44% | 443,158 | 60 |
| id 41 direct delta | 1,622,016 | 820,512 | 0.505859 / 49.41% | 826,290 | 88 |
| id 45 animated direct delta | 645,120 | 283,680 | 0.439732 / 56.03% | 289,266 | 35 |
| **Delta total** | **3,373,056** | **1,541,664** | **0.457053 / 54.29%** | **1,558,714** | **183** |

The compact payload total is 1,541,664 B. Its PRL-body total is 1,558,714 B;
the 17,050 B difference is section metadata (id 27: 5,686 B; id 41: 5,778 B;
id 45: 5,586 B), not an analyzer mismatch.

Cross-checks from the `RUST_LOG=info` run all agree:

- Compiler compaction lines: `1105920 -> 437472` (id 27),
  `1622016 -> 820512` (id 41), and `645120 -> 283680` (id 45).
- Compiler cap line: id 27 437,472 B; id 41 820,512 B; id 45 283,680 B;
  total 1,541,664 B.
- Corrected analyzer `(a)compacted` lines are exactly 437,472 B, 820,512 B,
  and 283,680 B respectively.
- PRL section table reports id 27 443,158 B, id 41 826,290 B, and id 45
  289,266 B. The packer's independent listing reports the same sizes and
  corresponding 60/88/35 CSR counts.

Before zero-drop, the compiler had 66/285/66 input entries for ids 27/41/45
(1,216,512 B / 5,253,120 B / 1,216,512 B). It retained 60/88/35 entries for
the dense baseline above. The analyzer's post-compaction 183-entry report has
an exact-zero fraction of 0.00546448; do not confuse that post-policy analysis
with the earlier input-entry drop counts.

### Validity and cap consequence

The campaign grid is 38x12x58 = 26,448 probes, of which 7,230 are valid:
**27.3367%** valid. The analyzer reports 450 bricks, 256 non-empty, 0 protected,
and all three delta sections present.

The cap is a fixed **256 MiB hard compile-error ceiling** (268,435,456 B) over
the three raw compact payloads. This campaign uses 0.5743% of that ceiling
(1,541,664 B), leaving 266,893,792 B. For the same payload mix, compaction
raises raw-payload capacity by **2.1879x** (`3,373,056 / 1,541,664`); equivalently
the 256 MiB cap corresponds to about 587,318,524 B of the measured dense
baseline. It does **not** select more lights or change light-selection policy.

## GPU / VRAM / boot attempt

Attempted command:

```text
env RUST_LOG=info POSTRETRO_GPU_TIMING=1 cargo run -p xtask -- run \
  --features dev-tools -- /private/tmp/postretro-delta-sh-measure/campaign-test.prl
```

- The engine compiled and launched, reporting adapter **AMD Radeon Pro 5300M**
  (`Metal`, `DiscreteGpu`). It reached `Window ready` without a reported wgpu
  validation error during renderer initialization.
- GPU compose timings are **unavailable**: the engine explicitly logged that
  this adapter lacks `TIMESTAMP_QUERY` and/or `TIMESTAMP_QUERY_INSIDE_ENCODERS`,
  then ran without GPU timing. The owner confirmed GPU timing cannot be
  established for this landing, so no compose-time figure is required or
  claimed.
- `ComposeStorageFootprint` is **unavailable**: the two-minute graphical run
  produced no level-loader or `SH compose ... storage footprint` log after
  `Window ready`; it was stopped with Ctrl-C (exit 130). Thus map-resource
  creation, full level boot, and the padded-bound-buffer footprint were not
  observed. Do not substitute PRL section bytes for this VRAM metric.
- Full first-frame boot with the fresh map and no validation errors is therefore
  **unconfirmed**. Only renderer initialization/window creation was observed.
- A follow-up mini-fixture load started the engine with the newly compiled PRL,
  but the terminal stream detached before level-install or footprint logs. The
  temporary process was stopped. This is inconclusive, not a successful
  end-to-end boot claim.
- A later user-run mini-fixture load reached `Window ready` and constructed the
  real 72x11x128 SH compose resources. Id 27 consumed 12,105,884 B of bound
  storage (12,074,688 B payload plus metadata); id 41 consumed 16,507,268 B
  (16,472,448 B payload plus metadata). Case-2 animated-direct compose was
  active with 1,152 CSR entries and a 1914x1908 atlas. No validation error was
  reported in the supplied load lines. That run predates the review follow-up
  which adds an id-45 `ComposeStorageFootprint` diagnostic; a restart with the
  final code is still required to record its at-rest bound-storage total rather
  than infer it from payload bytes.

## Focused verification

| Command | Wall time | Result |
|---|---:|---|
| `cargo fmt --all --check` after `45c6db65` | 5.18 s | **Failed.** Rustfmt reports drift in `level-compiler/src/main.rs`, `pipeline.rs`, and `sh_analyze.rs`; no reformat was made in this measurement task. |
| `cargo check -p postretro-level-compiler -p postretro-level-loader -p postretro-render-cpu -p postretro-renderer` after `45c6db65` | 2.55 s | Passed. |
| `CARGO_PROFILE_TEST_SPLIT_DEBUGINFO=off cargo test -p postretro-level-compiler --bin prl-build sh_analyze` | 0.80 s | Passed: 11, including mixed-mask rank, variable-length prefix, all-invalid zero-length, and all-valid dense-payload tests. |
| `CARGO_PROFILE_TEST_SPLIT_DEBUGINFO=off cargo test -p postretro-level-compiler --bin prl-build valid_probe_compaction` | 0.5 s | Passed: 3 id 27/id 41/id 45 x-fastest valid-tile compaction tests. |
| `CARGO_PROFILE_TEST_SPLIT_DEBUGINFO=off cargo test -p postretro-level-compiler --bin prl-build drop_then_compaction_caps` | 0.5 s | Passed: 3 cap-after-compaction / exact-zero-drop tests. |
| `CARGO_PROFILE_TEST_SPLIT_DEBUGINFO=off cargo test -p postretro-level-loader disagrees_with_id34` | 0.4 s | Passed: 3 id 27 reject / id 45 reject / id 41 selection-clear tests. |
| `CARGO_PROFILE_TEST_SPLIT_DEBUGINFO=off cargo test -p postretro-level-format mismatched_section_version` | 6.9 s | Passed: 4 matched (three delta stale-version tests plus one SDF test). |
| `CARGO_PROFILE_TEST_SPLIT_DEBUGINFO=off cargo test -p postretro-render-cpu delta_resolver` | 0.4 s after initial build | Passed: 3 within-cell rank and zero-length-prefix resolver tests. |
| `CARGO_PROFILE_TEST_SPLIT_DEBUGINFO=off cargo test -p postretro-renderer compaction` | 0.7 s after initial build | Passed: 2 combined id 41/id 45 metadata-buffer tests. |
| `CARGO_PROFILE_TEST_SPLIT_DEBUGINFO=off cargo test -p postretro-renderer skips_invalid` | 0.7 s | Passed: 2 invalid-local-before-read source tests. |
| `CARGO_PROFILE_TEST_SPLIT_DEBUGINFO=off cargo test -p postretro-renderer compose_layout_keeps_eight` | 0.7 s | Passed: 1 indirect eight-storage-buffer limit test. |

## Documented measurement limits

- GPU dense-vs-compacted composed-atlas parity for both maps: **unconfirmed**.
  No GPU dispatch/readback parity test exists; the stress map did not compile.
- Invalid-direct poisoned-texel GPU behavior and a source guard dedicated to the
  validity select: **unconfirmed**. Existing source behavior weights invalid
  corners to zero before direct-radiance multiplication, but there is no dedicated
  behavioral GPU or source-guard test.
- Consumer-stage bind-count non-regression: **review-only/unconfirmed** here.

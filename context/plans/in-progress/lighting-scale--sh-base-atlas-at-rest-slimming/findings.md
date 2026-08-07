# Findings — SH base atlas at-rest slimming

## Scope and method

This comparison uses the exact requested `campaign-test` bake configuration on
the same macOS 26.5.2 (`x86_64`) host:

```text
--sh-probe-spacing 1.0 --lightmap-density 0.8 --soft-shadow-samples 64
```

The before compiler/runtime source was detached at `70240c25` (the v8 state
immediately after this plan moved from `main`); the after source was
`b1d2a5ff` on `feature/lighting-scale--sh-base-atlas-at-rest-slimming`. The
source map did not differ between those revisions. Both compilers were
release builds in isolated target directories, their SH caches were first
warmed, and the reported artifacts were then produced with the same warm
cache. The v8 and v9 compiler commands were respectively run from the
baseline worktree and this worktree:

```sh
RUST_LOG=info /private/tmp/postretro-sh-base-v8-target/release/prl-build \
  content/dev/maps/campaign-test.map \
  -o /private/tmp/postretro-sh-base-metrics/campaign-test-v8-warm.prl \
  --sh-probe-spacing 1.0 --lightmap-density 0.8 --soft-shadow-samples 64 \
  --cache-dir /private/tmp/postretro-sh-base-v8-cache --no-tui -v

RUST_LOG=info /private/tmp/postretro-sh-base-v9-target/release/prl-build \
  content/dev/maps/campaign-test.map \
  -o /private/tmp/postretro-sh-base-metrics/campaign-test-v9.prl \
  --sh-probe-spacing 1.0 --lightmap-density 0.8 --soft-shadow-samples 64 \
  --cache-dir /private/tmp/postretro-sh-base-v9-cache --no-tui -v

RUST_LOG=info /private/tmp/postretro-sh-base-v9-target/release/prl-build \
  content/dev/maps/campaign-test.map \
  -o /private/tmp/postretro-sh-base-metrics/campaign-test-v9-uncompressed.prl \
  --sh-probe-spacing 1.0 --lightmap-density 0.8 --soft-shadow-samples 64 \
  --uncompressed-irradiance \
  --cache-dir /private/tmp/postretro-sh-base-v9-cache --no-tui -v
```

The v9 log's new compiler summary reports **57,128 / 194,028 valid probes**,
`compact 16,519,680 bytes`, `encoded 2,067,840 bytes`, `format tag 1`.
That confirms the recorded baseline probe counts rather than assuming them.

## Measured findings

Ratios below are `after / before`; a reduction factor is also shown where it
is more legible. Artifact size is `stat -f %z`, not a compiler display value.

| Measure | Before: v8 at `70240c25` | After: v9 BC6H default | After / before | v9 uncompressed debug path |
|---|---:|---:|---:|---:|
| id 34 (`OctahedralShVolume`) | 57,436,908 B | 3,621,256 B | 0.063048 (15.861× smaller; −53,815,652 B) | 18,073,096 B; 0.314660 (3.178× smaller; −39,363,812 B) |
| Total PRL | 111,797,146 B | 57,981,495 B | 0.518631 (1.928× smaller; −53,815,651 B) | 122,243,639 B; 1.093441 (+10,446,493 B) |

The uncompressed column is the intentional diagnostic bypass, not the
shipping default; the remaining v9 metadata/header changes make its total PRL
larger than v8 even though valid-only tiles shrink id 34.

Machine-readable record retained with this note:

```json
{
  "map": "campaign-test",
  "flags": ["--sh-probe-spacing=1.0", "--lightmap-density=0.8", "--soft-shadow-samples=64"],
  "before_commit": "70240c25",
  "after_commit": "b1d2a5ff",
  "valid_probes": 57128,
  "total_probes": 194028,
  "id34_bytes": { "v8": 57436908, "v9_bc6h": 3621256, "v9_rgba16f": 18073096 },
  "prl_bytes": { "v8": 111797146, "v9_bc6h": 57981495, "v9_rgba16f": 122243639 },
  "headless_load_seconds": { "v8": [1.12, 1.11, 1.11], "v9": [0.34, 0.33, 0.33] },
  "v9_devtools_base_atlas_vram": null,
  "v9_devtools_vram_reason": "no usable wgpu adapter"
}
```

### Dense geometry identity (parsed headers)

The values below are parsed field values, not byte-offset comparisons. Every
pre-existing dense-geometry value is identical across the paired bakes:

| Field | v8 | v9 |
|---|---|---|
| Grid origin | `(-72.33920288085938, -5.283199787139893, -65.83679962158203)` | same |
| Cell size | `(1, 1, 1)` | same |
| Grid dimensions | `(74, 23, 114)` | same |
| Dense atlas dimensions | `(2646, 2640)` | same |
| Tile dimension / border | `6 / 1` | same |
| Dense tiles per row / per layer / layers | `441 / 194040 / 1` | same |

The new v9 compact fields are deliberately separate: dimensions `(1440,
1434)`, 240 tiles per row, 57,360 tiles per layer, one layer, BC6H format tag
1, and 2,067,840 encoded bytes. They did not leak into the shared dense
sampler geometry.

### Base-atlas VRAM and runtime load timing

The headless runner loads the real PRL synchronously and completes CPU world
installation before exiting; it has no window, GPU, or display server. This
is the reproducible map-load timing available in this environment, so it does
not claim windowed first-frame time. Release `--features observability` cannot
be built here: `HeadlessSession` references
`postretro_scripting_core::watcher`, which is gated behind
`#[cfg(debug_assertions)]`. I therefore used matched **debug** runtimes and
the following command after one successful warm-up run, three times per
revision:

```sh
RUST_LOG=error /usr/bin/time -p \
  /private/tmp/postretro-sh-base-v9-target/debug/postretro \
  --headless /private/tmp/postretro-sh-base-metrics/v9-runspec.json > /dev/null

RUST_LOG=error /usr/bin/time -p \
  /private/tmp/postretro-sh-base-v8-target/debug/postretro \
  --headless /private/tmp/postretro-sh-base-metrics/v8-runspec.json > /dev/null
```

The debug v8 directory temporarily symlinked `scripts-build` to the release
`scripts-build` built from the same detached `70240c25` source. This merely
provides the engine-required script compiler sidecar; the measured runtime
binaries themselves are both debug builds. One successful run was discarded
to warm the OS cache and script path before each three-run series.

| Runtime map load (warm OS cache) | v8 | v9 | After / before |
|---|---:|---:|---:|
| Run 1 | 1.12 s | 0.34 s | 0.304 |
| Run 2 | 1.11 s | 0.33 s | 0.297 |
| Run 3 | 1.11 s | 0.33 s | 0.297 |
| Mean | 1.113 s | 0.333 s | 0.299401 (3.340× faster; −0.780 s) |

The v8 reference allocation is `2646 × 2640 × 8 = 55,883,520 B`
(`Rgba16Float`, calculated from the parsed dense header and the texture
format). The required v9 dev-tools once-per-load footprint log was attempted
through a real `capture,dev-tools` binary and is **not available**: wgpu
reported `frame capture requires a GPU adapter: No suitable graphics adapter
found` (`metal found no adapters`; the other enabled backends were likewise
unavailable). Therefore the v9 base-atlas VRAM and its ratio are **not
measured** in this environment. The v9 header/compile summary's 2,067,840 B
BC6H payload is retained as an artifact-size result above, not mislabelled as
a devtools VRAM observation.

| Base-atlas VRAM required source | Before | After | Ratio |
|---|---:|---:|---:|
| Once-per-load devtools footprint log | Not logged in v8; 55,883,520 B dense allocation is calculated from parsed dimensions and `Rgba16Float` | **Not measured:** no usable GPU adapter to run the v9 devtools log | N/A |

## Honesty-gate record

### Directly observed or automated

- **Compiler footprint summary:** observed once in the v9 info log with the
  valid/total count, compact bytes, encoded bytes, and format tag above.
- **Dense geometry identity:** observed by parsing the paired v8/v9 id-34
  headers; values are recorded above.
- **Sampler-side scope:** `git diff --name-only 70240c25..HEAD --
  crates/renderer/src/shaders` lists only `sh_compose.wgsl`; no sampler WGSL
  changed. The renderer diff contains no sampler bind-group layout edit. The
  base atlas is consumed by the compose grid binding; the sampler continues to
  receive the dense composed atlas.
- **Actual stale-version load:** the current v9 headless binary was pointed at
  the produced v8 PRL. It aborted at the loader with `octahedral sh volume
  section version 8, expected 9 — recompile the .prl with the current
  prl-build for the v9 compact-atlas format`; no session or GPU work followed.
  The focused parser regression also passed:
  `CARGO_TARGET_DIR=/private/tmp/postretro-sh-base-v9-target cargo test -p postretro-level-format octahedral_rejects_previous_section_version`
  (1 passed).
- **Marker implementation review (not a manual visual observation):**
  `MarkerMode::Validity` reads the metadata validity slice immediately;
  `MarkerMode::Irradiance` begins from its named neutral placeholder and swaps
  only when the composed-atlas readback completes. The readback uses
  `device.poll(Poll)`, not a blocking fixed-frame countdown, and is requested
  only when markers are visible in Irradiance mode. This establishes the
  intended mechanism, not its interactive behavior.
- **Capture isolation:** `encode_sh_probe_readback` is called by the windowed
  submit path, while offscreen capture owns a distinct submit/readback path;
  source inspection confirms the P11 ordering design but does not replace a
  live overlay exercise.
- **GPU capture attempt:** the real v9 PRL parsed successfully in the
  `capture,dev-tools` binary but renderer initialization failed because this
  environment exposes no usable wgpu adapter. It therefore provides neither a
  clean GPU boot pass nor a footprint log.

### Ordering pins P5–P11 (scope of observation)

| Pin | What was directly checked here | Status |
|---|---|---|
| P5 — one-frame overlay flick | Source review: turning `wanted` off only suppresses a new copy; `post_submit` still advances an already-copied/map-pending buffer. | Not live-tested; no claim about the re-enable experience or leaked state. |
| P6 — zero valid probes | Source review: the compact uploader supplies a 4×4 BC6H block or 1×1 RGBA16F texel for an empty compact payload, while the total atlas clamps dimensions to at least one. | Not boot-tested on an all-invalid fixture; no GPU adapter. |
| P7 — valid section, no usable probes | Source review identifies the separate `present == false` dummy path, but no fixture was loaded. | Not observed. |
| P8 — non-world frame | Source review: the windowed submit calls the readback after the normal frame submit, so it reads the retained composed texture; no dedicated non-world-frame run was possible. | Ordering implementation inspected only. |
| P9 — reload in flight | The callback captures only atomic readiness/pending flags; the live owner decodes/unmaps, avoiding a callback access to dropped GPU state. | Reload race not exercised. |
| P10 — animated-light cadence | No active-frame/animated-light overlay run was possible. | Not observed. |
| P11 — capture frame | Source review shows `encode_sh_probe_readback` is invoked only by `submit_windowed_frame`; offscreen capture has its own submit/readback path. The actual capture could not initialize wgpu. | Source-level check only; no live cadence result. |

The marker modes were not silently assumed: Validity and Irradiance are
distinct explicit `MarkerMode` branches in the reviewed source, but neither
could be selected in a live devtools UI without a usable adapter. Likewise,
no multi-second `RUST_LOG=info` windowed run could be made, so the
per-frame-noise gate is not passed from inspection alone.

### Pending or environment-gated at this point in the record

No source-level inspection is treated as a pass for the GPU/manual gates. The
stale-v8 reject and both three-run timing series are actual runs. Clean GPU
boot, dev-tools footprint log, explicit
Validity/Irradiance marker interaction, multi-second per-frame log check,
visual A/B (world/entity/fog and compact-remap seams), and the interactive
ordering pins P1–P11 are blocked by the absent adapter/display interaction.
P8/P11 have source-level ordering evidence only; P1–P7/P9/P10 have not been
observed in this environment.

## Required interpretation

**Composed-atlas VRAM and per-pixel sampler bandwidth are unchanged by this
spec.** The composed atlas remains dense and sampler shaders/bindings are
unchanged. The adaptive-base-probe-density sibling
(`lighting-scale--adaptive-base-probe-density`) owns the bandwidth lever; its
spike consumes this note's valid-probe accounting.

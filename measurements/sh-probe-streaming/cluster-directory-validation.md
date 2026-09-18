# Slice 2 cluster-directory validation

**Status: architectural proof complete; GPU portion of AC7 remains
`not-yet-evaluable` (2026-09-18).** Section 49 is deterministic, preserves the
old compiler output and cache contracts, round-trips through the production
loader, and remains an inert CPU-side index. This host exposed no Metal
adapter, so same-adapter output and allocation evidence was not fabricated from
CPU-only results. Windowed animated-billboard verification is also pending.

## Controls

| Control | Value |
| --- | --- |
| Source revision | `b3d61fd4e348d8c6b30714d82b9b5034eaa849bb` plus this validation helper/record |
| Source map | `content/dev/maps/gate-heavily-lit.map` |
| Source map SHA-256 | `96c107e4b3cb66e477b16e9376a305d44dffa544e70427859274e51784677faa` |
| Pre-directory baseline | SHA-256 `757dc6ae72e0c3362579b7ef5871c37f6bcbd82889bf1ce98fb6d3d1ea016637`; 30,480,375 bytes |
| Compiler | production `postretro-level-compiler` binary and packer |
| Container / loader | shared `level-format` reader/writer and production `level-loader` path |
| Cold cache mode | `--no-cache` |
| Irradiance | uncompressed RGBA16F for exact old-body comparison |
| Forced SH hierarchy scale | 1 |
| Cargo target | `/Users/dhiester/Projects/Personal/postretro/target` |

The two cold commands differed only in their explicit worker controls:

```sh
CARGO_TARGET_DIR=/Users/dhiester/Projects/Personal/postretro/target RAYON_NUM_THREADS=1 \
  cargo run -p postretro-level-compiler -- content/dev/maps/gate-heavily-lit.map \
  -o /private/tmp/postretro-sh-cluster-t7/gate-heavily-lit.cold-w1.prl \
  --no-cache --no-tui -j 1 --uncompressed-irradiance --sh-density-force-scale 1

CARGO_TARGET_DIR=/Users/dhiester/Projects/Personal/postretro/target RAYON_NUM_THREADS=8 \
  cargo run -p postretro-level-compiler -- content/dev/maps/gate-heavily-lit.map \
  -o /private/tmp/postretro-sh-cluster-t7/gate-heavily-lit.cold-w8.prl \
  --no-cache --no-tui -j 8 --uncompressed-irradiance --sh-density-force-scale 1
```

## Cold determinism and old-section preservation

| Evidence | 1 worker | 8 workers |
| --- | ---: | ---: |
| Total compiler time | 50.34 s | 11.88 s |
| SH bake | 22.51 s | 3.30 s |
| Lightmap bake | 26.52 s | 8.15 s |
| Cluster-directory stage | <0.01 s | <0.01 s |
| Pack | 0.10 s | 0.10 s |
| PRL bytes | 30,503,017 | 30,503,017 |
| Whole PRL SHA-256 | `cf2aca25f95b0a4309bc8d764778d40d2c569eb3468ad38ce18fe6ca7a5e1bd7` | same |
| Id 49 bytes / version | 22,620 / 1 | same |
| Id 49 SHA-256 | `376731e11d0b2f075261888e334b9df1e2bbf3144c338a83371af947a746d2e7` | same |

Both version-4 containers were byte-identical, not merely directory-identical.
The new id 49 entry is last. A table/body inspection compared the 22 old
entries against the retained pre-directory artifact in
`cluster-directory-baseline.md`: every section id retained its relative index,
every entry retained version 1, and every body length and SHA-256 matched
exactly. The unavoidable extra 22-byte table entry changes absolute payload
offsets only. These were uncompressed bakes, so no tolerance was applied.

The existing compact-output integration gate separately exercised production
BC6H. Its rule is intentionally not byte equality for the lossy encoder: both
cold products must identify BC6H and have equal compact-section length, while
the uncompressed products must be byte-identical. That gate passed, so this
validation neither weakens the BC6H exception nor applies it to id 49.

The compiler permutation regression also passed: permuting portal and BVH
primitive input order preserves the exact serialized directory and canonical
partition.

## Warm rebuild and bounded lifetime

Two eight-worker builds used the same isolated cache directory and otherwise
the cold recipe without `--no-cache`. The population build took 22.29 s. The
warm rebuild took 0.36 s and reported hits for the existing cached stages;
cluster-directory construction ran again uncached from finalized products.
Both PRLs were byte-identical (14,099,657 bytes, SHA-256
`0bdf73294de86a0d197104ce15cfe14c7751c2724ce2523973641d4cde0bd4ac`),
and both id 49 bodies matched the cold id 49 hash above.

The gate fixture produced 10 clusters, 911 coalesced ranges, 77 active affinity
cells, 77 covering references, and at most 77 adaptive nodes visited for one
cluster. Id 49 occupied 22,620 bytes. The reported construction metadata upper
bound was 23,652 bytes; construction took 4.408 ms on population and 3.592 ms
on the warm rebuild. Serialization retained the packer's one-payload-at-a-time
lifetime; it did not clone or re-encode an SH family.

The selected 64-primitive / 32-cell defaults remain grounded by the campaign
dry run in `cluster-directory-thresholds.md`: 181 clusters; primitive
p50/p95/max 0/62/64; cell p50/p95/max 1/15/32; 560,928 directory bytes;
462,096 dense addresses (88,080 max/cluster); 10,192 affinity addresses (1,920
max/cluster); 7,092 owned plus 3,100 halo addresses; 2,548 covering references;
587,156-byte construction and 1,148,084-byte serialization-peak upper bounds;
150.350 ms dry-run construction. This preserves explicit overlap/halo and peak
evidence rather than treating directory size as a resident-memory promise.

## Runtime-inert comparison

A current warm PRL was copied into the normal content tree so material lookup
used production roots. A test-only ignored integration helper then read that
file through `read_container`/`read_section_data` and rewrote it through
`write_prl`, omitting only id 49. It did not edit offsets or bytes in place.
The present file hash was `0bdf73294de86a0d197104ce15cfe14c7751c2724ce2523973641d4cde0bd4ac`;
the test-created removed copy hash was
`2661c6da73be92e536ce28d4c0e42003389fa7c2e220f007b25a873418ce524e`.

```sh
POSTRETRO_CLUSTER_SOURCE_PRL=/absolute/path/to/.slice2-cluster-directory-present.prl \
POSTRETRO_CLUSTER_STRIPPED_PRL=/absolute/path/to/.slice2-cluster-directory-removed.prl \
CARGO_TARGET_DIR=/Users/dhiester/Projects/Personal/postretro/target \
  cargo test -p postretro-level-format --test cluster_directory_capture_fixture \
  -- --ignored --nocapture

CARGO_TARGET_DIR=/Users/dhiester/Projects/Personal/postretro/target \
POSTRETRO_GPU_TIMING=1 cargo run -p xtask -- capture \
  /private/tmp/postretro-sh-cluster-t7/present.scene.json
```

The fixed scene used the map's player spawn at `[-3.556, 1.016, -2.54]`, yaw
-90 degrees, 1280x720, with 120 warmup and 120 sample frames. Renderer creation
failed before either file could be captured:

```text
frame capture requires a GPU adapter: No suitable graphics adapter found;
noop not requested, vulkan support not compiled in, metal found no adapters,
dx12 support not compiled in, gl not requested, webgpu support not compiled in
```

Because the same-adapter prerequisite was absent, the removed-file capture was
not run and no PNG or GPU report was published. Exact RGBA, SH allocation, and
per-frame GPU allocation/work comparison therefore remain
`not-yet-evaluable`; CPU loader evidence is not a substitute. Static code and
loader tests establish no renderer plumbing and unchanged legacy lighting
payloads when id 49 is absent. Manual windowed animated-billboard lighting is
pending on an adapter-equipped host.

## Verification commands and results

```sh
cargo test -p postretro-level-format cluster_directory
cargo test -p postretro-level-compiler --bin prl-build cluster_directory
cargo test -p postretro-level-loader --features load-prl cluster_directory
cargo test -p postretro-level-loader --features load-prl \
  load_prl_retains_scatter_for_valid_empty_animated_pair
cargo test -p postretro-level-compiler --bin prl-build \
  sh_cold_grouped_equals_monolithic_on_fixtures -- --ignored
cargo test -p postretro-level-compiler --bin prl-build \
  warm_sh_within_tolerance_on_fixtures -- --ignored
cargo test -p postretro-level-compiler --test compiler_cli_contract \
  gate_heavily_lit_cold_compact_sh_output_is_deterministic -- --ignored
cargo clippy -p postretro-level-format -p postretro-level-loader \
  -p postretro-level-compiler -- -D warnings
cargo fmt --all --check
cargo test --workspace
```

Focused results were 13 format tests, 3 compiler-directory tests (plus the
ignored manual helper), 4 loader-directory tests, and the valid-empty companion
regression, all passing. The closure-locality cases cover both a wide scale-0
grid and exact scale>0 cube membership. Cold grouped-vs-monolithic passed in
37.67 s. Warm SH tolerance passed in 67.61 s (`gate-heavily-lit`: mean 0.00437,
p99 0.04832, max 0.18560). The cold compact integration gate passed in 201.26
s. Touched-crate clippy and formatting passed.

Default workspace tests reached 763 passes, 2 ignored, and one localhost-bind
failure caused by sandbox policy. The same test was rerun outside the sandbox
and passed (1/1). `--workspace --all-features` is not a supported combined
configuration here: existing `observability/driver.rs` code refers to a
feature-excluded `scripting_systems::health`. Workspace-wide clippy also finds
pre-existing renderer argument-count and test-only complexity lints; the three
touched crates are warning-clean.

## Acceptance mapping

| AC | Result | Evidence |
| --- | --- | --- |
| AC1 | Pass | Canonical partition/property tests, disconnected/budget fixture, and pinned positive defaults/distribution record. |
| AC2 | Pass | Exact 1-vs-8-worker id 49 and whole-file equality, input permutation regression, valid warm rebuild. |
| AC3 | Pass | Codec/compiler/loader resource-inventory, valid-empty, partial-edge, compression-independent addressing, and closure tests. |
| AC4 | Pass | Ownership/halo, optional-companion agreement, and closure-locality regressions plus campaign overlap distribution. |
| AC5 | Pass | Production publication/load round-trip and named structural/semantic rejection matrix. |
| AC6 | Pass | Production emitted/parsed inventory tests, valid-empty animated pair, and existing optional-companion regressions. |
| AC7 | **Open / not-yet-evaluable** | Inert CPU storage/no renderer consumer is proven; same-adapter RGBA, GPU/SH allocations, per-frame work, and animated billboard remain pending because Metal exposed no adapter. |
| AC8 | Pass | All 22 uncompressed legacy bodies exact, cold/warm SH gates pass, BC6H gate retains its lossy rule, cache hits preserved, stage reported. |
| AC9 | Pass | Borrowed final-emission view, bounded closure/overflow tests, lifetime/peak/timing evidence, touched-crate supported builds. |

## Artifact and disk lifecycle

Only this record and the small ignored rewrite helper are retained. After
hashing/comparison, the temporary cold/warm PRLs, cache, scene files, hidden
content copies, and the pre-directory baseline directory were deleted. No PNG
or report was created.

Disk was checked at milestones. Free space fell from 13 GiB to 9.84 GiB, so a
crate-scoped clean of `postretro`, renderer, compiler, loader, and format
artifacts removed 36,398 files / 13.2 GiB. Final verification reduced free
space to 5.3 GiB; a second scoped clean removed 9,934 files / 4.6 GiB from
those crates, then an explicit scoped clean of the remaining PostRetro crates
removed 40,353 files / 22.2 GiB. No bare `cargo clean` was used.

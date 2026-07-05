# Research — level-compiler-tui

Grounding notes for the `prl-build` UX overhaul. Verified against source on branch
`claude/compiler-ux-improvements-905sxt`. All line numbers are `crates/level-compiler/src/` unless noted.

## main.rs is oversized

`main.rs` is **2616 lines** (`wc -l`). `fn main` (line 357) is one linear function spanning
357–1270 — parse args, cache setup, then 21 hard-coded inline stages. Far past the ~800-line
split threshold. Split-before-extend applies: extract the stage pipeline before adding the
Reporter/Governor.

## Stage inventory (from `start_stage(...)` / `timings.push(...)`)

21 timed stages, in order. Each fires `progress.start_stage("...")` then `timings.push((name, elapsed))`.

| # | start_stage label | timings name | Line | Parallel? | Skip condition |
|---|---|---|---|---|---|
| 1 | Parsing map | Parsing | 429 | no | — |
| 2 | Data script compilation | DataScript | 434 | no | no-op if no `data_script` |
| 3 | Texture color-space validation | TexValidation | 440 | no | — |
| 4 | BSP partitioning | Partitioning | 451 | no | — |
| 5 | Visibility computation | Visibility | 459 | no | — |
| 6 | Geometry extraction | Geometry | 479 | no | — |
| 7 | BVH build | BVH Build | 494 | no | — |
| 8 | NavMesh bake | NavMesh | 514 | no | early-out "no walkable region" |
| 9 | Lightmap bake | Lightmap Bake | 579 | **SERIAL** | placeholder if no static lights |
| 10 | SH volume bake | SH Bake | 781 | **par** | — |
| 11 | Delta SH volume bake | Delta SH Bake | 818 | **par** | `None` if no animated lights |
| 12 | Direct SH volume bake | Direct SH Bake | 841 | **par** | skipped if no static lights |
| 13 | Entity shadow light selection | EntityShadowLights | 890 | no | `None` if no DirectShVolume |
| 14 | Direct SH delta volume bake | Direct SH Delta Bake | 920 | **par** | `None` if no selection |
| 15 | Shadowmask atlas bake | ShadowmaskAtlas | 984 | no | `None` if no selection |
| 16 | Chunk light list bake | ChunkLightList | 1023 | no | — |
| 17 | Animated light chunks | AnimLightChunks | 1058 | no | empty if no animated |
| 18 | Animated light weight maps | AnimWeightMaps | 1073 | **par** | `None` if no animated chunks |
| 19 | SDF atlas bake | SDF Atlas Bake | 1155 | no | **only present** when `map_needs_sdf_atlas` |
| 20 | Texture mip bake | TextureMips | 1211 | no | — |
| 21 | Packing and writing | Packing | 1220 | no | — |

Stage 19 (SDF) is the only `start_stage` inside an `if` (guarded by `map_needs_sdf_atlas(&map_data.lights)`,
line 1154) — the only stage whose *row* is conditional. Every other stage always fires its
`start_stage`; several compute `None`/placeholder output and could render as "skipped." Most skip
conditions are predictable up front from `map_data.lights` (static count, animated count,
`map_needs_sdf_atlas`); NavMesh walkability and entity-shadow selection resolve mid-build.

## BuildProgress (current progress abstraction)

`struct BuildProgress { started: Instant, pb: Option<ProgressBar>, verbose: bool }` at line 49.
- `new(started, verbose)` — 56
- `start_stage(&mut self, msg)` — 64. Non-verbose: one `indicatif` spinner, template
  `"{elapsed:>4}  {spinner} {msg}"`, 100 ms steady tick. Verbose: `eprintln!` timestamped line.
- `finish(&mut self)` — 86.
Constructed at 427; driven inline by `fn main`. Build Summary table `println!` at 1259–1267.

## Logging

`env_logger::Builder::from_env(...default_filter_or(log_level)).init()` at line 367; `log_level` is
`"info"` when `--verbose` else `"warn"` (366). **99 `log::warn!` / `warn!` / `eprintln!` sites across
16 files** (`grep -rc` confirmed — matches brief). Warnings are fired-and-forgotten: nothing tallies
or collects them. Both env_logger and the indicatif spinner draw to stderr uncoordinated — that is
the clobbering bug.

**Plumbing consequence:** routing warnings through a Reporter by editing 99 call sites is the wrong
seam. Install a custom `log::Log` backend (`log::set_boxed_logger`) that forwards records to a shared
sink the active Reporter drains, and tallies `warn`+ counts. Preserves `RUST_LOG`/verbose filtering
without touching the 99 sites.

## Concurrency

- rayon uses the **default global pool** (all cores). No `ThreadPoolBuilder` / `build_global`
  anywhere in the crate (`grep` empty). rayon pools cannot be resized after construction — confirming
  the Governor approach over pool resizing.
- `rayon = "1"` is a workspace dep; the crate re-exports via `rayon.workspace = true`.
- **Parallel work-item sites** (all order-preserving `.map()/.flat_map().collect()` — no float reduce,
  no HashMap iteration; determinism depends on this):

| File | Line | Closure entry fn | Work item |
|---|---|---|---|
| `sh_bake.rs` | 245 `(0..total).into_par_iter()` | `bake_probe` | one SH probe (`layout.total_probes()`) |
| `sh_group.rs` | 674 `groups.par_iter()` | `bake_or_load_group` | one 4³ probe group |
| `direct_sh_bake.rs` | 222 `(0..total).into_par_iter()` | `bake_probe_tile` | one direct SH probe tile |
| `direct_sh_bake.rs` | 312 `affinity_lights.par_iter().zip(...)` | `bake_direct_delta_subblock` | one CSR delta sub-block |
| `delta_sh_bake.rs` | 168 `affinity_lights.par_iter().zip(...)` | `bake_subblock` | one CSR delta sub-block |
| `animated_light_weight_maps.rs` | 93 `chunks.par_iter()` | `bake_one_chunk` | one animated-light chunk |

- **Lightmap is fully serial, warm AND cold.** No rayon in `lightmap_bake.rs` or `lightmap_layer.rs`.
  Cold: `bake_monolithic_atlas` (459) with a plain `for (face_idx, placement) in placements.iter()`
  loop at 473. Warm: `bake_light_layer` per-light layers, also serial. **The core throttle cannot
  affect the lightmap stage** — must be documented. Pause must still reach it: `checkpoint()` inside
  the per-face loop.

## Progress / ETA quantifiability

Per-stage totals known before the parallel loop starts:
- SH: `layout.total_probes()` (sh_bake 244; sh_group via group count).
- Direct SH: `layout.total_probes()` (direct_sh_bake 215).
- Delta / Direct-delta: `affinity_lights.len()` (CSR entry count).
- Weight maps: `inputs.chunk_section.chunks.len()`.
- Lightmap: `placements.len()` (serial per-face).

`AtomicUsize` counters incremented inside the rayon closures feed the % display. They do **not** feed
output and do not touch the order-preserving collect, so the pre-BC6H byte-identity determinism
invariant (build_pipeline.md §Determinism) is preserved. Per-stage % is reliable; whole-build ETA is
best-effort (optional per-stage-timing priors persisted dev-local).

## Dependencies — discrepancy with brief

`crates/level-compiler/Cargo.toml` **direct** deps include `indicatif = "0.17"`, `log`, `env_logger`,
`rayon`, `blake3`, `postcard`, etc. The brief listed `console`, `is-terminal`, `termcolor`,
`number_prefix` as "present" — **they are transitive only** (in `Cargo.lock` via indicatif), NOT
declared in this crate's `Cargo.toml`, so they cannot be `use`d without adding them. `console`,
`is-terminal`, `number_prefix`, `termcolor` all appear in `Cargo.lock`; **`ratatui`, `crossterm`,
`termion` do not** — the TUI stack must be added. `std::io::IsTerminal` is in std (no dep). main.rs
uses only `std::io::stdin()` today (315, the `precheck_output_dir` prompt) — no TTY detection yet.

## TTY gating / fallback anchor

`precheck_output_dir` (308–355) already models graceful non-TTY handling: reads stdin, treats EOF
(read_line == 0, line 327) as "no answer → abort" rather than looping. The TUI must gate on
`stdout().is_terminal() && stderr().is_terminal()` and fall back to the plain reporter for
CI / xtask / pipes / non-TTY stdin.

## Constraints confirmed

- No GPU dep in this crate ("renderer owns GPU" does not apply). No `unsafe`. All changes confined to
  `crates/level-compiler/`.
- Existing CLI flags parsed in `parse_args_from` (1360): `-o -v --format --sh-probe-spacing
  --lightmap-density --soft-shadow-samples --sdf-voxel-size --cache-dir --cache-max-size --no-cache
  --release --uncompressed-irradiance`. `Args` struct at 1272. No `-j`/`--jobs`, no `--tui` yet.
  `help_text()` (1322) is built from live default constants — new flags must extend it.
</content>
</invoke>

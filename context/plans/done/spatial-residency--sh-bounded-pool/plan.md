# spatial-residency--sh-bounded-pool — plan of record

mode: resumable
status: done
read at: b5c39cab8

## Owner rulings

The plan stopped blocked on four Acceptance rows written against a slot-level reader epoch that does not exist: `evict` frees at once (`sh_streaming/rows.rs:128-135`), the allocator can reuse immediately, and queue order is the only protection (`lifecycle.rs:19-25`). The owner delegated the rulings (2026-11-05): re-examine the recommendations, apply what survives, proceed. The brief and `research.md` now carry the result.

- **Reader epoch removed.** Held on review. Slot retirement would protect no reader. Rows 3 and 7 take the wording first proposed.
- **R5 and R6 reworded.** Shifted on review. Same-drain eviction (Delegated answers) removes the "requested but unconfirmed" wait the first wording named. The one real wait left is a retiring growth generation, so both rows point there.
- **Planar mirrors do not join the bound in this build.** Shifted on review, from a first recommendation of yes. A sweep needs a crate move and a cached stage, and buys nothing measurable: three of the four bound maps have no water, `campaign-test` could rise 2,092 slots at most, and the hallway decides the cluster rule. Planar runs only at very high and ultra, so a mirror-inclusive figure charges default-tier players for an opt-in tier. Growth is already the logged net, and it is reached only after optional eviction, in a cell near the worst. `spatial-residency--lightmap-bounded-pool` also dropped its mirror sweep on the owner's decision, so no sweep exists; a large water map that grows reopens the question for both bounds. A mirror extension would still need no layout change, since id 49 stores figures rather than the cells behind them. Row 39 stays as written: it measures `reflection-lake-test`, and growth there is reported rather than extended here.
- **Refusal stays black, growth stays.** Held on review.
- **The hold stands** (owner, 2026-11-05), on the task 4 evidence below, with the pinned-clock rerun declined. Row 40's review is done; the open case, one absent cluster at `campaign-test` 1,416 m, stays for row 37's frames at the final gate.
- **Cluster size rule: today's limit stays.** A 2-layer pool gave no mandatory refusals and no growth. Static-only, one hallway shot stayed signed beyond noise, and it shows a mover. No rebake for cluster size.
- **Land step, rendering_pipeline.md §4.** Delete "It never requests probes.", false for SH on a completed mirror walk. `draft/lightmap-bounded-pool` makes the identical edit on the owner's instruction, so the two merge cleanly.
- **Id 49 keeps its baked-maximum entry** (owner, 2026-11-05). The brief stands as written: two entries, default lead and baked maximum. Recommended dropping it to match the lightmap's id 58; the owner kept it for the named memory-tier work, accepting a later refactor. Task 6 therefore prices both entries on every build.

## Task 4 evidence so far (2026-11-05)

Eight recorded runs, hallway and `campaign-test`, HEAD at 9d37ea6c9 against `today` at b5c39cab8, two runs per build, indirect-only shots (mask `0x6`), quiet-gated. The screen locked before the static-only discriminator ran. **Superseded by the owner's ruling below.** Raw data: session scratchpad `sh-task4/`, plus `measure/sh-bounded-pool`.

- Hallway held frames: 44 solid-cell frames, misses 579 against today's 323, all on held frames. Frames match today: worst block 0.05 of 255, noise 0.00.
- `campaign-test`: 217 held poses, 29 misses in both builds. Twelve frames near 1,416 m are darker at HEAD in all four pairings (−1.05 to −2.71 of 255). Eleven near 1,392 m are the reverse: `today` is dark at matching poses. Cause unproven. Reading the frames, both dim the whole scene evenly, and a pool cannot brighten, so a build-consistent clock offset against animated lights is the leading candidate. Static-only (mask `0x2`) discriminates.
- Cluster rule: no mandatory refusals and no growth at 2 layers, but 7 hallway shots exceed noise in all pairings, with mixed signs at one location (0136a +2.3, 0136s −3.2). Same confound; the same rerun settles it.
- Walk: mandatory union 159,554; live p50 131,472, max 203,315. Evictions 668–670 against about 1,041. Re-reads 468–470 against about 904. Peak `ioreg` in-use 3.0 against 4.1 GB.

**Static-only batch (mask `0x2`), 8 of 8 valid.** All 217 `campaign-test` held frames are within noise; 185 have real static light, and the frames at 1,392 m and 1,416 m are not among them, so that match says nothing there. Hallway held frames match (block 0.05, signed 0.000). Only 0184s stays signed beyond noise (+5.25 to +6.79), and it and 0185a show different geometry, both lit: a mover at a different moment.

**Clock.** HEAD reaches each point sooner, by 2.8 s at the hallway's end and about 1.1 s at `campaign-test`'s dark bands, because it stalls less (frames over 50 ms total 10.0 s against 13.0 s). Animated light and movers therefore sit at a different phase at matched poses. There is no frame-cost regression.

**Equal clock against equal pose, `campaign-test` 1,370–1,430 m.** Mean difference falls from 2.4–2.6 of 255 at equal pose to 0.10–0.17 at equal clock. Only 3 and 7 shot pairs line up, at differing poses, so this is strong but thin. Held frames with an absent cluster fell outside that comparison, so that case is untested there; HEAD shows one absent cluster at 1,416 m where `today` shows none. A pinned-clock rerun would settle both, but needs a cold rebuild the disk budget does not allow now.

## Review loop (2026-11-05)

Preflight passed at 2c67be9a0: fmt, clippy `--all-targets` with dev-tools, `cargo test` plain, dev-tools and `capture`, release check, crate graph. It reruns after the fixes below. Review panel: 10 reviewers (8 depth, 2 hygiene), no 🔴. Reference counts, journal rollback, wire format, bound and hashing, the paused clock and controller permits all traced clean.

Fixes, landed through the gate below:
- **Whole-map fact replaces the latch.** The renderer reports `pool_holds_whole_map` per drain only when every family is whole and nothing was ever released this level. A level reaching the cap any other way keeps evicting, so fragmentation stays recoverable.
- **`TargetClass::Held`.** A drawn resident on a held frame with no reach or pin class: a target, so departure leaves it alone, but not mandatory, not settled, and the last capacity victim after Hysteresis. In-flight work keeps its prior class. Clarifies row 19: "residents stay" holds except as the last resort under real pool pressure.
- **Cap refusals retry** once per hysteresis window without leaving targets, so a refused pin is not black until reload.
- Smaller: any retiring wait wins in `waits_on` (R5); bound plus 25% clamps to the device limit; `release_unbacked_compose_waits` loses its `was_pending` gate; `pool_bound` enforces the lead-0 entry; `bound.rs` accepts exactly the streamed families; `is_usable` agrees with the compiler; floor naming goes.
- Author text: Build Summary and `docs/level_design.md` wording, `sh_pool_cost.rs` and `shared_bytes.rs` comments. Build Summary reads `dense demand peaks in cell N, near (x, y, z) engine metres`, so row 17 still names the worst cell; the warning gives the position alone, because the headline has the id. Engine metres stay: the compiler has no clean inverse to map space.

Gate, first compile, no failures: workspace `cargo check --all-targets` with dev-tools; format 666, `prl-build` `sh_mandatory` 15, `sh_mandatory_bound_compile` 2, loader 333, render-cpu 643, renderer `sh_streaming` 226 and app `sh_` 285 and `level_streaming` 20, GPU required. Each of the twelve fixes was read in source; the trace baseline needed no change. No new findings, so no re-review. `settling_flag_lifts_the_sh_hold...` now uses a Band entry, because a one-cell fixture with any id 51 entry always reads. Scope addition: the device clamp also covers dense (`device_slot_limit`). For §4 at landing: add `Held`; stopping eviction needs every family whole and nothing released; "a refused pin stays unavailable" now covers device limits only. Known and bounded: evicting a Held resident reads as a horizon change next frame, lifting suppression once, and after the hold it may read back as Hysteresis.

Preflight passed again at ae99bc0b7, GPU required: fmt, clippy `--all-targets` with dev-tools, `cargo test` plain 14,480, dev-tools 14,588, `capture` 2,528, release check, crate graph.

**Final measurement setup (2026-11-05), waiting on an unlocked screen.** Cold v3 bakes at ae99bc0b7, `--release`, sit apart from the v2 PRLs, so `today` still runs. Every bound reproduces `research.md`: hallway 168,017 and 216,952 at the maximum lead (317.7 and 414.1 MiB), `campaign-test` 39,634 and 41,726, `occlusion-test` 18,327, `stress-street` 117,707 (212.8 MiB). Hallway cold bake took 1,413 s. `reflection-lake-test` bounds at 297,973 slots (329.0 MiB).

`stress-street` means default 1 m spacing with `--lightmap-density 0.25`, not the README's 4.0 m, which gives 2,158 slots and exercises nothing. No v2 PRL existed, so the same flags went through a compiler built at b5c39cab8; the pair differs only in id 49. The on-disk v2 `reflection-lake-test` used other bake settings (lightmap and SH sections differ), so it is no baseline for this map; row 39 reads the pool alone.

Tooling: `measure/sh-bounded-pool-final` at 6608e220a, on ae99bc0b7, no sizing levers, `postretro-final` built with `--profile profiling --features dev-tools`. It must never merge. Run `tools/measure-harness/pilot.sh`, then `batch.sh plan-final.txt`, with `T` and `PRL_DIR` set; `today` needs its own plan and `PRL_DIR` for `stress-street`. The `XCTRACE` mode is untested against the real engine. Row 39's tier comes from `settings.toml`, already `very_high`. Row 42's reload is a keypress: `Alt+Shift+L` loads `content/dev/maps/combat-demo.prl`, so that path needs the v3 bake with its v2 set aside. Scratchpad, which a reboot may clear: `/private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/54ee9970-513a-458c-8a48-d075ca2707a3/scratchpad/sh-final/` (`bin`, `bin-today`, `prl-v3`, `prl-v2`, `logs`).

**Final walks (2026-11-05), shipped sizing, cold v3 PRLs.** Thirteen recorded walks plus a shot-free Metal capture, all valid. No SH growth on any map, capacity constant, `at_whole_map_cap` 0 everywhere. Only the hallway evicts: `campaign-test`, `occlusion-test`, `stress-street` and `reflection-lake-test` hold the whole map on every walk frame, so that zero proves least there.

| Map | Capacity (slots) | SH pool | Misses | p50 / p99 ms |
|---|---|---|---|---|
| Hallway | 211,248 | 399 MiB | 579, all held; 0 on 22,399 portal frames | 17 / 44–45 |
| `campaign-test` | 42,048 | 79 MiB | 2 | 17 / 52–53 |
| `occlusion-test` | 18,432 | 31 MiB | 0 | 17 / 47–48 |
| `stress-street` | 118,096 | 213 MiB | 0 | 17 / 41–42 |
| `reflection-lake-test` | 297,992 | 329 MiB | 0 | 17 / 39 |

Hallway live peaked at 210,505 of 211,248; 2–3 mandatory shortfalls per walk, all placed by eviction; 77–78 optional refusals.

**Row 35, Metal Resource Events, `resource-size`, hallway.** Steady 2,170 MiB against 3,011; peak 2,574 MiB against 3,577, at 27 s. Video-memory-only figures are 2,009 and 2,415. Which column the spike read is unconfirmed. Steady is flat from 60 s to the end. The peak is lightmap pool growth (116/58/232 → 140/70/280 MiB, old and new alive together), not SH, so it belongs to `drafts/lightmap-pool-growth-peak.md`. It runs about 370 MiB above the brief's near-2.2 GB projection. A full route records about 19 GB of raw `xctrace` data in the user temp directory, left there on exit, so budget about 25 GB and delete it after.

**Shots inflate frame times.** Every frame over 100 ms in the shot runs is a held frame taking a shot. With no shots: 0 over 100 ms, max 86 ms, p99 23 ms.

**Row 41, owner ruling (2026-11-05): a memory row.** Pool size is constant across the hallway's 44 solid-cell frames. Frame time stays a finding, not part of the row: median 32.5 ms against 17 ms beside them, shot-free, not attributed to the pool. No reboot came between these runs, and only solid-cell frames slow, so it reads as drawing work rather than the Mac slowdown. Whether it predates this brief is unmeasured.

**Row 38, owner, `closet-reveal`.** Door opened; the owner watched the closet and saw steady lighting, no snap or pop, over two sessions. Logs: 0 SH misses, evictions, refusals or growth. Limit: the map's demand is 200 slots, so it cannot be under pool pressure, and no map here both evicts and opens a door. Unexplained, likely unrelated: `[candidate-cull] counter map failed: BufferAsyncError` at manual exit; tour exits never log it.

**Row 42, owner keypress.** Hallway loaded, then six `Alt+Shift+L` cycles to `combat-demo`, sampled every 2 s. In-use fell from 2,463 MiB to 1,441 MiB at the first unload, then held at 1,436–1,463 MiB through five reloads. Exit returned to 862 MB against 819 MB before launch, inside the 750–864 MB spread seen before launches tonight. The v2 `combat-demo.prl` is restored.

**Row 37, owner.** Reviewed the four worst `campaign-test` pairs: most sit in a room with a pulsing animated light, so brightness there is not a pool signal. Row closed.

**Row 39.** The columns cannot show a mirror in view. The pool holds the whole map, so a mirror cannot raise demand past it either way.

**Row 37, automated half.** On-disk v2 PRLs are not baselines: `campaign-test` differed from a fresh bake in 19 of 37 sections, which sizes hid, and `occlusion-test` was stale. Both were redone from a compiler at b5c39cab8 (`*-today2-*`), and now differ only in id 49. The hallway v2 differs only in ids 49 and 50, as designed. Equal clock, over noise: `campaign-test` 0 of 31 pairs, `occlusion-test` 0 of 87, `stress-street` 0 of 30, hallway 0 of 16. Matched pose: `campaign-test` 38 and 41 shots over noise, signed mean +0.065, split evenly by sign; worst −20.7 at `h01417492`, which has no absent cluster in `final`, so pool loss cannot explain it. `today` has 30 misses there against `final`'s 2, and the 1,416 m case is gone: `final` has no absent frames past 151 m. Frames at `today`'s miss sites sit within noise except `h01415741`, where `final` is brighter.

Landing corrections: "in-flight work keeps its prior class" above means a reach, pin or seam class; with none of those, in-flight drawn work on a held frame takes `Held`. `controller.rs`'s cap-refusal comment said "target or not"; the retry skips non-targets, so a cluster retries on re-entering them. Comment fixed at landing, behaviour unchanged. §Build Cache needs no epoch bump: every compiler change is in uncached stages, and no cached stage's output moved.

Accepted, not fixed: a victim that frees nothing is still evicted; path flips act as horizon changes; seam suppression can lift on the portal frame after a hold; a direct Pass B failure can promote one frame early.

## Corrections

- "Id 49 is emitted at pack, after id 51 exists, so the bound needs no new stage" → the directory is built in the uncached `StageId::ClusterDirectory` stage (`pipeline.rs:2092`) and serialized at `pack/section_plan.rs:497`. Per-cluster node sets exist only in `ChunkPlan` (`pack/cluster_sh_payloads/dense.rs:47-98`), which is built at `pack.rs:630` from a shared borrow of the directory. No new stage is needed, but plumbing is: lift `ChunkPlan`'s sets out, compute the bound in pack, pass it to id-49 serialization as an argument, and return it through `PrlFootprint` for the Build Summary (printed at `pipeline.rs:2175-2181`).
- Wire `worst_units` is one number per family → sparse capacity is two-dimensional, `(entries, tile_f16)` (`floor.rs:240-285, 376-385`). Family entries carry `u32 worst_entries, u32 worst_tile_f16`. The meaning is unchanged; the wire shape changes.
- "Roles reduce to dense and affinity" → after owner removal, role equals `resource.domain` (validated at `cluster_directory.rs:562-573`). Under "no unread fields stay behind", the range record drops role entirely.
- The brief omits id 50's own role word → each sparse row record carries one (`pack/cluster_sh_payloads/sparse.rs:237`, checked at `decode.rs:246-250`). It goes too, and `CLUSTER_SH_PAYLOADS_VERSION` moves 1 → 2. Both id-49 version constants move: `CLUSTER_DIRECTORY_VERSION` and `CLUSTER_DIRECTORY_CONTAINER_VERSION`. The mismatch error gains the id-51 style "re-bake the level" text.
- "`RowRefTable` already counts affinity rows" → it counts compose-membership contributors, not storage lifetime (`row_refs.rs:21, 131-146`). Sparse rows are written by one owner (`sparse_install.rs:43`), and `install_undoable` errors on a live row (`allocator.rs:205`). Node slots and probe words have no count at all. Three new counts are needed, each with a journal undo: nodes, probe words, sparse rows. The journal's undo list is `install_journal.rs:13-30`.
- Not in the brief: **a second referencer must wait for the first writer's compose.** `required_compose_epochs` returns the current epoch when a cluster wrote nothing (`sh_streaming.rs:504`), and today only `owner_dependencies` holds it back. With owners gone, a cluster whose nodes are all shared could promote before they compose and publish uncomposed words. Simplest shape: take the highest required epoch among clusters still pending promotion, so no per-node state is needed. Added proof: `shared_reference_waits_for_first_writers_compose`.
- Brief first slice lists bound, hold and capacity-aware eviction, without reference counts → closure demand cannot fit a 2-layer pool (the research table's "Hold only" row stays at 1,440 MiB), so runtime reference counts are in the slice. The PRL stays v2 there: the runtime stops reading owners before the format drops them.
- "`begin_drain` carries the Settling flag and `update_targets` does not" → the controller already holds it as `misses_suspended`, set before `update_targets` by both Settling and capture's preload (`capture/prepared.rs:328`). Rename it for what it now means. Capture keeps full demand with no extra plumbing, matching the lightmap's `update_capture_view`.
- Honest misses need the drawn set, not `sh_visible_cells` → on a fallback that set omits the mirror's drawn cells (`planar.rs:551`). The hold targets from `sh_visible_cells` and counts misses from `DemandFrame::visible_cells`.
- `requested_resident_bytes` units differ from the pool's: dense `isolated_tile_bytes` against `dense_slot_bytes`; sparse 16 B + 2 B per f16 against 8 B + 4 B per pair. The bound therefore stores slots and units as the brief says, and never derives from this field. Its additive consumers go with the policy split below. The whole-map figure must be a unique union, never a sum.
- Boundary inventory omits per-slot bytes → the 1,664 B figure lives only in `research.md`. `floor.rs:201-220` holds the engine-side function. Build Summary needs it, so slot and unit byte costs move to `postretro-level-format` beside the default lead.
- Build, task 1: the pending-epoch wait alone can hang, because a writer evicted before compose leaves no pass to reach its epoch. `release_unbacked_compose_waits` (`rows.rs`) clears the wait once no dirty resident row remains; the epoch bump and dirty clear happen together at commit, so an empty dirty set means the work committed. Known limit: after a direct Pass B failure, a cluster can promote one frame early.
- Build, task 1: `RowRefTable` now counts stored words and rows rather than per-cluster references. Upload staging rolls back and discards its batch. `MissingDenseOwner` became `UncoveredDenseProbe`. Halos raise single-cluster demand, so the largest-cluster floor rises (64 → 192 slots on the nine-cluster fixture) until task 7 replaces it.
- Build, task 2: held protection reads the drawn set, a superset of `sh_visible_cells`, so a mirror-only resident is not freed under the frame drawing it; it adds no reads. Reach and pins still read on held frames, as the lightmap does. SeamWarm warms nothing new on a held frame and keeps residents already warm, because seam residents have no hysteresis. Paused hysteresis does not age, so a long run can accumulate residents until the capacity victim list reclaims them (task 3). With no `DemandFrame`, misses fall back to `visible`, as today.
- Build, task 3: growth sizes to `needed` (this install, hard) and `wanted` (first-fit of the mandatory unique union against current free ranges), clamped to the device limit, with no doubling. Union placement carries no headroom yet, so fragmentation could force a second growth after retirement; task 4 measures it, task 7 sets it. Dense can still grow before a sparse device-limit refusal, as today. A retiring generation outlives reset until submitted work completes, per R6. Seam activation still lifts suppression without a horizon change, as today. Row 10's read starts after eviction, from a new view, because a resident has no read in flight.
- Build, task 5: the range record keeps its trailing reserved word, so it is 16 bytes. `CLUSTER_SH_PAYLOADS_CONTAINER_VERSION` stays 1; only the internal id-50 version moves. `bound_count` is capped at 2. Id 48 is affinity-domain but not a pool family, so the bound omits it. Modules named `ownership` became `stored_nodes`.
- Order, task 5 ran before task 4 finished: the evidence run was stopped by a locked screen, and format work depends on none of its answers. A failed cluster rule changes one constant, and the hold is runtime-only. Task 4 keeps its own binaries and v2 PRLs, so nothing under `content/` rebakes until it reports.
- Build, task 6: "camera cell" means not solid and not exterior, matching id 51's `camera_cells`, so solid cells have no set to exceed the bound. With no camera cells the bound is the whole map. Legacy pack with no id 50 stores `bound_count` 0. The serialized directory is a clone carrying the bound (6.3 MB on the hallway). Probe patches are not hashed: the Decision covers nodes and rows. Build Summary also gives the worst cell's position. The bound loop costs 150–170 ms on the real hallway (7,881 cells, 6,294 clusters), one pass for both leads, so no set skipping. It matches the offline oracle exactly on all three maps. The hallway's real BC6H data passes verification (9,988 shared nodes, 30,972 shared rows). The on-disk `occlusion-test` PRL is stale: a fresh bake gives 18,327 slots, as `research.md` has it, against 18,208 on disk.
- Build, task 7: the 2.25 fragmentation figure stays but now only sets layer size, so §4 needs rewording. The largest-cluster minimum is gone, so a cluster outside every mandatory set grows the pool: the logged safety net. A stored bound entry not matching the runtime's default lead is refused by name (`MandatoryBoundMismatch`).
- Build, task 7 follow-up: a pool that holds the whole map stops evicting, so it never fragments. Drains report `pool_holds_whole_map` from real capacity and the controller latches it, covering load and growth alike. §4's "evicted even on a map within budget" changes with it. When dense holds the map, sparse starts whole too, unless past a device limit: a clarification of row 31, needed for the rule to hold. `POOL_LAYERS` = 8. Hallway by arithmetic: initial 211,248 slots (335.2 MiB), cap 739,368 (1,173.3 MiB), overshoot past whole map 8,360 slots (13.3 MiB), first-growth step 26,406 slots (41.9 MiB), copy peak 712.4 MiB. Cap refusals are their own cause and bucket (`at_whole_map_cap`), not a device limit.
- Open, measured at task 9: a sparse family at its own cap while dense is not still evicts, and could fragment and refuse a mandatory row, rendering black. Read `at_whole_map_cap` on every walked map. A level reaching the cap by growth after evictions has the same residual, and needs demand far past the bound.
- Pool cap "whole map" → `AtlasShape` rounds to whole layers, so the cap reads as the whole-map slot count rounded up to whole layers. Today's growth has no whole-map clamp (`gpu.rs:178-188`); task 7 adds it. Risk measured in task 4: first-fit needs contiguous ranges, so a pool at the cap could fragment and refuse on near-whole-map levels.
- `stream_resident_volume` "owner closure" wording also lives in `sdk/TrenchBroom/postretro.fgd:809-810` and `docs/level_design.md:600`. Both are modder-facing, so task 8 owns them.
- A level with id 49 and no id 34 has no ranges → `bound_count` 0, and the pool path is unreachable there.
- `mover-light-flow--probe-channel`: rechecked as the brief asked. Its section is whole-resident outside id 50, so it has no owner dependency.

## Delegated answers

- **Why refused surfaces render black** — the §4 contract holds, and the floor is simply zero. All-zero words drop every corner in `sh_probe_weight` (`sh_sample.wgsl:211-229`), the sample returns 0 (`:408-444`), and the world shader adds `ambient_floor`, which is `DEFAULT_AMBIENT_FLOOR = 0.0` (`renderer_types.rs:125`), changed only by the dev slider. Refusal does render black, so the fixed-pool rival stays rejected and growth stays. Raising the floor or adding a miss colour would reopen it; both are outside this brief.
- **Mechanism and policy under references** — the renderer owns capacity truth, the controller owns order, and neither estimates the other's numbers. On a drain with mandatory work ready, the batch carries an ordered victim list of optional residents: SeamWarm and Band by today's pressure comparator, Hysteresis last. When a mandatory install cannot place, the renderer rolls it back, evicts down the list until the shortfall is met (counting real frees through references, zeros submitted before reuse), and retries in the same drain. With the list exhausted it grows once and warns; past a device limit it refuses. An optional install that cannot place is refused without eviction, and that cluster is suppressed like a pressure victim, so it does not thrash. Evicted victims are suppressed until the horizon changes. `DEFAULT_GPU_FLOOR_BYTES` and the additive `apply_budget_policy` projection are deleted, so no second capacity truth remains.
- **Pool headroom** — 25% over the bound, dense and sparse alike. It rests on one hallway route at a 2-layer pool, 21% over the bound: no growth, no mandatory refusal, 5–8 mandatory shortfalls all placed by eviction, 61–62 optional refusals. High-water is pool-limited, so this shows 21% suffices, not that it is the minimum; 25% rounds up from what was tested. Fragmentation: hallway placement failures with free slots numbered 5 and 9, none mandatory. `campaign-test` at 42,025 slots (whole map 41,925) saw no growth, refusals or shortfalls, but churn was low, so that is weak evidence.
- **Whether mirror cells join the bound** — no, for this build; see *Owner rulings*.
- **Id 49 tail placement** — a tail, as the Wire format section says. The decoder's exact-length check becomes two-phase: header-derived prefix, then tail. A header field gains nothing, because the bound's length varies.

## AC-to-proof

| AC | Row (short) | Proof | Status | Result |
|---|---|---|---|---|
| 1 | Shared node survives either eviction, freed with both, slots reusable | `shared_node_survives_either_eviction_and_frees_with_the_last` | achievable as stated | pass |
| 2 | Same node on one drain written once | `two_clusters_one_drain_write_a_shared_node_once` | achievable as stated | pass |
| 3 | Last reference clears before reuse; live reference clears nothing | `last_reference_clears_before_reuse_and_live_reference_clears_nothing` | achievable as stated | pass |
| 4 | Sampleable with no other cluster resident | `halo_cluster_samples_alone` | achievable as stated | pass |
| 5 | Partial install failure restores counts (R4) | `failed_install_restores_every_reference_count` | achievable as stated | pass |
| 6 | Light over two clusters accumulates once, either order | `shared_light_accumulates_once_in_either_order` | achievable as stated | pass |
| 7 | Reload leaves nothing (R10) | `reload_mid_flight_leaves_nothing_from_the_old_level` | achievable as stated | pass |
| 8 | Evict and install on one drain, both orders (R1) | `evict_and_install_one_drain_keep_one_live_copy` | achievable as stated | pass |
| 9 | Evict and re-request in a frame (R2) | `evict_and_rerequest_counts_references_once` | achievable as stated | pass |
| 10 | Read finishing for an evicted cluster dropped (R3) | `read_finishing_after_eviction_is_dropped` | achievable as stated | pass |
| 11 | Hint or pin makes only that cluster resident | `pin_makes_only_its_cluster_resident` | achievable as stated | pass |
| 12 | Disagreeing chunks fail the build, naming both | `disagreeing_chunks_fail_the_build_naming_both` | achievable as stated | pass |
| 13 | Old version refused by name | `old_directory_version_is_refused_by_name` | achievable as stated | pass |
| 14 | No owner field, sentinel or role (grep gate) | grep gate, pattern scoped to SH: bare `owner` also hits shadowmask `FaceOwners` | achievable as stated | pass |
| 15 | Bound equals hand-computed worst cell | `bound_matches_hand_computed_worst_cell_at_each_lead` | achievable as stated | pass |
| 16 | Small map's bound equals whole map | `wholly_mandatory_map_reports_the_whole_map` | achievable as stated | pass |
| 17 | Build Summary reports; warns only above threshold | `build_summary_reports_bound_and_warns_above_threshold` | achievable as stated | pass |
| 18 | No usable id 51 reports whole map | `no_id51_reports_the_whole_map` | achievable as stated | pass |
| 19 | Non-portal: no reads, residents stay, misses count | `each_fallback_path_holds_and_counts_misses` | achievable as stated | pass, read with the clarification in Review loop: residents stay except as the last capacity victim (`Held`) |
| 20 | No id 51: Visible every frame, new cells load | `no_id51_keeps_visible_demand_on_every_path` | achievable as stated | pass |
| 21 | Portal path: drawn stay Visible | `portal_path_keeps_drawn_clusters_visible` | achievable as stated | pass |
| 22 | Leaving fallback restores demand that frame | `leaving_fallback_restores_demand_on_that_frame` | achievable as stated | pass |
| 23 | Settling on fallback keeps full demand | `settling_on_fallback_keeps_full_demand` | achievable as stated | pass |
| 24 | Settling ending on fallback keeps what it loaded (R9) | `settling_end_on_fallback_keeps_residents` | achievable as stated | pass |
| 25 | Residents survive a long fallback run (R7) | `long_fallback_run_ages_out_nothing` | achievable as stated | pass |
| 26 | Teleport during hold (R8) | `teleport_during_hold_follows_the_new_cell` | achievable as stated | pass |
| 27 | No growth within the bound | `walk_within_bound_never_grows` | achievable as stated | pass |
| 28 | Full pool evicts optional, lowest first | `full_pool_evicts_optional_lowest_priority_first` | achievable as stated | pass |
| 29 | Past capacity grows once, warns; device limit refuses | `past_capacity_grows_once_and_warns_by_name`, log capture | achievable as stated | pass |
| 30 | Pool never exceeds whole map | `pool_never_exceeds_the_whole_map` | achievable, cap read in whole layers (Corrections) | pass |
| 31 | Initial size equals bound plus headroom | `initial_pool_is_bound_plus_headroom_capped` | achievable, as row 30 | pass |
| 32 | Evict before growing, grow before refusing | `capacity_order_is_evict_grow_refuse` | achievable as stated | pass |
| 33 | Deferred behind a retiring generation (R5) | `deferred_behind_retiring_generation_evicts_and_grows_nothing` | achievable as stated | pass |
| 34 | Growth with a generation retiring (R6) | `growth_with_generation_retiring_keeps_contents_until_done` | achievable as stated | pass |
| 35 | Hallway ceiling | owner, in-engine; task 9 runs the walk | manual | measured, a finding not a gate: steady 2,170 against 3,011 MiB, peak 2,574 against 3,577 MiB, SH pool 399 against 1,440 MiB, no SH growth. Steady is 122 MiB and peak 526 MiB over 2 GB; the peak is lightmap growth |
| 36 | Cluster size rule | task 4 evidence; task 9 confirms | manual | pass, owner ruling: today's limit stays |
| 37 | Frames on four maps | owner, in-engine | manual | pass, owner reviewed: 0 pairs over noise at equal clock on all four maps; the frames that differ at matched pose sit in a room with a pulsing animated light |
| 38 | Theatrical moments | owner, in-engine | manual | pass, owner saw steady lighting on `closet-reveal`, logs clean. Limit: 200 slots, so no pool pressure; no map here both evicts and opens a door |
| 39 | Planar mirrors | `reflection-lake-test` at a planar tier; growth is reported, not extended here | manual | pass: no growth, and the pool holds the whole map on `reflection-lake-test`, so a mirror cannot exceed it. Whether a mirror was in view is unconfirmed |
| 40 | Held frames stay lit | task 4 evidence; owner reviews before the hold stands | manual | pass, owner ruling: the hold stands |
| 41 | Clipped camera | owner, in-engine | manual | pass, owner ruling, as a memory row: pool size unchanged. Frame time is a finding: median 32.5 against 17 ms on solid-cell frames |
| 42 | Cleanup | owner, in-engine | manual | pass: six reloads held at 1,436–1,463 MiB; exit 862 against 819 MB |

Walk, shot and frame-diff tools stay on `spike/memory-measure-tour`. They are ported to a throwaway measurement branch and never merge here.

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Renderer and render-cpu reference counts: nodes, probe words, sparse rows, journal undos, shared compose wait. Runtime stops reading owners; PRL still v2. Rewrite closure tests to the new contract, keeping their edges | worker, renderer + render-cpu | — | done: renderer `sh_streaming` 203 pass, app `sh_streaming` 110 pass, clippy clean; rows 1–6, 8, renderer half of 9–10 |
| 2 | App crate: remove owner closure from the controller and topology; hold with per-path demand, paused hysteresis, honest misses from the drawn set, no-id-51 opt-out | worker, `postretro/src/sh_streaming` | 1 | done: app `sh_streaming` 124 pass, whole bin 2460 pass, clippy clean; rows 9–11 (app half), 19–26 |
| 3 | Capacity split: same-drain victim list, evict → grow → refuse; delete the floor and additive policy | worker, renderer + app seam | 2 | done: renderer `sh_streaming` 217 pass with GPU required, app `sh_` 271 pass, clippy clean; rows 7, 10, 27–29, 32–34 |
| 4 | Evidence slice: 2-layer pool, today's clusters, hallway and `campaign-test`. Records held-frame misses and frames, cluster rule, headroom, fragmentation at the cap. **Owner checkpoint: the hold returns to the owner here if frames darken** | worker, measurement branch | 3 | done: 16 recorded runs; hold accepted by owner; cluster rule keeps today's limit; headroom 25%; rows 36, 40 |
| 5 | Format: move tests out of `cluster_directory.rs` first, then id 49 v3, id 50 v2, bound tail, default lead and byte costs to level-format, old-version refusal | worker, format + loader | — | done: format 664, renderer 1274 with dev-tools, app 2463; 4 tests red until task 6 rebakes `sh-streaming-hinted-door.prl`; row 13 |
| 6 | Compiler: stop producing owner data, per-node and per-row hash verification, bound over camera cells, Build Summary, no-id-51 entry | worker, compiler | 5 | done: `prl-build` bin 2315 pass, `sh_mandatory_bound_compile` 2, format 664, loader 333, app `sh_` 271; fixture rebaked, four fixture tests green; rows 12, 15–18. Full app bin deferred to task 9 |
| 7 | Pool sized once from the bound; whole-map cap; retire the 256 MiB floor for SH | worker, renderer | 5, 6 | done: app bin 2472, renderer with GPU 1288, render-cpu 643, loader 333, gate passes; rows 27, 30, 31 |
| 8 | Author text: FGD, `docs/level_design.md`; grep gate | integrating executor | 6 | done: gate passes; row 14 |
| 9 | Cold rebake, manual rows, preflight, review panel | integrating executor | 7, 8 | done: preflight at ae99bc0b7; 16 recorded walks and a Metal capture; every row has a result |

Workers run sequentially on the feature branch's warm target: tasks 1–3 share `targeting.rs` and compile through each other. The integrating executor reviews each diff, owns `plan.md`, and commits.

**Hot paths.** `update_targets` and the drain run per frame: no new per-frame allocation beyond today's sets, and held frames issue no reads. Reference updates happen on install and eviction, never per frame. The bound runs on every build, warm id-51 hits included, so it uses a stamp array over nodes per camera cell and its time is reported in Build Summary.

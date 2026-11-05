# spatial-residency--sh-bounded-pool — plan of record

mode: resumable
status: approved
read at: b5c39cab8

## Owner rulings

The plan stopped blocked on four Acceptance rows written against a slot-level reader epoch that does not exist: `evict` frees at once (`sh_streaming/rows.rs:128-135`), the allocator can reuse immediately, and queue order is the only protection (`lifecycle.rs:19-25`). The owner delegated the rulings (2026-11-05): re-examine the recommendations, apply what survives, proceed. The brief and `research.md` now carry the result.

- **Reader epoch removed.** Held on review. Slot retirement would protect no reader. Rows 3 and 7 take the wording first proposed.
- **R5 and R6 reworded.** Shifted on review. Same-drain eviction (Delegated answers) removes the "requested but unconfirmed" wait the first wording named. The one real wait left is a retiring growth generation, so both rows point there.
- **Planar mirrors do not join the bound in this build.** Shifted on review, from a first recommendation of yes. A sweep needs a crate move and a cached stage, and buys nothing measurable: three of the four bound maps have no water, `campaign-test` could rise 2,092 slots at most, and the hallway decides the cluster rule. Planar runs only at very high and ultra, so a mirror-inclusive figure charges default-tier players for an opt-in tier. Growth is already the logged net, and it is reached only after optional eviction, in a cell near the worst. `spatial-residency--lightmap-bounded-pool` also dropped its mirror sweep on the owner's decision, so no sweep exists; a large water map that grows reopens the question for both bounds. A mirror extension would still need no layout change, since id 49 stores figures rather than the cells behind them. Row 39 stays as written: it measures `reflection-lake-test`, and growth there is reported rather than extended here.
- **Refusal stays black, growth stays.** Held on review.
- **Land step, rendering_pipeline.md §4.** Delete "It never requests probes.", false for SH on a completed mirror walk. `draft/lightmap-bounded-pool` makes the identical edit on the owner's instruction, so the two merge cleanly.
- **Id 49 keeps its baked-maximum entry** (owner, 2026-11-05). The brief stands as written: two entries, default lead and baked maximum. Recommended dropping it to match the lightmap's id 58; the owner kept it for the named memory-tier work, accepting a later refactor. Task 6 therefore prices both entries on every build.

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
- Pool cap "whole map" → `AtlasShape` rounds to whole layers, so the cap reads as the whole-map slot count rounded up to whole layers. Today's growth has no whole-map clamp (`gpu.rs:178-188`); task 7 adds it. Risk measured in task 4: first-fit needs contiguous ranges, so a pool at the cap could fragment and refuse on near-whole-map levels.
- `stream_resident_volume` "owner closure" wording also lives in `sdk/TrenchBroom/postretro.fgd:809-810` and `docs/level_design.md:600`. Both are modder-facing, so task 8 owns them.
- A level with id 49 and no id 34 has no ranges → `bound_count` 0, and the pool path is unreachable there.
- `mover-light-flow--probe-channel`: rechecked as the brief asked. Its section is whole-resident outside id 50, so it has no owner dependency.

## Delegated answers

- **Why refused surfaces render black** — the §4 contract holds, and the floor is simply zero. All-zero words drop every corner in `sh_probe_weight` (`sh_sample.wgsl:211-229`), the sample returns 0 (`:408-444`), and the world shader adds `ambient_floor`, which is `DEFAULT_AMBIENT_FLOOR = 0.0` (`renderer_types.rs:125`), changed only by the dev slider. Refusal does render black, so the fixed-pool rival stays rejected and growth stays. Raising the floor or adding a miss colour would reopen it; both are outside this brief.
- **Mechanism and policy under references** — the renderer owns capacity truth, the controller owns order, and neither estimates the other's numbers. On a drain with mandatory work ready, the batch carries an ordered victim list of optional residents: SeamWarm and Band by today's pressure comparator, Hysteresis last. When a mandatory install cannot place, the renderer rolls it back, evicts down the list until the shortfall is met (counting real frees through references, zeros submitted before reuse), and retries in the same drain. With the list exhausted it grows once and warns; past a device limit it refuses. An optional install that cannot place is refused without eviction, and that cluster is suppressed like a pressure victim, so it does not thrash. Evicted victims are suppressed until the horizon changes. `DEFAULT_GPU_FLOOR_BYTES` and the additive `apply_budget_policy` projection are deleted, so no second capacity truth remains.
- **Pool headroom** — pending. Task 4 measures it; the value is recorded here.
- **Whether mirror cells join the bound** — no, for this build; see *Owner rulings*.
- **Id 49 tail placement** — a tail, as the Wire format section says. The decoder's exact-length check becomes two-phase: header-derived prefix, then tail. A header field gains nothing, because the bound's length varies.

## AC-to-proof

| AC | Row (short) | Proof | Status |
|---|---|---|---|
| 1 | Shared node survives either eviction, freed with both, slots reusable | `shared_node_survives_either_eviction_and_frees_with_the_last` | achievable as stated |
| 2 | Same node on one drain written once | `two_clusters_one_drain_write_a_shared_node_once` | achievable as stated |
| 3 | Last reference clears before reuse; live reference clears nothing | `last_reference_clears_before_reuse_and_live_reference_clears_nothing` | achievable as stated |
| 4 | Sampleable with no other cluster resident | `halo_cluster_samples_alone` | achievable as stated |
| 5 | Partial install failure restores counts (R4) | `failed_install_restores_every_reference_count` | achievable as stated |
| 6 | Light over two clusters accumulates once, either order | `shared_light_accumulates_once_in_either_order` | achievable as stated |
| 7 | Reload leaves nothing (R10) | `reload_mid_flight_leaves_nothing_from_the_old_level` | achievable as stated |
| 8 | Evict and install on one drain, both orders (R1) | `evict_and_install_one_drain_keep_one_live_copy` | achievable as stated |
| 9 | Evict and re-request in a frame (R2) | `evict_and_rerequest_counts_references_once` | achievable as stated |
| 10 | Read finishing for an evicted cluster dropped (R3) | `read_finishing_after_eviction_is_dropped` | achievable as stated |
| 11 | Hint or pin makes only that cluster resident | `pin_makes_only_its_cluster_resident` | achievable as stated |
| 12 | Disagreeing chunks fail the build, naming both | `disagreeing_chunks_fail_the_build_naming_both` | achievable as stated |
| 13 | Old version refused by name | `old_directory_version_is_refused_by_name` | achievable as stated |
| 14 | No owner field, sentinel or role (grep gate) | grep gate, pattern scoped to SH: bare `owner` also hits shadowmask `FaceOwners` | achievable as stated |
| 15 | Bound equals hand-computed worst cell | `bound_matches_hand_computed_worst_cell_at_each_lead` | achievable as stated |
| 16 | Small map's bound equals whole map | `wholly_mandatory_map_reports_the_whole_map` | achievable as stated |
| 17 | Build Summary reports; warns only above threshold | `build_summary_reports_bound_and_warns_above_threshold` | achievable as stated |
| 18 | No usable id 51 reports whole map | `no_id51_reports_the_whole_map` | achievable as stated |
| 19 | Non-portal: no reads, residents stay, misses count | `each_fallback_path_holds_and_counts_misses` | achievable as stated |
| 20 | No id 51: Visible every frame, new cells load | `no_id51_keeps_visible_demand_on_every_path` | achievable as stated |
| 21 | Portal path: drawn stay Visible | `portal_path_keeps_drawn_clusters_visible` | achievable as stated |
| 22 | Leaving fallback restores demand that frame | `leaving_fallback_restores_demand_on_that_frame` | achievable as stated |
| 23 | Settling on fallback keeps full demand | `settling_on_fallback_keeps_full_demand` | achievable as stated |
| 24 | Settling ending on fallback keeps what it loaded (R9) | `settling_end_on_fallback_keeps_residents` | achievable as stated |
| 25 | Residents survive a long fallback run (R7) | `long_fallback_run_ages_out_nothing` | achievable as stated |
| 26 | Teleport during hold (R8) | `teleport_during_hold_follows_the_new_cell` | achievable as stated |
| 27 | No growth within the bound | `walk_within_bound_never_grows` | achievable as stated |
| 28 | Full pool evicts optional, lowest first | `full_pool_evicts_optional_lowest_priority_first` | achievable as stated |
| 29 | Past capacity grows once, warns; device limit refuses | `past_capacity_grows_once_and_warns_by_name`, log capture | achievable as stated |
| 30 | Pool never exceeds whole map | `pool_never_exceeds_the_whole_map` | achievable, cap read in whole layers (Corrections) |
| 31 | Initial size equals bound plus headroom | `initial_pool_is_bound_plus_headroom_capped` | achievable, as row 30 |
| 32 | Evict before growing, grow before refusing | `capacity_order_is_evict_grow_refuse` | achievable as stated |
| 33 | Deferred behind a retiring generation (R5) | `deferred_behind_retiring_generation_evicts_and_grows_nothing` | achievable as stated |
| 34 | Growth with a generation retiring (R6) | `growth_with_generation_retiring_keeps_contents_until_done` | achievable as stated |
| 35 | Hallway ceiling | owner, in-engine; task 9 runs the walk | manual |
| 36 | Cluster size rule | task 4 evidence; task 9 confirms | manual |
| 37 | Frames on four maps | owner, in-engine | manual |
| 38 | Theatrical moments | owner, in-engine | manual |
| 39 | Planar mirrors | `reflection-lake-test` at a planar tier; growth is reported, not extended here | manual |
| 40 | Held frames stay lit | task 4 evidence; owner reviews before the hold stands | manual |
| 41 | Clipped camera | owner, in-engine | manual |
| 42 | Cleanup | owner, in-engine | manual |

Walk, shot and frame-diff tools stay on `spike/memory-measure-tour`. They are ported to a throwaway measurement branch and never merge here.

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Renderer and render-cpu reference counts: nodes, probe words, sparse rows, journal undos, shared compose wait. Runtime stops reading owners; PRL still v2. Rewrite closure tests to the new contract, keeping their edges | worker, renderer + render-cpu | — | done: renderer `sh_streaming` 203 pass, app `sh_streaming` 110 pass, clippy clean; rows 1–6, 8, renderer half of 9–10 |
| 2 | App crate: remove owner closure from the controller and topology; hold with per-path demand, paused hysteresis, honest misses from the drawn set, no-id-51 opt-out | worker, `postretro/src/sh_streaming` | 1 | done: app `sh_streaming` 124 pass, whole bin 2460 pass, clippy clean; rows 9–11 (app half), 19–26 |
| 3 | Capacity split: same-drain victim list, evict → grow → refuse; delete the floor and additive policy | worker, renderer + app seam | 2 | done: renderer `sh_streaming` 217 pass with GPU required, app `sh_` 271 pass, clippy clean; rows 7, 10, 27–29, 32–34 |
| 4 | *(paused: screen locked 2026-11-05 11:17; tooling and binaries ready on `measure/sh-bounded-pool`)* Evidence slice: 2-layer pool, today's clusters, hallway and `campaign-test`. Records held-frame misses and frames, cluster rule, headroom, fragmentation at the cap. **Owner checkpoint: the hold returns to the owner here if frames darken** | worker, measurement branch | 3 | |
| 5 | Format: move tests out of `cluster_directory.rs` first, then id 49 v3, id 50 v2, bound tail, default lead and byte costs to level-format, old-version refusal | worker, format + loader | — | done: format 664, renderer 1274 with dev-tools, app 2463; 4 tests red until task 6 rebakes `sh-streaming-hinted-door.prl`; row 13 |
| 6 | Compiler: stop producing owner data, per-node and per-row hash verification, bound over camera cells, Build Summary, no-id-51 entry | worker, compiler | 5 | |
| 7 | Pool sized once from the bound; whole-map cap; retire the 256 MiB floor for SH | worker, renderer | 5, 6 | |
| 8 | Author text: FGD, `docs/level_design.md`; grep gate | integrating executor | 6 | |
| 9 | Cold rebake, manual rows, preflight, review panel | integrating executor | 7, 8 | |

Workers run sequentially on the feature branch's warm target: tasks 1–3 share `targeting.rs` and compile through each other. The integrating executor reviews each diff, owns `plan.md`, and commits.

**Hot paths.** `update_targets` and the drain run per frame: no new per-frame allocation beyond today's sets, and held frames issue no reads. Reference updates happen on install and eviction, never per frame. The bound runs on every build, warm id-51 hits included, so it uses a stamp array over nodes per camera cell and its time is reported in Build Summary.

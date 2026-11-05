# spatial-residency--sh-bounded-pool — plan of record

mode: resumable
status: blocked
read at: b5c39cab8

## Why this is blocked

Four Acceptance rows are written against a slot-level reader epoch that does not exist. Each needs the owner's restatement (rows 3, 7, 33, 34 below). No Decision premise is false. One further owner ruling is open: whether planar mirrors join the bound (see *Owner rulings needed*). On approval, set `status: approved`, apply the rulings to the brief, and start task 1.

**Evidence.** `evict` frees dense ranges and sparse rows in the same call (`sh_streaming/rows.rs:128-135`), and the first-fit allocator can reuse them at once (`allocator.rs:50-60`). Safety is queue order: promotion and eviction share one upload batch, submitted before any install in that drain (`lifecycle.rs:19-25`), so the zero words land before reuse (comment in `rows.rs`, "Publish zeros before freed ranges can be reused"). No reader persists past that ordering. The only compose epochs are promotion gates (`required_compose_epochs`, `sh_streaming.rs:504`), and compose staleness is per row, not per slot. The only retirement is a whole growth generation, one slot per family, released on submitted-work-done (`gpu/growth.rs`; a second growth returns `GpuRetirementPressure`). "Reader epoch" first appears in this brief's draft commit `0d8ae7aeb`, with no earlier source.

Building slot retirement would satisfy the rows literally, but it adds a mechanism with no reader to protect. The proposed wording below keeps each row's intent and points it at the wait states that are real.

## Owner rulings needed

1. **Restate rows 3, 7, 33, 34.** Exact wording is in the AC-to-proof table. `research.md` R5 and R6 follow.
2. **Planar mirrors join the bound?** Assessment sent to `spatial-residency--lightmap-bounded-pool`'s drafting session at the owner's request. Recommendation: yes, one mirror-inclusive figure, no wire layout change, with the sweep shared between both bounds. Three sub-decisions sit with the owner because both bounds need one answer: where water-plane grouping and the seed rule live (`postretro-render-cpu` is only a dev dependency of the compiler), where the sweep runs (a cached stage, not pack), and whether default-tier players pay for a planar-only figure. This does not block tasks 1–4. It gates task 6's mirror input only.

## Corrections

- "Id 49 is emitted at pack, after id 51 exists, so the bound needs no new stage" → the directory is built in the uncached `StageId::ClusterDirectory` stage (`pipeline.rs:2092`) and serialized at `pack/section_plan.rs:497`. Per-cluster node sets exist only in `ChunkPlan` (`pack/cluster_sh_payloads/dense.rs:47-98`), which is built at `pack.rs:630` from a shared borrow of the directory. No new stage is needed, but plumbing is: lift `ChunkPlan`'s sets out, compute the bound in pack, pass it to id-49 serialization as an argument, and return it through `PrlFootprint` for the Build Summary (printed at `pipeline.rs:2175-2181`).
- Wire `worst_units` is one number per family → sparse capacity is two-dimensional, `(entries, tile_f16)` (`floor.rs:240-285, 376-385`). Family entries carry `u32 worst_entries, u32 worst_tile_f16`. The meaning is unchanged; the wire shape changes.
- "Roles reduce to dense and affinity" → after owner removal, role equals `resource.domain` (validated at `cluster_directory.rs:562-573`). Under "no unread fields stay behind", the range record drops role entirely.
- The brief omits id 50's own role word → each sparse row record carries one (`pack/cluster_sh_payloads/sparse.rs:237`, checked at `decode.rs:246-250`). It goes too, and `CLUSTER_SH_PAYLOADS_VERSION` moves 1 → 2. Both id-49 version constants move: `CLUSTER_DIRECTORY_VERSION` and `CLUSTER_DIRECTORY_CONTAINER_VERSION`. The mismatch error gains the id-51 style "re-bake the level" text.
- "`RowRefTable` already counts affinity rows" → it counts compose-membership contributors, not storage lifetime (`row_refs.rs:21, 131-146`). Sparse rows are written by one owner (`sparse_install.rs:43`), and `install_undoable` errors on a live row (`allocator.rs:205`). Node slots and probe words have no count at all. Three new counts are needed, each with a journal undo: nodes, probe words, sparse rows. The journal's undo list is `install_journal.rs:13-30`.
- Not in the brief: **a second referencer must wait for the first writer's compose.** `required_compose_epochs` returns the current epoch when a cluster wrote nothing (`sh_streaming.rs:504`), and today only `owner_dependencies` holds it back. With owners gone, a cluster whose nodes are all shared could promote before they compose and publish uncomposed words. A referencing install takes the latest required epoch of what it shares. Added proof: `shared_reference_waits_for_first_writers_compose`.
- Brief first slice lists bound, hold and capacity-aware eviction, without reference counts → closure demand cannot fit a 2-layer pool (the research table's "Hold only" row stays at 1,440 MiB), so runtime reference counts are in the slice. The PRL stays v2 there: the runtime stops reading owners before the format drops them.
- "`begin_drain` carries the Settling flag and `update_targets` does not" → the controller already holds it as `misses_suspended`, set before `update_targets` by both Settling and capture's preload (`capture/prepared.rs:328`). Rename it for what it now means. Capture keeps full demand with no extra plumbing, matching the lightmap's `update_capture_view`.
- Honest misses need the drawn set, not `sh_visible_cells` → on a fallback that set omits the mirror's drawn cells (`planar.rs:551`). The hold targets from `sh_visible_cells` and counts misses from `DemandFrame::visible_cells`.
- `requested_resident_bytes` units differ from the pool's: dense `isolated_tile_bytes` against `dense_slot_bytes`; sparse 16 B + 2 B per f16 against 8 B + 4 B per pair. The bound therefore stores slots and units as the brief says, and never derives from this field. Its additive consumers go with the policy split below. The whole-map figure must be a unique union, never a sum.
- Boundary inventory omits per-slot bytes → the 1,664 B figure lives only in `research.md`. `floor.rs:201-220` holds the engine-side function. Build Summary needs it, so slot and unit byte costs move to `postretro-level-format` beside the default lead.
- Pool cap "whole map" → `AtlasShape` rounds to whole layers, so the cap reads as the whole-map slot count rounded up to whole layers. Today's growth has no whole-map clamp (`gpu.rs:178-188`); task 7 adds it. Risk measured in task 4: first-fit needs contiguous ranges, so a pool at the cap could fragment and refuse on near-whole-map levels.
- `stream_resident_volume` "owner closure" wording also lives in `sdk/TrenchBroom/postretro.fgd:809-810` and `docs/level_design.md:600`. Both are modder-facing, so task 8 owns them.
- A level with id 49 and no id 34 has no ranges → `bound_count` 0, and the pool path is unreachable there.
- `mover-light-flow--probe-channel`: rechecked as the brief asked. Its section is whole-resident outside id 50, so it has no owner dependency.

## Delegated answers

- **Why refused surfaces render black** — the §4 contract holds, and the floor is simply zero. All-zero words drop every corner in `sh_probe_weight` (`sh_sample.wgsl:211-229`), the sample returns 0 (`:408-444`), and the world shader adds `ambient_floor`, which is `DEFAULT_AMBIENT_FLOOR = 0.0` (`renderer_types.rs:125`), changed only by the dev slider. Refusal does render black, so the fixed-pool rival stays rejected and growth stays. Raising the floor or adding a miss colour would reopen it; both are outside this brief.
- **Mechanism and policy under references** — the renderer owns capacity truth, the controller owns order, and neither estimates the other's numbers. The renderer defers work it cannot place and reports the shortage in `ShDrainOutcome`. While a shortage stands, the controller sends optional residents as an ordered victim list: SeamWarm and Band by today's pressure comparator, Hysteresis last. The renderer evicts down that list only until the shortage is met, counting real frees through references, and returns what it evicted. With victims exhausted and none pending, it grows once. `DEFAULT_GPU_FLOOR_BYTES` and the additive `apply_budget_policy` projection are deleted, so no second capacity truth remains.
- **Pool headroom** — pending. Task 4 measures it; the value is recorded here.
- **Whether mirror cells join the bound** — taken back by the owner; see *Owner rulings needed*.
- **Id 49 tail placement** — a tail, as the Wire format section says. The decoder's exact-length check becomes two-phase: header-derived prefix, then tail. A header field gains nothing, because the bound's length varies.

## AC-to-proof

| AC | Row (short) | Proof | Status |
|---|---|---|---|
| 1 | Shared node survives either eviction, freed with both, slots reusable | `shared_node_survives_either_eviction_and_frees_with_the_last` | achievable as stated |
| 2 | Same node on one drain written once | `two_clusters_one_drain_write_a_shared_node_once` | achievable as stated |
| 3 | Reader epoch delays the free | see wording | **needs restatement:** "Evicting the last reference to a shared row or node zeroes its words and clears its row in a batch submitted before any install in that drain can reuse its slots; with another reference live, nothing is cleared or freed." |
| 4 | Sampleable with no other cluster resident | `halo_cluster_samples_alone` | achievable as stated |
| 5 | Partial install failure restores counts (R4) | `failed_install_restores_every_reference_count` | achievable as stated |
| 6 | Light over two clusters accumulates once, either order | `shared_light_accumulates_once_in_either_order` | achievable as stated |
| 7 | Reload leaves nothing (R10) | see wording | **needs restatement:** "Reload with reads in flight, references live and a growth generation retiring leaves no references, pending reads or retiring generation from the old level (R10)." |
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
| 33 | Waiting on retiring slots (R5) | see wording | **needs restatement:** "Mandatory work deferred for capacity while its evictions are requested but unconfirmed triggers no growth and no further eviction on later frames; it installs on the drain they free (R5)." |
| 34 | Growth with retirement pending (R6) | see wording | **needs restatement:** "A growth needed while a prior generation is retiring defers the install and starts no second growth; the old generation's contents stay until submitted work is done, then it frees and the install proceeds (R6)." |
| 35 | Hallway ceiling | owner, in-engine; task 9 runs the walk | manual |
| 36 | Cluster size rule | task 4 evidence; task 9 confirms | manual |
| 37 | Frames on four maps | owner, in-engine | manual |
| 38 | Theatrical moments | owner, in-engine | manual |
| 39 | Planar mirrors | owner, in-engine; wording follows the mirror ruling | manual |
| 40 | Held frames stay lit | task 4 evidence; owner reviews before the hold stands | manual |
| 41 | Clipped camera | owner, in-engine | manual |
| 42 | Cleanup | owner, in-engine | manual |

Walk, shot and frame-diff tools stay on `spike/memory-measure-tour`. They are ported to a throwaway measurement branch and never merge here.

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Runtime reference counts: nodes, probe words, sparse rows, journal undos, shared compose wait. Owners ignored at runtime; PRL still v2. Rewrite closure tests to the new contract, keeping their edges | integrating executor | rulings | |
| 2 | Hold: per-path demand, paused hysteresis, honest misses from the drawn set, no-id-51 opt-out | worker, `postretro/src/sh_streaming` (concurrent with 1 only if its tests need no renderer change) | rulings | |
| 3 | Capacity split: shortage report, victim list, evict → grow → refuse; delete the floor and additive policy | integrating executor | 1, 2 | |
| 4 | Evidence slice: 2-layer pool, today's clusters, hallway and `campaign-test`. Records held-frame misses and frames, cluster rule, headroom, fragmentation at the cap. **Owner checkpoint: the hold returns to the owner here if frames darken** | integrating executor | 3 | |
| 5 | Format: move tests out of `cluster_directory.rs` first, then id 49 v3, id 50 v2, bound tail, default lead and byte costs to level-format, old-version refusal | integrating executor | 4 | |
| 6 | Compiler: stop producing owner data, per-node and per-row hash verification, bound over camera cells, Build Summary, no-id-51 entry, mirror input per ruling | integrating executor | 5 | |
| 7 | Pool sized once from the bound; whole-map cap; retire the 256 MiB floor for SH | integrating executor | 5, 6 | |
| 8 | Author text: FGD, `docs/level_design.md`; grep gate | integrating executor | 6 | |
| 9 | Cold rebake, manual rows, preflight, review panel | integrating executor | 7, 8 | |

**Hot paths.** `update_targets` and the drain run per frame: no new per-frame allocation beyond today's sets, and held frames issue no reads. Reference updates happen on install and eviction, never per frame. The bound runs on every build, warm id-51 hits included, so it uses a stamp array over nodes per camera cell and its time is reported in Build Summary.

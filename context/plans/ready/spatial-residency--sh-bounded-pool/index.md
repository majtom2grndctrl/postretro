# spatial-residency--sh-bounded-pool

Brief · resumable · reads: `context/lib/index.md` §1.1 · `context/lib/rendering_pipeline.md` §2, §4 (Cluster SH residency) · `context/lib/build_pipeline.md` §Compiler pipeline, §PRL section IDs · `context/lib/development_guide.md` §1.4 · `context/lib/testing_guide.md` §Resource bounds · `context/plans/large-map-spatial-residency.md` · read at 2851e10fb · evidence: `research.md`

## Problem
A memory spike raised by the owner found `stress-warren-hallway-inspection` allocating 3,011 MiB steady and 3,577 MiB at peak on Metal, against the 2 GB target. The SH pool is the largest cause, at 1,440 MiB: baked owner closure makes residency follow cluster id order instead of sight lines, and doubling overshoots what is live. Each growth also holds old and new pools alive together. An anticipated risk compounds this: non-portal paths make every drawn cluster mandatory, which the spike saw only from a tour camera inside solid cells, but one such frame would break any fixed bound. When this is done, the compiler knows the worst mandatory SH demand of any camera cell and reports it to the author, the pool is sized from that figure once at level load, and shared SH storage lives exactly as long as a resident cluster uses it.

## Decisions
- **Reference counts replace owner closure.** A shared dense node, probe patch or sparse row is written by the first resident cluster that needs it and freed with its last reference. Id 50 chunks already carry full local payloads, so no cluster depends on another. This reverses `plans/done/sh-probe-streaming--cluster-directory`'s lowest-covering-owner rule and the closure guarantees in `rendering_pipeline.md` §4, because closure reaches about 3x the slots a walk uses. Hints and pins apply to the named cluster alone, so `stream_resident_volume`'s author-facing "owner closure" wording in `build_pipeline.md` and `plans/done/sh-streaming--reveal-gate-and-warm-horizon`'s settle set change with it.
- **Owner data leaves the PRL.** No unread fields stay behind, and the compiler stops producing them. Every map rebakes. See Wire format.
- **A physical light still accumulates once.** Deduplication is by stored identity: a shared row or node has one live copy however many clusters reference it. This is the rule `rendering_pipeline.md` §4 gave ownership, and it survives ownership.
- **Shared bytes are verified, not assumed.** First writer wins only because every chunk slices one source. The compiler fails the build if two chunks disagree on a shared node or row.
- **SH holds on every non-portal path, as the lightmap does.** Drawn clusters add no reads there, hysteresis pauses for the run so residents are not aged out, and absent drawn clusters count as misses. This keeps the mandatory bound true: one clip into a wall must not make the map mandatory. The `rendering_pipeline.md` §4 "every visibility path" sentences change. Settling keeps full demand, so a level never starts dark. The spike's hold hid its misses, so the first slice must show held frames stay lit before this stands; if they do not, it returns to the owner.
- **A level with no usable id 51 opts out.** Only Visible and pins remain without reach, so a hold would stop SH loading new areas. The hold stays off and the bound is the whole map, the SH equivalent of the lightmap's all-resident escape.
- **The compiler computes the bound; bake over compute.** Worst mandatory demand is taken over every camera cell, with pins, after reference-count deduplication. It is stored at the default lead and at the baked maximum, so a later memory tier can vary lead without a rebake. Id 51 is unchanged: measured against what a player can see all around, it is close, and it almost never misses a drawn cell (`research.md`).
- **The bound reaches the author.** Build Summary reports SH mandatory bytes and the worst cell for every level, and warns above 512 MiB, about what the other pools leave of the target. Automatic, with no author key, like occluder classification.
- **The pool is sized once at load** from the bound plus measured headroom, capped at the whole map. Texture arrays cannot grow in place, so finer growth would add copy peaks. This replaces the SH use of the 256 MiB floor as a starting size. Memory tiers stay with `large-map-spatial-residency.md`; its rule that a tier is valid only if every camera cell's mandatory set fits can now be checked against this bound.
- **Optional data yields to mandatory data at real capacity.** Band and SeamWarm residents are evicted first to admit mandatory work, and Hysteresis last, because it stops doorway thrash. Today priority shedding runs against a CPU budget only, and mandatory work cannot evict anything. This changes `plans/done/sh-streaming--pool-growth-device-limit`'s order and §4's "refusal evicts nothing": evict, then grow, then refuse.
- **Growth stays as a logged safety net.** Demand past the pool (a raised dev lead lever, Settling on a fallback path) grows it once and warns by name. One memory peak beats black surfaces, on the current reading that refusal renders black; see Open questions.
- **Cluster size is closed by rule, in build order.** Keep today's limit if a bake-sized pool gives no mandatory refusals and no indirect shot above noise on the hallway. Otherwise take the largest limit that passes. It stays an engine constant, never an author key. Walks prove the frames; the bound, which covers every cell, sets the size.
- **Non-goals.**
  - Shadow pool resolution: a fixed 427 MiB deserves its own experiment, and it trades quality for memory.
  - Lightmap pool growth: same pattern, separate pool and contract. Total peak therefore stays above 2 GB, projected near 2.2 GB, and `drafts/lightmap-pool-growth-peak.md` carries the follow-up under `development_guide.md` §1.4.
  - Exact PVS, and any change to id 51 or cell shapes.
  - Grouping cells into rooms: cluster grain is this brief's lever, and an all-cells bound needs no room concept.
  - E20's ledger: it reports this work and does not depend on it.

## Acceptance
### Automated
**Reference counts:**
- [ ] A node shared by two resident clusters survives evicting either one, is freed when both are gone, and its slots are then reusable.
- [ ] Two clusters, neither depending on the other, installing the same node on one drain write it once.
- [ ] Evicting a cluster while a reader's epoch still needs a shared row delays the free, and it completes afterwards.
- [ ] A cluster becomes sampleable with no other cluster resident, on a fixture that today requires an owner first.
- [ ] An install failing after some references were taken frees any node it alone wrote and restores every count, wherever the failure lands (R4).
- [ ] A light covering two clusters accumulates once, resident in either order.
- [ ] Reload with reads in flight and slots pending retirement leaves no references, pending reads or retired slots from the old level (R10).
- [ ] A cluster evicted and a cluster needing the same node installed on one drain, in either order, leave the node resident once with its data intact and no stale zero word (R1).
- [ ] A cluster evicted and re-requested in the same frame is resident again afterwards with its references counted once (R2).
- [ ] A read finishing for a cluster evicted while in flight is dropped: nothing installs and no references or slots remain (R3).
- [ ] A hint or pin on one cluster makes only that cluster resident.

**Format and compiler:**
- [ ] Two chunks that disagree on a shared node or row fail the build, naming both. Agreeing chunks pass.
- [ ] A section at the old version is refused by name.
- [ ] No owner field, sentinel or owner role remains in the format, compiler or runtime (grep gate).
- [ ] Bound on a hand-computed fixture equals the expected worst cell at each stored lead. It counts a shared node once, and includes pins.
- [ ] A map small enough to be wholly mandatory reports a bound equal to the whole map.
- [ ] Build Summary reports the bound and worst cell for every level, and warns only above the threshold, tested on both sides of it.
- [ ] A level with no usable id 51 reports the whole map as its bound.

**Hold:**
- [ ] On each non-portal path, drawn clusters add no reads, residents stay, and an absent drawn cluster counts a miss.
- [ ] With no usable id 51, drawn clusters stay Visible on every frame, and new cells load as the camera moves.
- [ ] On the portal path, drawn clusters stay Visible.
- [ ] A frame leaving a fallback path restores normal demand on that frame.
- [ ] During Settling on a fallback path, demand stays full.
- [ ] Settling ending on a fallback frame keeps what Settling loaded and does not shrink or evict the pool (R9).
- [ ] Residents kept through a fallback run are not aged out by it, and are not all dropped on the first portal frame after (R7).
- [ ] A teleport during a held run keeps mandatory demand for the new cell and makes it resident once the portal path returns, within the pool (R8).

**Pool:**
- [ ] A level walks with no growth when mandatory demand stays within the bound.
- [ ] A full pool evicts optional residents to admit mandatory work, lowest priority first, and refuses optional work without evicting mandatory work.
- [ ] Mandatory demand past capacity grows once and warns by name; a device limit still refuses by name.
- [ ] Pool size never exceeds the whole-map size.
- [ ] Initial pool size equals the stored bound plus headroom, capped at the whole map, and the old 256 MiB floor no longer sets it.
- [ ] Mandatory demand past capacity evicts optional residents before growing, and grows before refusing.
- [ ] Mandatory work waiting on slots still retiring does not trigger growth or further eviction on later frames; it installs when they free (R5).
- [ ] A growth while slots are pending retirement keeps those slots' contents until their epoch passes and frees them afterwards (R6).

### Manual
All on the Mac 5300M, Metal, windowed, cold PRL rebaked at this commit, using `Metal Resource Events` and the 2 km hallway route at 7 m/s from `research.md`.
- [ ] **Hallway ceiling.** SH pool and total steady allocation recorded against the spike's 3,011 MiB; peak allocation recorded against 3,577 MiB, with no SH growth during the walk. Measure and report; the target is a finding, not a gate.
- [ ] **Cluster size rule.** Result recorded, with the limit chosen and why.
- [ ] **Frames.** Indirect-only shots stay within each map's noise on the hallway, `campaign-test`, `occlusion-test` and `stress-street`, excluding shots a mover explains. The `campaign-test` darkening from the hold is explained, or its frames go to the owner to accept.
- [ ] **Theatrical moments.** A closet reveal and a door opening on a map with both: no dark frame and no refusal.
- [ ] **Planar mirrors.** Mandatory demand with a mirror in view stays within the pool on a map with one, or the bound is extended.
- [ ] **Held frames stay lit.** On the hallway and `campaign-test`, with misses counted on held frames, the count and frames captured there are recorded, and surfaces on those frames are compared against today. Owner reviews before the hold stands.
- [ ] **Clipped camera.** Pushing the view into a wall leaves pool size and frame time unchanged.
- [ ] **Cleanup.** Exit returns the driver's in-use memory to baseline, and level reload does not grow it.

## Path
Non-binding.
- **Riskiest assumption first.** First slice: bound, hold with honest miss counting and capacity-aware eviction, on a 2-layer pool, today's clusters. It proves held frames, traces refusal rendering and settles the cluster rule before format work depends on them. If the rule fails, try 16 cells, then 8.
- **Runtime seams.** `derive_node_ownership`, `request_owner_closure`, `dependent_first_release_order`, `sparse_capacity_floors`. `RowRefTable` already counts affinity rows. Local branch `spike/memory-measure-tour` shows one working shape, not production code, and holds the walk and frame-diff tools the manual rows need.
- **Sizing and eviction seams.** `plan_initial_pool_floor`, `grow_dense`, `apply_budget_policy`, `is_pressure_eligible`. Lightmap `growth_reserve_layers` is a sibling precedent, not a match.
- **Hold seams.** `PathDemand::of`, `LightmapDemand::update`. `begin_drain` carries the Settling flag and `update_targets` does not, so it needs plumbing. Tests `non_portal_path_keeps_sh_visible_and_stage_lead_classes`, `planar_mirror_adds_no_sh_reads_on_main_fallback`.
- **Compiler seams.** Id 49 is emitted at pack, after id 51 exists, so the bound needs no new stage. Selection in `cluster_directory_bake`; `requested_resident_bytes` in `pack/cluster_sh_payloads`; Build Summary precedent in `mover_light_gate/summary.rs`.
- **Rival shape.** Fixed-size pool with no growth at all: no peak and less code. Rejected provisionally, on refusal rendering black.
- **Doors.** Per-direction depth raises bytes per slot, and the bound prices it automatically. `E25--map-transitions` leaves shared versus per-level pools open; per-level bounds serve either. `mover-light-flow--probe-channel` is in progress and showed no owner dependency, but check again at build.
- **Test churn.** Closure tests in `controller_tests.rs`, `controller_refusal_tests.rs` and `node_ownership_tests.rs` assert the old contract. Rewrite them to the new one; do not delete their edges.
- Files past ~800 lines this extends: check at build, split first.

## Wire format
All integers little-endian and unsigned. Old PRLs are refused by version; pre-release, no migration.

| Surface | Change |
|---|---|
| Id 49 version | 2 → 3 |
| Range record | Drops `owner_cluster_id`, and `DENSE_OWNER_SENTINEL` goes with it. Roles reduce to dense and affinity; `Owned` and `Halo` merge |
| Id 49 tail | After cluster hints: `u32 bound_count`, then `bound_count` entries ascending by lead: `u32 lead` (id 51 fixed-point units), `u32 worst_dense_slots`, `u32 worst_cell`, `u32 family_count`, then `family_count` × (`u32 resource_index`, `u32 worst_units`), ascending by `resource_index`, for affinity-domain resources. Zero families encodes as `family_count` 0. Two entries: default lead and baked maximum, or one when the maximum does not exceed the default. With no usable id 51, one entry at lead 0 holding the whole map, `worst_cell` `u32::MAX` |
| Id 50 `requested_resident_bytes` | Bytes for every node and row the chunk touches, not only those it owned; the `semantic` validator follows |

## Boundary inventory
| Name | Sides | Note |
|---|---|---|
| `worst_dense_slots`, `worst_units` | Compiler → PRL → residency plan | Physical slots and sparse units, not source bytes |
| Default lead | `postretro-level-format` → compiler and runtime | Moves out of the engine binary, so the bound and the runtime cannot differ |
| Bound report | Compiler → Build Summary | Developer and modder facing log text, no script surface |

## Open questions
- Why refused surfaces render black when §4 says an all-miss sample reaches the ambient floor. If that contract holds, the fixed-pool rival reopens — **delegated**: the executor traces it in the first slice and reports it at the plan stop; growth stays unless the owner changes it
- Mechanism and policy under references: cluster bytes stop being additive, so the controller's ranking cannot know what a victim frees, and `DEFAULT_GPU_FLOOR_BYTES` must not become a second capacity truth — **delegated**: the executor settles the split and reports it in the plan of record
- Pool headroom over the bound: the first-fit high-water runs above live, and the value comes from measurement — **delegated**: the executor measures and reports it in the plan of record
- Whether planar mirror cells join the bound — **delegated**: the executor decides from the manual row and reports it
- Exact id 49 tail placement if a header field reads better — **delegated**: the executor reports it in the plan of record

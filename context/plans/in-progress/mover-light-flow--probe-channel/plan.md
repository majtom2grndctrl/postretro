# mover-light-flow--probe-channel — plan of record

mode: resumable
status: proposed
read at: 44881f797

Feature branch: `feat/mover-light-flow--probe-channel`. The brief was claimed on `main` at 44881f797. Every symbol cited by Decisions and Path was re-read at that commit. Since the brief's 33ac590, the bake, format and physics crates changed only in `reflection_probes`, and `frame_loop/mod.rs` kept its mover sites. No Decision premise is false. No Acceptance row needs restatement.

## IP rules for workers

Owner-reviewed patent avoidance (`ip-check.md`). Paste the brief's first Decision verbatim into every worker brief and check it in review:

> **Driven by mover state, never by light state.** Neither half recomposes because a light changed. Any compose gate is framed as residency or visibility. The channel stores per-light direct visibility and per-mover absolute static bounce, never a swapped ambient or AO layer (`ip-check.md` §Piece 7). The bounce half sums static lights only, so it never covers a light that carries a curve.

## Corrections

Bake and cache:

- "Evaluate one sample set under both traces" → `soft_visibility_with` (`lightmap_bake.rs:1107`) takes one trace and returns one f32. A new two-trace function returns both clear counts under one joint escalation. **Clarification of Bake row 1:** where the baked trace early-outs at 4 samples and the joint trace escalates, "its baked visibility" reads as the static count from the same joint sample set. Otherwise q can exceed 1, and static blocking is priced into one side only, which is the asymmetry the Path rules out. Everywhere else the two are the same number. q is the static-and-posed count over the static count, so q ≤ 1 by construction.
- "Direct memo shaped like `mover_light_probes`" → that key folds `geometry_content_hash`, which moves on a retexture, and it has no seed axis. Bake row 12 needs a retexture to re-bake no direct entry. So the q key folds an occlusion-only geometry hash (positions, indices, solidity, as the SDF key does) and the light's seed axis.
- "Duplicating one static light … no direct entry of another light" → direct-SH seeds use the light's position in `static_lights`. A duplicate appended last shifts nothing, and the row holds as `layer_cache_tests.rs` models it. **Clarification:** the test appends. A duplicate inserted mid-list shifts later lights' seeds, so their id-41/45 bytes and q legitimately change, equal to cold.
- "No patch cache on an id-34 hit" → true of `PatchCache`, but the gate's `PatchSet` is always built and moved into `bake_sh_base`, and neither survives its return. The bounce bake needs a changed `bake_sh_base` handoff. `stage_events::PATCH_CACHE_DROPPED` already gives Bake row 11 its proof.
- "Posed brushes join the occluders" → `RaytracingCtx` holds one static BVH, and `PosedBrush` is analytic planes. Occlusion wraps cheaply: `segment_clear(static) && !segment_touches_brush(posed)`. For the bounce gather, the plan is closest-hit = min(static hit, analytic entry distance into the posed convex brushes), so no BVH rebuild per pose. If posed faces must be patches (open question, task 4), they need triangles from `extract_kinematic_mover_geometry`, and that cost goes in the slice's report.
- "`storage_level_ceilings` gains the section" → not the only reader. `sh_density.rs:665` `has_delta_entry` blocks scale merges, and `delta_pin_counts` and `apply_forced_scale_constraints` take `DeltaSectionsRef` too. All must see the section, or a scale-1 node can span affected probes. The id-41 cap goes in at `classify_direct_levels` and must survive `apply_runtime_safe_envelope`.
- "The SH gate" → two gates in `admit_sh_working_sets`. The bounce bake is priced in the base gate (`surface_radiance/working_set.rs`), which runs before the material stage, so `MoverPoses::resolve` hoists ahead of it. It depends only on `map_data`. The error text "before patch shading" gets generalized.
- `probe_gate` is a centre-segment prefilter and misses penumbra probes. The q bake uses its own sample-level reach test.
- Bounce bake order → it needs nothing from the q bake, so the plan runs it directly after the id-34 gather, while the patch cache is still alive. It then outlives no delta bake, which lowers the peak. This meets P21's only hard bounds: after the gather, before the base classifier, and one drop before atlas preparation.
- id-45 "roster index" → `AnimatedBakedLights` index; same meaning. Id-34's dense tile carries alpha; bounce tiles drop it. BC6H round trip: the encoder needs dimensions in multiples of 4, so a 6×6 tile pads to 8×8, and the decode uses `decode_bc6h_block`.
- The section plumbing follows the `ghost_links` pattern: `PostBakeDeltaSections`, `pack/section_plan.rs`, `finalized_publication.rs`, and loader `staged.rs`, `entities.rs`, `prl.rs`.

Runtime:

- **Compose binding walls.** Engine limit is the default 8 storage buffers per stage. Pass B and `sh_compose` use 8 of 8; Pass A uses 6 of 8. `GridDims` is exactly 64 KiB with no headroom. Plan: one renderer-owned channel storage buffer per pass, holding entries, q bytes, tiles and a per-mover t block at its head, so one slot serves all channel data. Freeing that slot in Pass B and `sh_compose` means merging two read-only `u32` arrays; task 8 picks the pair. The engine limit is not raised.
- The GPU layout need not match the wire layout. The loader re-sorts entries light-major per cell, so the multi-mover product is one linear walk and no shader needs a nested scan.
- "`PassMembership` takes per-mover membership" → that struct has 3 bools and per-source epochs. Per-mover membership is a new per-mover row bitset, plus a "t changed" input to `ComposePlannerFrame`. A shared row reads every mover's current t, so no per-mover epochs are needed (P16).
- `use_promotion_subtraction` needs the dynamic mask bit. Channel terms gate on `BAKED_DIRECT_STATIC` alone, and with the dynamic bit off they compose at w = 0.
- `sh_compose` has no clamp today. The new clamp applies at channel-touched probes only, so "absent equals today's" holds.
- Legacy `should_dispatch` needs a t-changed term, committed with the dispatch as `last_composed_mask` is (P12).
- "Force t through the dev override" → none exists; only promotion overrides do. Capture has no mover posing, and its schema is `deny_unknown_fields`. See the slice method below.
- "Collect where `runtime_movers.rs` builds `KinematicMoverInstance`" → `collect_with_mirror` skips movers with no `Transform` or bounds, and held world-less frames never call `set_kinematic_mover_draws`. So the t collector reads the registry independently of draw culling, and it has its own setter, called from `held_level_visibility` as well. The renderer's t state resets at level install (P13).
- `path_coordinate` is in waypoint-index units and ignores `KINEMATIC_WAYPOINT_MIN_SEGMENT_LENGTH`. A new arc-length function matches `position_for_phase`'s degenerate rule (P23).
- `LIT_ENDPOINT_DRIFT` is function-local (`capture_frame.rs:662`) and gets hoisted. The "T8 ordering" is a test comment in `direct_sh_compose.rs:1050`; the streamed equivalent is commit-after-encode in `compose_plan.rs:312-334`. Allocation proof uses `crates/sim/src/alloc_probe.rs`.

**Slice method.** The Path's two slices assumed a runtime override, and format work is gated behind the bounce cost. The slices bake a pose straight into the existing sections instead. A dev-only compiler switch poses one named mover at one terminus: ids 35/41/45 trace with it as an occluder, or id 34 is replaced by S^b. Capture draws the mover at that pose. That PRL is the composed t = 1 atlas, so it tests the same premise with no format or compose code. The switch is deleted before review.

## Delegated answers

- **Section name and widths:** `MoverProbeChannel` (`build_pipeline.md` already uses it), id 57, widths as the brief's Wire format table.
- **Clamp rule:** per-texel `max(0)` on the summed base tile, before id-27 deltas add. The tile stores per-direction irradiance and the sampler's weights are convex, so non-negative texels give non-negative irradiance in every direction.
- **q for a valid probe inside a posed brush:** open; task 3 decides it against Manual row 4.
- **Posed brushes as patches:** open; task 4 decides it against Bake row 3.

## Risks for the owner's skim

- **Global later-bounce drift (highest).** Later bounces carry a mover's effect across every connected surface. A flat or dark tile's round-trip error can be near zero, and under `--uncompressed-irradiance` the bar is exact. So the affected domain could spread well past the mover, which breaks Bake row 3's "beyond the doorway's reach store no entry" and inflates bytes. Task 4 measures it. If the bar admits far probes, I stop and report; the bar is a Decision and I won't change it.
- **Gate prediction before the domain is known.** The base gate runs before any ray, but stored bounce bytes depend on the derived domain. An all-valid-probes bound could refuse maps that would fit: the hallway has 11 movers. Task 4 reports the bound against the measured peak.
- **Bake row 3's reference.** A static door changes faces, patches and cells, not only occlusion. I expect closet probes to agree, since both are near dark. If they miss the BC6H bar, I stop and report without relaxing it.
- **Bake row 4's "no brighter".** Posed walls as patches could brighten one direction near the doorway. The row is proved on tile mean unless the owner wants per-texel.

## AC-to-proof

Rows number in brief order within each group.

| AC | Row | Proof | Status |
|---|---|---|---|
| Bake 1 | q_k equals posed over baked visibility, same seed | `q_matches_soft_visibility_under_one_sample_set` | achievable, with the clarification above |
| Bake 2 | `mover-setpiece` direct entries | `setpiece_direct_entries_match_lights` | achievable as stated |
| Bake 3 | Dark closet equals static reference | `closet_bounce_matches_static_reference` | achievable as stated; risk above |
| Bake 4 | Raised room indirect entries, no brighter | `setpiece_room_bounce_entries_no_brighter` | achievable; tile mean |
| Bake 5 | Derived domain covers a full re-bake | `bounce_domain_covers_full_rebake` (test-only) | achievable as stated |
| Bake 6 | Storage caps under entries | `channel_cells_cap_storage_levels` | achievable as stated |
| Bake 7 | Directional gated, spinner none | `directional_gated_spinner_absent` | achievable as stated |
| Bake 8 | Animated light leaves bounce bytes alone | `animated_light_leaves_bounce_bytes` + compose check | achievable as stated |
| Bake 9 | Bytes equal across `-j` and cold builds | `channel_bytes_deterministic` | achievable as stated |
| Bake 10 | Gate covers peak; named refusal | `live_bytes` harness + `bounce_gate_refuses_before_rays` | achievable as stated; risk above |
| Bake 11 | Patch cache drops once (P21) | `PATCH_CACHE_DROPPED` hook test | achievable as stated |
| Bake 12 | Mover and static edits warm equal cold | `mover_edit_warm_equals_cold` family | achievable, with the appended-duplicate clarification |
| Bake 13 | id-34 hit, bounce miss (P20) | `id34_hit_bounce_miss_equals_cold` | achievable as stated |
| Bake 14 | Edit order over one cache (P19) | `edit_order_warm_equals_cold` | achievable as stated |
| Bake 15 | Byte-identical parity | opt-in ignored baseline test | achievable as stated |
| Bake 16 | Malformed section fails load | loader validation tests | achievable as stated |
| Bake 17 | Grep gate; half-only sections load | grep test + `half_sections_load_and_compose` | achievable as stated |
| Scalar 1 | t at termini, mid-travel, uneven segments, holds | `kinematic_mover` unit tests | achievable as stated |
| Scalar 2 | Ramp math and parse | ramp math + `parse_kinematic_mover` tests | achievable as stated |
| Scalar 3 | Reversal continues | physics unit test | achievable as stated |
| Scalar 4 | Two movers, one tick | collector test | achievable as stated |
| Scalar 5 | Arrival at last waypoint (P1) | collector test | achievable as stated |
| Scalar 6 | Host and client equal t | beside `client_predicted_mover_phase_matches_host_door_occlusion_phase` | achievable as stated |
| Scalar 7 | Terminus with no segment (P9) | physics unit test | achievable as stated |
| Scalar 8 | First-waypoint arrival and portal seal (P11) | collector + `rebuild_blocked_portals` test | achievable as stated |
| Scalar 9 | Frames between ticks; two-tick frame (P10) | frame-loop test | achievable as stated |
| Scalar 10 | Relevel and restart (P13) | lifecycle test | achievable as stated |
| Scalar 11 | Snapshot-set phase (P14) | client apply test | achievable as stated |
| Scalar 12 | Culled mover still gates (P15) | collector test | achievable as stated |
| Scalar 13 | Degenerate paths (P23) | physics test + compose NaN check | achievable as stated |
| Scalar 14 | Spawn before first tick (P4) | first-frame test | achievable as stated |
| Scalar 15 | t′ of 0 composes S₀ | ramp + compose test | achievable as stated |
| Scalar 16 | t pass allocates nothing | `alloc_probe` test | achievable as stated |
| Compose 1 | S₀, ungated, exact mix | headless GPU, `POSTRETRO_REQUIRE_GPU` | achievable as stated |
| Compose 2 | Stripped section equals today's | headless GPU | achievable as stated |
| Compose 3 | Indirect mix, two movers, clamp | headless GPU | achievable as stated |
| Compose 4 | Two movers compose their product | headless GPU | achievable as stated |
| Compose 5 | Animated curve; SDF untouched | headless GPU | achievable as stated |
| Compose 6 | Promoted arm | headless GPU | achievable as stated |
| Compose 7 | Quiet frame; row entering mid-travel | planner tests + headless GPU | achievable as stated |
| Compose 8 | `off` equals `sync-proof` | headless GPU | achievable as stated |
| Compose 9 | t change with no recorded compose (P12) | planner + legacy tests | achievable as stated |
| Compose 10 | One held, one travelling (P16) | planner test | achievable as stated |
| Compose 11 | Slot re-tenanted (P17) | headless GPU | achievable as stated |
| Compose 12 | Pass routing (P18) | planner test | achievable as stated |
| Compose 13 | Own tile equals id-41-backed (P22) | headless GPU | achievable as stated |
| Compose 14 | Mask toggle mid-travel (P8) | headless GPU | achievable as stated |
| Manual 1 | Leak diagnosis | owner, captures | manual; blocks the build |
| Manual 2 | Bounce-slice cost | hallway builds, owner's gate | manual; blocks format work |
| Manual 3 | Dark closet closed and ramping | owner, in-engine | manual |
| Manual 4 | `mover-setpiece` station 2 A/B | owner, in-engine | manual |
| Manual 5 | Skinned entity darkens and relights | owner A/B | manual |
| Manual 6 | Mover's own faces no darker | owner A/B | manual |
| Manual 7 | Promotion agrees within drift | capture stills, owner reads | manual |
| Manual 8 | Direct compose ≤ 0.2 ms per mover | 5300M, `async` and `off` | manual |
| Manual 9 | `sh_compose` ≤ 0.2 ms per mover | 5300M | manual |
| Manual 10 | Section bytes in `research.md` | five foundation maps | manual; measure and report |
| Manual 11 | Full-hallway cold build | idle machine, named `-j`, `--no-cache` | manual; owner sets the gate |

## Tasks

Three stops are built in. Task 1 stops for the owner's verdict. Task 4 stops for the cost gate, and format work (tasks 7 onward) waits on it. Task 5 stops if the fix outgrows the brief.

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | **Leak diagnosis.** Author `content/dev/maps/mover-closet.map` and README, build it and a static-door reference with today's compiler, capture both. Stop for the owner's verdict (Manual 1). | integrating executor | — | |
| 2 | **Mover state and capture pose.** Arc-length t in physics; registry-driven collector; renderer setter with install reset; capture forced mover t (whole pose) and forced static promotion weight. Scalar rows that need no compose. | delegated slice, physics and game; executor owns the setter seam | 1 | |
| 3 | **Direct first slice.** Two-trace soft visibility and q (Bake 1). Bake `setpiece_room` raised through the slice switch; A/B station 2 and the room's faces. Decide the inside-brush rule. | integrating executor | 1, 2 | |
| 4 | **Bounce slice.** Hand the patch cache out of `bake_sh_base`; posed gather for the closet door and raised room; Bake 3; patches decision; domain drift measured. Cold and warm mover-only hallway cost. Stop for the owner's gate (Manual 2). | integrating executor | 3 | |
| 5 | **Promotion pop.** Root-cause `drafts/animated-promotion-direct-sh-pop.md`; measure the id-41 arm; fix. Runs while task 4's gate is with the owner. Stop if it outgrows the brief. | integrating executor | 2 | |
| 6 | **Bake, whole.** Hoist poses; q bake for every mover and light with memo; bounce bake for every mover with memo; derived domain; gate pricing; delete the slice switch. | integrating executor | 4 (owner's gate) | |
| 7 | **Format and load.** Section 57, ramp keys and FGD, validation, every `DeltaSectionsRef` reader, own-tile flag after the drop and coarsening, loader hand-off of (s, e). Bake rows close here. | delegated slice, format and loader; executor owns the pipeline seam | 6 | |
| 8 | **Compose.** Free one slot in Pass B and `sh_compose`; channel buffer and upload; Pass A, Pass B, `sh_compose`; legacy t-changed dispatch. | integrating executor | 7 | |
| 9 | **Streamed planner.** Per-mover row bitsets, t-changed trigger, pass routing, commit-after-encode, residency and re-tenant cases. | integrating executor | 8 | |
| 10 | **Integration proof.** Remaining automated rows; five-map bytes and hallway cold build (Manual 10, 11); 5300M timing (Manual 8, 9); owner A/B packet (Manual 3–7). | integrating executor | 5, 9 | |
| 11 | **Preflight and review.** `/preflight`, `/review-panel` and `/fix-review-findings` loop, full relevant gate, results column. Wait for "land the plane." | integrating executor | 10 | |

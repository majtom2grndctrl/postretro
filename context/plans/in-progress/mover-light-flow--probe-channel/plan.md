# mover-light-flow--probe-channel — plan of record

mode: resumable
status: blocked
blocked on: direction-aware probe visibility, its own spec (owner verdict 2026-11-04, see Leak diagnosis result)
read at: 44881f797

Feature branch: `feat/mover-light-flow--probe-channel`. The brief was claimed on `main` at 44881f797. Every symbol cited by Decisions and Path was re-read at that commit. Since the brief's 33ac590, the bake, format and physics crates changed only in `reflection_probes`, and `frame_loop/mod.rs` kept its mover sites. No Decision premise is false. No Acceptance row needs restatement.

## IP rules for workers

Owner-reviewed patent avoidance (`ip-check.md`). Paste the brief's first Decision verbatim into every worker brief and check it in review:

> **Driven by mover state, never by light state.** Neither half recomposes because a light changed. Any compose gate is framed as residency or visibility. The channel stores per-light direct visibility and per-mover absolute static bounce, never a swapped ambient or AO layer (`ip-check.md` §Piece 7). The bounce half sums static lights only, so it never covers a light that carries a curve.

## Corrections

Places where source, or another row of the brief, disagrees with the brief's text, or where the brief leaves a constraint unpriced.

- **Bake row 1 denominator (clarification).** The row says "divided by its baked visibility"; the Path says to decide escalation jointly so an early-out does not price static blocking into one side only. The two differ in one case: the baked trace agrees all-clear at 4 samples (v = 1.0) while the posed trace splits and escalates. Read literally, q then carries static blocking the baked v skipped. **Owner ruling, 2026-11-03:** the denominator is the static count from the same joint sample set. The deciding reason is the multiply Decision: q must hold only the mover's own effect, or two movers on one light count a hidden static occluder twice. Early-outs fire only on full agreement, so q ≤ 1 under either reading; this is about the asymmetry, not a bound. A fully sealed mover stores q = 0 either way. `soft_visibility_with` (`lightmap_bake.rs:1107`) takes one trace, so a two-trace function is new.
- **Direct memo key.** The Decision says "shaped like `mover_light_probes`", but that key folds `geometry_content_hash`, which serializes `GeometryResult` with texture names and indices, so a retexture moves it. Bake row 12 needs a retexture to re-bake no direct entry. The q key folds an occlusion-only geometry hash (positions, indices, solidity, as the SDF key does) and the light's seed axis, which the gate key lacks.
- **Compose binding walls (unpriced by the brief).** Engine limit is the default 8 storage buffers per stage. Pass B and `sh_compose` use 8 of 8; Pass A uses 6 of 8. `GridDims` is exactly 64 KiB with no headroom. Proposal: one renderer-owned channel storage buffer per pass, holding entries, q bytes, tiles and a per-mover t block at its head. Freeing that slot in Pass B and `sh_compose` means merging two read-only `u32` arrays; task 8 picks the pair and checks their lifetimes match. The engine limit is not raised.
- **"Force t through the dev override."** No override exists yet, and format work waits on the bounce gate. See the slice method below.
- id-45 "roster index" → `AnimatedBakedLights` index; same meaning.

## Build notes

Source facts the brief's pins already require or allow. None contradicts the brief. They are here so a resumed session need not re-derive them.

Bake and cache:

- P20/P21 patch lifecycle: `PatchCache` is built only on a miss and the gate's `PatchSet` always; neither survives `bake_sh_base`'s return, so its handoff changes. `stage_events::PATCH_CACHE_DROPPED` proves Bake row 11.
- `DeltaSectionsRef` gaining the section reaches `delta_pin_counts` and `apply_forced_scale_constraints`, but `sh_density.rs:665` `has_delta_entry` lists its views by hand and must be extended. The id-41 cap goes in at `classify_direct_levels` and must survive `apply_runtime_safe_envelope`.
- Bake row 12's duplicate is appended, as `layer_cache_tests.rs` models it. Direct seeds use position in `static_lights`, so a mid-list insert shifts later lights' seeds and their entries, equal to cold.
- The SH gate is two gates in `admit_sh_working_sets`. The bounce bake is priced in the base gate (`surface_radiance/working_set.rs`), which runs before the material stage, so `MoverPoses::resolve` hoists ahead of it; it depends only on `map_data`. The error text "before patch shading" gets generalized.
- `probe_gate` is a centre-segment prefilter and misses penumbra probes. The q bake uses its own sample-level reach test.
- Id-34's dense tile carries alpha; bounce tiles drop it. BC6H round trip: the encoder needs dimensions in multiples of 4, so a 6×6 tile pads to 8×8, and the decode uses `decode_bc6h_block`.
- Section plumbing follows the `ghost_links` pattern: `PostBakeDeltaSections`, `pack/section_plan.rs`, `finalized_publication.rs`, and loader `staged.rs`, `entities.rs`, `prl.rs`.

Proposals, not findings; task 4 settles both by measurement:

- Posed occlusion: `segment_clear(static) && !segment_touches_brush(posed)`. For the bounce gather, closest-hit = min(static hit, analytic entry distance into the posed convex brushes), avoiding a BVH per pose. If posed faces must be patches, they need triangles from `extract_kinematic_mover_geometry`.
- Bounce bake placement: the Path runs it after the q bake. Running it directly after the id-34 gather also meets P21 and stops the patch cache outliving the delta bakes, but it may overlap the volume store, so its peak is unmeasured. The Path's order stands unless the measured peak favours the change.

Runtime:

- The GPU layout need not match the wire layout. The loader re-sorts entries light-major per cell, so the multi-mover product is one linear walk.
- Per-mover membership is new: a per-mover row bitset plus a "t changed" input to `ComposePlannerFrame`, not an extension of `PassMembership`'s 3 bools. A shared row reads every mover's current t, so no per-mover epochs are needed (P16).
- `use_promotion_subtraction` needs the dynamic mask bit. Channel terms gate on `BAKED_DIRECT_STATIC` alone, and with the dynamic bit off they compose at w = 0.
- The new `sh_compose` clamp applies at channel-touched probes only, so "absent equals today's" holds. Legacy `should_dispatch` needs a t-changed term, committed with the dispatch as `last_composed_mask` is (P12).
- Hand-over at the `rebuild_blocked_portals` sites, as the Path says, covers held world-less frames, which never call `set_kinematic_mover_draws`. `collect_with_mirror` also skips movers with no `Transform` or bounds, so t reads the registry directly. The renderer's t state resets at level install (P13).
- `path_coordinate` is in waypoint-index units and ignores `KINEMATIC_WAYPOINT_MIN_SEGMENT_LENGTH`. A new arc-length function matches `position_for_phase`'s degenerate rule (P23).
- `LIT_ENDPOINT_DRIFT` is function-local (`capture_frame.rs:662`) and gets hoisted. The streamed equivalent of the "T8 ordering" is commit-after-encode in `compose_plan.rs:312-334`. Allocation proof uses `crates/sim/src/alloc_probe.rs`.

**Slice method.** The Path's two slices assumed a runtime override, and format work is gated behind the bounce cost. The slices bake a pose straight into the existing sections instead. A dev-only compiler switch poses one named mover at one terminus: ids 35/41/45 trace with it as an occluder, or id 34 is replaced by S^b. Capture draws the mover at that pose. That PRL is the composed t = 1 atlas, so it tests the same premise with no format or compose code. The switch is deleted before review.

## Delegated answers

Adopted by the owner, 2026-11-03. Two are firm leans that a slice still has to confirm.

- **Section name and widths:** `MoverProbeChannel` (`build_pipeline.md` already uses it), id 57, widths as the brief's Wire format table.
- **Clamp rule:** per-texel `max(0)` on the summed base tile, before id-27 deltas add. The tile stores per-direction irradiance and the sampler's weights are convex, so non-negative texels give non-negative irradiance in every direction.
- **q for a valid probe inside a posed brush:** test it without the brush containing it, keeping the mover's other brushes as occluders. Nothing lives inside a brush; these probes feed only that mover's own faces. Dark drags lit faces down, and full ignores the other brushes. Task 3 confirms against Manual row 4.
- **Posed brushes as patches:** yes, with their albedo, unless task 4 prices them out. The Problem asks for bounce that matches a bake with the mover in place, and a black door leaves a dark halo on its bright side. Task 4 confirms against Bake row 3 and reports the cost.

## Owner answers

Adopted 2026-11-03.

- `light_ramp_start` / `light_ramp_end`: keep the names.
- Bake row 4's "no brighter" is proved on tile mean. A posed wall can correctly brighten one direction.
- If the static reference closet glows, direction-aware probe visibility comes first, as its own spec, and this brief pauses. The leak is foundation work that hits every thin wall. Task 1 still stops for the owner's verdict on the captures.
- Bounce cost gate: still open; the owner sets it from task 4's report. Proposed shape: gate mainly on warm one-mover edit time, with cold growth held to a ratio of SH Bake.

## Risks for the owner's skim

- **Global later-bounce drift (highest).** Later bounces carry a mover's effect across every connected surface. A flat or dark tile's round-trip error can be near zero, and under `--uncompressed-irradiance` the bar is exact. So the affected domain could spread well past the mover, which breaks Bake row 3's "beyond the doorway's reach store no entry" and inflates bytes. Task 4 measures it. If the bar admits far probes, I stop and report. The bar is a Decision, and a perceptual floor on dark or flat tiles would change it, so that is the owner's call.
- **Gate prediction before the domain is known.** The base gate runs before any ray, but stored bounce bytes depend on the derived domain. An all-valid-probes bound could refuse maps that would fit: the hallway has 11 movers. Task 4 reports the bound against the measured peak.
- **Bake row 3's reference.** A static door changes faces, patches and cells, not only occlusion. I expect closet probes to agree, since both are near dark. If they miss the BC6H bar, I stop and report without relaxing it.
- **The door's closet face (Manual 3).** That face also samples bright-side probes, with backface rejection off and mover-absent depth moments. A bright-side probe looking toward the door sees the door's lit face, so the closet face could read bright even with correct channel data. This is `research.md`'s leak-across-walls risk. Task 4's slice PRL is the first evidence; if it glows, I stop and report.

## Leak diagnosis result (task 1, Manual 1)

**Owner verdict, 2026-11-04: the reference glows.** The owner sees a little glow in the static-door column of `verdict-sheet.png`. Under the owner's ruling, direction-aware probe visibility comes first as its own spec, and this brief pauses. Do not start task 2. Resume here once that spec has landed, and re-capture the reference before continuing. The closet bar stands unchanged.

Built with today's compiler at 7af769fba from `content/dev/maps/mover-closet.map`. Captures are in `captures/`; `verdict-sheet.png` holds the six panels that matter. Values are post-tonemap sRGB, 0–255, so they rank brightness but are not linear light.

The reference glows at the worst lattice phase, in a narrow band at the door:

| Build | Station 3 frame mean | Station 2 frame mean | Floor 0.04 m past the door plane | 0.26 m | 0.48 m |
|---|---|---|---|---|---|
| Today, mover door | 6.60 | 6.83 | 23 | 19 | 17 |
| Reference, door 8 units back (the fixture) | 0.50 | 0.15 | 13 | 2 | 0 |
| Reference, door flush with the closet wall | 0.26 | 0.14 | 17 | 10 | 1 |
| Reference, no plane straddles the door | 0.00 | 0.00 | not profiled | not profiled | not profiled |

The floor columns are means across the doorway's width. In the fixture's 8-unit reveal, 0.07–0.18 m before the door plane, the reference floor reads 18–21 against today's 25–27, so the band there is nearly as bright as today's build.

- **Most of the closet's glow is the bake's.** Today's glow fills the room. The reference's stays within about 0.5 m of the door. The channel can remove the first, not the second.
- **The rest is the sampler's, and it depends on the lattice.** The first reference capture read pure black because a probe plane fell inside the door slab, which leaves no valid bright-side probe sharing a cell with the closet. Sweeping the closet's north wall through a full period (y 768, 760, 752, 744, 736) gave leaks at exactly the phases the lattice predicts, for both door placements. About 59% of placements of a 16-unit door are exposed: (39.37 − 16) ÷ 39.37. The fixture is fixed at y 760, the worst phase measured, so Bake row 3 cannot pass by luck.
- **Why it leaks, by reading, not by experiment.** A probe 3.6 units from the door sees the bright room in most directions, so its one omnidirectional depth mean stays large and `sh_corner_depth_visibility` does not reject the closet fragment behind the door. This matches the Decision's prediction. No experiment here isolates it.
- **Fixture checks.** Door recognition seals the doorway (`1 of 1`, portals 4 and 5). Static-direct-only captures read black in the closet in all four builds (frame mean ≤ 0.0012), so no bright-room light reaches it directly. The bright room reads 143. The gate's 345 texels for the closed door are therefore outside the closet; the soft emitter's far edge reaches the doorway floor as far as y 493, inside the door slab's footprint, which likely accounts for them.
- **Door's closet face (Manual 3 risk).** Today it reads 1.7 at slab centre, through the fog, against the reference's 0. That is mover-absent data, so it says nothing yet about the face under a gated bake. Task 4 remains the first evidence.
- **Water.** Turning the reflection term off changes no pixel statistic in today's build, so the pool's teal is not the reflection probe. Its source was not isolated. The reference's water reads black.

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
| 1 | **Leak diagnosis.** Author `content/dev/maps/mover-closet.map` and README, build it and a static-door reference with today's compiler, capture both. Stop for the owner's verdict (Manual 1). | integrating executor | — | Done. Owner verdict 2026-11-04: reference glows, so the brief pauses for direction-aware probe visibility. See Leak diagnosis result. |
| 2 | **Mover state and capture pose.** Arc-length t in physics; registry-driven collector; renderer setter with install reset; capture forced mover t (whole pose) and forced static promotion weight. Scalar rows that need no compose. | delegated slice, physics and game; executor owns the setter seam | 1 | |
| 3 | **Direct first slice.** Two-trace soft visibility and q (Bake 1). Bake `setpiece_room` raised through the slice switch; A/B station 2 and the room's faces. Decide the inside-brush rule. | integrating executor | 1, 2 | |
| 4 | **Bounce slice.** Hand the patch cache out of `bake_sh_base`; posed gather for the closet door and raised room; Bake 3; patches decision; bounce placement by measured peak; domain drift measured. Cold and warm mover-only hallway cost. Stop for the owner's gate (Manual 2). | integrating executor | 3 | |
| 5 | **Promotion pop.** Root-cause `drafts/animated-promotion-direct-sh-pop.md`; measure the id-41 arm; fix. Runs while task 4's gate is with the owner. Stop if it outgrows the brief. | integrating executor | 2 | |
| 6 | **Bake, whole.** Hoist poses; q bake for every mover and light with memo; bounce bake for every mover with memo; derived domain; gate pricing; delete the slice switch. | integrating executor | 4 (owner's gate) | |
| 7 | **Format and load.** Section 57, ramp keys and FGD, validation, every `DeltaSectionsRef` reader, own-tile flag after the drop and coarsening, loader hand-off of (s, e). Bake rows close here. | delegated slice, format and loader; executor owns the pipeline seam | 6 | |
| 8 | **Compose.** Free one slot in Pass B and `sh_compose`; channel buffer and upload; Pass A, Pass B, `sh_compose`; legacy t-changed dispatch. | integrating executor | 7 | |
| 9 | **Streamed planner.** Per-mover row bitsets, t-changed trigger, pass routing, commit-after-encode, residency and re-tenant cases. | integrating executor | 8 | |
| 10 | **Integration proof.** Remaining automated rows; five-map bytes and hallway cold build (Manual 10, 11); 5300M timing (Manual 8, 9); owner A/B packet (Manual 3–7). | integrating executor | 5, 9 | |
| 11 | **Preflight and review.** `/preflight`, `/review-panel` and `/fix-review-findings` loop, full relevant gate, results column. Wait for "land the plane." | integrating executor | 10 | |

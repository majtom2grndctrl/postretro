# Review panel findings (b6fc0c4ee..e51ed190e)

## Hygiene — reach index slice (sonnet)
- 🟡 portals.rs:154-155 `distribute_winding` doc: 107-col line; describe tolerance fully. Fix: "...front side first. Splits use `tolerance.epsilon`; each piece of at least `tolerance.min_area` reaches `on_pair` with the two children it ends between: leaves, or nodes `descend` refused."
- 🟡 portals.rs:13-20 `PORTAL_EPSILON` / `MIN_PORTAL_AREA_M2` docs read as the only tolerance; add "The shipped value; [`PortalTolerance`] carries the per-call override (the reach flood uses a tighter one)". `generate_portals` doc: add "under [`PortalTolerance::SHIPPED`]".
- 🟡 build_pipeline.md:214 step 12 still says reach cull "decided and not yet built"; all new headers point there; flood.rs:15-17 cites it for τ argument. → landing docs task (planned at land the plane).
- 🟢 lightmap_reach.rs:195-197 `chart()` doc "as for the shaft scene" → "as `shaft_cull`'s scene does".
- 🟢 flood.rs:12-20 FLOOD_PORTALS doc: optional "engine metres".

## Hygiene — coverage proof slice (sonnet)
Code:
- 🟢 gate_map_geometry.rs:4 redundant `#![cfg(test)]` (main.rs:40 gates it). Drop inner.
- 🟢 face_removal_seam.rs:21 `with_removed_faces` restores predicate only on normal return; use a drop guard.
- 🟢 settle.rs:59 `Measure::boxes` doc "grown by τ" → "τ plus a rounding slack" (caller passes tau + ROUNDING_EPSILONS*EPS*(1+extent)).
- 🟢 settle.rs `partners` O(n²) loop — optional why-comment.
Drift:
- 🟡 partition/manifold.rs:1-2, 23-24 header + `check_watertight` doc still sells parity as the hole detector. Fix header: "Edge-parity watertightness diagnostic (info-level): odd-covered edge spans in the emitted world-face set. Not a hole test; the coverage proof owns hole warnings." Doc "(dropped faces)" → "(usually a dropped face; also spans that are not holes)". Keep "never fails the build".
- 🟡 hinge_tests.rs:1,17,30,56,65 name abandoned rules (end-stub, seam rule, fan, "(row 1d)"). Rewrite as present-tense properties; keep geometry numbers.
- 🟡 coverage_proof.rs:~68-73 (WELD_RADIUS_METERS doc) and settle.rs:20 point at in-progress plan files (weld-sweep.md, plan.md row 1c, weld-consensus). State the substance (named gap 3 wording; δ_min 50 µm measured, δ = 100 µm) and drop plan paths.
- 🟡 R1..R22 labels in weld_tests.rs / hinge_tests.rs come from plan file; collide with face label "R1" in hinge_tests (225, 266). Drop Rn prefixes; rename face labels.
- 🟡 build_pipeline.md:193 and 214 "decided, not yet built" → landing docs.
- 🟢 face_removal_seam.rs:2-3 "(research P7 of lightmap-reach-cull)" plan pointer; drop.
- 🟢 gate_map_geometry.rs header: note shared by census and coverage-proof gate-map tests.
- 🟢 test_fixtures.rs:2 §4 exists (verified by other reviewer).

## Hygiene — walk + stage slice (sonnet)
Code:
- 🟡 `warn_coverage_holes` (pipeline/lightmap_stage/reach.rs ~31-70) is proof presentation; move to pipeline/coverage_proof_stage.rs, call `coverage_proof_stage::warn_coverage_holes`; drop the lightmap_stage re-export and the upward import.
- 🟢 lightmap_stage.rs ~514 and ~620 duplicate `stage_exact_skips(window)` + `reach.build_index(...)`; make a `StageReach` method taking `window`.
- 🟢 reach_walk.rs `check_rewalk` bumps `pairs_rewalked` (hidden side effect); move the add to the caller in chart_walk.rs beside `pairs_culled`.
Drift:
- 🟡 chart_walk.rs:1-2 header: append "the reach-culled walk and verify re-walk".
- 🟡 Plan refs in durable comments: face_removal_seam.rs:2-3 (research P7); reach_verify.rs:55 ("as the brief's warm gate rows do"); reach_verify_walk_tests.rs:2-5 + inline P1/P2/P4 at 55,102,212; reach_warm_tests.rs:3 (research P3, P5). State the invariant; drop P-numbers and "the brief".
- 🟡 chart_cull.rs:15-20 ExactSkips doc "No flag, env var or cache key holds this" beside new env var: clarify `POSTRETRO_LIGHTMAP_REACH_VERIFY` only checks the reach cull, does not switch it (on the `reach_cull` field doc or there).
- 🟡 build_pipeline.md 193/214/217 → landing docs.
- 🟢 reach_walk.rs `check_rewalk` doc fragment "Whether they differ." → "Returns whether they differ."
- 🟢 reach.rs `StageReach::closed: Vec<bool>` needs a doc (solid or exterior leaf).

## Contract verifier — verify switch and stage logs (opus)
- 🟡 1. "Always on in tests" false for tests baking through the prl-build binary: `REACH_VERIFY_DEFAULT = cfg!(test)` only covers prl-build's own unit tests. 13 integration tests in crates/level-compiler/tests/ (sh_base_warm_cold.rs, lightmap_working_set_contract.rs, compiler_cli_contract.rs, buried_lights_compile.rs, animated_weight_maps_fixtures.rs, …) use `Command::new(env!("CARGO_BIN_EXE_prl-build"))`; engine capture tests crates/postretro/tests/capture_support/mod.rs launch prl-build too. None set POSTRETRO_LIGHTMAP_REACH_VERIFY. Fix (Decision says always on in tests): set `.env("POSTRETRO_LIGHTMAP_REACH_VERIFY","1")` in those harnesses, except where a test counts lightmap cache reads/hits (verify bypasses lightmap layer + section reads) — opt those out with "0" and a why-comment.
- 🟡 2. reach.rs `finish()`: `baked == 0` branch logs "stage served from cache" even on an uncached build where no partition exists (compose branch with `layer_lights.is_empty()`, e.g. all static lights `_shadow_type sdf`). Fix: "served from cache" only when the section memo hit or loaded > 0; otherwise say no pair was walked.
- 🟢 3. Full section-memo hit prints "stage served from cache (0 partition(s) loaded, …)" — name the memo ("served from the lightmap_section memo") instead of 0 partitions.
- 🟢 4. `reach_verify_enabled()` still reads env under cfg(test); a dev shell with =0 turns verify off in unit tests. Under cfg(test) ignore env, keep only VERIFY_OVERRIDE seam (and unit-test the resolver directly, as now).
- 🟢 5. manifold.rs header frames parity as hole surfacer (dup of coverage-proof hygiene); `MAX_SAMPLES` doc "Max open-edge locations to surface" now also caps proof samples — say so.
- 🟢 6. Verify on + warm: `[cache] lightmap_section miss` printed for a deliberate bypass; log "bypassed (reach verify)" instead (check the gate/tests that grep this text).

## Correctness tracer — walk and verify (opus): no 🔴/🟡; all brief rules hold by execution
- 🟢 1. = contract #2 (SDF-only map "served from cache"); fix: tie wording to `!compose_lightmap || loaded > 0`, not `baked == 0`. (2x)
- 🟢 2. = contract #6 (bypass logged as miss, lightmap_stage.rs:456-459 / :318). (2x)
- 🟢 3. bypass_reads depends only on verify; map with cull off re-bakes cold and re-walks nothing. Brief says "while it is on … bypassed" literally → NOT acting (owner-text).
- 🟢 4. build_index runs on warm builds where every layer hits; lazy build optional → NOT acting (small; owner ruled small-map cost fine).
- 🟢 5. After a recorded mismatch, every remaining culled pair still re-walks before failing → optional; NOT acting.
- 🟢 6. = contract #4 (env overrides cfg(test) default). (2x)

## Correctness tracer — coverage proof (opus): no 🔴/🟡; every handoff holds
- 🟢 1. coverage_proof.rs:173-175 `limit = tau − 256·ε·(1+extent)` squared without a floor: inf extent (or > ~8.8e9 m) → limit negative → limit² huge → fails OPEN. Fix: if extent not finite or limit <= 0, report everything unproven (fail closed); add a unit test with a non-finite / huge vertex.
- 🟢 2. clusters.rs:18-20, settle.rs:85,101,216,264: `opposed()` and side/opposite tests read the WELDED direction; an edge ~2δ long can rotate ~90° under the weld, so two same-way sub-mm stored edges could pass `opposed`. Under named gap 3, but deviates from weld-consensus §1.5 (welded identity only for cancellation keys + placing ends). Fix: test `opposed` (and side/opposite) on stored `to − from`; keep it only if gate maps stay 0/0/0 (run the ignored gate_map_tests with POSTRETRO_COVERAGE_MAPS) and R-tests stay green; otherwise revert and document the welded-direction use in a code comment.
- 🟢 3. edges.rs:78 `face_brushes.get(face).copied().flatten()` hides misalignment; add `debug_assert_eq!(face_brushes.len(), geometry.face_index_ranges.len())` in `prove_coverage`.
- note: settle.rs:63 comment says degenerate triangle distance is NaN; Ericson returns finite distance to segment — fix comment wording.

## Seam tracer — proof to walk (opus): no 🔴/🟡; order, same-triangles, P7 seam, handoff, indexing, degenerate inputs, warm path all correct
- 🟢 1. = contract #2 "served from cache" on SDF-only cold build (3x).
- 🟢 2. = walk tracer #4 (index built on all-layer-hit warm) — NOT acting.
- 🟢 3. coverage proof par_iter (coverage_proof.rs:187-190, clusters.rs:42) runs without governor enter()/checkpoint (build_pipeline.md §Governor). ~0.1 s. Proof tracer said other non-ray stages also skip it. Fix agent: check what comparable non-ray parallel stages do; follow that convention (enter once per parallel item or a checkpoint), or leave and say why.
- 🟢 4. = contract #6 bypass logged as miss (3x).
- Note → landing docs: cache side of the named gaps. Cull inputs (proof result, BSP solidity, exterior set) are in no cache key by design; inside a named gap a wrongly culled pair is cached and served warm until geometry in its key bound changes; verify mode bypasses caches so still catches it. Record in build_pipeline.md step 12 named gaps.

## Correctness tracer — reach index (opus): no 🔴/🟡; wiring, eligibility, R, f32/f64, flood soundness, portal change (shipped callers byte-identical), slab, query all verified
- 🟢 1. slab.rs:282-312 `ChartFrame::reach`: if `descend` touches no leaf, open_leaves empty → `culled_chart` !any → culled (fail open, unreachable today). Fix: "no leaf touched ⇒ None (kept)" guard in `chart()`; unit test via seam if cheap.
- 🟢 2. flood.rs:30 `SEED_SLACK` and slab.rs:54 `EMITTER_SLACK` are absolute 1 mm; f32 rounding ~1e-7·|coord| passes 1 mm beyond ~8 km (WINDING_HALF_EXTENT 16,384 m). R's slack scales (REACH_SLACK_PER_METER·|centre|). Fix: add a per-metre term like R's (only grows the seed sphere / emitter clearance → more conservative), and update the docs.

## Adversarial tester — reach exactness (opus): no byte-moving input outside the named gaps; verify catches any break
- 🟡 A1. Per-texel skip silently degrades to zero past ~233 m from world origin (bytes exact, speed-up lost). slab.rs:35,125-135: `slack = (POSITION_SLACK_PER_METER 4e-6 + 2.4e-7)·magnitude + 1e-6`, magnitude = component-wise max of |origin| + |u|·span_u + |v|·span_v; slab floor `lowest = ε(1−1e-5) − slack·|dual₂| − radius·1e-5` falls to ~1e-5 (= DESCENT_TOLERANCE) at M ≈ 233 m → every texel box touches its own face's brush solid → mark_traced marks every texel. A room at x = 240 m, or a 90 m chart at 150 m. Large maps are where the benefit is meant to land. Slack ~7× looser than needed (real f32 error at 233 m ≈ 1.4e-4 m). Fix direction: build each texel's slab from its actual f32 `chart_texel_world_position` (as the walk does) into f64 frame coords, leaving only the few-add slack scaled by the real coordinate; same constant feeds `heights[1]` (emitter clearance). MUST stay sound (slab holds every trace start the walk uses). Red test first: two-rooms fixture translated ~300 m (and ~1 km) → texels skipped > 0, bytes equal culled vs unculled under verify at both floors.
- 🟢 A2. R's footprint term (TexelFrame::radius, texel_footprint.rs:267-272 → lightmap_reach.rs:183-193) assumes orthonormal axes; frames accepted to |u·v| = 1e-3 → offset up to √(1+1e-3)·radius. Absorbed by R's spare 2ε + 2e-5/m for texel steps < ~4 m. Fix: scale footprint term by √(1+skew) (or 1+skew) — cheap and exact.
- 🟢 A3. = reach tracer #2 (SEED_SLACK / EMITTER_SLACK absolute 1 mm; "far below this" comment wrong at large coords). Scale with |c| like REACH_SLACK_PER_METER.
# Review round 2 findings (e51ed190e..80610f03e)

## Hygiene (sonnet)
Code:
- 🟡 H1 chart_cull.rs:113 `pub(crate) const POSITION_SLACK_PER_METER` only used in its own file (:200) → private `const`.
- 🟢 H2 slab.rs:22 `FRAME_SKEW_LIMIT`/`FRAME_NORM_LIMIT` duplicate shaft_cull.rs:53 values; reuse shaft_cull's constants (or "keep equal" note).
- 🟢 H3 capture_support/mod.rs:55 bare env literal → add "keep in sync with prl_build_support::REACH_VERIFY_ENV / reach_verify.rs" note.
- 🟢 H4 coloured_bounce_source_gates.rs:461 `--help` on raw Command — optional move to prl_build(); skip unless trivial.
- 🟢 H5 flood.rs `reach_radius` doc "emitter target lies within `size`" — seed now uses `emitter.target_radius()`; state that target rounding past `size` is inside R's margins (REACH_MARGIN_EPSILONS + REACH_SLACK_PER_METER) — CHECK the claim holds numerically (target_radius adds 1 mm + 1e-6·(|c|+size); R margin must cover the TARGET's rounding, not the seed's 1 mm slack — the seed's 1 mm is seed-sphere conservatism, the target itself is within size + rounding).
Drift:
- 🟡 D1 manifold.rs:53-55 and :118 over-width, spliced doc lines → re-flow.
- 🟡 D2 reach_warm_tests.rs:33, 72, 110 prose "served from cache" where line now says memo → "served from the `lightmap_section` memo" (keep line 142's legit assertion).
- 🟡 D3 reach_hole_warning_tests.rs:236 same.
- 🟡 D4 coverage_proof/tests.rs:145, 289, 304 "P6:"/"P8:" prefixes → drop.
- 🟡 D5 settle.rs:20 "named gap 3" points at absent list → drop the numbering (say "a named gap of the reach cull") or point to build_pipeline.md §step 12 (landing will add it).
- 🟢 D6 compiler_cli_contract.rs:863 "Acceptance row 8" pre-existing → leave (out of scope).
- 🟢 D7 flood.rs ~17-25 FLOOD_PORTALS doc missing blank `///` between paragraphs.

## Adversarial — slab bound round 2 (opus): could not break it; containment holds for every input tied to code
- 🟡 A1 Test gap: no test drives the measured-drift path at a size that matters. Far test offsets map x only (engine −z); charts axis-aligned → height drift exactly 0, in-plane drift ~6e-5 hidden in the pad; a sign flip in `drift = frame_coords(centre) − exact` (slab.rs:146), dropping drift_lo/hi from `extent` (slab.rs:175), or `centre.length()` → one component would pass every test. Fix: direct containment unit test — oblique chart (normal off every axis) ~1 km out on all three axes; reproduce the walk's f32 arithmetic for every texel and footprint offset (`world_p + offset`, `+ normal·ε`, then segment_ray's `from + dir·ε` toward emitter targets, see texel_footprint.rs:541,566; occlusion.rs:178); assert every start's `frame_coords` lies in `texel_box(tx, ty)`; assert `emitter_clears` ⇒ segment length ≥ ε. Prove the test bites: temporarily flip the drift sign / drop drift_hi and confirm it fails, then restore.
- 🟢 A2 lightmap_reach.rs:295 `culled_chart` builds descent+mask before checking `emitter_clears`; check emitter_clears against the frame first (cost only). Optional.

## Tracer round 2 (opus): no 🔴/🟡; all flows clean
- 🟢 T1 clusters.rs:20-22 `opposed()` comment + pairing_tests.rs:133-164 test claim the weld can reverse a <2δ edge end for end; it cannot (single linkage + rule 3b ⇒ (B−A)·(b−a) > 0; rotation < 90°). Real case: two same-way edges rotated ~85° opposite ways read welded-opposed (~170° apart). Fix comment; rebuild test from that reachable pair (both welds must be legal: clusters ≤ δ wide, no two corners of one triangle).
- 🟢 T2 settle.rs:198-210 stored sides can yield a false hole (short tilted non-reference member + twin both same side when stored run ~0); fails closed; keep stored sides; name the false-hole cost in the settle.rs header.
- 🟢 T3 serial phases lack checkpoint() — SKIP (0.1 s).
- 🟢 T4 coverage_proof.rs:271-289 + coverage_proof_stage.rs: non-finite spans print "NaN m"/"inf m" in info line and warning; print "unbounded" when not finite (and midpoints similarly if cheap).
- 🟢 T5 compiler_cli_contract.rs:256-262 opt-out in shared helper `compile_fixture_for_layer_cache_order` covers cold/seeded bakes too; lightmap_working_set_contract.rs:79-85 opts out every cached build. Narrow to the bakes that count lightmap cache reads.
- 🟢 T6 no automated test drives the non-test `reach_verify_enabled` (env read, OnceLock, warn-once). Add an integration test (binary, small fixture) with a malformed value: exactly one live warning line (note: the end-of-build WARN summary replays every WARN once — count accordingly or assert on the live line), verify off (no "re-walked" in stage line).

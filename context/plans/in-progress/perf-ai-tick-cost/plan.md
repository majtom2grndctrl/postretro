# perf-ai-tick-cost — plan of record

mode: resumable
status: approved
read at: 6405e57a

Feature branch: `claude/determined-volta-bts6r0` (claim pushed to `main` at 6405e57a).

## Corrections
- Brief read at efdab38d6 → no file under `crates/ai`, `crates/sim/src/nav`, `combat_positioning*`, `agent_steering*`, `ai_host.rs` or `sim/` changed since. Every cited symbol was re-read; every Decision premise holds as stated.
- Path suggests "nav index and route scratch first" → planning around it by landing the P2 digest, the AI sub-stages and the multi-arena harness first. The manual rows report the positioning sub-stage "before and after each change"; that sub-stage and the harness must exist at the commit before each change.
- AC "registry column walks per AI pass (counted behind the registry's `test-support` feature)" → `postretro-entities` has a `test-support` feature but no walk counter. Planning around it by adding a counter in `EntityRegistry::iter_with_kind`, compiled only under that feature.
- AC "the steering sub-stages under `sim_steering`" → `sim_steer_agents`, `sim_steer_pose` and `sim_steer_movers` already exist (`sim/cpu_stages.rs`). Planning around it by asserting them in the same sub-stage test, with no new steering stage.
- Path "Route scratch … behind every A* entry point" → `NavGraph::connected_now` (the topological fallback when `has_tactically_direct_target_path`'s funnel refuses, in levels with mover records) also allocates a region-sized `seen` per call. It is a flood fill, not A*, and runs off the per-tick path. Planning around it by leaving it unchanged; the positioning cache already cuts its calls to one per target and position.
- Path "the stress row needs harness work to read [stages]" → `StageFrame::scope` takes `&self`, so `AiTickInputs` carries an `Option<&StageFrame<SimStage>>`. The row builds its own frame for the timing-on run.

## Delegated answers
- Where the pawn index lives — `postretro-ai`, new module `pawn_index.rs` beside `targeting.rs`. Its buffers live in `AiRuntime` and are cleared and rebuilt at the start of every pass (P1), so no allocation recurs once warm and the existing zero-allocation due-scan test keeps passing.
- Whether a faction bucket gains a grid — no grid in this brief. Buckets are scanned linearly, which meets "tracks the hostile pawns it must inspect". The converged horde puts the horde in the allied bucket, which the walk skips. A grid inside a large hostile bucket (faction war) waits for a measured row that shows it hot.
- Clearance query cell size — the batch's spacing (`2 × agent radius`, `dynamic_min_spacing`). Each test visits the cells spanned by `[p − s, p + s]` on X and Z, computed with `floor`. Rounding is monotonic, so no peer within `s` falls outside the span. The exact `distance_xz < s` test, claimant exclusion and non-finite skip run unchanged on the visited peers.
- Where route scratch lives — `NavScratch` in `postretro-sim::nav`, generation-stamped. The AI pass owns one in `AiRuntime`, lent to compute's reachability query and the positioning batch. The steering pass creates one per pass. A generation wrap clears every stamp before reuse (P5b).
- P2 digest OS — the checked-in digest is recorded in this Linux container at the pre-change commit and replayed here. The owner's Windows digest is recorded on that desktop for the manual stress row, per `research.md` §Ordering pins (`sin`/`cos` may differ across platforms). The replay test fails, printing the digest, when no digest exists for its OS. A missing digest is never a silent pass.
- AI sub-stage labels — `sim_ai_targeting`, `sim_ai_positioning`, `sim_ai_perception`, `sim_ai_graph` and `sim_ai_apply`, all children of `sim_ai`, following the `sim_steer_*` precedent. Targeting, perception and graph scopes open per enemy and sum. Positioning wraps query building and the batch, after `resolve_combat_slots`' early returns. Apply covers slot write-back, attack-turn arbitration and `apply_outcomes`.
- Scene for AC 4 — the 1 and 64 brains chase without an action. Firing an entity-raycast weapon walks the `Health` column per shot, which is weapon cost, not target selection. The 60,000 non-pawn entities fit under the registry's `u16` slot bound.

## AC-to-proof

| AC | Proof | Status |
|---|---|---|
| 1 Target selection matches brute-force reference across randomized scenes (every listed scene class) | `pawn_index` tests: seeded randomized scenes vs `targeting::reference_select` (today's scan, `#[cfg(test)]`), plus one named test per listed edge class | achievable as stated |
| 2 P1: second of two consecutive ticks matches reference | `target_selection_after_apply_mutates_pawns_matches_reference` | achievable as stated |
| 3 No refresh for allied non-attackers or hostiles past the first eligible; LOS only up to it plus ledger attackers; refresh count equal at 0 and 128 allied peers | refresh and perception counters at the seam, `#[cfg(test)]` | achievable as stated |
| 4 0 vs 60,000 non-pawn entities: same targets; column walks per pass equal for 1 and 64 brains | entities `test-support` walk counter; AI test over both registries | achievable as stated |
| 5 Batched positioning matches uncached reference on every assignment field across randomized scenes (every listed class) | `combat_positioning` tests vs `select_combat_positions_batch_uncached` (`#[cfg(test)]`) | achievable as stated |
| 6 Fresh chase batch: grounding, docked-mover test and target route at most once per distinct position, 1 and 16 enemies | scoring-seam counters, `#[cfg(test)]` | achievable as stated |
| 7 P4: clearance examines only nearby agents | peer-visit counter on the proximity query, 128-agent batch | achievable as stated |
| 8 Two consecutive batches (dock, gate close) match uncached | positioning test | achievable as stated |
| 9 Indexed lookup equals linear scan (every listed point class) | nav tests vs `region_at_linear` / `resolve_region_at_linear` (`#[cfg(test)]`) | achievable as stated |
| 10 P3: 4,000+-region lookup examines only nearby candidates | region-visit counter, finite points | achievable as stated |
| 11 P2: identical per-tick targets, latches, slot claims for chase, strafe, strafe+hits infight at 32/64/128 | `replan_stress_digest` (ignored, release) replaying the checked-in Linux digest after tasks 4, 5, 7 and 9 | achievable as stated |
| 12 Route scratch equals fresh-allocation reference on ~4,000-region graph, listed cases and P5 | nav path tests vs `find_path_from_fresh` (`#[cfg(test)]`) | achievable as stated |
| 13 Zero allocations in a warm adjacent-region search; per-node resets equal on 4,000 vs 10 regions | `alloc_probe` around the search step; reset counter on `NavScratch` | achievable as stated |
| 14 Sub-stages report under `sim_ai`, steering ones under `sim_steering`, ≤ parent; positioning absent with no engaged enemy; none with timing off | sim tick test with an enabled `StageFrame` | achievable as stated |
| 15 Existing AI, steering, positioning, nav suites pass unchanged | `cargo test -p postretro-ai -p postretro-sim` | achievable as stated |
| M1 Stress row on owner's Windows desktop, baseline and goals | owner, release | manual |
| M2 Multi-arena row on owner's desktop, before and after each change | owner, release; harness lands in task 3 | manual |
| M3 Steering, windowed `stress-env-volumes`, before and after the nav index | owner, release with dev-tools | manual |

## Tasks

Sequential on the feature branch: free disk (29 GB) leaves no room for a worktree build. Each task commits with its `plan.md` row.

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | P2 digest. Add a per-tick (enemy, target, retaliation latch, slot bits) digest to the stress row for chase, strafe and strafe+hits infight at 32/64/128. Record it at 6405e57a and check it in. Add the ignored release replay test, then run it twice. Riskiest premise: the row is deterministic enough to gate. | integrating executor | — | done: `crates/ai/fixtures/replan_stress_digest.linux.txt` recorded at 807e8cc0 (source = 6405e57a); replay passed twice in separate processes (~17 s release); a one-nibble edit fails with "strafe\|64 … diverged within ticks 120..150". Digest folds target, latch, slot and position bits per agent per tick. |
| 2 | AI sub-stages: five `SimStage` variants under `Ai`; `AiTickInputs` carries the frame; scopes in compute, `resolve_combat_slots` and the pass; stress row's timing-on run reads them. AC 14. | integrating executor | 1 | |
| 3 | Multi-arena row: port `spike_arena_row` from efdab38d's `spike-visible-rates.diff` without its per-role AI filter. Four ≥64×64 m arenas on ~4,000 regions; spread waves 64+436 and 128+872; roles costed by whole-pass differences. Print-only, ignored. Shares a 4,000-region graph builder with tasks 4–5. | integrating executor | 2 | |
| 4 | Nav point-to-region index: cell → region CSR built in `NavGraph::from_section` over the union of region rects. `region_at` / `resolve_region_at` visit the cells their tolerance spans, order-independent by lexicographic min with index last. Non-finite points keep the linear scan. Linear scans stay test-only. AC 9, 10. | integrating executor | 3 | |
| 5 | Route scratch: `NavScratch` with generation-stamped `g_score`, `came_from`, `rode_from` and a reused heap. Scratch variants of every A* entry point; existing signatures wrap a fresh scratch for cold callers. Hot callers (AI reachability, positioning, steering replans) take the pass's scratch. Fresh-allocation reference stays test-only. AC 12, 13, P2 replay. | integrating executor | 4 | |
| 6 | Targeting references: keep today's scan as `#[cfg(test)]` `reference_select`; add the entities walk counter and the seeded randomized-scene harness, green against current code. | integrating executor | 5 | |
| 7 | Faction-first pawn index: per-pass build beside the snapshot pass (movement holders, then brain-only holders, not the snapshot filter), buckets on exact faction bits, hostility per bucket pair. Stride from hostile buckets. Due-tick walk in (distance `total_cmp`, group, slot) order, stopping at the first eligible hostile. Ledger attackers evaluated in scan order; refresh takes faction from the bucket. AC 1–4, P2 replay. | integrating executor | 6 | |
| 8 | Positioning reference: keep the uncached batch as `#[cfg(test)]`; seeded randomized-scene harness green against current code. | integrating executor | 7 | |
| 9 | Positioning cache: split `score_candidate`. Per-batch cache keyed on (target bits, position bits) for grounding, docked mover and tactically direct target route. Per-enemy clearance via the per-batch proximity query, LOS and agent route. Nothing kept across batches. AC 5–8, P2 replay. | integrating executor | 8 | |
| 10 | Container stress row and multi-arena timings (informational, Linux), full AI/sim suites, `/preflight`, then `/review-panel` → `/fix-review-findings` loop. | integrating executor | 9 | |

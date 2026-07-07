# E10 — Enemy Combat Positioning

## Goal

Enemies choose stable combat destinations around the player they target instead of chasing that player's exact center point. A small wave should pressure and surround the player, hold an attack band, and avoid body pileups without adding a full tactics system.

## Scope

### In scope

- **Candidate combat positions.** Generate deterministic candidate points in an engagement ring around a target position — the per-enemy chosen target from `E10--enemy-mp-target-selection`, passed in as a plain `Vec3` so the generator stays target-agnostic. Candidates are ordered from stable inputs so ties resolve identically across runs.
- **Engagement radius source.** The first slice derives the ring radius from the resolved AI attack range already carried by the brain/descriptor. Tests may pass an explicit radius through the pure query. Do not add a hidden module-level combat radius constant.
- **Reachability and occupancy filters.** Reject candidates outside the navmesh, candidates with no path from the agent, and candidates whose capsule cannot occupy the static world. Static occupancy is a real capsule placement check, not a navmesh proxy: a valid candidate has a walkable floor under it and a capsule at the candidate center that does not penetrate static world geometry. If the current collision API lacks that check, add a small Rust-only helper beside the existing capsule/ray queries. Reject or heavily penalize candidates already claimed by another enemy, or too close to another enemy's capsule footprint.
- **Scored selection.** Score remaining candidates by attack-band distance, path cost, line of sight when available, flank/angle preference, separation from other agents, and hysteresis toward the current selected position (`COMBAT_SLOT_SWITCH_MARGIN`; see Task 2). The score is a lower-is-better metric cost anchored in metres. Favor spacing enough to avoid multiple enemies selecting or pushing into the same occupied area.
- **AI integration.** In `Alert` / chase behavior, set the agent destination to the selected combat position — a ring point around the per-enemy chosen target (`{entity, position}` carried on `EnemyOutcome` by `E10--enemy-mp-target-selection`), NOT the shared raw `player_pos`. Each enemy rings the player it targets. `Attack`, `Death`, damage timing, and animation state stay governed by the existing brain FSM.
- **Scarce-slot fallback.** If fewer valid slots exist than enemies, use the valid slots for the best-scoring claimants and let the rest fall back to the existing chase/block behavior for their chosen target. Do not force multiple enemies into the same invalid or occupied point just to satisfy the ring pattern.
- **Deterministic slot claims.** Resolve claims from a frozen per-tick enemy snapshot, not from mutable per-enemy iteration order. Identical inputs must choose the same accepted slots regardless of registry iteration order; ties break by stable entity id.
- **Stability.** Small player motion must not churn destinations every tick. Keep the last selected combat position while it remains valid enough. The selected position and its validity/hysteresis timer persist on `BrainComponent` (with the other think-stride FSM state), added as additive `#[serde(default)]` fields — no wire/format version bump.
- **Debuggability.** Unit tests cover candidate generation, scoring, slot spreading, hysteresis, and determinism. A `dev-tools` overlay may draw candidates/scores if useful during playtest.

### Out of scope

- Full cover system.
- Squad tactics, patrol logic, scripted flanks, or influence maps.
- Ranged-projectile behavior.
- ORCA/RVO or predictive crowd avoidance. Existing separation remains the local crowd layer.
- Navmesh format or bake changes.
- Per-archetype tactical descriptor fields. Add those later only if playtest needs different enemy positioning styles.

## Acceptance criteria

- [ ] Multiple enemies chasing one player select distinct reachable combat positions and do not all path to their target's center point (runnable unit test with a hand-built navmesh and several agents; the pure selector is fed target positions directly, so it needs no `select_target` wiring).
- [ ] An enemy near the player holds or adjusts within an engagement band instead of pushing into the player capsule (runnable FSM/positioning test; no renderer required).
- [ ] When the target player's raw position is unreachable or already crowded, the enemy picks a reachable nearby combat position if one exists; if none exists, it falls back to the existing chase/block behavior without panicking.
- [ ] When nav reachability accepts a candidate but static capsule occupancy rejects it, the selector skips that candidate and chooses the next valid option or falls back cleanly. Runnable unit test; no renderer required.
- [ ] When fewer valid combat slots exist than enemies, no two enemies are forced into the same invalid/occupied slot. Extra enemies fall back to existing chase/block behavior or keep a valid incumbent slot if hysteresis allows it.
- [ ] Slot claims are deterministic and order-independent: the same frozen enemy/target/candidate inputs produce the same accepted slots and fallbacks even when the enemies are presented to the selector in a different order; exact ties break by stable entity id. Runnable unit test; no renderer required.
- [ ] In a multiplayer setup with enemies targeting different players, combat slots are generated around each enemy's chosen target position, not around a single global player position.
- [ ] Candidate choice is stable against `COMBAT_SLOT_SWITCH_MARGIN`: with the deferred score-term weights held neutral so the score reduces to its metric (metres) cost, a target nudge that leaves the best challenger within `COMBAT_SLOT_SWITCH_MARGIN` of the incumbent's cost keeps the current combat slot (no destination change); a nudge that makes a challenger beat the incumbent by more than the margin switches slots. Runnable unit test asserting both directions; no renderer required.
- [ ] Scoring is deterministic. Identical inputs produce identical selected positions and tie breaks.
- [ ] Existing steering, stuck recovery, path-preservation, separation, and locomotion-animation tests remain green (assumes `E10--enemy-stuck-recovery` has landed — see Sequencing).
- [ ] Manual check on `content/dev/maps/campaign-test`: a small wave pressures the player more naturally than raw chase-to-player, with less jerky left/right correction when enemies crowd or collide, and no new wall-hugging or wedge regressions.

## Tasks

### Task 1: Candidate Query

Add a pure candidate generator/filter/scorer in a NEW module, `combat_positioning` (`crates/postretro/src/combat_positioning.rs`), decoupled from steering. It takes a small explicit input struct, `CombatQuery` { `agent_pos: Vec3`, `engagement_radius: f32`, `target_pos: Vec3` (target-agnostic — the caller passes whichever target it chose), and borrowed handles to `NavGraph` + `CollisionWorld` } — it deliberately does NOT reuse steering's module-private `AgentSnapshot` (`agent_steering.rs:231`), so the EQS-like selector stays decoupled and deterministically unit-testable. Production callers pass the resolved AI attack range as `engagement_radius`; tests may pass explicit values. Generate a small ring/radial set around `target_pos`, filter by navmesh region membership + path reachability (`NavGraph::region_at`, `nav::find_path` — path length summed from its returned `Vec<Vec3>` waypoints) and static occupancy, score (attack-band error is the primary term AC2 turns on; the fuller weighted term set lives in Scope/Open questions and stays playtest-deferred), and return candidates in deterministic tie-broken order.

Static occupancy is its own filter. A candidate is occupiable only when ground-stick finds a walkable floor within the step envelope and a capsule placed at the candidate center is not penetrating the static `CollisionWorld`. If the existing collision facade only exposes sweeps/rays, add a narrow helper for capsule-vs-static-world placement. Keep it Rust-only and static-world-only. Do not infer capsule occupancy from navmesh membership or path success. The other-agent occupancy snapshot is added in Task 2 (kept out of the base `CombatQuery` so the pure reachability/scoring core has no crowd dependency).

### Task 2: Slot Occupancy and Hysteresis

Persist the selected combat position on `BrainComponent` (`crates/entities/src/components/brain.rs`), NOT `AgentComponent`: combat-slot selection is a tactical think-stride decision and belongs with the other AI/FSM state (`attack_cooldown_remaining_ms`, `think_stride_counter`); the agent merely steers to the resulting destination. Add two additive `#[serde(default)]` fields — `combat_slot: Option<Vec3>` (selected position; `None` until first selection) and `combat_slot_hold_ticks: u32` (its validity/hysteresis countdown) — both seeded in `BrainComponent::from_descriptor`, following the `death_despawn_remaining_ms` (`brain.rs:149`) / `locomotion_moving` (`brain.rs:154`) serde-default precedent. Extend `CombatQuery` (Task 1) with the frozen other-agent combat-slot snapshot (built from the AI tick's existing per-brain snapshot — each `EnemySnapshot` already carries the brain) and the incumbent `combat_slot` (re-scored this tick from the same query — no persisted cost field, so the two added fields stay the only new brain state), so both the occupancy penalty and the hysteresis term are computed inside the pure scorer and the AI tick just writes the winner back to `BrainComponent`. Penalize candidates occupied, claimed, or too near another enemy. Resolve contested claims from the frozen snapshot with a stable entity-id tie break so accepted slots do not depend on registry iteration order; a later enemy cannot steal an accepted slot unless it wins by the same score and hysteresis rules applied to every claimant. Apply hysteresis with a pinned constant, `COMBAT_SLOT_SWITCH_MARGIN` (world-units / metres): keep `combat_slot` while it stays valid (on-navmesh, path-reachable, unclaimed) and no challenger beats its cost by more than the margin; switch only when a challenger clears the margin or the current slot goes invalid. `combat_slot_hold_ticks` bounds how long a slot persists before a forced re-score. Score-term WEIGHTS stay deferred to playtest; only the switch margin is pinned.

### Task 3: AI Integration

Replace the chase arm's destination write at `ai.rs:545` with combat-position selection, COMPOSED with `E10--enemy-mp-target-selection` (ordering predecessor — see Sequencing). That spec rewrites `:545` to carry the per-enemy chosen target (`{entity, position}` on `EnemyOutcome`, resolved by `select_target`); this spec takes that chosen target's `position` as the engagement-ring CENTER, runs `combat_positioning` (Task 1/2) to pick the combat slot, and writes THAT as the destination — the two edits compose (target choice → ring center → ring point), they do not clobber. Each enemy rings the player it targets, never the shared raw `player_pos`; add a test with two target players to lock this in. Preserve existing behavior when no candidate is available, or when all valid slots are claimed by better-scoring enemies: fall back to the chosen target's raw position so the FSM still chases, attacks, clears steering, and dies through the current paths. Runs only when a nav graph and a chosen target exist; with neither, behavior is unchanged.

### Task 4: Tests and Diagnostics

Add unit tests for candidate reachability, slot spreading, scarce-slot fallback, order-independent slot claims, hysteresis against `COMBAT_SLOT_SWITCH_MARGIN`, and deterministic tie breaks — all against the pure `combat_positioning` module fed target positions directly, so they need no `select_target` wiring. Add a `BrainComponent` back-compat test that a serialized brain missing `combat_slot` / `combat_slot_hold_ticks` deserializes to the seeded defaults (the `brain_serde_defaults_missing_locomotion_latch` precedent). Add a `dev-tools` candidate overlay only if manual tuning needs visibility into scores.

## Sequencing

**Ordering predecessors (other specs must land first):**
- `E10--enemy-mp-target-selection` — provides the per-enemy chosen target (`{entity, position}` on `EnemyOutcome`, via `select_target`) that Task 3 consumes as the ring center. It rewrites `ai.rs:545` to carry the chosen target; this spec composes onto that edit (ring point around the chosen target) rather than re-deriving a single shared `player_pos`. Without it, `EnemyOutcome` carries no target and Task 3 has nothing to ring.
- `E10--enemy-stuck-recovery` — lands the `stuck_ticks` / `unstick_window_remaining` recovery and its tests. The regression AC ("stuck recovery … tests remain green") is measured against that sibling's suite, so it must exist first.

**Phase 1 (sequential):** Task 1 — establishes the pure selection surface.
**Phase 2 (sequential):** Task 2 — consumes Task 1 output and adds persisted stability on `BrainComponent`.
**Phase 3 (sequential):** Task 3 — consumes the selector and writes destinations in the AI tick, composed onto the mp-target-selection `:545` edit.
**Phase 4 (concurrent):** Task 4 — tests land with each task; diagnostics are optional after integration.

## Rough sketch

- Keep the first version deterministic and cheap: a fixed ring of candidate offsets around the target position, sorted by angle/index, then filtered/scored.
- Candidate filtering uses existing runtime surfaces: `NavGraph::region_at` for region membership, `nav::find_path` for path reachability (path length summed from its returned waypoints — it yields `Option<Vec<Vec3>>`, not a scalar cost), a narrow `CollisionWorld` capsule placement query for static occupancy, optional ray checks for line of sight, and agent snapshots for slot occupancy.
- Use an EQS-like shape, not a general EQS framework: generate candidates, run tests/filters, score, choose best. The data stays Rust-internal for now.
- Score terms should be normalized and simple: attack-band error, path length, line-of-sight bonus, separation penalty, flank/angle preference, current-slot hysteresis bonus.
- Pinned constant: `COMBAT_SLOT_SWITCH_MARGIN` (world-units / metres, default `1.0`) — the cost improvement a challenger must beat to unseat the incumbent slot, or the slot is kept. The score is a lower-is-better metric cost anchored in metres: attack-band error + path length are costs in metres; LOS/flank/hysteresis enter as metre-equivalent cost reductions and separation as a cost addition. A challenger must lower the incumbent's cost by more than the margin to switch. Pinning it makes AC4 runnable pass/fail independent of the still-deferred score-term weights.
- Store only the minimum stability state on `BrainComponent`: `combat_slot: Option<Vec3>` (selected point) and `combat_slot_hold_ticks: u32` (validity timer). Avoid reservations that survive despawn or death; recompute from the frozen agent snapshot each tick.
- Resolve slot claims from one frozen snapshot. Do not let registry iteration order decide which enemy keeps a contested slot; use stable entity id only as a tie break after score and hysteresis.
- The combat point is a destination, not movement authority. Agent steering still owns path following, separation, acceleration, stuck recovery, and collision.
- Combat positioning does not change facing arbitration. Moving `Alert`/`Attack` enemies still face resolved velocity; stopped engaged enemies still face their selected target. Yaw changes remain rate-limited by `E10--enemy-facing-slew`.
- Treat scarce valid slots as normal. The selector may return `None` for an enemy when every candidate is invalid, unreachable, or claimed by a better-scoring peer. The AI integration then uses the existing raw-target chase fallback.

## Boundary inventory

None. This draft adds Rust-internal AI/navigation behavior. It does not add wire, PRL, FGD, TypeScript, or Luau surface. The two new `BrainComponent` fields are engine-internal (`BrainComponent` is never reachable through `worldQuery`) and land as additive `#[serde(default)]` fields — a back-compat persistence change with no wire/PRL/format version bump, mirroring how `steer_velocity` / `stuck_ticks` landed on `AgentComponent`. If per-archetype positioning fields are promoted later, that follow-up needs its own boundary inventory.

## Open questions

- **Line of sight cost.** Static-world ray checks are useful for attack pressure, but the first melee enemy can work without hard LOS. Decide during implementation whether LOS is a filter, a score bonus, or deferred.
- **Target side preference.** A flank bias can make waves read better, but it can also feel artificial in narrow rooms. Start with a weak score term or leave it disabled until playtest.
- **Navmesh clearance escalation.** If selected combat positions are navmesh-reachable but the capsule cannot physically occupy them, do not patch this plan with bake work. Open a separate navmesh-clearance / capsule-exact refinement draft.
- **Candidate density.** If fixed rings produce too few good slots in narrow rooms, tune the candidate set inside this plan. Do not add influence maps, squad tactics, or descriptor policy fields as part of this slice.

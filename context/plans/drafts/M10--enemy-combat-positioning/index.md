# M10 — Enemy Combat Positioning

## Goal

Enemies choose stable combat destinations around the player instead of chasing the player's exact center point. A small wave should pressure and surround the player, hold an attack band, and avoid body pileups without adding a full tactics system.

## Scope

### In scope

- **Candidate combat positions.** Generate deterministic candidate points around the target player in an engagement ring. Candidates are ordered from stable inputs so ties resolve identically across runs.
- **Reachability and occupancy filters.** Reject candidates outside the navmesh, candidates with no path from the agent, and candidates whose capsule cannot occupy the static world. Reject or heavily penalize candidates already claimed by another enemy.
- **Scored selection.** Score remaining candidates by attack-band distance, path cost, line of sight when available, flank/angle preference, separation from other agents, and hysteresis toward the current selected position.
- **AI integration.** In `Alert` / chase behavior, set the agent destination to the selected combat position instead of the player's raw position. `Attack`, `Death`, damage timing, and animation state stay governed by the existing brain FSM.
- **Stability.** Small player motion must not churn destinations every tick. Keep the last selected combat position while it remains valid enough.
- **Debuggability.** Unit tests cover candidate generation, scoring, slot spreading, hysteresis, and determinism. A `dev-tools` overlay may draw candidates/scores if useful during playtest.

### Out of scope

- Full cover system.
- Squad tactics, patrol logic, scripted flanks, or influence maps.
- Ranged-projectile behavior.
- ORCA/RVO or predictive crowd avoidance. Existing separation remains the local crowd layer.
- Navmesh format or bake changes.
- Per-archetype tactical descriptor fields. Add those later only if playtest needs different enemy positioning styles.

## Acceptance criteria

- [ ] Multiple enemies chasing one player select distinct reachable combat positions and do not all path to the player center (runnable unit test with a hand-built navmesh and several agents).
- [ ] An enemy near the player holds or adjusts within an engagement band instead of pushing into the player capsule (runnable FSM/positioning test; no renderer required).
- [ ] When the player's raw position is unreachable or already crowded, the enemy picks a reachable nearby combat position if one exists; if none exists, it falls back to the existing chase/block behavior without panicking.
- [ ] Candidate choice is stable: small player movement below the hysteresis threshold does not cause destination churn every tick.
- [ ] Scoring is deterministic. Identical inputs produce identical selected positions and tie breaks.
- [ ] Existing steering, stuck recovery, path-preservation, separation, and locomotion-animation tests remain green.
- [ ] Manual check on `content/dev/maps/campaign-test`: a small wave pressures the player more naturally than raw chase-to-player, with no new wall-hugging or wedge regressions.

## Tasks

### Task 1: Candidate Query

Add a pure candidate generator/filter/scorer over `NavGraph`, the target player position, the agent position, `CollisionWorld`, and a frozen snapshot of other agents. Generate a small ring or radial set around the player, filter by navmesh region/path reachability and static occupancy, then return scored candidates in deterministic order.

### Task 2: Slot Occupancy and Hysteresis

Track the current selected combat position per brain or agent. Penalize candidates occupied or claimed by other enemies. Keep the current position while it remains reachable and close enough to the new optimum, so small player motion does not create destination churn.

### Task 3: AI Integration

Replace the chase arm's raw `set_destination(player_pos)` call with combat-position selection when a nav graph and target player exist. Preserve existing behavior when no candidate is available: the FSM still chases, attacks, clears steering, and dies through the current paths.

### Task 4: Tests and Diagnostics

Add unit tests for candidate reachability, slot spreading, fallback behavior, hysteresis, and deterministic tie breaks. Add a `dev-tools` candidate overlay only if manual tuning needs visibility into scores.

## Sequencing

**Phase 1 (sequential):** Task 1 — establishes the pure selection surface.
**Phase 2 (sequential):** Task 2 — consumes Task 1 output and adds persisted stability.
**Phase 3 (sequential):** Task 3 — consumes the selector and writes destinations in the AI tick.
**Phase 4 (concurrent):** Task 4 — tests land with each task; diagnostics are optional after integration.

## Rough sketch

- Keep the first version deterministic and cheap: a fixed ring of candidate offsets around the player, sorted by angle/index, then filtered/scored.
- Candidate filtering uses existing runtime surfaces: `NavGraph` for region membership and path reachability, `nav::find_path` for path cost, `CollisionWorld` capsule/ray queries for static occupancy and optional line of sight, and agent snapshots for slot occupancy.
- Use an EQS-like shape, not a general EQS framework: generate candidates, run tests/filters, score, choose best. The data stays Rust-internal for now.
- Score terms should be normalized and simple: attack-band error, path length, line-of-sight bonus, separation penalty, flank/angle preference, current-slot hysteresis bonus.
- Store only the minimum stability state: selected point and maybe a validity timer. Avoid reservations that survive despawn or death; recompute from the frozen agent snapshot each tick.
- The combat point is a destination, not movement authority. Agent steering still owns path following, separation, acceleration, stuck recovery, and collision.

## Boundary inventory

None. This draft adds Rust-internal AI/navigation behavior. It does not add wire, PRL, FGD, TypeScript, or Luau surface. If per-archetype positioning fields are promoted later, that follow-up needs its own boundary inventory.

## Open questions

- **Line of sight cost.** Static-world ray checks are useful for attack pressure, but the first melee enemy can work without hard LOS. Decide during implementation whether LOS is a filter, a score bonus, or deferred.
- **Target side preference.** A flank bias can make waves read better, but it can also feel artificial in narrow rooms. Start with a weak score term or leave it disabled until playtest.
- **Navmesh clearance escalation.** If selected combat positions are navmesh-reachable but the capsule cannot physically occupy them, do not patch this plan with bake work. Open a separate navmesh-clearance / capsule-exact refinement draft.

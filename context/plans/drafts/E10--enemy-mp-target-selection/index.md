# Enemy Multiplayer Target Selection

> **Status:** draft
> **Epic:** E10 (enemy movement / AI), consuming the E15 multiplayer foundation.
> **Related:** `context/plans/roadmap.md` Epic 10 · Epic 15 (charter: *gameplay-specific
> networking lives with the feature epics*) · `context/research/cell-visibility-substrate.md`
> (the intended future resolver behind this spec's target-selection seam) · the E10 "Enemy
> line-of-sight + cover" bullet (exact eye-to-target BVH raycast — the visibility predicate this
> seam is built to accept).

---

## Problem

Enemies only chase the host / local player; in a session, every remote client is ignored. Root
cause is a single-target resolve in `player_position`
(`crates/postretro/src/scripting/systems/ai.rs:245`): it returns one `Vec3`, taken from
`registry.local_player_pawn()` (the pawn **this machine** controls) and falling back to the *first*
`iter_with_kind(ComponentKind::PlayerMovement)` entity. Both are single-target, and
`local_player_pawn` is inherently client-side — a headless / dedicated server has none. So
server-side AI targeting binds to the host pawn and never considers remote client pawns. **Co-op is
functionally broken for every non-host player:** enemies neither chase nor threaten them.

The enumeration primitive needed to fix this **already exists** and is even used here as the
fallback: `iter_with_kind(ComponentKind::PlayerMovement)` surfaces all player pawns server-side
(client pawns carry `PlayerMovement`). So no E15 foundation change is required — this is pure E10
gameplay networking, which per E15's charter lives with the feature epic.

## Goal

Server-authoritative enemy target selection over **all** player pawns, so each enemy chases the
appropriate player (nearest, or nearest-visible) regardless of which client controls them.
Single-player behavior is bit-for-bit unchanged. Land the selection behind a **named, extensible
chokepoint** so a future view-independent cell-visibility broad-phase (see
`cell-visibility-substrate.md`) can plug in without re-touching the FSM.

## Scope

### In scope
- Replace `player_position`'s single-pawn resolve with a **`select_target` seam** that ranks over
  all `PlayerMovement` pawns and returns the chosen `{ entity, position }` for a querying enemy.
  v1 policy: nearest by distance.
- Thread the chosen target through the FSM targeting input (`player_pos` consumer at `ai.rs:334`)
  and the acquisition-gated leash (`ai.rs:205`).
- Keep `local_player_pawn` for its legitimate client-side uses (camera / prediction / health owner);
  **remove it from the AI targeting path only**.

### Out of scope
- The damage-target path (`pawn_with_health` / `damage_target`, `ai.rs:343`) — a distinct concern,
  unchanged.
- The exact eye-to-target LOS / cover raycast (separate E10 bullet). This seam is built to *accept*
  a visibility predicate; it does not implement one.
- Any cell-visibility / broad-phase substrate. The seam is a named plug point only
  (`cell-visibility-substrate.md`); building the substrate waits for its own real consumer.
- Aggro / target-switch feel tuning beyond the minimum needed to avoid per-tick thrash (a feel pass
  is a follow-up if a playtest demands it).
- Any network / replication change — the pawn set is already server-visible via `iter_with_kind`.

## Acceptance Criteria

1. `cargo build -p postretro` and focused `cargo test -p postretro <ai filter>` pass; no new `unsafe`.
2. **Client is targetable.** With two `PlayerMovement` pawns at different distances (host + client),
   `select_target` returns the nearer pawn — including when that pawn is the client. Verified in the
   `ai_tests` harness. (Automated.)
3. **Single-player unchanged.** With one pawn, selection returns that pawn identically to today;
   existing `ai_tests` stay green. (Automated.)
4. **No `local_player_pawn` in the targeting path.** `rg 'local_player_pawn'
   crates/postretro/src/scripting/systems/ai.rs` returns no hits on the targeting path. (Automated
   grep in review.)
5. **The seam is real and documented.** The chokepoint is a single named function/type whose
   signature admits an injectable visibility/relevance predicate (`impl Fn(EntityId) -> bool` or an
   equivalent seam) without changing the FSM or its callers, with a doc-comment naming
   `context/research/cell-visibility-substrate.md` as the intended future resolver. (Review.)
6. **Manual-visual:** in a local 2-client playtest, nearby enemies chase *both* players; neither is
   ignored. (Manual — not machine-verified.)

## Tasks

### Task 1 — Target-selection seam over all player pawns
`crates/postretro/src/scripting/systems/ai.rs`. Replace `player_position` (`:245`) with a
`select_target(registry, from: Vec3, visible: Option<impl Fn(EntityId) -> bool>) -> Option<TargetPawn>`
chokepoint: iterate `iter_with_kind(ComponentKind::PlayerMovement)`, optionally filter by the
`visible` predicate, rank by distance from the querying enemy, return `{ entity, position }`. Remove
the `local_player_pawn()` branch from this path. Update the FSM targeting consumer (`:334`) and the
acquisition-gated leash (`:205`) to use the per-enemy chosen target. Keep the `visible` seam unused
(pass `None`) until the LOS bullet lands — pre-emptive wiring for a planned trigger, not dead code.

### Task 2 — Tests + seam documentation
Extend `ai_tests` with a two-pawn case (host + client at different distances → nearer chosen; assert
the client *can* be chosen). Keep the single-pawn cases green. Doc-comment the `select_target`
chokepoint as the named extension point and point it to `context/research/cell-visibility-substrate.md`.

## Decisions

- **Nearest, server-authoritative, no `local_player_pawn`.** Targeting is a server concern over the
  full pawn set; `local_player_pawn` is a client-side convenience and must not leak into AI. Nearest
  is the v1 policy; visibility-gating rides the LOS bullet through the `visible` seam.
- **Seam now, substrate later.** The chokepoint is the named plug point per the *hardcoded-but-seamed*
  principle. The cell-visibility broad-phase is built *with* its own real consumer (E15 Phase 4
  relevance / Epic 12 audio), not here — see `cell-visibility-substrate.md`.

## Risks

- **Target thrash between near-equidistant pawns.** Two players at similar range could flip-flop the
  target per tick. Existing acquisition-gating (targets re-evaluate on think ticks only, `:205`)
  already damps this; add small hysteresis only if a playtest shows thrash.
- **Selection cost.** `iter_with_kind(PlayerMovement)` × enemies is O(enemies × players) per think
  tick — trivial at co-op scale. If enemy counts ever make it hot, the `visible`/selection seam is
  exactly where a cell-visibility broad-phase drops in — a measured, future concern
  (`cell-visibility-substrate.md`), not a v1 cost.

## Related work

- The **target-selection seam** this spec lands is the plug point named in
  `context/research/cell-visibility-substrate.md` for a future view-independent AI-perception
  broad-phase. This spec builds only the thin, real (nearest-of-all-pawns) version behind that
  chokepoint; it does **not** build the substrate.
- E10 "Enemy line-of-sight + cover" — its exact eye-to-target BVH raycast is the `visible` predicate
  this seam accepts.

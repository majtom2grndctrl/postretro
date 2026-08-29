# Co-op Triggers, Interaction Events, and Semi-Random Trap Pools

> **Read this when:** drafting the last of Epic 18 — co-op respawn/player-leave policy (E18-R)
> or the playable capstone encounter (E18-E). The rest of Epic 18's opening spec set (trigger
> fan-out, plate/button semantics, co-op activation policy, spawner/closet containment, seeded
> trap-pool arming) has shipped.
> **Status:** trigger fan-out, activation policy, spawner + closet containment, seeded trap-pool
> arming, enemy groups, and timed reactions are built. Durable shape lives in
> `context/lib/networking.md`. What remains: co-op respawn/player-leave policy and the playable
> capstone encounter.
> **Related:** `context/lib/networking.md` · `context/plans/done/E18--trigger-event-fanout` ·
> `E18--coop-activation-policy` · `E18--spawner-and-closet-containment` ·
> `E18--trap-pools-seeded-arming` · `E18--enemy-group-handle` · `E18--timed-reaction-steps` ·
> roadmap Epic 18.

---

## Shipped

Trigger detection and fan-out (`on_fire`/`on_exit` with paired gating, the effect-based
consequential/presentation/lifecycle dispatch split, bind-at-install), pressure-plate and button
authoring patterns, co-op activation policy (any/N-simultaneous/all), the `entity_spawner` entity
with spawn-time replication registration, closet containment (the dormant-until-armed aggro gate
that stops reveal-closet enemies aggroing through walls or pathing through closed doors before the
reveal), and seeded host-side trap-pool arming at level install are all built —
`context/plans/done/E18--trigger-event-fanout`, `E18--coop-activation-policy`,
`E18--spawner-and-closet-containment`, `E18--trap-pools-seeded-arming`,
`E18--enemy-group-handle`, `E18--timed-reaction-steps`. The durable shape — trigger volumes as
baked map data, the trap-pool arming's host-only load-time shape, the consequential/presentation
dispatch split, the general RNG posture (host-only, load-time, consequences-only) — lives in
`context/lib/networking.md`.

## Still open

| Gap | Severity | Notes |
|---|---|---|
| No co-op respawn / player-leave policy | High | Charter-named. `playerDied` fires a mod-bound game-flow verb; there is no respawn path. A lethal-trap capstone playtest hits this immediately. |

**Dead-pawn occupancy is undecided.** The trigger system iterates all player pawns with no alive
check, so a corpse on a plate holds it down. Undecided; gate in E18-R.

### Remaining spec sequence

| # | Spec (working name) | Contents | Depends on |
|---|---|---|---|
| E18-R | **Minimal co-op respawn policy** | Dead pawn respawns at a placement after a delay (netcode half shipped: M15 P3 respawn-as-teleport). Owns: `playerDied` one-shot latch re-arms on respawn (second death must fire again); dead-state gating of trigger/occupancy interaction (with B) | E15 P3 (shipped); coordinates with E18-B (shipped) |
| E18-E | **Playable co-op encounter (capstone)** | One authored level: plate/button puzzle → staged reveal → semi-random spawn-closets; co-op playtest incl. late join and player death; folds in trigger-state wire mirror if playtest demands | E18-A–D (shipped), E18-R |

### Cross-epic interactions for the capstone

- **E17-E (doors/blocking movers)** is a companion, not a prerequisite: closets work with
  displace-only movers. Pull it forward if playtests show door crush/blocking/interruption
  mattering to trap feel. A combined E17-E + E18-E playtest wave matches E17's own "later C + E
  wave" guidance.
- **E17-F (doors as occluders) is not a prerequisite for any of these specs — the dependency runs
  the other way.** The closed door renders as opaque geometry, so depth testing hides the closet
  interior visually even though the portal is baked open; the real first-playtest spoilers were
  the aggro/pathing gaps, now fixed by the closet containment gate (agent-vs-mover awareness more
  generally — steering ignoring movers outside this gate — is still a broader unowned gap). The
  E18-E capstone is the kind of concrete set-piece / profiled evidence E17-F says it is waiting
  for — E18 produces F's motivating consumer, not the reverse.
- **E17-B (kinematic visual parity)** raises closet-door presentation; independent, schedule on
  visual demand.
- **Epic 12 (spatial audio)** is the biggest jump-scare force multiplier but not a blocker; note
  that baked audio occlusion cannot model door state (movers are outside static bakes), one more
  reason spawn-flavor closets are the default.
- **Epic 15 Phase 4** (late-join at scale) and runtime-level-lifecycle × co-op remain out of
  scope; E18-E observes late-join behavior, it does not re-architect it.

## Risks and non-goals (resolved)

The reveal-closet aggro/pathing risk, the host-local-by-default posture for presentation
reactions, and the RNG/determinism discipline (arming stays strictly at install time and
host-side; never per-tick or client-side) are now built and documented in
`context/lib/networking.md`. The non-goals this doc named — no live-VM script callbacks, no
shared-seed client re-simulation, tags over Quake-style `targetname`/`target` pairing, no
line-of-sight triggers, no general logic-gate entity graphs, no mid-session pool re-rolls or
wave-director system — all held; none were revisited.

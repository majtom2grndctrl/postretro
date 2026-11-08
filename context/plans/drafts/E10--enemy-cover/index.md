# E10--enemy-cover

Brief · resumable · Epic 10 · reads: `context/lib/entity_model.md` §7, §7c · `context/lib/scripting.md` §1, §11, §12 · `context/lib/build_pipeline.md` §Navigation bake, §Build Cache, §Ray traversal, §Custom FGD, §PRL section IDs · `context/lib/networking.md` §Combat authority, §World-LOS-only validation · read at 7ee9389f

Depends on `drafts/E10--enemy-combat-movement`: `reposition` with `arc` and `minTravel`, the slot scorer's per-query constraints, directional clip resolution, and attack turns.

## Problem
The owner wants "smarter" enemies that take cover, crouch behind low cover, fire from it, and relocate when flanked. Today none of it is possible: the combat-slot scorer rejects any slot the target cannot see, the navmesh carries no cover data, and enemies have no posture. This is a requested capability. It overrides the prior exclusion of protective cover, which `plans/done/E10--enemy-line-of-sight-cover` deferred until player shots became rays; hitscan now resolves as a ray against the world and posed hit zones.

When this is done:
- An author marks an activity to move into full or half cover, crouch, peek and fire, and break out when flanked. A typical map needs no cover authoring.
- A designer adds or suppresses cover spots in TrenchBroom.
- Routes into cover, and arc repositions, prefer paths the target is less likely to see.

## Decisions
- **Cover is baked and authorable** (`build_pipeline.md` §Navigation bake reserves cover hints as a navmesh record kind).
  - prl-build derives cover points on the navmesh rim beside static obstructions. Each carries a position, a wall normal, and a height class, `full` or `half`.
  - They land in a new table of the navmesh section. The section version bumps; the previous version still decodes, with no cover and one warning, as v2 does today.
  - A sibling cache stage builds the table and folds the tracer epoch, so a navmesh-only change and a tracer change each rebake it.
  - `ai_cover` (`@PointClass`) adds a spot; its `height` is optional and measured at bake when omitted. `ai_cover_suppress` (`@SolidClass`) removes derived spots inside its brushes. Authored spots are never suppressed.
  - Static world only: a door or mover neither gives nor blocks cover. Mover-aware cover waits for `E17--movers-block-shots-and-sight`, whose combined sight check the host needs to confirm a spot behind a docked mover; the roadmap carries it as an Epic 17 follow-up.
- **Cover folds into `reposition`.** `reposition.cover: "full" | "half" | "any"` draws candidates from cover points instead of the ring; absent, the ring applies as today.
  - A candidate qualifies when it is hidden from the target's eye at the eye height its class implies, standing for `full` and crouched for `half`, and the target is within the activity's attack range from it.
  - `arc` and `minTravel` still filter, so "take cover lateral to the player" is one activity. With no qualifying point, the enemy holds, as the ring does.
  - A string, not a boolean: cover has kinds, and later destination kinds join the same property.
- **Posture is per activity: `posture: "stand" | "crouch"`**, the vocabulary player life states use (`scripting.md` §12). Omitted means stand.
  - Crouch lowers the eye, which perception and exposure read, and the damage box of an enemy without hit zones. An enemy with hit zones already crouches through its posed zones.
  - The collision capsule stays full height, so standing up never sticks.
  - Posture is authored, never inferred from the cover point's class: a sniper may stand behind a crate.
  - **Clips compose, in the player's naming** (`entity_model.md`: `crouch_idle`, `crouch_forward`). Under crouch, an animation `X` resolves to `crouch_X`, then takes the directional suffix (`crouch_walk_left`). A missing name falls back through `crouch_X`, then the standing chain to `X`. Meshes bind these state names to library clips such as Mixamo's `idle_crouching`.
  - **Posture replicates.** Enemy state carries a posture bit, so a client's copy of an enemy without hit zones lowers its hit volume too and half cover means the same against a remote shooter; the protocol version bumps. An enemy with hit zones already crouches on clients through its posed zones.
  - **A posture change skips the sight grace.** On the tick posture changes, perception reads sight from the new eye with no grace window carried over, so ducking back into cover never reads as flanked.
- **Exposure is the existing eye ray.** `brain.targetVisible` turning true while in cover means flanked; the author transitions out. No new brain fact.
- **Routes avoid likely sight.** The pathfinder gains a non-negative per-crossing cost, which keeps its heuristic admissible.
  - Cover and arc repositions add a penalty to a crossing whose cell is coupled with the target's cell in the baked cell-visibility relation (id 46). It is a table lookup, coarse by design: "could see", not "sees". Cells split only at occluding brushes, so the penalty steers between rooms, never around a crate inside one.
  - Chase, strafe and plain repositions keep today's costs. A level without id 46 pays no penalty.
- **Host-only, outside the compatibility digest.** Cover points and route costs are host AI state; posture reaches clients as animation state and the posture bit.
- **Shipped content adopts it.** `content/dev` gains a cover fixture, and `sdk/behaviors/reference` a cover pattern; the rifle squad gains crouching states bound from the Mixamo library.
- **Non-goals:**
  - **Exposure to players other than the target.** One target keeps exposure at one ray per enemy per tick; target selection already prefers the nearest visible player.
  - **Splash-proof cover.** Splash keeps its single occlusion ray to the nearest body point; low cover protects only when it blocks that ray.
  - **Engine anti-camping.** Pacing is taste, authored with `timeInActivityMs` and attack turns (`scripting.md` §1).
  - **Squad roles**, such as one suppressing while another flanks, beyond what attack-turn waiters already produce.
  - **A baked directional exposure table.** The runtime ray costs nothing measurable.
  - **Cover and peek clips.** Peeking is a posture change between authored activities.

### Scripting surface
```ts
// Proposed design
// rifle trooper behavior (excerpt): take half cover lateral to the target,
// crouch, stand to fire on a turn, break out when flanked
offense: {
  initial: "cover",
  activities: {
    cover:  { motion: "reposition", animation: "run", facing: "travel",
              reposition: { cover: "half", arc: [45, 135] } },
    settle: { motion: "hold", animation: "idle", posture: "crouch" },  // plays crouch_idle
    peek:   { motion: "hold", animation: "idle_aiming",
              action: { attack: "shoot" }, attackTurn: true },
  },
  transitions: {
    cover:  [{ to: "settle", when: brain.timeInActivityMs.ge(600) }],
    settle: [{ to: "peek",  when: brain.attackTurnFree },
             { to: "cover", when: brain.targetVisible }],               // flanked
    peek:   [{ to: "settle", when: brain.attacksFiredInActivity.ge(3) }],
  },
}
```
```
// TrenchBroom
{ "classname" "ai_cover" "origin" "64 0 16" "height" "half" }
{ "classname" "ai_cover_suppress" /* brushes */ }
```
The Luau twin mirrors it with Luau call syntax.

## Acceptance
### Automated
**Bake**
- [ ] On a fixture with a wall and a crate, the bake derives `full` points beside the wall and `half` points beside the crate, normals facing away from each. Open floor gets none.
- [ ] Two bakes of one map produce byte-identical cover tables. A derived point exactly on an `ai_cover_suppress` plane is removed, identically across bakes (pin P12).
- [ ] A map with no cover round-trips an empty table, and `cover: "any"` holds.
- [ ] `ai_cover` adds a spot with its authored class, or the measured class when `height` is omitted. `ai_cover_suppress` removes derived spots inside it and keeps an authored spot inside it.
- [ ] A navmesh of the previous version decodes with no cover and one warning, and its level still paths.
- [ ] A tracer epoch change rebakes cover; a light-only edit does not.
- [ ] A docked mover beside a candidate neither adds nor removes cover.
- [ ] Two peers whose maps differ only in cover authoring pass the compatibility check.

**Choosing cover**
- [ ] `cover: "full"` ends at a point hidden from the target's eye at standing eye height; `"half"` at one hidden at crouched eye height and visible standing; `"any"` at either.
- [ ] A cover point out of the attack's range from the target is never chosen.
- [ ] `arc` and `minTravel` filter cover candidates, with a point just inside and just outside each bound.
- [ ] With no qualifying point, the enemy holds and does not oscillate. Without `cover`, `reposition` uses the ring exactly as the combat-movement brief specifies.
- [ ] Two enemies seeking cover on one tick never claim one point (pin P1).
- [ ] The target walking around the cover flips `targetVisible` to 1 on the first tick its eye ray clears, and the authored transition fires.
- [ ] The target standing on a cover point never has it claimed (pin P10).
- [ ] A level change or hot reload that alters cover drops held cover claims; the next entry re-picks from the new table (pin P11).

**Posture**
- [ ] Crouch lowers the eye and an enemy-without-zones' damage box, never the capsule. Standing up beside a low ceiling does not stick.
- [ ] A hitscan ray at standing chest height over half cover misses a crouched enemy without hit zones and hits it standing. The same holds through posed zones on an enemy with them.
- [ ] `walk` under crouch moving left resolves to `crouch_walk_left`, falling back to `crouch_walk`, then `walk_left`, then `walk`, when names are missing (pin P2).
- [ ] Posture changes on the activity-entry tick, and perception that tick reads the new eye (pin P3).
- [ ] Crouching back into cover after a peek leaves `targetVisible` at 0 on the posture-change tick when the crouched eye is hidden; a crouched eye still in sight reads 1 (pin P4).
- [ ] Co-op loopback: a remote shot at standing chest height over half cover misses a crouched enemy without hit zones, and hits it standing.
- [ ] A `stand` activity at a `half` point stays standing.
- [ ] A peer from before the protocol bump is refused at handshake.

**Routes**
- [ ] With two equal-length routes, a cover reposition takes the one whose cells are not coupled with the target's cell; a chase takes either as today. Without id 46, both cost as today.
- [ ] The penalty never makes a reachable cover point unreachable.

**Surface and scale**
- [ ] In both runtimes, an unknown `cover` or `posture` value, or `cover` on an activity whose motion is not `reposition`, rejects with a pathed error.
- [ ] The Scripting surface example parses identically in TypeScript and Luau and runs as `content/dev` content.
- [ ] A wave-scale stress row with every agent seeking cover stays within the perf brief's AI budget, against its engaged baseline.

### Manual
- [ ] On the cover fixture, the rifle squad takes cover, crouches behind crates, peeks and fires on turns, and breaks cover when flanked.
- [ ] Co-op loopback: a client sees the same crouches and peeks.

## Path
- **Seams to read first.**
  - `bake_navmesh_with_movers` and `erode` (`crates/level-compiler/src/navmesh_bake.rs`): erosion drops its boundary cells, so either keep them or search outward from the eroded rim. Wall normals need the triangle list or a bake ray.
  - The bake BVH from `bvh_build::build_bvh` in the compiler pipeline is in scope before the NavMesh stage but not passed in. Bake rays go through `ray_traversal::BoundedRay`.
  - `NavMeshSection` (`crates/level-format/src/navmesh.rs`): the v2 decode precedent; sorted, deduplicated tables.
  - `score_candidate` and the slot batch (`crates/sim/src/combat_positioning/scoring.rs`): cover is a candidate source and a hidden-from-target filter; the ring's visible-from filter stays for the ring.
  - `perception::enemy_eye` and `HealthComponent.hitbox`: the posture seams.
  - `astar_corridor` (`crates/sim/src/nav/path.rs`): the cost sites a hook touches.
  - The cell-visibility relation (id 46) and the cell locator: map a crossing to its cell.
- **Shape.** Cover points as a navmesh table read into `NavGraph`, queried by region near the enemy. Rival: a separate PRL section, which skips the version bump but splits one query surface in two.
- **First slice.** The bake table and its decode, proven on the fixture. It falsifies the riskiest assumption: that the eroded rim and a few bake rays find usable spots and height classes.
- **Watch for.** `drafts/perf-ai-tick-cost` touches the same batch; route penalties apply per replan, and replans are budgeted.
- **Rival to baking, rejected:** runtime candidates from region rims tested by rays each search. AI already costs 5.8 ms against a 1.0 ms target, and baking matches the reserved record kind.
- **Order.** The combat-movement brief lands first; its `reposition`, scorer constraints and clip resolution are this brief's base.

## Open questions
- Cover point spacing and the rim search distance — **delegated**: report them.
- The half and full height thresholds — **delegated**: seed from player crouch and stand heights, and report.
- The route penalty weight and the candidate radius around the enemy — **delegated**: tune on the cover fixture and report.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| cover destination | `RepositionParams::cover: Option<CoverKind>` | `"cover"`: `"full"` \| `"half"` \| `"any"` | `reposition.cover?` | `reposition.cover` | n/a |
| posture | `BehaviorActivityDescriptor::posture: Option<Posture>` | `"posture"`: `"stand"` \| `"crouch"` | `posture?` | `posture` | n/a |
| cover hint | compiler-consumed entity | navmesh cover table | — | — | `ai_cover` (`@PointClass`): `origin`, `height` (`full`/`half`, optional) |
| cover suppression | compiler-consumed entity | none | — | — | `ai_cover_suppress` (`@SolidClass`) |
| crouch clips | host resolution | state names `crouch_X`, `crouch_X_left`, … | — | — | n/a |
| replicated posture | enemy replicated state bit | one bit in the enemy state record; protocol version bumps | — | — | n/a |

## Wire format
New table in the navmesh section (id 36), appended after mover links; mirrors the existing tables. Little-endian. A `u32` entry count, then per entry: position (`f32` × 3), wall normal (`f32` × 3, unit, horizontal), height class (`u8`: 0 full, 1 half), source (`u8`: 0 derived, 1 authored), region index (`u32`). Empty list: count 0. Entries sorted by region index, then position. The section version bumps by one; the previous version decodes with an empty table.

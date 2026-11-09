# E10--enemy-cover

Brief · resumable · Epic 10 · reads: `context/lib/entity_model.md` §7, §7c · `context/lib/scripting.md` §1, §11, §12 · `context/lib/build_pipeline.md` §Navigation bake, §Build Cache, §Ray traversal, §Custom FGD, §PRL section IDs · `context/lib/networking.md` §Combat authority, §World-LOS-only validation, §Version gates · read at 7ee9389f

Depends on `drafts/E10--enemy-combat-movement`: `reposition` with `arc` and `minTravel`, the slot scorer's per-query constraints, directional clip resolution, and attack turns.

## Problem
The owner wants "smarter" enemies that take cover, crouch behind low cover, fire from it, and relocate when flanked. Today none of it is possible: the combat-slot scorer rejects any slot the target cannot see, the navmesh carries no cover data, and enemies cannot crouch. This is a requested capability. It overrides the prior exclusion of protective cover, which `plans/done/E10--enemy-line-of-sight-cover` deferred until player shots became rays; hitscan now resolves as a ray against the world and posed hit zones.

When this is done:
- An author marks an activity to move into full or half cover, crouch on arrival, peek and fire, and break out when flanked. A typical map needs no cover authoring.
- A designer adds or suppresses cover spots in TrenchBroom.
- Routes into cover, and arc repositions, swing wide of the target's surroundings.

## Decisions
- **Cover is baked and authorable** (`build_pipeline.md` §Navigation bake reserves cover hints as a navmesh record kind).
  - prl-build derives cover points on the navmesh rim beside static obstructions. Each carries a position, a wall normal, and a height class, `full` or `half`.
  - They land in a new table of the navmesh section, whose version bumps. The previous version decodes with no cover and one warning; older versions no longer decode and need a rebake.
  - A navmesh input, a tracer change, or a cover-authoring edit rebakes the section; a light-only edit does not.
  - `ai_cover` (`@PointClass`) adds a spot; its `height` is optional and measured at bake when omitted. `ai_cover_suppress` (`@SolidClass`) removes derived spots inside its brushes. Authored spots are never suppressed.
  - An `ai_cover` off the navmesh, or with no wall to take a normal from, warns at compile and is dropped. An invalid `height` is a compile error naming the entity.
  - Static world only: a door or mover neither gives nor blocks cover. Mover-aware cover waits for `E17--movers-block-shots-and-sight`, whose combined sight check the host needs to confirm a spot behind a docked mover; the roadmap carries it as an Epic 17 follow-up.
- **Cover folds into `reposition`.** `reposition.cover: "full" | "half" | "any"` draws candidates from cover points instead of the ring; absent, the ring applies as today. A string, not a boolean: cover has kinds, and later destination kinds join the same property.
  - A `full` point qualifies when the target's eye cannot see the standing eye there. A `half` point qualifies when it cannot see the crouched eye but can see the standing eye, so a peek works.
  - The target must be within attack range from the point: the activity's attack, or the archetype's longest declared attack range when the activity has none.
  - `arc` and `minTravel` still filter. Agent spacing and docked-mover overlap still apply; the ring's direct-path filter does not, since it rejects most spots behind walls.
  - Qualifying points rank by path cost, nearest first. With none, the enemy holds, as the ring does.
- **Body stance per activity: `stance: "stand" | "crouch"`.** Crouching is behavior, not cosmetics: hiding behind cover, or a melee enemy coiling for a sweep. The `stance()` builder from `drafts/E10--enemy-stances-and-telegraphs` shares the word on purpose. Player life states call the same idea `posture` (`scripting.md` §12); enemies diverge to keep one word for the pose an enemy takes.
  - It resolves like `motion`: the innermost activity that declares it wins, and stand applies when none does.
  - **`crouchHeight`**, authored per archetype in metres, sets the crouched top of the damage box and the crouched eye. An archetype that crouches without it fails load, naming it. An enemy with hit zones also crouches through its posed zones.
  - The collision capsule stays full height, so standing up never sticks.
  - Stance is authored, never inferred from the cover point's class: a sniper may stand behind a crate.
  - **Clips compose, under the player's `crouch_` prefix** (`entity_model.md`: `crouch_idle`). Under crouch, an animation `X` resolves to `crouch_X`, then takes the directional suffix (`crouch_walk_left`). A missing name falls back through `crouch_X`, then the standing chain to `X`. Meshes bind these state names to library clips such as Mixamo's `idle_crouching`. An archetype that crouches on a mesh without `crouch_idle` fails load, as a player's forced crouch without crouch support does.
  - **Stance replicates** as its own component kind, so every enemy's client copy, meshed or not, lowers its hit volume and half cover means the same against a remote shooter. The wire and snapshot versions bump.
  - **A stance change clears the sight grace.** The next perception, the first tick the new activity's rows evaluate, reads the new eye with no grace carried over, so ducking back into cover never reads as flanked.
- **Exposure is the existing eye ray.** `brain.targetVisible` turning true while in cover means flanked; the author transitions out.
- **`@brain.arrived`** reads 1 once the activity's motion goal is reached and 0 while moving or with no goal. It is scope-relative and serves any move, so an author crouches on arrival, not mid-route.
- **Routes swing wide of the target.** Cover and arc repositions add a penalty to a crossing whose cell is near the target's cell in the baked cell-visibility relation (id 46): a graded coupled pair, which is portal-graph proximity, not sight. The penalty only raises cost and never removes a route. Chase, strafe and plain repositions keep today's costs, and a level without id 46 pays none. Sight-accurate routes belong with mover-aware cover.
- **Host-only, outside the compatibility digest.** Cover points and route costs are host AI state; stance reaches clients as animation state and its component.
- **Shipped content adopts it.** `content/dev` gains a cover fixture, and `sdk/behaviors/reference` a cover pattern; the rifle squad gains `crouch_` states bound from the Mixamo library and a `crouchHeight`.
- **Non-goals:**
  - **Exposure to players other than the target.** One target keeps exposure at one ray per enemy per tick; target selection already prefers the nearest visible player.
  - **Splash-proof cover.** Splash keeps its single occlusion ray to the nearest body point; low cover protects only when it blocks that ray.
  - **Engine anti-camping.** Pacing is taste, authored with `timeInActivityMs` and attack turns (`scripting.md` §1).
  - **Squad roles**, such as one suppressing while another flanks, beyond what attack-turn waiters already produce.
  - **A baked directional exposure table.** The runtime ray costs nothing measurable.
  - **Cover and peek clips.** Peeking is a stance change between authored activities.

### Scripting surface
```ts
// Proposed design
// rifle trooper behavior (excerpt): take half cover lateral to its approach,
// crouch on arrival, stand to fire on a turn, break out when flanked
behavior: {
  crouchHeight: 1.1,
  activities: { engage: { layers: { offense: {
    initial: "cover",
    activities: {
      cover:  { motion: "reposition", animation: "run", facing: "travel",
                reposition: { cover: "half", arc: [45, 135] } },
      settle: { motion: "hold", animation: "idle", stance: "crouch" },  // plays crouch_idle
      peek:   { motion: "hold", animation: "idle_aiming",
                action: { attack: "shoot" }, attackTurn: true },
    },
    transitions: {
      cover:  [{ to: "settle", when: brain.arrived }],
      settle: [{ to: "cover", when: brain.targetVisible },               // flanked first
               { to: "peek",  when: brain.attackTurnFree }],
      peek:   [{ to: "settle", when: brain.attacksFiredInActivity.ge(1)
                                     .or(brain.timeInActivityMs.ge(1500)) }],
    },
  } } } },
}
```
```
// TrenchBroom
{ "classname" "ai_cover" "origin" "64 0 16" "height" "half" }
{ "classname" "ai_cover_suppress" /* brushes */ }
```
The Luau twin mirrors it with Luau call syntax (`brain.attacksFiredInActivity:ge(1):or(...)`).

## Acceptance
### Automated
**Bake**
- [ ] On a fixture with a wall and a crate, the bake derives `full` points beside the wall and `half` points beside the crate, normals facing away from each. Open floor and a ledge's drop get none.
- [ ] Two bakes of one map produce byte-identical cover tables. A derived point exactly on an `ai_cover_suppress` plane is removed, identically across bakes (pin P12).
- [ ] A map with no cover round-trips an empty table, and `cover: "any"` holds.
- [ ] `ai_cover` adds a spot with its authored class, or the measured class when `height` is omitted. `ai_cover_suppress` removes derived spots inside it and keeps an authored spot inside it. A derived and an authored spot at one position leave one entry.
- [ ] An `ai_cover` off the navmesh, or with no wall in reach, warns and is dropped. An invalid `height` fails the compile naming the entity.
- [ ] A navmesh of the previous version decodes with no cover and one warning, and its level still paths. An older version is refused with a rebake message.
- [ ] A tracer change and a cover-authoring edit rebake cover; a light-only edit does not.
- [ ] A docked mover beside a candidate neither adds nor removes cover.
- [ ] Two peers whose maps differ only in cover authoring pass the compatibility check.

**Choosing cover**
- [ ] `cover: "full"` ends at a point the target's eye cannot see standing; `"half"` at one hidden crouched and visible standing; `"any"` at either.
- [ ] A cover point out of attack range from the target is never chosen, measured by the activity's attack or, with none, the longest declared range.
- [ ] A spot behind a wall whose walk to the target is long qualifies; one overlapping another agent or a docked mover does not.
- [ ] Among qualifying points, the one with the lower path cost wins.
- [ ] `arc` and `minTravel` filter cover candidates, with a point inside and outside each bound.
- [ ] With no qualifying point, the enemy holds and does not oscillate. Without `cover`, `reposition` uses the ring exactly as the combat-movement brief specifies.
- [ ] Two enemies seeking cover on one tick never claim one point (pin P1).
- [ ] The target walking around the cover flips `targetVisible` to 1 on the first tick its eye ray clears, and the authored transition fires.
- [ ] The target standing on a cover point never has it claimed (pin P10).
- [ ] A level change or hot reload that alters cover drops held cover claims; the next entry re-picks from the new table (pin P11).
- [ ] `arrived` reads 0 en route and 1 from the tick the goal is reached; it reads 0 under a motion with no goal.

**Stance**
- [ ] Crouch lowers the eye and an enemy-without-zones' damage box to `crouchHeight`, never the capsule. Standing up beside a low ceiling does not stick.
- [ ] A hitscan ray at standing chest height over half cover misses a crouched enemy without hit zones and hits it standing. The same holds through posed zones on an enemy with them.
- [ ] An outer activity's crouch carries into an inner activity that declares no stance; an inner `stand` overrides it.
- [ ] `walk` under crouch moving left resolves to `crouch_walk_left`, falling back to `crouch_walk`, then `walk_left`, then `walk`, when names are missing (pin P2).
- [ ] An archetype that crouches without `crouchHeight`, or on a mesh without `crouch_idle`, fails load naming it.
- [ ] A stance change follows pins P3 to P8: the box changes on the entry tick, perception reads the new eye the next tick with no grace, and ducking after a peek never reads as flanked when the crouched eye is hidden.
- [ ] Co-op loopback: a remote shot at standing chest height over half cover misses a crouched enemy without hit zones, and hits it standing. A late joiner sees the crouched volume (pin P9).
- [ ] A `stand` activity at a `half` point stays standing.
- [ ] A peer from before the version bump is refused at handshake.

**Routes**
- [ ] With two equal-length routes, a cover reposition takes the one whose cells are not near the target's cell in id 46; a chase takes either as today. Without id 46, both cost as today.
- [ ] The penalty never makes a reachable cover point unreachable.

**Surface and scale**
- [ ] In both runtimes, an unknown `cover` or `stance` value, a non-positive or non-finite `crouchHeight`, or `cover` on an activity whose motion is not `reposition`, rejects with a pathed error.
- [ ] The Scripting surface example parses identically in TypeScript and Luau and runs as `content/dev` content.
- [ ] A wave-scale stress row with every agent seeking cover stays within the perf brief's AI budget, against its engaged baseline.

### Manual
- [ ] On the cover fixture, the rifle squad takes cover, crouches behind crates, peeks and fires on turns, and breaks cover when flanked.
- [ ] Co-op loopback: a client sees the same crouches and peeks.

## Path
- **Seams.** `research.md` lists them by symbol: the bake and erosion, the bake BVH, `NavMeshSection`, the slot scorer, `enemy_eye` and the hitbox, `astar_corridor`, and id 46 with the cell locator.
- **Mechanisms.** The cover table folds into the navmesh stage key with the tracer epoch and the cover entities. The route penalty is a non-negative per-crossing cost, which keeps A*'s heuristic admissible. Perception runs before transitions, so a stance change clears grace and the next tick reads the new eye.
- **Shape.** Cover points as a navmesh table read into `NavGraph`, queried by region near the enemy. Rival: a separate PRL section, which skips the version bump but splits one query surface in two. Rival to baking: runtime candidates from region rims tested by rays each search, rejected because AI already costs 5.8 ms against a 1.0 ms target.
- **First slice.** The bake table and its decode, proven on the fixture. It falsifies the riskiest assumption: that the eroded rim and a few bake rays find usable spots and height classes, and tell a wall from a ledge.
- **Watch for.** `drafts/perf-ai-tick-cost` touches the same batch; route penalties apply per replan, and replans are budgeted.

## Open questions
- Cover point spacing and the rim search distance — **delegated**: report them.
- The half and full height thresholds — **delegated**: seed from player crouch and stand heights, and report.
- The route penalty weight and the candidate radius around the enemy — **delegated**: tune on the cover fixture and report.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| cover destination | `RepositionParams::cover: Option<CoverKind>` | `"cover"`: `"full"` \| `"half"` \| `"any"` | `reposition.cover?` | `reposition.cover` | n/a |
| body stance | `BehaviorActivityDescriptor::stance: Option<BodyStance>` | `"stance"`: `"stand"` \| `"crouch"` | `stance?` | `stance` | n/a |
| crouch height | `BehaviorGraphDescriptor::crouch_height: Option<f32>` | `"crouchHeight"` | `crouchHeight?` | `crouchHeight` | n/a |
| arrival fact | `BRAIN_INPUTS` tail | `"@brain.arrived"` | `brain.arrived` | `brain.arrived` | n/a |
| cover hint | compiler-consumed entity | navmesh cover table | — | — | `ai_cover` (`@PointClass`): `origin`, `height` (`full`/`half`, optional) |
| cover suppression | compiler-consumed entity | none | — | — | `ai_cover_suppress` (`@SolidClass`) |
| crouch clips | host resolution | state names `crouch_X`, `crouch_X_left`, … | — | — | n/a |
| replicated stance | new replicated component kind | next component discriminant; `stand` / `crouch`; wire and snapshot versions bump | — | — | n/a |

## Wire format
New table in the navmesh section (id 36), appended after mover links; mirrors the existing tables. Little-endian. A `u32` entry count, then per entry: position (`f32` × 3), wall normal (`f32` × 3, unit, horizontal), height class (`u8`: 0 full, 1 half), source (`u8`: 0 derived, 1 authored), region index (`u32`). Empty list: count 0. Entries sorted by region index, position by `to_bits`, normal by `to_bits`, class, then source, and unique. The section version bumps by one; the previous version decodes with an empty table.

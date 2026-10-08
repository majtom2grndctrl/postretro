# sdk-addressing-model — research

Facts read at 649702b6f. No relevant path differs at 2bc52d471. These findings inform the brief but do not decide it.

## Addressing forms today
| Form | Wire | Resolved | Allowed in `sequence` |
|---|---|---|---|
| Snapshot handle method (mover, trigger, light, fog) | `{id, primitive, args}` | Id baked when `setupLevel` runs, checked at dispatch (`dispatch_sequence` → `registry.exists`; `BoundTarget::resolve`). A stale id warn-skips, generation-checked. | yes |
| Tag primitive (`damage`, `grantHealth`, `grantAmmo`, `addSlot`; raw fog, emitter and animation descriptors) | `{primitive, tag, args}` | Each fire, by `dispatch_primitive` → `query_by_component_and_tag(Transform, tag)`, or in-tick through `BoundTarget::Tag` | no |
| Group handles `enemies({tag}).update`, `spawner({tag}).fire` (TS and Luau) | the same tag primitive; the spawner primitive resolves over the spawner component only | the same | no; reached through `fire(reaction)` |
| Subject tokens `@activators`, `@trigger` | `target` or a sentinel `id` | From the trigger fire context; warn-skipped on name dispatch | yes |
| Impact tokens `impact.target`, `impact.source` | impact effects, spelled as methods (`impact.source.grantAmmo(…)`) | Impact dispatch only | n/a |
| `on.emitter` | `args.at` | Named-event fire, legal only as `playSound` `at` | parseable |
| `progress: { tag, at, fire }` | tag-keyed kill counter | Kill credit by tag | n/a |

- `SequenceTarget` is `Entity | Activators | FiredTrigger | Wait | Fire` and has no tag arm.
- A reaction body is exactly one of progress, primitive or sequence. There is no array body.
- Tag resolution is a full scan of registry slots in slot order. There is no tag index.
- Trigger-event binding (`bind_event`, `trigger_bindings.rs`) resolves its tag to volume ids once, at install.
- `damagedEnemies({…})` (`sdk/lib/ui/presentation.ts`) is a presentation overlay source evaluated per damaged NPC. It is not a reaction source, and no reaction source evaluates a condition per non-player entity.

## Map entities and changing sets
| Kind | Placed by | Members change after install? |
|---|---|---|
| mover, light, fog, emitter, trigger, spawner | map: trigger-volume records via `TriggerVolumeBridge`; `entity_spawner` via its built-in classname handler | no |
| npc | map and spawners. Spawned NPCs are untagged today (`spawner.rs`, test `repeated_fire_is_stateless_and_spawns_untagged_runtime_enemies`) so their kills cannot decrement a tag-keyed `progress` total. That total is captured at load, and the tracker fires at `killed / total ≥ at`. | yes |
| player | join and leave | yes |

- `getMapEntities` tells map-placed from spawned through `DescriptorProvenance.spawn_path`; nothing filters on it today. Provenance is written only for descriptors with a `canonicalName`, so the predicate to pin is "map placement or no provenance". It excludes runtime spawns, player-spawn and network-slot paths.
- For a fixed kind, resolving at install and resolving at each use give the same set. A group only behaves differently when its kind's membership changes. That is why map kinds get members only, and groups exist only for NPCs and players.
- Every content `world.query` targets a map kind. Introspection beyond addressing happens only in arena-lights (sorting by position and staggering per light) and trigger-fanout-fixture (reading ids to build names).
- `collect_membership` reads only id-baked `setLightAnimation` steps. A tag-targeted light animation is invisible to the build.

## Machines
- A connected client installs map kinds locally from the `.prl`.
- `filter_out_client_host_replicated_placements` drops placements carrying a brain or a touchable. Those arrive by replication through `ClientReplication::apply_snapshot`, carrying a transform and presentation only, with no tags, brain or health.
- Host and client `EntityId`s are never compared. Movers bind by `mover_id` (`find_loaded_mover_entity`). `NetworkId` appears in no scripting crate.
- Gates:
  - `dispatch_primitive` gates only `addSlot`, under `owner_slot_writes_enabled`.
  - The spawner primitive is gated by `SpawnContext` runtime-spawn authority.
  - `updateEnemyState` filters to brain-bearing targets.
  - The scheduler is disabled on clients, so a `wait` tail never runs there.
- A client `addSlot` returns silently by contract, and a test pins it (`dispatch_primitive`). Group commands on a client follow that pattern.
- A disconnect hold unbinds the seat, and a pawn that outlives the hold resolves to nothing (`seat.rs`).
- Not verified: that each machine runs `setupLevel` itself (inferred from crossings and `levelLoad` running on clients).

## Mod-global definitions
- Each map has one data script, named by worldspawn's `data_script` key. The build evaluates only that script for light membership (`build_pipeline.md` §Script-derived light membership).
- Mod-global definitions are declared at mod init, before any level exists, and are composed into each level whose catalog tags match.
- Tag-keyed mod-global use today:
  - `factionSentimentTriggerEvents` (`content/dev/scripts/faction-sentiment.luau` → `ModManifest.triggerEvents`);
  - `defineTriggerPool` (`levels: ["trap-pools"]`);
  - `defineImpactEvent` policies.
  `start-script.ts` relies on these deliberately: "unique target tags make these mod-global policies work for both catalog and direct CLI map loads".

## Precedents
Recalled from engine documentation, not fetched; the Fyrox and Bevy specifics in particular were not checked against docs.
- Snapshot behind `get`/`find`: Godot `get_nodes_in_group`, Roblox `CollectionService:GetTagged`, Unity `FindGameObjectsWithTag`, Unreal `GetAllActorsWithTag`, Fyrox `find_by_name`.
- Live addressing:
  - Names resolved when used: Godot `call_group`, Source I/O targetnames, Unity `BroadcastMessage`.
  - Held query values: Unreal `FGameplayTagQuery` and `FMassEntityQuery`, Bevy `Query`, Flecs cached queries.
- Subject on events: Source `!activator`, `!caller`, `!player`; Bevy observer targets; jQuery delegated `.on`, where `this` is the subject; Flecs observers.
- Cautionary case: the DOM. `querySelectorAll` returns a static list while `getElementsByClassName` returns a live one, under similar verbs. The type has to carry liveness, not only the verb.
- Hostility as a relation, not a kind: Borderlands 2's mission where the quest giver asks the player to shoot them.

## Rivals
- **`select(kind, filter)` as one live verb.** Rejected. It clashes with Luau's builtin `select` (the SDK calls `select("#", ...)` in `data_script.luau`, and `DATA_SCRIPT_FIELDS` lifts builders to bare globals). It also clashes with the IR's `runtime.select` and with the web, where `querySelectorAll` and `d3.select` return snapshots. And its meaning drifted by kind.
- **A `mapEntities(kind)` group beside `getMapEntities`.** Rejected. Map kinds have fixed membership, so the group adds nothing for them.
- **A kindless `tagged(tag)` group.** Rejected as duplicating the `{ tag }` filter every constructor already takes. It would only have served groups that mix kinds, and nothing needs one.
- **Free target-taking verbs (`damage(target, n)`).** Rejected for methods on targets. The method form was already the majority: handle methods, group methods and `impact.source.grantAmmo`.
- **Keep `world.query` and document it as a snapshot.** Rejected. The name says nothing, and `world` mixed the snapshot with imperative gravity calls.
- **The kind name `enemy`.** Rejected; hostility is relational (faction sentiment, `scripting.md` §5, and the brain scope's hostile fact, §11). `characters` and `actors` include players, `agents` clashes with the nav agent component, and `creatures` is wrong for human NPCs.
- **Retire mod-global trigger events.** Rejected for `defineTriggerEvent`. Retiring them loses automatic application across levels.
- **Mod-attached level scripts (R2).** Deferred; see §Doors.
- **SDK-side desugar of group steps into fired reactions.** See the brief's Path.

## Consumers
Migrate all in one pass.
- **`world.query` / `world:query`:**
  - `content/dev/scripts`: arena-lights, switch-demo, closet-reveal, coop-two-button-puzzles, spawner-test, fog-pulse-demo, stress-warren, script-light-membership-fixture, a11y-strobe-test, trigger-fanout-fixture (`.ts` and `.luau`).
  - `content/dev/maps/*.generated.ts` and `tools/gen_stress_map.py`.
  - SDK: `sdk/lib/world.{ts,luau}`, the `sdk/lib/entities/*` wrappers, the typedef templates `sdk_lib.d.ts` / `sdk_lib.luau`, `typedef/mod.rs`, and the Luau prelude in `luau_prelude.rs` / `luau.rs`.
  - Tests: `light_membership.rs` tests and stubs, `dep_json_cli.rs`, `entity_world_primitives.rs` tests.
  - Docs: `docs/scripting-reference.md` (`## world.query`, gravity); `context/lib/scripting.md` §4, §10.1, §10.6, §10.7, §12; `entity_model.md`; `movement.md`; `postretro.fgd` comments.
- **`enemies(` / `spawner(` / `updateEnemyState`:**
  - Content: closet-reveal, spawner-test, trap-pools, the generated maps, `gen_stress_map.py`.
  - SDK and tests: `sdk/type-tests/e18-enemy-group.ts`, `parity_tests.rs`, `data_script.luau`, `sdk/lib/entities/{enemies,spawners}.ts`.
  - Engine: the `updateEnemyState` handler and its tests.
- **Free verbs and plain-string targets** (`damage`, `grantHealth`, `grantAmmo`, `addSlot`, `armTrigger`, `disarmTrigger`):
  - `parity_tests.rs` uses `grantHealth("players", …)`, `grantAmmo("players", …)` and `addSlot("players", …)`, which become `players()`.
  - Grep the rest: `(damage|grantHealth|grantAmmo|addSlot|armTrigger|disarmTrigger)\(`.
- **Hand-written descriptors:**
  - Migrate: `applyDamage` on tag `player` (combat-demo-reaction, reference content).
  - Stay raw:
    - `setAnimationState` (anim-demo-reaction) and `setFogScatter` (fog-pulse-demo, arena-lights), which have no typed builder;
    - `armTrigger`/`disarmTrigger` by tag (trigger-fanout-fixture), a fixture that deliberately covers both the raw tag path and the id path.
- **Engine sites this touches beyond the Path:**
  - `reaction_validation.rs` (V2, V3 and V4a over trigger events and sequence steps);
  - the progress tracker (install-time membership);
  - `spawner.rs` (the synthetic map entity's tags);
  - `trigger_commands.rs` (`Spawn` and the group targets).
- **`onTriggerEvent`:**
  - Level scripts use the member `on`.
  - `faction-sentiment.luau` moves to `defineTriggerEvent`.
  - Also content, sdk, crates tests and `context/lib/scripting.md` §12.
- **Generated:** `sdk/types/postretro.d.ts` and `postretro.d.luau`.

## Doors
- **Mod-attached level scripts.** `ModManifest` would list scripts with a `levels` selector, each running `setupLevel` in every matching level's data context and composed with the map's own. Inside them, `getMapEntities` works, so mod-global triggers, pools and impact policies could use members. The cost:
  - a new lifecycle: several data scripts per level, a composition order, hot-reload dependencies, both runtimes;
  - the build sees only the map's script, so attached scripts would have to be barred from animating static lights, or the compiler would have to learn about mod manifests.
- **Mod-global commands over map entities.** `getMapEntities` errors outside a level, so a mod-global reaction cannot command a mover, spawner, fog volume or trigger. No content needs it today. Cheapest form: a manifest-only tag-keyed command mirroring `defineTriggerEvent`, excluding lights, which the build pass cannot see.
- **`.on` over NPCs** needs:
  - a host-only per-entity predicate source over `EntityScope`;
  - an `on.npc` subject token;
  - a per-tick cost bound, because tag resolution is a full scan.
  Enemy death emits no event today.
- **A spawned-by relationship.** Today a spawned NPC's provenance records only that it was runtime-spawned (`DescriptorProvenance.spawn_path`), not which spawner made it. A sparse spawned-by index, like the pawn-to-seat map, would allow `npcs({ spawnedBy: s })` when two spawners share a tag but only one's output is wanted. The precedent is Bevy-style relationship data (`ChildOf` and `Children`).
- **A hostility filter on `npcs`** over faction sentiment, relative to a player.
- **`E22--kinematic-assemblies` spec 4** (runtime-addressable assemblies): members if assemblies are map-placed, a group if they appear at runtime.
- **The crossings redesign** owns `onStateCrossing`, `defineImpactEvent`, `defineTriggerPool` and `progress` under the source rule.
- **Client `wait` warning.** On a client, the scheduler's `enroll` (`reaction_scheduler.rs`) warns once per reaction for any `wait`. Every machine runs the same reactions, so this is co-op noise from valid content, the same reasoning that keeps group commands silent. It predates this brief; a follow-up should demote it to debug.
- **Vocabulary sweep:** `damagedEnemies`, enemy overlays and the `networking.md` "enemy" wording.

## Ordering pins
| Id | Scenario | Ordering | Expected outcome |
|---|---|---|---|
| A1 | A group step in a trigger's tick keeps its kind | `[npcs({ tag: "x" }).damage(5)]` Enter-bound to `t`; an NPC and a player pawn both carry `x`; P enters `t` on tick k | The NPC takes 5 on tick k and the pawn takes none. A kindless tag binding would hit the pawn. |
| A2 | A tagless group in a trigger's tick | `players().grantHealth(10)` as a primitive body and `npcs().update({ aggro: true })` as a step before any wait, both Enter-bound; P enters on tick k | Both bind at install and apply on tick k. Neither logs "has no target tag; not binding". |
| A3 | A spawner member in a trigger's tick | `[s.fire()]` Enter-bound; sibling `s2` carries the same tag; P enters on tick k | `s` spawns its count on tick k and `s2` spawns nothing. No "requires a fire-time tag target" warning. |
| A4 | Spawn, then address the spawned | `[s.fire(), npcs().update({ aggro: true })]`; name-fired, Enter-bound before any wait, and after a `wait` | Every NPC `s` just spawned is aggroed in the same drain or tick as its spawn, on all three paths. |
| A5 | One spawner member fired twice | `[s.fire(), s.fire()]` in one drain | Two batches, 2 × count NPCs. Steps are not deduplicated. |
| A6 | Membership changes during a wait | `[wait(800), players().grantHealth(10)]` and `[wait(800), npcs().damage(5)]`; during the wait one player joins, one player's seat is released, one NPC spawns and one despawns; landing drain | Recipients are whoever exists at landing: the joiner and the new NPC, never the released player or the despawned NPC. |
| A7 | A subject token after a wait | `[wait(800), on.activators.grantHealth(5)]` beside `[wait(800), players().grantHealth(5)]`; install | The first is dropped with an error naming it (E18 V4a). The second installs and lands. |
| A8 | Interruptible wait bound only by a member `on` | `t.on("enter", [reveal])`, `reveal` holding `wait(800, { interruptible: true })`; sibling `t2` has the same tag and no binding; P enters then leaves `t` before landing; P enters and leaves `t2` | `reveal` installs, because V3 counts the member binding. V5 derives the exit edge for `t` only, and leaving `t` cancels the tail. `t2` does nothing. On a `once` volume, V2 drops the reaction. |
| A9 | Three binding sources on one edge | brush `on_fire`, mod-global `defineTriggerEvent` and level `t.on("enter")` on volume `t`; one Enter | Brush runs first, then mod-global, then level, each in authored order. |
| A10 | Mod hot reload with member events | Level installed with `t.on("enter", [r])`; a mod hot reload recomposes; next Enter on `t` | `r` still fires for `t`, and only for `t`. |
| A11 | Mixed reaction on a connected client | Client-drained `levelLoad` body `[door[0].start(), npcs().update({ aggro: true })]`; client install | The mover member step applies; the group step applies nothing. |
| A12 | Light member beside a group step, at build | `[l.pulse({ … }), npcs().update({ aggro: true })]`; build-time light pass | `l`'s membership is reserved exactly as it is without the group step. Today one step without an id makes the pass skip the whole sequence (`light_membership.rs`). |

## Impact on E16--player-events
That brief, on branch `e24/stress-preflight`, changes as follows before it resumes:
- **Spelling:**
  - `players.on(edge, fire, options?)` becomes `players().on(edge, fire, options?)`, extending the `players()` group this brief ships. Luau uses `Postretro.players():on(...)`.
  - Drop the "`players` is a frozen SDK namespace" decision.
  - Drop "first source not spelled `onX`; aligning is the redesign's call". The rule now exists: per-member and per-group sources are `.on` on their target. Only tag-keyed declarations and crossings wait for the redesign.
- **Group clause:** `players()` already exists as a command target (`players().grantHealth(…)`). It means seat-bound pawns, skips held seats, and on a connected client applies nothing and logs nothing above debug. E16 adds `.on` and the `on.player` token, takes no filter in v1, and evaluates on the host each authoritative tick.
- **`on.player`** stays as both a command target and a `byPlayer` owner. Subject tokens are named for their kind.
- **Methods:**
  - `damage(on.player, 5)` becomes `on.player.damage(5)`.
  - `addSlot(on.player, progress.level, 1)` becomes `on.player.addSlot(progress.level, 1)`.
  - `on.player` carries `damage`, `grantHealth`, `grantAmmo` and `addSlot`, like `on.activators`.
- **Manifest key:** `playerEvents` stays.
- **System steps:** this brief hands E16 the question of whether system reactions become sequence entries.
- **Example:** its array reaction bodies (`[addSlot(…), updateState(…), playSound(…)]`) are not a body shape today. A body is one primitive or a `{ sequence }`, and system reactions such as `playSound` and `updateState` are not sequence entries. E16 must either use shapes that exist or decide to add multi-effect bodies.
- **`getGameState()` stays** for now; `gameState` went to the scripting-API session.
- **Boundary inventory:** the source-builder row becomes `players().on(…)`; params unchanged.
- **research.md §Symptom watch:** the spelling bullet is resolved by this brief.
- **Dependency:** E16 builds after this brief lands `players()`.

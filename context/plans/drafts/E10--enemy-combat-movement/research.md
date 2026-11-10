# E10--enemy-combat-movement — research

Read at 5932287f. Findings that inform the brief but do not decide it.

## Motion and facing today
- `MotionVerb` (foundation `data_descriptors/types/behavior.rs`) has six parameterless variants. `steering_for` (`crates/ai/src/graph_eval.rs`) maps `ChaseTarget` to `Chase`, `Freeze` to `Hold`, and the rest to `Clear`. `position_goal_steering` (`crates/ai/src/steering.rs`) overrides the position goals with `MoveTo`.
- `action_for_path` returns no action under a position-goal verb, so position goals cannot fire.
- `facing_direction` (`crates/ai/src/compute.rs`):
  - `MoveTo` faces velocity.
  - With `committed_aim`, faces the aim.
  - When engaged, faces velocity first, then the aim.
  - The attack gate needs post-slew yaw within `ATTACK_FACING_TOLERANCE_RADIANS` (π/12).
- The line-of-sight brief named a strafe or kiting verb as out of scope.

## Knockback
- `AgentComponent::add_knockback` adds the impulse to both `velocity` and `knockback_velocity`, and sets `GroundRef::Airborne` on an upward push.
- `tick_impl` (`crates/sim/src/agent_steering.rs`) stores steering plus knockback in `agent.velocity`. `path_state` copies it into `AgentPathState::velocity`, which facing and `locomotion_intent` both read.
- No AI code checks ground state. `KnockbackResponse` defaults are `air_drag: 0` and `control: 1`.

## Combat slots
- A tick has three passes: snapshot, then `compute::evaluate` per enemy, then the batch `resolve_combat_slots`, then serial `apply_outcomes`. All of them walk brains in slot-index order.
- `select_combat_positions_batch` sorts claimants by `EntityId::to_raw`. Proposals compete by score with a global `min_spacing` and are accepted greedily.
- The held slot is `BrainComponent::combat_slot` plus `combat_slot_hold_ticks` (`COMBAT_SLOT_HOLD_TICKS` = 8).
  - While the incumbent is retained, challengers are not scanned.
  - Once the hold expires, the previous slot is not passed and carries no penalty. So `apply_hysteresis` never chooses anything in this caller.
- Think stride gates acquisition only. Every enemy is evaluated every tick.

## Attack firing
- Step 4 of `evaluate` builds an immutable `PendingAttack`. `apply_outcomes` is the first place enemies are arbitrated against each other.
- A projectile weapon keeps advancing through `AiWeaponControllers::advance_actor` while `weapon_aim` is set. A shot can therefore land ticks after the request.
- No per-target shared state exists. `AiRuntime` holds programs, line-of-sight grace, weapons and warn latches.
- Untested: enemy multi-shot bursts. The executor appears to continue the sequence, but no AI test covers it. The brief adds a row for it.

## Precedents
- **Mod-wide scalar:** `factionSentimentDecay`.
  - Parsed by `drain_faction_sentiment_decay_js` and `drain_faction_sentiment_decay_lua`.
  - Stored via `FactionRegistry::with_sentiment_decay`.
  - Hashed in `mod_compatibility_digest`.
- **Player locomotion naming:** `MeshDescriptor.animations` typedoc. It names `walk_forward`/`walk`, `walk_backward`, and `jump_loop` on a launch without a jump.
- **Directional clips:** `content/dev/models/clip_libraries/mixamo_rifle/library.gltf` ships `walk_left`, `walk_right`, `walk_backward`, and their `run_*` siblings. `rifle_trooper` and `rifle_warrior` bind it.

## Industry reference (owner conversation)
- Doom 2016/Eternal grant a few attack tokens per player, per attack type (melee, ranged, charged), with counts set per difficulty; enemies without one reposition. A spent token returns after a cooldown, which spaces heavy attacks. Sources are write-ups of id's GDC talk, not the talk: [Game Developer, "Cyber Demons"](https://www.gamedeveloper.com/design/cyber-demons-the-ai-of-doom-2016-), [xbloom.io](https://xbloom.io/2021/03/19/designing-a-combat-system/), [AITokenSystemUE](https://github.com/intaxwashere/AITokenSystemUE). Other implementations keep a small melee pool and a separate ranged pool so archers never crowd out melee ([Unreal RPG devlog](https://pavankrao-qlp.github.io/portfolio/blog/unreal-rpg-devlog-part-1---state-tree-based-ai--unreal), [StraySpark](https://www.strayspark.studio/blog/attack-token-system-ue5-group-combat-that-feels-fair)). Tokens are those per-type pools, and `refreshMs` is the cooldown.
- "Longest waiting wins" keeps slot-index order from starving the same enemy every time.

## Attack pool precedent
- An attack pool's members are a tag, as in `plans/done/E18--enemy-group-handle`: `_tags` on placed enemies, and `entity_spawner.spawned_tags` passes tags to each spawn (`crates/sim/src/spawner.rs`). `content/dev/maps/closet-reveal.map` already groups its closet this way.
- Map members with `start` and `stop` from triggers and scripts: trap members (`getMapEntities("trap")`, `scripting.md` §12 Entity addressing).
- Engines pair an entry-firing trigger volume with tag- or group-addressed targets: Godot `Area3D.body_entered` plus groups, Unreal `OnActorBeginOverlap` plus Gameplay Tags, Unity `OnTriggerEnter` plus tags, Bevy with Avian `On<CollisionStart>` observers. None selects members by volume. A placement-free logic entity reached by name (Quake `trigger_relay`, `trigger_counter`; Source `logic_relay`) holds settings that would otherwise repeat at each caller.

## Flanking
- The target's facing is not on `TargetPawn` (`crates/ai/src/targeting.rs`). The owner chose the enemy's entry bearing over target facing: aim moves too often, and a remote player's host-side yaw lags.
- The shipped positioning brief planned a flank or angle preference term and never shipped it (`plans/done/E10--enemy-combat-positioning`).
- Ring: `RING_DIRECTIONS` (8) × `RADIAL_MULTIPLIERS` around the target (`generated_positions`, `crates/sim/src/combat_positioning.rs`); score is `score_candidate` (`combat_positioning/scoring.rs`).
- Flank routes out of sight need pathfinder cost hooks (`astar_corridor` takes none) and multiply A* work; deferred to the cover brief.

## Attack pool rivals, rejected
- One active attack pool at a time, a start ending the last: co-op players in two arenas would share one profile, and chained arenas save only one `stop` wire.
- Membership as the live tag: a tag edit silently moves enemies, and every overlap needs a winner. Event-driven assignment gives each enemy one pool, set by `start` and by tagged spawns; only a spawn matching two running pools needs one, and it takes the latest start, since scripted starts are the power user's override.
- Per-enemy overrides in one shared pool (the earlier draft): a budget-1 group at the head of the line held back every other enemy on its target. A script-only `startAttackPool` command: map-only authors could not build one. Trigger commands carrying the member tag and profile: an arena with two entries repeats the settings, and nothing named exists to stop. A brush volume selecting members: a second region doing the tag's job. Per-token limits inline in a map string: token names belong to code, so maps name a profile.

## Budget shape rivals, rejected
- One class per archetype: an enemy with a grenade and a claw would queue its claw behind lobbers, and a slam and a jab would share one cost.
- One `total` per player with classes inside: a sniper's turn takes a melee attacker's slot, and a heavy limit frees the instant its turn ends, so heavy attacks bunch.
- Lanes with classes nested inside: two counting rules (a lane counts cost, a class counts turns), and no limit can span melee and ranged, so a slam and a rocket cannot share "one heavy at a time". Tokens subsume both: a slam draws `melee` and `heavy`.
- Engine-fixed melee and ranged tokens, inferred from the attack's kind: pacing is taste (`scripting.md` §1), and a charged or thrown attack fits neither.
- Archetype-level default draws: no shipped enemy needs them, and they add a second resolution rule. Additive later.
- Token references as plain objects (`{ token, cost }`) beside a handle-based declaration: two styles. `token.with({ ... })` keeps every reference a handle, and its fields mirror the declaration's.
- `attackTurnFree` as a member of `brain`: `brain` is a fixed fact table pinned to `BRAIN_INPUTS` by the typedef drift test; named reads are free functions (`state`, `defense`).

## Pin table

Pins draw one token unless they say otherwise, and "limit N" is its `limit`.

| id | scenario | ordering | expected outcome |
|---|---|---|---|
| P1 | Turn arbitration within one tick | Every request and release on a target is known before any grant. Grants go by longest wait; equal waits go to the earlier brain slot index; the longest waiter on each token reserves its cost there (pin P32). | The longest waiter wins even when a fresher requester has the earlier slot index. The grantee may start on the grant tick. |
| P2 | Attack refused for want of a turn | Compute proposes an attack; arbitration refuses it the same tick. | Nothing commits: no cooldown, count, event or weapon request. The once-per-entry fire stays open. |
| P3 | Unmarked grant with no attack | Turn granted; an earlier outcome in the pass kills the target, or the weapon is still executing. | The turn is released that tick. Nothing commits. |
| P4 | Unmarked weapon attack ends | Turn granted, shots land over later ticks, the execution clears inside an apply walk. | The turn holds through the execution and the cooldown. The freed turn is grantable from the next tick. No other start lands between shots. |
| P5 | Holder leaves the AI set | Quiescence, queued despawn, inventory ownership, killed earlier in the pass, aggro closing, graph reseat. | Stops counting from that tick. A waiter is granted no later than the next tick. A returning enemy re-requests. |
| P6 | Holder switches target | Tick N: compute selects B while holding a turn on A, possibly mid-burst. | A started execution is cancelled and A's turn frees on tick N. The enemy requests on B behind B's existing waiters. |
| P7 | Guards read turn facts | Compute refreshes facts before arbitration. | Facts show the previous tick's turns and counts, refreshing counts included. `hasAttackTurn` first reads true the tick after the grant. Several waiters may see `attackTurnFree(attack)` true for one token; exactly one is granted. |
| P8 | Undrawn attack beside an oversized request | Limit 2; an enemy in an activity marked for an attack with `draws: []`; a cost-3 request waits. | The undrawn attack lands without counting and reads `hasAttackTurn` true in its marked activity. Cost 3 is granted when nothing is counted. |
| P9 | Strafe with no usable neighbor | Both neighbors unusable, or the batch gives the next slot to another strafer. | A batch loser reverses once. With neither neighbor usable it holds its slot, facing and firing, and does not flip again until a neighbor frees. |
| P10 | Launch and landing | Upward push on tick N, landing on tick M; activity and steering unchanged. | `jump_loop` after the fall delay, or the current clip when none is declared. Tick M switches back to the resolved clip. |
| P11 | Restored from serialized state | A holder and a waiter, one a member of a running attack pool, are serialized and restored. | No turn held, wait clocks at zero, refreshing counts dropped; the first tick arbitrates from scratch. Running pools, their start order and each enemy's assignment are restored as saved, never re-derived by replaying starts or tags. |
| P12 | Longest waiter does not fit | Limit 2; cost-1 enemies rotate so a turn is always held; a cost-3 enemy has waited longest. | It reserves the whole token, so no later requester is granted past it. It is granted once nothing is counted against the token, refreshing counts included, within a bounded wait. |
| P13 | Enemy changes pool while holding a turn | Tick N: an attack pool at limit 1 starts while two cost-1 members hold default-pool turns on one target, or stops while its members hold its turns. | No turn is revoked; each counts against its holder's new pool and ends by its own rule. No grant in that pool lands until, on every token drawn, held and refreshing cost plus the requester's cost fits. |
| P14 | A token at the head of the line | `melee` at 4, `heavy` at 1, cost 1 throughout; a slam draws `melee` and `heavy`, a jab `melee`. One slam holds; a second has waited longest; two jabs request after it. | The waiting slam reserves one `melee` and the `heavy`. Both jabs are granted past it, filling `melee` to 4 with the reservation. It is granted the tick the holding slam releases, ahead of any later slam. |
| P15 | Attack pool request timing | Tick N: a trigger command or trigger-bound step starts B while A runs; two triggers each start a different attack pool sharing members; a start and a stop of one pool land on one tick; a named, player-event or post-wait start is issued. | A trigger or trigger-bound request applies before tick N's arbitration. Every other request latches to tick N+1 and applies first there. Requests apply in issue order: both started pools run, a shared member takes the later-issued assignment, and a start then stop of one pool leaves it stopped. A `start` on a running pool re-assigns its tag holders and moves its rank to latest. |
| P16 | Target moves during a `reposition` | Tick N: entry records the bearing from target to enemy and the enemy's position; ticks N+1…: the target walks across the room. | The arc stays measured from the tick-N bearing, the minimum travel from the tick-N position; the slot is re-scored around the target's current position. Leaving and re-entering records a new bearing and position. |
| P17 | Restore mid-reposition or mid-strafe | Tick N: a repositioner records its entry bearing and position, and a strafer its direction; serialize on tick N+k, then restore. | The first restored tick records a new bearing and position as a fresh entry, and the strafer re-picks its direction by the first-entry rule. No oscillation on that tick. |
| P18 | Head of line per token | `melee` at 3, `heavy` at 1. One slam (`melee`, `heavy`) holds; a second slam waits longest; a cost-3 `melee` draw waits behind it; then a cost-1 `melee` request. | The second slam heads `melee` and `heavy` and reserves one of each; the cost-3 draw reserves nothing yet. The cost-1 request fits beside the slam's reservation and is granted. Once the second slam is granted, the cost-3 draw heads `melee` and reserves the whole token. |
| P19 | One enemy switches attack while waiting | `lobber` at 1, `melee` at 3; a grenade draws `lobber`, a claw `melee`. Enemy A's grenade holds; enemy B's grenade has waited longest; B's graph then requests its claw; then enemy C requests a grenade. | B keeps its wait clock and is granted the claw if `melee` has room. B's dropped grenade request no longer blocks `lobber`, so C's grenade is granted the tick A's releases. |
| P20 | Marked switch mid-execution | Tick N: an enemy mid-execution of a multi-shot `slam` weapon attack, in a chain marked for `slam`, enters a sibling marked for `jab`, which draws the same token. | The slam turn holds until the execution clears, then frees. The jab request stands from N, its wait starting at N, and is granted no earlier than the slam turn frees. No jab lands between slam shots. |
| P21 | Holder switches attack by mark | Limit 2, cost 1. X holds; enemy E holds a `slam` turn; W has waited since tick 10. Tick N: E enters a sibling marked for `jab`. | The slam release is known before grants. E's jab wait starts at N, so W is granted on N and E's jab waits behind W. |
| P22 | Waiter's clock across boundaries | W first requests on target A at tick 10, stops requesting during ticks 20–40 (left its marked activity, or out of range), requests again at 41, switches target to B at 50, changes pool at 60, then leaves the AI set and returns, or its graph reseats. | A gap keeps the tick-10 clock. A target switch restarts it behind B's waiters. A pool change keeps it in the new pool. Leaving the AI set or a reseat drops the request and its clock. |
| P23 | Turn-free fact with an own request | Limit 3. X holds cost 1. Enemy B's cost-3 slam has waited longest and reserves the whole token; C, a fresh cost-1 requester, waits behind it. Both read `attackTurnFree("jab")`, a cost-1 jab on that token. | B's own pending request is left out of B's fact and B's clock applies, so B reads true. C reads false while B's slam reserves the token. An enemy holding a turn that shares a token with the jab reads false. |
| P24 | Waiter switches off a blocking draw | Limit 3. X holds cost 1. B's cost-3 slam has waited longest and reserves the whole token; D requests cost 1 after it. Tick N: B's graph requests its cost-1 jab. | From N, B's slam request blocks nothing. B's jab keeps B's clock and is granted on N, and D is granted on N because 1+1+1 fits. |
| P25 | Request source within one compute | An offense selector's row guard reads `attackCooldownMs`, so the cooldown-seed pass resolves `jab` and the fire step resolves `slam`. | The request names the attack the fire step resolves. An unmarked request is made only when every fire-step gate passes for that attack. |
| P26 | Refresh after a heavy turn | `melee` at 3, `heavy` at `{ limit: 1, refreshMs: 4000 }`. Tick N: slam H's turn ends. Jabs request from N+1; slam H2 has waited since before N. Variants: a pool change, target switch or serialize-and-restore during the refresh. | `heavy` stays counted until 4 s after N; H2 is granted on the first tick after that, ahead of later slams. H's `melee` draw frees at N, since `melee` sets no refresh, so jabs are granted meanwhile. A refreshing count stays with the pool and target it was counted in, and a restore drops it. |
| P27 | Multi-token waiter among single-token traffic | `melee` at 3, `heavy` at 1, `ranged` at 2. Slam S (`melee` ×2, `heavy`) waits longest. Rockets (`ranged`, `heavy`) and jabs (`melee`) keep requesting; a rocket holds `heavy`, a jab holds `melee`. | S reserves two `melee` and the `heavy` from its first request. Later jabs fit only beside S's `melee` reservation, and no rocket takes `heavy` while S waits. S is granted the tick the holding rocket's `heavy` frees. |
| P28 | Head of line beside a refreshing count | `heavy` at `{ limit: 2, refreshMs: 4000 }`; a turn ends on tick N, leaving one refreshing count; a cost-3 `heavy` draw waits longest; cost-1 `heavy` requests follow. | The refreshing count is counted for admission and for the reservation. No cost-1 request is granted past the cost-3 draw, which is granted on the first tick nothing is counted against `heavy`. |
| P29 | Refresh after an interrupted turn | `heavy` at `{ limit: 1, refreshMs: 4000 }`. A slam turn ends on tick N without the slam landing: a grant that does not start, a target switch, the holder killed, or a reload changing its `draws`. | `heavy` stays counted until 4 s after N, exactly as after a landed slam. |
| P30 | Refresh lifetime | A count refreshing in attack pool A on target T; A stops on tick N and restarts on N+k inside the refresh; separately, a turn granted in the default pool ends after its holder joined A; the level restarts; T leaves the world. | A refreshing count lands in the pool the turn counts against when it ends. Stopping a pool drops its refreshing counts, so a restart begins open. A level restart or change drops every refreshing count, and a target leaving the world drops its tallies. |
| P31 | Fit within one pass | Grants proceed in wait order; an earlier grant this tick consumes room a later waiter needs. | Fit and reservations are judged against counts that include this tick's earlier grants and reservations. A replay grants the same set. |
| P32 | Who reserves | Several waiters draw one token. | Only the longest waiter on a token reserves there: its cost, or the whole token when its cost exceeds `limit`. Later waiters reserve nothing until they head it. A grant fits when held, refreshing and reserved counts plus its cost stay within `limit`. |
| P33 | Spawn matching two running pools | Pool A starts, then B; a spawner whose `spawned_tags` carry both member tags spawns; variants: the spawn lands on the tick of a start, and a serialize-and-restore sits between the starts and the spawn. | The spawn joins B, the later-started pool. A start on the spawn's tick applies first (pin P15), so it counts. Start order is restored with the running pools, so a restored spawn joins the same pool. |

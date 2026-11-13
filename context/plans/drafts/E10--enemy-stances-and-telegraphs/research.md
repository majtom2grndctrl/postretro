# E10--enemy-stances-and-telegraphs — research

Read at 5932287f; refreshed at 2cb88013. Findings that inform the brief but do not decide it.

## Telegraphs today
- A windup is an authored activity left by a `timeInActivityMs` guard. Shipped examples:
  - `reference-enemy.ts`: slam, with windup 250 ms, commit 150 ms, recover 1450 ms.
  - `limitator.ts`: aim 550 ms, then fire.
- The attack fires once per activity entry, on the first tick that passes cooldown, range, aliveness, line of sight and facing (step 4 of `compute::evaluate`).
- The only engine delay is `SPAWN_ATTACK_WINDUP_MS` in `crates/sim/src/spawner.rs`.
- `IrNode` has no RNG. `BRAIN_INPUTS` holds only deterministic facts.
- **The randomness posture** (`networking.md`): engine randomness is host-only, load-time, consequences only. Weapon pellet spread is the one per-tick exception, seeded by `pellet_rng_seed(shells_fired, salt_name, slot)`.
  - Trigger pools roll once at load from `entropy_seed()`.
  - Chance-based pain and random patrol were both deferred for want of this story: `drafts/E10--enemy-stagger`, `done/E10--enemy-behavior-descriptor-syntax`.

## Replay-stable identity
- `EntityId` packs index and generation. Its index comes from a LIFO free list, so it depends on spawn order.
- The determinism gate (`crates/sim/src/sim/determinism_tests.rs`) runs `SpawnOrder::BetaThenAlpha` and compares by role label.
- `DescriptorProvenance` is per descriptor, not per instance.
- `BrainComponent` has no entry counter or entry tick. Its per-depth timers and attack counts zero on re-entry.

## Co-op delivery
- `SNAPSHOT_TICK_INTERVAL` = 2, about 30 Hz. `WireMeshAnimationState` carries the current state name only, latest value per snapshot. So a state shorter than one interval can be skipped, and a same-name re-entry is invisible.
- The client restarts the clip locally on a name change (`switch_animation_state`, local clock). No host clip time crosses the wire.
- **Behavior-entry sounds** play in the frame loop's Audio stage from `pending_ai_events`. That queue is always empty on a client, because clients never run `simulate_tick`.
- **Melee is silent on clients.** `publish_observer_weapon_cues` skips emissions without a `shot_id`, so a contact attack's sound never reaches a client. A weapon attack's sound rides `WeaponCue.additional_sound`.
- **Observer cues** (`freeze_observer_weapon_cue`) anchor to an entity `NetworkId` and go to all participating clients except the owner. An enemy has no owner, so every client receives them.
- **Version gates.** A new message bumps `PROTOCOL_ID` ("PRLD" at 2cb88013). `WIRE_VERSION` (39) bumps only when an append-layout guard shows a shipped encoding changed. `SNAPSHOT_VERSION` (25) bumps for snapshot-record changes.

## Stagger without engine work
- **What works today:**
  - An impact policy (`defineImpactEvent`) can read `impact.amount` and `impact.target.state(n)`, and write `setState`.
  - Guards read `@state.*`.
  - Wildcard rows that target the current activity are filtered out.
- **What does not:**
  - No reaction writes per-entity state.
  - `onEnter` carries no subject.
  - `applyDamage` (`AppDrain`) runs no impact policy.
  - Policies have no clock, so authors would need a mod `accumulate` slot.
  - Two hits on one tick can erase a threshold pulse.
- So stagger is expressible, but fragile. One engine fact fed at the chokepoint removes the clock, the race and the `applyDamage` gap.
- **Stale wording, outside this brief:** `entity_model.md` §7c and the `state()` typedef say reactions write per-entity state. In production only impact policies and spawn seeding do.

## Stage order (`crates/sim/src/sim/mod.rs`)
- The stages run: Movers, Movement, Triggers, Projectiles, AI, Traps, Steering, Weapons.
- Projectile hits are seen by the brain the same tick.
- Player hitscan lands after AI, so the brain sees it next tick. Enemy melee lands in the AI apply pass, so its target also sees it next tick.

## Industry reference (owner conversation)
- An attack has three phases: anticipation, active, recovery. The tell lasts longer the more an attack hurts.
- Tells use several channels: pose, a unique sound, VFX, barks.
- For Honor's stances name the attack while the release timing varies. Doom Eternal's Marauder flashes before its axe.
- Unreal, Unity and Godot all leave this to animation notifies. Postretro times it in logic, which suits determinism and co-op.

## Review round 1 corrections
- The host restarts nothing on same-name re-entry. `switch_animation_state` returns already-in-state, and `restart_animation_clip` has no production caller. So a replicated entry sequence would have made clients exceed the host, and it was dropped.
- Luau has no object spread, and the SDK has no merge helper. So `merge()` is what makes the fragment approach mirror in Luau.
- An `onEnter` `playSound` already reaches every peer. A host-only reaction with no audience resolves to Everyone (`system_commands.rs`). Only the descriptor `sound` field was host-only.
- `drafts/E10--enemy-stagger` was deleted on this branch. Its accumulator design is superseded by `damageInActivity`.

## Pin table

| id | scenario | ordering | expected outcome |
|---|---|---|---|
| P1 | Release row and `breakOn` row both pass | The hold timer reaches its threshold on the tick the damage sum reaches `breakOn.damage`. | Break wins; release is never entered. Lowering emits the break row first. |
| P2 | Damage around a transition tick | On tick N a hit lands before the AI pass, the brain leaves A for B, then a second hit lands after it. | The first counts toward A only. The second counts toward B and is read on B's first evaluation. |
| P3 | Activity entered and left on one tick | A reseat seats the initial activity and its own row passes in the same pass. | Its rows read its own fresh roll. Only the final leaf announces; nobody plays the skipped sound. |
| P4 | Nested levels entered by one descent | A composite and its nested initial seat on one tick; later the nested level transitions. | Independent rolls per level. A nested transition redraws and zeroes only the nested level. |
| P5 | Selector guard reads a scope-relative fact | A move or offense selector row guards on the roll or damage sum. | It reads its own level's values, matching a transition guard there. |
| P7 | A → B → A inside one snapshot interval | Activity A, with a sound, is entered, left and re-entered between two snapshots. | Each peer plays A's sound twice. The client's clip follows the latest replicated state. |
| P9 | Sound without a live client entity | A sound reaches a client before the enemy's snapshot, after it despawned, or while the client holds it. | Plays once at the host-captured position whether or not the client holds the enemy, never at the client's interpolated pose, and never replays. |
| P10 | Weapon attack sound | An enemy weapon attack carries a sound. | It rides `WeaponCue.additional_sound` and plays once. The enemy cue never duplicates it. |
| P11 | Killed in the pass that enters a stance | An earlier outcome in the apply pass kills the enemy that compute moved into a stance. | No entry sound and no cue on any peer. |
| P12 | Hold at zero or equal bounds | `holdMs: 0`, `[0, 0]`, `[x, x]`. | All load. Zero leaves on the first evaluation; `[x, x]` matches a fixed `x`. |
| P13 | Initial activity at spawn | A fresh brain seats its initial activity before per-instance spawn state is set. | The first evaluation reads this enemy's own salted roll. Two same-tick spawns differ, and replay reproduces both. |
| P14 | Stance entered on a client's join tick | On tick N an enemy enters its stance; the client begins participating that frame, before or after the host publishes tick N's cue. | A cue published after participation began plays once, at the captured position if the enemy is not yet materialized (P9). One published before is never delivered. Materializing the enemy mid-stance plays no sound. |
| P15 | Telegraph sound on the host | The Audio stage plays an activity's or contact attack's `sound` from its AI emission; a `PlaySound` routed to every peer is also pushed to the host's local queue (always, with no server). | The host plays it once, with or without a server: the routed copy goes to clients only, or the emission copy is dropped. |
| P16 | Pose timer and a lost turn on one evaluation | The pose's timer reaches the hold threshold on the evaluation that first sees the hold's turn missing (the tick before, the target switched to one with no room, or no target remains). | Turn-lost wins: the hold leaves for its rest and the release is never entered. Lowering emits the turn-lost row before the release row. |
| P17 | Speed across an activity boundary | Tick N enters a `speed: 2` activity; tick M leaves it. A reseat seats an activity with `speed` and leaves it in one pass. | Steering on tick N runs under the 2× speed cap and acceleration; steering on tick M under 1×. The reseated-and-left activity never steers under its speed. |
| P18 | Charge reach edges | (a) The target is inside the charge's reach on the release's entry tick. (b) The chase never reaches within `release.ms`. | (a) The contact fires on the entry tick. (b) The release leaves for its recover with no hit, the recover never fires it, and the turn ends once, when the cycle leaves its mark. |
| P19 | Stance entry, then death later in the tick | Compute moves the enemy into its stance on tick N; a Weapons-stage hit kills it on tick N. | The stance sound plays once on every peer, at the captured origin. |
| P20 | Zero release or recover | `release.ms: 0`, `recover.ms: 0`. | Both load. The release fires at most once, on its entry tick, and leaves on the next evaluation. A zero recover leaves on its first evaluation. |
| P21 | Retired | A hold can no longer be seated by spawn or reseat: `initial` may name a stance's rest, never a hold. | — |
| P22 | Break and turn-lost rows both pass | On the first evaluation that sees the missing turn, the damage sum also reaches `breakOn.damage`. | Break wins: the hold leaves for `breakOn.to`. Lowering emits the break row before the turn-lost row, and that before the release row. |
| P23 | Stance outside an engaging ancestor | A stance sits at root level beside a chase. During its planted hold a co-op re-rank changes the best target, or the target leaves sight for one tick. | The marked path keeps its target and committed aim; the turn holds and the release fires. |
| P24 | Two enemies queued for one shared stance | Under one free token, enemies A and B rest in `overhead`; both request `slam` on tick N; A is granted. | A strikes on N+1. B keeps resting with its request pending, no pose drop and no second stance sound, and is granted in wait order. Accepted (owner). |
| P25 | Retired | Superseded by P28; its setup cannot starve. | — |
| P26 | Grant in the rest | A rest's queued request is granted on tick N. (a) On N+1 its row passes. (b) On N+1 another row passes first. | (a) The enemy enters the hold; the hold's mark keeps the turn. (b) The turn ends on N+1 and its refresh starts; nothing fired. |
| P27 | Re-rank between grant and hold | Granted on N; on N+1 a co-op re-rank would prefer another target. | The resting enemy keeps its target, enters the hold, and the turn survives. |
| P28 | Costly attack under cheap pressure | Limit 3 on `melee`, no refresh. Two sweepers hold two `melee` turns and re-request as they end; brute Y rests in `overhead`, slam first (`melee` cost 2 plus `heavy`), later wait slot. | Y becomes head of line and reserves `melee`; sweeper requests wait behind it; Y is granted once two `melee` free. Under a turn-free gate with no request, Y waits forever. |
| P29 | Place in line across changes | Y requests `slam`, then its slam `when` fails and `sweep`'s passes; later Y leaves `overhead` for `approach` and returns on the same target. | Y's wait clock is kept throughout: it neither moves behind later waiters nor restarts. |
| P30 | Facts across a pose | 20 damage in `overhead`'s rest, then `slam`'s hold entered, then 20 more. Separately, a hold loses its turn and returns to the rest. | `breakOn: 25` trips on the second hit. The return keeps the pose's timer, roll and damage sum. |
| P31 | First row refreshing | `slam` first, its `heavy` refreshing for 4 s; `sweep` second, free. | The brute keeps requesting `slam` and rests; `sweep` is not requested while `slam`'s `when` and range pass. |
| P32 | Release time vs roll | A rest branches `slam` on `activityRoll < 0.5`, else `sweep`; both share `holdMs: [500, 1200]`. | Each attack's release times span [500, 1200] across many strikes, independent of the roll that chose it. |
| P33 | Target switch mid-tell | A hold holds its turn; on tick N its target switches. (a) The new target has room. (b) It has none, or no target remains. | (a) The mark is granted against the new target; the tell carries on and strikes it. (b) The hold returns to its rest on N+1, pose unbroken. |
| P34 | Fresh entry into a pose | The rest is entered from a recover, from a `"*"` row, after an ancestor reseat, or as the same stance's copy in another level. | Each plays the stance sound and starts timer, damage sum and roll fresh. |
| P35 | Break vs granted strike in the rest | On one evaluation the rest's granted strike row and `breakOn` both pass. | Break wins; the turn ends and its refresh starts. |
| P36 | Grant then no row | A rest's request is granted on N; on N+1 no row passes. | The turn ends on N+1 and its refresh starts; the next request queues behind earlier waiters. |
| P37 | Exhaustive list drops | `attacks: [jab]`. (a) A rest's only row strikes `slam`. (b) `coil` is reached only through `slam`'s `endingStance`. | (a) The row drops, and the reached rest with no way out rejects load naming it. (b) `coil` is not generated; its authored rows drop with a log. |
| P38 | Release draw replay | Two same-descriptor enemies rest in one shared stance and are granted on one tick; the run repeats with their spawn order reversed. | Each enemy's release draws match by role across both orders; the two enemies' draws on one tick differ; no wire record carries a draw or its seed. |
| P39 | Switch past the margin | (a) A rest's grant on tick N; on N+1 another player is nearer by more than the switch margin. (b) Same, during a hold. | (a) The target switches on N+1; the hold is entered on the turn read held, the old turn ends with its refresh on the old target, and the hold's mark re-requests against the new target (P33). (b) As P33. No build locks the target while a turn is held. |
| P40 | Drop to a fixed point | `attacks: [sweep]`. `coil` is reached only through `slam`'s `endingStance`; its authored rows include `{ to: bellow }`, `bellow`'s only route in; `bellow`'s only row strikes `charge`. | Neither `coil` nor `bellow` is generated; every dropped row logs once; load succeeds. |
| P41 | Guard reads under a list | `attacks: [jab]`. (a) A row to `slam` whose `when` reads `attackTurnHeld(slam)`. (b) A row to `"approach"` whose `when` reads `attackTurnFree(slam)`. (c) An unreached rest's authored row reading `attackTurnFree(slam)`. | (a) and (c) drop with a log and never reject. (b) rejects load naming the guard's path. |

## Review round 2 corrections
- `WeaponCuesMessage` sends one cue per message and carries a `participation_epoch`. `MAX_WEAPON_CUES` is 64 (`WeaponCuesMessage::is_valid`, `crates/net/src/wire.rs`), and `send_weapon_cues` silently skips an invalid (over-cap) message. So the enemy cue goes one per message.
- `MAX_DELAY_MICROS` (250 ms) is `pub(crate)` in `crates/netcode/src/interpolation.rs`, and netcode depends on sim. So the floor constant lives outside netcode, guarded by a netcode test (precedent: `SPAWN_ATTACK_WINDUP_MS`, `enemy_replication_harness_test.rs`).
- `"*"` rows evaluate before every activity's own rows at that level (`graph_eval.rs`). So they can leave a release and win a tie with the break row.

## Refresh at 2cb88013 (after combat movement)
- **Turn timing.** Compute, then arbitration, then apply (`run_ai_tick_with_host`). A mark entered on tick N requests and can be granted in tick N's `arbitrate_attack_turns`; `attackTurnHeld` reads it on N+1. The hold's one-tick turn-lost exit holds.
- **Firing is gated.** A drawn attack under a mark fires only with its turn; a dropped proposal keeps the once-per-entry fire open, so a release entered while still waiting fires on its first granted tick. Only the hold's exit row keeps an ungranted enemy out of `_release`. Undrawn attacks are exempt.
- **No turn time limit.** `retain_held` keeps a turn while the path's mark names its attack, so a same-attack chain holds it indefinitely (owner: allowed, documented). A token is consumed on grant, and its refresh starts when the turn ends, fired or not (`roster.ended_for` → refresh ledger).
- **Self-edges.** `first_enabled_target` skips a row whose `to` is the current activity, `"*"` included.
- **Damage sum.** `points_dealt` = absorbed + health loss (`drain_layers`).
- **Stage order.** Remote hits already authorized land in Projectiles, before AI; local and freshly commanded remote hitscan land in Weapons, after AI.
- **Brain serde.** `BrainComponent` derives serde with defaulted fields; the only production consumer serializes (the observability dump). Deserialization is test-only.
- **Content drift.** The limitator's `aim` was replaced by `close` / `wait` (strafe) / `burst` (`attackTurn: "shoot"`) / `move` (reposition). The brute's slam cycle carries `attackTurn: "slam"` marks and a windup turn-lost exit at 250 ms. Tests pinning it: `reference_behavior_graph` and its `reference_enemy_*` users (`crates/ai/src/ai_tests.rs`), `the_shipped_reference_enemy_graph_is_identical_in_both_authorings`, `dev_mod_budget_paces_reference_enemies_and_troopers`.
- **Speed.** No per-activity speed exists; `move_speed` is per archetype, copied onto the agent at spawn.
- **Naming.** `LocomotionStance` (player, `crates/sim/src/sim/player_locomotion/jump.rs`) is engine-internal, not modder-facing.
- **Direction review (validate-plan, 2026-11-11): Under-scoped**, resolved by the owner:
  - Release clips replay the windup from frame 0 (`smash_windup` and `swipe_windup` are prefix cuts) while damage fires on entry. Clip-timed attacks (roadmap) builds first; three brute releases block on it.
  - Per-activity `speed` stays a multiplier, recorded as a witting divergence from the statecharts brief and `sample.ts`. Owner: much ms-timed API predates clip awareness; expect to pay it down toward clip-referenced timing.
  - Sounds reuse `PresentationCommand::PlaySound` rather than a new reliable message, one route with `onEnter` and E12 step 4. The pose remains the tell when a packet drops.

## Review round 1 corrections (2026-11-12)
- **Self-chaining withdrawn** (owner). A self-chained hold's turn-lost exit is a self-edge `first_enabled_target` skips, so a target switch mid-chain (`retain_held` ends the turn) sent the enemy into its release without a turn; a cycle shorter than its attack's cooldown replayed its tell and struck nothing. `after` naming the stance's own hold or `_recover` rejects load.
- **`then` renamed `after`**: `then` is a Luau keyword, so `{ then = ... }` does not parse.
- **Planted pose.** A leaf with no `motion` inherits its nearest ancestor's (`graph_eval.rs`), so the builder writes `motion: "hold"` on all three.
- **Floor without SDK surface.** Neither runtime has a script warn primitive (Luau `print` logs at info; QuickJS has no console), and no Rust constant reaches `sdk/lib`. The floor bounds the seeded default only.
- **The promise is the turn.** `attackCooldownMs` reads 0 on a path with no action (`compute.rs`), so no hold guard can read the release's cooldown.
- **Walk playback.** The host scales an uncalibrated locomotion clip by `agent.move_speed` (`sim/mod.rs`, `locomotion_rate_ratio`); the client by the descriptor's graph `move_speed` (`snapshot_apply.rs`). Writing the multiplier into `agent.move_speed` alone would split them.
- **Host double-play.** `route_player_presentation` re-pushes an Everyone `PlaySound` to the host's local queue, and with no server pushes every command locally (`presentation_commands.rs`); the Audio stage already plays the AI emission (P15). The onEnter route sends per client through `send_presentation`.
- **Owner on length:** a brief that ships a feature with its first consumers stays one unit even past the line target (recorded in `/draft-brief`).

## Review round 2 corrections (2026-11-12)
- **States, not clips.** `animation:` names a state in the archetype's `mesh.animations`; an undeclared name only warns at runtime. The brute today declares `idle`, `walk` (playing the `run` clip, `travelSpeed` 2.2), `attack_jab` and `attack_slam` (both `punch`), `flinch` (`hit_reaction`) and `death`.
- **Rate scaling.** Only a locomotion state (one that pursues without acting, `entity_model.md` §7c) rate-scales today, so a charge release would slide its feet without the moving-release rule.
- **Engaged ancestors.** Target retention and committed aim come from an engaging ancestor; a stance outside one lost its turn to a re-rank or a tick out of sight. Owner direction: the engine should make placements work rather than reject them.
- **Combos.** An `after` into another stance's hold is unconditional, so with `heavy` refreshing an overhead ↔ jab chain flashed the overhead for a tick before each jab. Owner: chain through a gated sibling.
- **Position goals.** A leaf with a position-goal motion and an attack rejects load (`recursive.rs`), so `release.motion` excludes them by name.

## Owner reshape (2026-11-12): typed attacks, stance as an attack's prerequisite
- **Owner direction:** strong typing over string keys in SDK APIs; a stance is mentioned by an attack as its prerequisite, not the other way round, so one stance can lead to several attacks; `defineStance` named by its binding.
- **Binding-name precedent.** `apply_binding_name_sugar` (`crates/script-compiler/src/lib.rs`) names `defineStore`, `definePresentationTemplate`, `defineImpactEvent`, `defineDamageType`, `defineDefenseLayer` and `defineAttackToken`. The last three key on an object-literal first argument. Luau has no compile step and passes names.
- **Attacks today.** `BehaviorGraphDescriptor::attacks: BTreeMap<String, AttackParams>`; references are strings in `action.attack`, `attackTurn`, and `attackTurnFree` / `attackTurnHeld` (`sdk/lib/brain.ts` builds `"@attackTurnFree." + attack`). About 16 references across 8 shipped scripts.
- **Why commit on entry.** A drawn attack under a mark fires only with its turn, and the hold must hold it before the tell starts or the tell can promise a strike that cannot come. Picking the attack at release would need the hold to hold every candidate's turn, or an engine stance state; both rejected.

## Review round 3 corrections (2026-11-12)
- **Owner:** handle types `Attack` and `Stance`; Luau `cycles:enter(slam)`; holds named `<attack>_<stance>`; Rust column lists the type registry, typedef templates and Luau export lists; `defineMod` lowers bare entities too; `release` iff `stance`; equal same-name declarations load (Luau `require` has no module cache, `luau_require.rs`); shared-stance contention accepted (P24); body option keeps the name `stance` (crouch attack sets are a real use).
- **Owner on resting:** basic enemies leave a hold that lacks its turn; a higher-ranked enemy may rest in the pose waiting for the turn, so the telegraph is only partly predictable (`holdForTurn`).
- **Owner on Luau typos:** a misspelled handle must be observable to agents. Candidates: strict globals in data scripts, `luau-analyze` in the scripts check, or a language server; the executor picks and reports.
- **`scripts-build` does no type checking** (`crates/script-compiler`, `main.rs`), so TypeScript strings must also reject at runtime.
- **Binding-name sugar** names only a direct top-level binding; an inline `defineAttack({...})` gets no name and throws, as `defineAttackToken` does.

## Owner reshape (2026-11-13): rows target attacks and stances; stances are poses to rest in
- `stanceCycles()`, `merge()`, `enter()` and the cycle-wide `after` read as three moving parts for one idea. Owner: keep `defineStance` (no `after`), put `stance` and `after` on `defineAttack`, since an attack may land in a different stance than it started.
- **Model.** A stance is a pose an enemy can rest in, unmarked and holding no turn. `{ to: slam }` strikes (commits, marked hold); `{ to: overhead }` rests. `after` names an activity or a `Stance`; omitted, the attack returns to where it started. Resting replaces `holdForTurn`, and a self-chain lands in an unmarked rest, so no turn is held across a chain.
- **Where expansion runs.** `defineEntity` already walks the descriptor to lower handles; `defineMod` gains the same walk for bare entities (round 3), so both expand rows that target attacks and stances.

## Owner refinement (2026-11-13): beginning and ending stances; de-repeated rows
- `defineAttack` takes `beginningStance` and an optional `endingStance` defaulting to it, so every attack ends resting in a pose; "return to where it started" and its per-start copies retire.
- Owner-approved de-repetition: a row to an attack carries its own turn and `maxRange` gates; the behavior infers its attacks from references; a row to an attack is legal only out of its beginning stance's rest, so a lost turn drops back into the same pose.

## Direction review (2026-11-13): starvation, pose sound, expander
- **Starvation.** With a turn-free gate on every row and no turn held by a rest, an enemy joined the line only when `would_grant` already passed (`facts.rs`), so head-of-line reservation never engaged and a slam drawing 2 `melee` plus `heavy` could lose to sweeps forever. Owner: the rest waits in line; a row into a marked hold requests its turn (engine change; today requests come only from a marked path, `next_request`).
- **Pose sound.** `AiCue::Entered` plays the entered leaf's `sound` on every entry (`sound_events/descriptors.rs`). Owner: the expander keeps holds silent and returns a lost hold to a silent rest copy; no engine sound rule.
- **Expander.** Owner: expansion runs in `scripting-core` lowering at load, one implementation for both runtimes, rather than twin `sdk/lib` expanders.
- **Roadmap drift.** E12 step 4 appended a sound variant to `ServerPresentationPayload`; this brief uses the existing `PresentationCommand::PlaySound`.

## Review round 4 (2026-11-13): owner resolutions
- **Declarations reach Rust inside handles.** Lowering runs once per mod load and staged reload, in the JSON-before-serde step of `js/entity.rs` and `lua/entity.rs`, so its cost is off every level load and tick.
- **Queued rows** are a lowering-only `GuardedRow` field; a rest's grant takes a mark's lifetime; place in line is kept (P26–P29, P31). Precedence: mark > rest row > pending unmarked attack.
- **Pose** is a lowering-only activity field: no clip or sound replay, shared scope-relative facts, self-row skip, target retention (P30). It replaces the silent rest copy.
- **`when` optional** on every row; **floor** read by lowering, with a load warning below it; **`initial` may be a Stance**; strings naming generated activities reject.
- **Attacks list** exhaustive when present (variants with fewer attacks); weapon-gated attacks are a follow-up brief.
- Turn facts register no attack and reject string or `nil` at the builder.

## Review round 5 (2026-11-13): owner resolutions
- **Roll leak.** A shared pose roll set both a rest's branch and the hold's release time, so timing named the strike. Owner: each hold draws a hidden release time (P32).
- **Target switch mid-tell** keeps the tell when the new target has room (P33). **Chip damage** across a pose breaking a waiting enemy is intended.
- Authored `pose`/`queue` reject; the agent check covers every misspelled Luau global; the exhaustive list drops in a fixed order (P37); hand-written marks do not queue (non-goal).
- **Superseded in this file:** earlier rounds' "builder", `stanceCycles`/`merge`/`enter`/`after`/`then`, `holdForTurn`, the silent rest copy, and "the floor bounds the seeded default only" describe retired shapes; the brief's Decisions are current.

## Review round 6 (2026-11-13): final round, no Blockers
- Owner: the release draw shares the roll's seed on its own stream and the randomness exception covers it; a `breakOn` row is not a rest's way out.
- **Reach after a switch.** Owner: some enemies should range-check, some should not, and stances held while walking that strike on reach are a long-term goal; out of scope here, so a tell may whiff against a far new target. Logged with mobile stances as a follow-up.
- Retention is hysteresis, not a lock: a player nearer by more than the switch margin takes the target (P39). Drops repeat to a fixed point (P40); only kept rows' guards are checked (P41).

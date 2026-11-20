# E16--status-effects-and-hazards — research

Read at e7438238. Findings that inform the brief; the brief's Decisions conclude.

## Routing
- Combines roadmap E16 "DoT / environmental death policy" and "status effects / DoT" (draft session, owner, 2026-11-20). The two parts are independent: status ticks and fluid applications are in-tick producers and never use the script-damage evaluation. The combination is for scope, not dependency, so script-damage evaluation lands first as its own slice; it fixes a live defect and needs no wire change.
- `onDamage` per-attack aggregate: no consumer in `docs/`, `content/`, `sdk/`, drafts or ready plans. Its names appear only in `context/research/combat-events.md`. The dev mod rewards per hit and per kill (`content/dev/scripts/combat-lifecycle.ts`, `leveling.ts`). Owner made it demand-gated.

## Damage producers today
| Producer | Variant | Runs policy |
|---|---|---|
| Hitscan, pellets, projectile contact, splash, enemy melee, enemy ranged, `projectile_trap`, remote hit declarations, mover crush | InTick | yes |
| `applyDamage` reaction (`health/reactions.rs` `dispatch`, source id `script.applyDamage`, no attacker) | AppDrain | no |

- Gate: `ImpactPolicyRuntime::evaluate_pending_in_registry[_with_overlay_sink]` `continue` on non-InTick; `discard_app_drain_pending` drops the rest once per frame.
- Consequence: `sweep_deaths` latches an AppDrain kill and freezes credit, but non-player removal and kill reporting run only through `run_end_of_frame_removal_pass` for policy-marked despawns. The corpse stays.
- No environmental producer except crush. No fall damage. Triggers fire on Enter and re-arm after `rearm_ms`, so no continuous-volume damage. No status or DoT mechanism; `DeferredEffectKind` is `Despawn` and `SetHealth` only.

## Script damage sites
`dispatch` runs from:
| Site | When | Role |
|---|---|---|
| Trigger-bound `BoundTriggerCommand::Damage` | Triggers stage, in-tick | host / SP |
| Teleporter bindings | Movement, in-tick | host / SP |
| `PlayerEventTable::run_tick` | after each fixed tick | host / SP |
| `drain_frame_named_events` | frame drain | EveryMachine origins; raw-tag arm runs on clients |
| residual follow-ups | frame drain | host |
| `wait` tails (`drain_landings_resolving_seats`) | frame drain | host lands; client drops |
| `run_crossing_stage` | post-tick, after HUD republish | every machine |
| UI presses (`ui_actions.rs`) | before the tick loop | every machine |
| `levelLoad` | install | every machine; kind form host-only |

- The in-tick takers call `take_impact_dispatches()`, so a script impact queued before an in-tick hit is taken and dropped by that hit's evaluation. Evaluating only at frame end would miss Triggers, teleports, PlayerEvents, UI presses and `levelLoad`.
- The evaluator needs only the registry and its own `ScriptCtx`; nothing tick-only. At the discard site no registry or slot-table borrow is outstanding.
- Frame-end caveats: no role check in the evaluator (clients produce AppDrain via the raw-tag arm); `publish_health_from_registry` must run first on host/SP and never on clients; `@state` and store reads seed at evaluation time while `@impact.*` is frozen at damage time.
- Re-entrancy: no impact effect applies damage or fires an event; the batch is taken up front. Store and life-state writes can feed next-frame crossings or next-tick player events, one hop per frame.
- Frame order: tick loop (in-tick evaluation, then player events) → script drains → command drain → HUD republish → crossings → command drain → per-owner save → discard → removal pass → overlay roster → send. A despawn at the discard site is reaped by the removal pass the same frame; `FrameEndRemoval::is_kill` holds through `depleted`.
- The substrate brief names no concrete hazard for evaluating script impacts; its "API footgun" is carrying the producer tag early so a later evaluator needs no reshape.

## Fluids
- Authoring: `defineFluid` in manifest `fluids`; map `fluid_volume` names the fluid; PRL id 52 records carry the fluid name; runtime resolves names to declaration indices at install (`install_environment_volumes`). Clients resolve against the host's `FluidMovementTable`, which carries name and viscosity only.
- Membership: `resolve_environment` → `ResolvedEnvironment { fluid, immersion, tick_eye_in_fluid, .. }`. Coverage of the capsule segment; feet exactly on the surface are dry.
- Coverage: players only (`resolve_at_tick_start`). Agents resolve gravity only, corpses fall, projectiles resolve nothing (`entity_model.md` §7 Who resolves what).
- Persistence: `ResolvedEnvironment` is a local. `ImmersionComponent` keeps the fraction, not the fluid; `MovementState::Swim { fluid }` keeps it only while swimming. A wading pawn's fluid is lost after movement.
- Remote pawns step only with a command this tick, so their immersion can be one tick stale. Host-only status application tolerates that.
- E24 non-goal: "Damage specific to one fluid belongs on that fluid's own data, as a later field."

## Tick order
`simulate_started_tick_with_presentation_aim`: deferred effects → Movers → Movement (player fluid resolve) → Triggers, Touch → sentiment decay → Projectiles → AI → Traps → Steering (crush) → Weapons → Death sweep → Assist. Layer recharge runs after `begin_tick`, before `finish_tick_start`. The status stage goes after Weapons so every in-tick application on tick N precedes that tick's due ticks and the sweep sees a lethal tick.

## Extension map
- Handle kind: `HandleRef` (`crates/foundation/src/defense.rs`); SDK `defineMod` declaration maps in `sdk/lib/data_script.ts` / `.luau`; name allowlists in `luau_prelude.rs`, `luau_require.rs`, `script-compiler/src/light_membership.rs`; binding-name sugar arms in `script-compiler/src/lib.rs`; typedef templates and `typedef/common.rs`; manifest field in `sim/src/scripting/primitives/manifest.rs`. Self-contained precedent: attack budgets (`data_descriptors/attack_budgets.rs`).
- Reads: `defense(h)` lowers to `@defense.<layer>` in every scope, cached by `DefenseReads`. Impact scope: `EntityScope::resolve_input`. Brain: `resolve_brain_input` / `BrainInputRef`, runtime `BrainScope`, load check `check_guard_layer_reads`. Bool precedents: `impact.type` (`@impact.type.<name>`), `attackTurnHeld`.
- Effects: `grantLayer` path — SDK `sourceImpactEffect` → `bind_effect` → `BoundEffect` → `plan_effect` → consequential lane (exhaustive match) → `apply_effect`. Reaction twin in `crates/sim/src/grant/mod.rs`; strict-grant lists in `data_descriptors/{js,lua}/manifest.rs`.
- Chokepoint call for a tick: `apply_damage_with_context` with `DamagePayload { amount, impulse: ZERO, damage_type }` and `DamageContext { source_id, attacker, weapon, zone: None, point: None, producer: InTick }`. The type rides the payload, not the context. Stale attackers are filtered by `resolve_command_recipient`.
- `.damage` options drop the reaction on an unknown key; `defineDamageType` options reject load. Both stay reserved and empty here.

## Lifecycle and replication
- Death latch (`HealthComponent.death_handled`) clears nothing else today; recovery via `set_health_absolute` resets latch, credit and ledger.
- Carry: `CarriedField` is exhaustive only over `CarriedState` fields (`crates/netcode/src/seat.rs`); statuses never enter `CarriedState`, so there is nothing to mark skipped. Save format `LoadoutRecord` and wire `CarriedLoadoutWire` are untouched.
- Enemy overlay roster is capped, damage-driven and expiring: wrong home for persistent presence. `WireMeshAnimationState` reaches only animated meshes; `WirePlayerMovementState` only pawns. Ghost links added an entity component payload and bumped WIRE and SNAPSHOT; that is the precedent.
- No entity-attached condition-held presentation exists. `BillboardEmitter` toggles by tag. The audio core drops a looping request that carries an anchor (`crates/audio/src/audio_core.rs`, pinned by a spatial test), so status sound is a one-shot per tick, which anchored requests allow. Without attached presentation, replicated presence has no reader, so the brief ships both.
- Owner-private per-name slots enter the state-schema fingerprint (`compute_fingerprint`, `crates/netcode/src/state_slots.rs`); a fingerprint mismatch rejects whole state batches, health included (`networking.md` §Version gates). `player.status.<name>` therefore derives from replicated presence: a local-only slot (`ReplicationScope::None`, outside the fingerprint) filled on a client from its own pawn's presence through the host's name table, as `player.lifeState` is. It carries no time left.
- The tuning payload is JSON with its own epoch (`TUNING_PAYLOAD_EPOCH`); the fluid table and the life-state names each advanced it.
- `emitter()` is exported at runtime (`sdk/lib/index.ts`) and declared in Luau, but missing from `sdk/types/postretro.d.ts`.
- `mod_compatibility_digest` covers factions and trigger lanes only, which clients simulate against. A client uses status presence for presentation only, so it follows the fluid pattern instead: the host sends a name table and the client maps by name (`networking.md`: hash only what cannot be replicated). Digest gating would hold a client below participation over a cosmetic difference and demote every client on a host hot reload that adds a status.

## Instance cap
- Keyed by (status, applier). Co-op hordes: four players plus environment appliers per status, a few statuses per mod. 16 instances per entity covers every plausible combination with a fixed inline array, which keeps the stage allocation-free. Replacing the instance with the least time left drops the one contributing least.

## Pinned orderings
| Id | Scenario | Ordering | Expected |
|---|---|---|---|
| P1 | Instance's own tick on its expiry tick; its policy reapplies it (`"refresh"`/`"stack"`) | Tick lands → its policies apply → expiry judged on the time left they leave | Instance survives with phase unmoved; next tick one `tickMs` later |
| P2 | Weapon hit refreshes or stacks an instance on its expiry tick | Weapons (apply) → status stage (due tick, expiry) | Due tick lands at its old phase; instance continues |
| P4 | Fluid application vs due ticks on one tick | Status stage: this tick's fluid applications first, then due ticks and expiry | Re-entering lava on a lingering instance's last tick keeps it and its phase |
| P5 | Quantization | `tickMs` and `durationMs` each round up to whole fixed ticks; counts follow ticks | `burning` 500/3000 lands 6 ticks; `tickMs` 1–16 tick every fixed tick and 17 every second one at 60 Hz |
| P6 | Due tick on a target at health ≤ 0 before the latch | Status stage before sweep | Skipped: no chokepoint, no impact, no policy; the sweep clears the instance |
| P7 | Several instances on one entity due the same tick | Creation order | First lethal tick credits its applier |
| P8 | A tick's policy applies or clears statuses during the stage | Apply and clear take effect at once; each due tick re-checks its instance is live at its turn | Cleared instance does not land; a new instance does not tick this tick; dedupe counts it |
| P9 | Clear and due tick on one tick | Trigger-bound clear (before stage) vs player-event or frame-drain clear (after) | Before: no tick. After: tick landed |
| P10 | Application outside a tick (UI press, `wait` tail, frame-end policy, zero-tick frame) | Latched to the next fixed tick, applied before that tick's status stage | Two UI presses across tickless frames add one stack; phase starts at that tick |
| P11 | Lethal remote hit that waited on fire authorization | Post-tick remote-hit sweep latches → clears statuses → next tick's stage | No instance ticks after that latch |
| P12 | Lethal hit and immediate `setHealth` recovery on one tick | Recovery before sweep: latch never sets | Instances survive; a recovery after the latch starts with none |
| P13 | Script impact landing in a tick before its sweep, with or without a later in-tick hit | Evaluates right after its damage applies | Evaluates before that sweep either way |
| P31 | In-tick script damage (teleporter binding in Movement, trigger command in Triggers) followed by another hit on the same target that tick | The script impact evaluates right after its damage applies, as an in-tick hit evaluates right after its hit | The next hit's eligibility check, chokepoint, credit and damage memory see the target after the script impact's policies |
| P32 | Script impacts pending when a tick starts (UI press, `levelLoad`, player events after the previous tick) | Evaluated in landing order at tick start, before latched applications apply and before any stage reads health or statuses | A brain guard and a latched application on tick N see every earlier script impact's policies, with or without a hit earlier that tick |
| P30 | Script impact landing before the status stage | Already evaluated where it landed (P13, P31, P32) → fluid applications → due ticks | A due tick sees health and statuses after every earlier script impact's policies, with or without another hit that tick and whatever entity the stage visits first |
| P33 | Remote hit that waited on fire authorization applies a status after tick N's status stage | Counts on tick N, as an in-tick producer of N: applies at once, does not tick on N, dedupes with N's applications by that applier | First tick lands `tickMs` after N; an application latched to N+1 by the same applier is a new application |
| P14 | Script impact from a player event after tick N; tick N+1 in the same frame or the next | Evaluates at tick N+1's start, or at frame end if N+1 runs next frame | Evaluates before tick N+1's sweep, independent of frame packing |
| P15 | Script impact queued by `levelLoad` | Queued at install; level unload clears the queue | Evaluates at the reveal frame's first evaluation point against that level's policies; an abandoned load drops it |
| P16 | Frame-end script-impact evaluation | Health republish → evaluate in landing order → overlay sink → removal pass, in both the frame loop and the headless driver (`observability/driver.rs`), which has its own removal pass | Policies read post-damage `player.health`; enemy enters damaged-enemy roster |
| P17 | Remote pawn with no command this tick; respawn | Carried fluid persists until the next resolve; respawn resets it with immersion | Stalled pawn in lava keeps burning; respawned pawn out of lava takes none |
| P18 | 17th instance; ties; until-cleared | Least time left goes; ties → oldest; until-cleared ranks most | New instance always enters |
| P19 | Hot reload changes `tickMs`, `durationMs`, `maxStacks` | Next due tick kept; later spacing uses new `tickMs`; time left unchanged until next application; stacks clamp next tick | No tick skipped or doubled at reload |
| P20 | Host reload reorders statuses; name table arrives after a snapshot | Presence mapped by the last installed table | Brief cosmetic mis-map, accepted; no revision stamp |
| P21 | Status live less than one snapshot interval | Presence sampled at serialization | May never show on a client |
| P22 | Client joins mid-burn | Presence before name table install | Shows nothing, then shows from first snapshot after install |
| P23 | Applier rejoins (new pawn id) and reapplies | New id → new applier | Old instance credits its source id only; new instance is separate |
| P25 | Clear lands while an application of that kind to that target is latched | Clear removes live instances and discards the latched application, as a trap stop discards a pending start | Apply then clear: no instance on the next tick. Clear then apply: one instance |
| P26 | Latched application meets a boundary before it applies | Judged when made and again when it applies | Dropped if the target is stale or despawned, at health ≤ 0 at either moment, respawned or rejoined since, or its kind was removed by hot reload; level change and same-map restart discard every latched application |
| P27 | Application from the player-event pass or the post-player-event evaluation after tick N | After tick N's status stage: latched as an off-tick application | Counts on N+1 for dedupe and phase |
| P28 | tickSound cadence | Presence appears → first one-shot one `tickMs` later → every `tickMs` while presence holds; presence clearing stops it; presence reappearing restarts the count | On the host the sound coincides with the first instance's damage ticks; a client's cadence follows its own presence, not host ticks |
| P29 | Policy applies a status from an impact with no damager | Applier = that impact's source id | Trap damage → `script.applyDamage`; scalding tick → `fluid.lava`; ticks credit that id and no player |
| P24 | `damage: 0` (marker) status tick | Never enters the chokepoint | No impact, policy, layer restamp or contributor; instance stays live for reads and presence |


## Roadmap updates at promotion
- E12: add an entity-attached looping sound (the audio core refuses anchored loops today); status presentation can adopt it later.
- E16 Combat Feedback & Economy: replace "DoT / environmental death policy" with this brief; mark `onDamage` per-attack aggregate demand-gated, pointing shot-id threading to `drafts/E10--stealth-opener`.
- E16 Damage & Defenses: "status effects / DoT" points to this brief.
- Fix the dual-wield line's citation of an "activation-trigger fork" that `weapon-model.md` §9 does not contain.

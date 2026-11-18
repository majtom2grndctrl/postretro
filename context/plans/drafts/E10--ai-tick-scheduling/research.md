# E10--ai-tick-scheduling — research

Read at 3366eeeb7. Four bounded research reports from the draft session, kept as derivation. Where a report's recommendation differs from the brief, the brief's Decisions win: the owner chose id 51 for perception, a 2 Hz coarse full tick, merged cadence and arena scope, and build-at-target-scale over measure-first.

## Stress baseline

Roadmap Epic 10, AI at scale (owner request 2026-11-10), from the E10 combat-movement stress row (`crates/ai/src/replan_stress_tests.rs`, results in `done/E10--enemy-combat-movement/plan.md` §Review loop). Windows desktop, median AI tick for a converged wave: 1.4 ms at 64 strafers, 3.1 ms at 64 chasers; 4.0 ms and 8.8 ms at 128. Cost grows superlinearly; AI runs single-threaded on the host. `perf-ai-tick-cost` measured 64 agents at about 5.8 ms `sim_ai` and 2.5 ms steering on `stress-env-volumes`, spiralling at 128.

## Why id 51, and its limits

- id 51 `CellResidencySet`: for each camera cell, every cell within portal-path lead L plus each such cell's visible set (runtime portal walk sampled from eye points, dilated one portal hop), tagged with the smallest lead at which it becomes mandatory; teleporter arrival cells join as zero-length routes (`rendering_pipeline.md` §Cluster SH residency). It is sampled, not conservative, but it is the same set that keeps a visible cell's lightmap resident.
- Absent when the level cannot stream (loader portal test failed). Showcase: 65.5 MiB.
- Doors are not applied to id 51; closed doors only make it wider than needed.
- Rejected inputs: id 46 `perceivable` (portal connectivity only; true across a connected map); a new sightline PVS bake (compiler work, owner chose id 51); a runtime path radius (cannot tell a far vista from a far hidden room).


## Ordering pins

Acceptance rows cite these ids.

| id | scenario | ordering | expected outcome |
|---|---|---|---|
| P1 | Enemy holds an attack turn and skips its AI on a tick (dormant interval, or any tick it is not scheduled) | Arbitration on the skipped tick runs without the holder's outcome | The held turn still counts against its pool's capacity on every skipped tick; it is not reported departed and starts no refresh; no other enemy is granted that capacity until the holder releases it; a queued grant made while it is skipped opens on its next run |
| P2 | Enemy leaves the AI set (picked up, quiescent, queued despawn) versus enemy skipped by the scheduler | Both miss one or more AI ticks | Leaving the AI set still returns the enemy holding nothing, requesting nothing, with no wait clock; only a scheduler skip keeps turns |
| P3 | Dormant NPC promoted k ticks into its dormant interval (became perceivable, damaged, gained target, aggro opened) | Promotion tick versus the k ticks it has not run | Its timers advance by k ticks; it moves one ordinary tick on the promotion tick; no catch-up movement |
| P4 | Dormant enemy runs its whole tick at 2 Hz and decides every two seconds; a perceived idle NPC checks acquisition and decides at 15 Hz, noticing through its meter | Run versus decide phase | A dormant enemy decides on every fourth dormant run, whatever its phase or distance band; the tactical range applies only to engaged enemies |
| P5 | NPC whose spawn tick is offset from its decide phase, in each state (target inside the tactical range, target beyond the tactical range, perceived idle, dormant) | Decide interval phase versus step phase | Each decides on its state's interval rounded to its own decide ticks, for every spawn offset; never "never due" |
| P6 | Dormant NPC takes damage on tick N during or after the AI stage (another NPC's attack, host hitscan, trap shot, a remote hit awaiting that tick's fire authorization) | Tier choice runs before the AI stage | Moved up by tick N+1's tier choice and runs at its new tier on N+1; damage before the AI stage (projectile impacts, remote hits ready at the input boundary) moves it up on N |
| P7 | Player teleports or respawns into a cell whose set covers dormant NPCs, on tick N | Movement (teleport) runs before the tier choice | Those NPCs are perceived on tick N and run at their perceived tier from N |
| P8 | Dormant NPC at the fastest authored `moveSpeed` walks toward a player's visible cells | Dormant step moves it 0.5 s of distance in one tick; scheduler reads its cell next tick | It is promoted while still in the AI lead, never first observed already inside a cell visible from a player's cell |
| P9 | Enemy changes cell without steering moving it: spawned, slot reused the tick another enemy despawned, script-set position, mover push, knockback | The scheduler reads a cached enemy cell; non-steering moves can leave it stale | Its cell is located before the next scheduler read; a re-used slot inherits no cell, hold or dormant phase from its previous occupant |
| P10 (superseded by P29) | NPC stands on a moving lift or in a closing door's sweep at 15 Hz or 2 Hz | Mover advances every tick; carry and displacement run every tick apart from the NPC's sweep, which runs at its tier rate | NPC ends on the same side of every mover surface as at 60 Hz: carried exactly once per tick, pushed or blocking, never embedded or dropped through |
| P11 | NPC quantity advanced both in a whole-runtime prelude every tick and in the enemy's scaled step (weapon resources, turn clock) | Prelude every tick versus dormant step | Each quantity advances exactly once per elapsed tick |
| P13 | Dormant NPC owed a replan contends with higher-tier NPCs for the shared per-tick replan budget | Wait converted to time grows a whole step per run | Replan tier order holds for every NPC with the blocked-route wait bound; within a replan tier, a paced NPC (leap in flight) goes first, then engaged before idle, idle before dormant |
| P14 | Player arrives in the largest arena; every enemy there promotes on one tick | N promotions, N replans owed, on one tick | Fixed step does not spiral; replan backlog drains at the budget rate |
| P15 | Enemies spawn interleaved or at a regular stride (archetype spawns a companion entity, wave spawns interleave) | Pool from a mixing hash of each NPC's identity | Busiest phase at most mean + 2√mean + 1, no phase empty at 64 or more; exact ±1 balance is not possible from identity alone (two spawners both count from 0; ordinals stepping by 4) |
| P17 | Tick with no pawn in play or down: before first spawn, every seat loading or reconnecting, headless without a pawn | Perceiver union is empty | Every NPC is perceived; a dead player's pawn still counts |
| P18 | Converted window shorter than one dormant step (LOS grace 50 ms, combat-slot hold) evaluated at 2 Hz | Window expiry inside a step | Expires on the first step at or past its duration, never on an earlier step |
| P19 (retired: 30 Hz cap dropped) | Clip-timed jab entered at each 30 Hz step phase | Entry on a decide tick; moment check per 30 Hz step | First damage lands at the clip moment, rounded to the first 30 Hz step at or past it, for every phase and frame split |
| P20 | Unperceived enemy has line of sight to a player whose set does not include the enemy's cell (id 51 is sampled) | Perception picks the rate; acquisition picks the target | It notices that player through its meter on its dormant checks, a first sighting gaining nothing, then promotes |
| P21 | Dormant NPC acquires a target during its own dormant run | AI (acquisition) runs before steering in the same tick | That tick's steering moves it one ordinary tick; its timers are not credited again |
| P22 (retired: 30 Hz cap dropped) | NPC spawned, teleported, script-set or slot-reused with fewer than two steps since | Tick pose interpolates its last two steps | Its tick pose is its new pose from that tick; no draw or hit blends from its old pose or a previous occupant's |
| P23 (retired: 30 Hz cap dropped) | NPC changes tier mid step (dormant to any, idle to engaged, engaged to close, and moving down) | Tick pose spans steps of different lengths | Tick pose moves continuously and never backwards; hits and draw agree on every tick of the change |
| P24 (retired: 30 Hz cap dropped) | NPC steps on tick N; a projectile contacts it (stage 5) and a host hitscan hits it (stage 9) on N | Tick pose read before and after the NPC's step in one tick | Both read the same tick pose for N; the step taken on N enters the tick pose from N+1 |
| P25 (retired: 30 Hz cap dropped) | NPC's last two steps have different grounds (mover then world, world then mover, or two movers) | Tick pose interpolates in the mover frame of one step | Drawn and hit pose move continuously on every tick between the steps; no frame switch shows as a jump |
| P26 (retired: 30 Hz cap dropped) | Catch-up frame runs several ticks and crosses one or more cadence edges while NPCs at 30, 15 and 2 Hz step inside it | Steps per tick versus one post-loop replicate per frame | Every NPC step in the frame reaches each client stamped with its own tick; none is overwritten by a later step before sending |
| P27 | Engaged NPC's queued row requests on a decide tick; arbitration grants it that tick | Grant on tick T; AI runs on T+1..T+3 without evaluating guards; next decide on T+4 | The row opens at the NPC's first decide after the grant, for every decide phase; the turn is never held across a decide that leaves its row closed; other waiters on the pool are granted as at 60 Hz |
| P28 | Engaged NPC's target distance crosses its tactical range outward and back inward within one decide interval, and repeatedly | Band read at each decide, with a 10 m margin | It switches between routing and slot choice at most once per crossing of the margin; a slot held inside is not dropped by a target stepping just past the band and back; a far NPC never claims a slot |
| P29 | Dormant NPC on a moving lift or in a door's sweep; its 2 Hz step falls on a tick the mover moves, and the mover leaves, docks or reverses between steps | Carry and displacement every tick; the 2 Hz sweep, ground ref and release on step ticks only | Carried exactly once per tick, step ticks included; ends on the same side of every mover surface as at 60 Hz; a mover that leaves it between steps releases it no later than its next step, with release velocity applied once; never embedded or dropped through |
| P30 | Reposition entered beyond its tactical range; target comes 10 m inside it | Entry bounds (arc bearing, entry position, `minTravel`) fixed at entry; slot choice starts inside | Its arc and `minTravel` are measured from where it begins choosing slots, not from its far entry |
| P31 | Idle NPC's notice meter across acquisition checks: sight continuous, sight lost and regained, two players with one visible, a candidate dropping out of the bounded set, a reload changing `fillMs` mid-fill | Meter advanced on each acquisition check, per candidate, by the time since the previous check when both saw the candidate | Acquires on the first check at or past the meter reaching 1, never earlier; a first sighting gains nothing; drain applies while the candidate fails candidacy or line of sight; each player's meter is independent; a candidate evicted from the set restarts at zero; a reload keeps current values and applies the new rates from the next check |
| P32 | A meter's candidate leaves the scan: despawns (a killed hostile NPC's queued despawn, a client disconnect), becomes inventory-owned, or stops being a pawn | Drain runs only for candidates the check visits; a departed member is never visited | On the first check after it departs, its slot frees and its value is gone; a slot is never held by a candidate the scan cannot visit, so eight departed members never block an NPC from noticing a ninth candidate |
| P33 | More than 8 candidates in reach: nearer hostile NPCs behind a wall fail line of sight while a farther player in view passes; or 8 members are draining when a new candidate passes | Bound applied to the nearest few, versus candidacy and line of sight tested per member | A candidate that passes candidacy and line of sight takes a slot whenever fewer than 8 passing candidates are nearer; it evicts a failing member before any passing one, the lowest value first; nearer candidates that fail never keep a passing one unmetered |
| P34 | A player whose meter on an NPC reads above zero dies, or dies in view and keeps filling, then respawns on the same pawn id in that NPC's sight; or the NPC's aggro gate closes with a meter at 0.9 and later reopens | Meters keyed by pawn identity; respawn keeps the identity; a closed gate runs no selection | The respawned pawn's meter restarts at zero on its respawn tick; a closed gate empties every meter; noticing takes a full fill in both cases |
| P35 | Decide tick whose acquisition check raises a meter past a guard's threshold, or to 1 | Acquisition check versus guard evaluation in the same decide | Guards read the value this check produced; on the decide a meter reaches 1, guards see `hasTarget` and `brain.awareness` 1 together; no guard reads the previous check's value |
| P36 | Two candidates' meters reach 1 on one check | Meters advanced, then ranking among those at 1 | The NPC acquires by today's ranking among those at 1; acquiring clears the other's meter |
| P37 | `fillMs` or `drainMs` of 0, or above 0 and below one decide interval | Rise and drain divided by the authored duration | Each acts as exactly one interval: `fillMs` 0 and `fillMs` 30 notice on the same check; no division by zero; 0 is accepted at load |
| P38 | Hot reload sets an archetype's `alwaysActive` true while its NPCs are dormant mid step, or false while they are unseen; some placements set `always_active` | Reload commit versus the next tick's tier choice | NPCs without a placement key move up on the first tier choice after the commit, with timers credited as any move up; set false, they move down only after the hold; a placement's key is never overridden by a reload; the NPC stores which of three it has (from the archetype, forced on, forced off), since `enabled_on_spawn` records no such state |
| P39 | Unseen `alwaysActive` NPC walking toward a trigger volume, already following a patrol route, versus starting its walk on a guard | Route following every tick; guards on decide ticks | On a route it crosses on the same tick as the same NPC perceived; a guard-started walk crosses no later than one decide interval after its 60 Hz tick; neither crosses later than a perceived idle NPC would |
| P41 | A meter emptied or restarted between two checks that both see its candidate: aggro gate closed then reopened (minutes later, or within one tick), a pawn respawning in view after its body was sighted, an evicted candidate re-admitted; the NPC had a target on its last check before the gate closed | Credit for "this check and the previous one" versus the reset | An emptied or restarted meter treats its next sighting as a first sighting, gaining nothing; the gate empties meters on the close itself, even if it reopens before the NPC next runs; the first check after a gate reopens counts as following a check with no target; noticing takes a full fill from the first check that sees the candidate after the reset |
| P43 | A hit from a candidate that holds no meter slot while 8 nearer passing candidates fill the set | Bound-8 admission by distance versus a hit setting its attacker's meter | The hit always gives its attacker a slot at 1, evicting the lowest-valued member; the NPC acquires them once they pass candidacy and line of sight |
| P44 | Dormant NPC that sighted a candidate on its last dormant check moves up mid step; the candidate stays in view | Move-up credit to timers versus meter credit between checks | The meter gains only at checks, by the time between two sighted checks; moving up adds nothing to it; the first idle check after moving up gains the time since the last dormant check, once |
| P47 | A player respawns while NPCs target them: one mid-burst on them, one holding an attack turn or a combat slot on them, one waiting in line for a turn on them, one retaliating against them | Respawn at tick start, before the AI stage, versus a target held until the next check | On the respawn tick each such NPC treats the pawn as a target that left the world: its turns on the pawn end and record their refresh, a burst aimed at it stops, its combat slot frees, its wait clock on the pawn clears, and its retaliation target drops; noticing the pawn again starts a new wait; today's no-target path, which keeps an executing turn and the wait clock, is not enough; the drop counts as a check with no target, so the pawn is never re-acquired at once and is noticed through a full fill |
| P55 | An NPC that had a target on its previous check finds several candidates passing on this one, with different meters and one a recent attacker | Instant re-acquire versus meter values | It picks by today's ranking among those passing, meters ignored: a retaliation-ranked attacker first, then the nearest |
| P53 | A resting NPC whose activity does not engage selects a player; the next check misses them (one check, or longer than `drainMs`) | Selection kept only on its check versus losing a target | The missing check loses the target and every meter drains from where it stands, the lost one from 1; the next sighting re-selects at once while the meter is near 1; past `drainMs` it needs a full fill |
| P54 | A respawn drops the target while teammates are in view: one hit the NPC during the fight, one did not | Clear-on-acquire and hit-sets-1 versus the respawn drop | The hitter's meter is 1 and they are acquired at once; the other's meter was cleared at acquisition and needs a full fill; the respawned player's meter is zero |
| P57 | A hit sets its attacker's meter to 1 on a tick between checks, up to a dormant interval after the last check | Drain interval versus the hit's tick | The meter drains from the hit's tick, never from the previous check |

P12 and P16 retired with defense-layer recharge and `thinkIntervalMs`.

---

## Report: Sim LOD inventory A: per-tick work for one brain-bearing enemy

Read-only research. Sources: `context/lib/entity_model.md` §5 (Update Order), §7c; `crates/sim/src/sim/mod.rs::simulate_started_tick_with_presentation_aim`; `crates/ai/src/{lib,compute,apply,targeting,engine_floor,perception,clip_clock,heading,launch,engaged_motion,combat_slots,weapon_controller}.rs`; `crates/ai/src/attack_turns/{mod,lifecycle}.rs`; `crates/sim/src/agent_steering.rs`; `crates/sim/src/scripting_systems/health.rs`; `crates/sim/src/sim/defense_layers.rs`; `crates/sim/src/impact_effects.rs`; `crates/postretro/src/frame_loop/mod.rs`.
Cited by symbol, not line. "NV" = not verified (not opened).

Production wiring: the frame loop calls `sim::simulate_started_tick_with_presentation_aim` once per fixed tick (`TICK_DURATION` = 16 667 µs, `crates/foundation/src/lib.rs`), passing `postretro_ai::tick_runner!(&mut app.ai_runtime)`, which expands to `postretro_ai::run_ai_tick_with_host`. `simulate_tick` is the headless/test wrapper over the same body.

### Key time-handling legend

- **dt-param**: takes `tick_dt` / `dt_ms` argument and scales by it. Calling once every N ticks with dt×N is *arithmetically* fine, though stability/quantization may differ.
- **tick-count**: counts calls (`+= 1`, `saturating_sub(1)`, `% stride`). Calling every N ticks slows it by N×.
- **const-dt**: reads `TICK_DURATION` / host tick itself; ignores passed dt.
- **anim_time**: presentation/wall animation clock (frame-scaled), not sim time.

Key global gotcha for any per-enemy skip: `attack_turns::admit` (`crates/ai/src/attack_turns/lifecycle.rs`) **clears an enemy's attack turns if it was not admitted on the previous AI tick** (`seen_tick + 1 != tick`). Skipping a brain for even one tick drops its turns/request/wait seniority. Any LOD that skips the AI set for a brain must either keep it in the snapshot set or change this rule.

### Stage-by-stage inventory (tick order)

Stage numbers follow entity_model §5.

### 0. Tick start (frame loop, before sim body)

| Item | Symbol / path | State advanced | Time | Stride-skipped? | Conf |
|---|---|---|---|---|---|
| Transform snapshot prev←current | `sim::begin_tick` → `registry.snapshot_transforms()` (`crates/sim/src/sim/tick_start.rs`) | every entity's previous transform (interp) | none (copy) | no | high |
| Defense-layer recharge | `defense_layers::recharge_defense_layers` (`crates/sim/src/sim/defense_layers.rs`) | layer refill for entities carrying defense layers (enemies only if authored) | **const-dt** (`TICK_DURATION`) + `registry.host_tick()` / `last_hit_tick` | no | high (only for enemies with layers; med on whether shipped enemies have them) |
| Deferred impact effects aging | `impact_effects::tick_deferred_effects` (`crates/sim/src/impact_effects.rs`), first thing in the sim body | `remaining_us` of pending deferred effects (e.g. delayed despawn, death anim) on the enemy | dt-param (`tick_micros(tick_dt)`) | no | high |

### 1. Kinematic movers

| Item | Symbol | State | Time | Skipped? | Conf |
|---|---|---|---|---|---|
| Host pin pretest over living agents | `kinematic_mover::run_mover_pin_pretest` | mover yields/pins involving agents | dt-param | no | med (NV internals; doc §5 row 1 says "over player pawns and living agents") |

### 2–4. Player movement / triggers / touchables
No per-enemy work, except triggers can run commands that flip an enemy's `aggro_armed` (`setNpcState`, `crates/sim/src/scripting/reactions/npc_state.rs`) or spawn enemies. Conf high.

### 5. Authoritative projectile flight

| Item | Symbol | State | Time | Skipped? | Conf |
|---|---|---|---|---|---|
| Enemy-owned projectiles fly / impact | `projectile_stage::advance` (`crates/sim/src/sim/projectile_stage.rs`) | projectile entities (owner-agnostic; enemy launches are ordinary projectile entities) | dt-param (+`anim_time` passed, NV for what) | no — projectiles are separate entities, not brain work | med (NV internals) |
| Spawn-tick grace close | `projectile_stage::finish_spawn_tick` after weapons | newly launched enemy projectiles (`enemy_projectile_spawns`) | none | no | high |
| Faction sentiment decay | `decay_faction_sentiment` (sim mod.rs) | global sentiment overlay (not per enemy) | dt-param | n/a | high |

### 6. AI brain tick — `run_ai_tick_with_host` (`crates/ai/src/lib.rs`)

Prelude (whole-runtime, every tick):

| Item | Symbol | State | Time | Skipped? | Conf |
|---|---|---|---|---|---|
| Attack-pool request drain | `attack_pools::apply_attack_pool_requests` (sim) | pool membership | none | no | high |
| Turn clocks | `AttackTurnRuntime::begin_tick` (`attack_turns/mod.rs`) | `tick` (+1), `now_ms` (+dt_ms), refresh-ledger prune | both: `tick` tick-count, `now_ms` dt-param | no | high |
| Pool / program / weapon sync | `turns.sync_pools`, `programs.sync`, `programs.warn_unclipped_moments`, `weapons.sync` | bound guard programs (derived), weapon controllers | none | no | high |
| AI weapon resource advance | `AiWeaponControllers::begin_tick(dt_ms)` → `advance_authoritative_weapon_resource` (`weapon_controller.rs`) | each enemy weapon's resources (ammo/charge regen) | dt-param | no | high |
| Snapshot + admit | snapshot loop in `run_ai_tick_with_host`; `attack_turns::admit`; `turns.remap_brain` | clones every non-quiescent, non-held `BrainComponent`; clears turns if not seen last tick | **tick-count** (`seen_tick`) | no — every admitted brain | high |
| Previous line rebuild | `previous.rebuild` | turn-fact line for guards | none | no | high |

Per enemy, `compute::evaluate` (`crates/ai/src/compute.rs`), in order:

| # | Item | Symbol | State | Time | Stride-skipped? | Conf |
|---|---|---|---|---|---|---|
| a | Reseat / path validation | `programs.take_reseat`, `brain.has_valid_active_path`, `reseat_to_initial` | active path | none | no | high |
| b | Attacker ledger aging | `BrainComponent::age_recent_attackers(dt_ms)` | `recent_attackers[*].time_since_damage_ms` | dt-param | no | high |
| c | Home distance | `distance_xz(position, home_anchor)` | fact only | none | no | high |
| d | **Target offer scan** (aggro armed only) | `targeting::target_offers` → `target_candidates` | none persisted; walks every player pawn + every brain-bearing peer, computes hostility & nearest | none | **NO — runs every tick**, both retained and unretained branches, because its `nearest` prices the stride. O(enemies × candidates) per tick. (Retained branch: the scan runs only on due ticks; `target_candidate` lookup of the retained id runs every tick.) | high |
| e | Stride gate | `targeting::acquisition_due` → `engine_floor::think_stride_for_distance` | reads `think_stride_counter` | tick-count | — (this *is* the stride) | high |
| f | Fresh acquisition: candidacy filter + candidate LOS raycasts + ranking + retaliation + hysteresis | `targeting::select_target_with_attacker_ledger`, `perception::enemy_eye`, `perception::raw_target_perception` (→ `collision::line_of_sight`), `engine_floor::is_meaningfully_closer` | selected target | dt_ms passed to retaliation scoring | **YES — only on due ticks.** Non-due unretained → no target; non-due retained → keep retained | high |
| g | Named attack cooldowns | loop over `attack_cooldown_remaining_ms` | per-attack remaining ms | dt-param | no | high |
| h | Damage recency | `brain.time_since_damage_ms += dt_ms` | `time_since_damage_ms` | dt-param | no | high |
| i | Activity clocks | `BrainComponent::tick_activity_timers(dt_ms)` (skipped on the entry-pending tick) | `time_in_activity_ms[depth]` | dt-param | no | high |
| j | Clip clock advance | `clip_clock::begin_tick` | `clip_clock.time/previous_time`, foreign-switch latch close, lazy clip facts + loop phase | dt-param × mesh `animation.rate` (game time, not anim clock). Moment test is a window `(previous, time]`, so a larger dt still catches a moment, but the fire tick quantizes | no — doc §7c: "Decision cadence work keeps the accumulator and the moment check on every tick" | high |
| k | `animationDone` | `clip_clock::animation_done`, `entered_animation_done` | fact | derived from j | no | high |
| l | Stride counter | `brain.think_stride_counter.wrapping_add(1)` | counter | tick-count | no (always advances) | high |
| m | Selected-target perception | `perception::perceive_target` → `collision::line_of_sight` + `debounce_los` | `los_grace` map (host-only side table in `AiRuntime`) | **tick-count**: `LOS_GRACE_TICKS = 3` | **NO** — one static-world LOS ray per tick per enemy *with a selected target* (reuses the fresh-acquisition ray on due ticks) | high |
| n | Sight memory | `last_known_target_pos`, `time_since_target_visible += dt_ms` | those fields | dt-param | no | high |
| o | Target hostility fact | `targeting::is_hostile` | fact | none | no | high |
| p | `targetReachable` route query | `nav::find_path_from` | `brain.target_reachable` | none | **YES** — only `if evaluate_acquisition` (cached otherwise; skipped while riding / between docks) | high |
| q | Aggro gate closed branch | `reseat_to_initial`, `SteeringIntent::Clear` | path | none | n/a | high |
| r | Turn facts into guard scope | `TurnFactReader::read` via `scope.refresh_attack_turns` | guard scope | none | no | high |
| s | Guard fact refresh (twice) | `BrainScope::refresh` with `BrainFacts` (incl. `acquisition_due`) | bound scope | none | no | high |
| t | Action resolve for cooldown/standoff | `graph_eval::action_for_path`, `standoff_distance_for_action` | — | none | no | high |
| u | **Behavior-graph transitions** | `graph_eval::select_transition_path` | active path, entry pending flags | none (guards read time facts) | **NO — every armed tick** (invariant "Guards evaluate every tick") | high |
| v | Motion resolve + position goal | `graph_eval::resolved_motion`, `steering::position_goal_steering` (patrol cursor/home) | `patrol_cursor/direction` | none | no | high (patrol internals med) |
| w | Retention bookkeeping | `acquired_target`, `retaliation_acquired_target` | those fields | none | no | high |
| x | Engagement + heading/facing | `graph_eval::engages_path`; `heading::resolve` → `launch::advance`, `facing::slewed_yaw_toward(FACING_TURN_RATE * tick_dt)`, travel variant | `brain.launch.{impact_yaw, air_ticks}`, predicted yaw, `presented_travel_variant` (NV which) | slew dt-param; **`launch.air_ticks` tick-count** vs `FALL_DELAY_TICKS` | no | high |
| y | Clip moment settle | `clip_clock::settle` | `moment_open`, `moment_timed`, drawn-state sync | none (reads j's window) | no | high |
| z | Attack proposal gates | inline in `compute::evaluate`: cooldown, range, alive (`selected_target_alive`), `perception::fire_gate`, facing tolerance, `bound.resolved_projectile_attack` | `PendingAttack` (immutable) | none | **NO** (doc: attack-in-range/cooldown "run every tick regardless") | high |
| aa | Entry emission | `take_entry_event_pending` → `EnteredActivity` | — | none | no | high |
| bb | Engaged-motion ring | `engaged_motion::plan` | `brain.engaged_motion` (strafe direction / reposition entry memory) | none (strafe re-aims 45° from current bearing each tick) | no | high |

Batch stages after compute (all outcomes):

| Item | Symbol | State | Time | Skipped? | Conf |
|---|---|---|---|---|---|
| Combat slots | `combat_slots::resolve_combat_slots` → `combat_positioning::select_combat_positions_batch` (sim; NV internals), `engaged_motion::settle` | `brain.combat_slot`, `combat_slot_hold_ticks` | **tick-count** (`COMBAT_SLOT_HOLD_TICKS = 8`) | no; early-out when no outcome is engaged; scoring uses nav + collision (path-length weight) | high (scoring cost NV) |
| Attack-turn arbitration | `attack_turns::arbitrate_attack_turns` (`lifecycle.rs`, `arbitration.rs`) | turns, grants, waits (`since_tick`), reservations, refresh ledger, weapon-execution cancels | waits: tick-count (`since_tick`); refresh: `now_ms` dt-param | no | high |

Per enemy, `apply::apply_outcomes` (`crates/ai/src/apply.rs`):

| Item | Symbol | State | Time | Skipped? | Conf |
|---|---|---|---|---|---|
| Publish brain | `registry.set_component(brain.clone())` | whole `BrainComponent` (clone every tick) | none | no | high |
| Entry emission | `AiEmission { cue: Entered, address: on_enter }` + `entity_emitter` | → frame-end reaction drain + activity entry sound | none | no | high |
| Locomotion intent | `agent_steering::path_state` → `LocomotionIntent::from_velocity` | — | none | no | high |
| Steering command | `agent_steering::set_destination` / `clear_destination` (Hold clears only on entry) | `AgentComponent.destination` (plan untouched) | none | no | high |
| Facing write | `facing::slewed_yaw_toward(FACING_TURN_RATE * tick_dt)` → `Transform.rotation` | yaw | dt-param | no | high |
| Contact attack | `host.apply_damage` (Health chokepoint), `commit_attack_fire` | target HP, cooldown, `attacks_fired_in_activity`, emissions | none | no | high |
| Weapon (projectile) attack | `AiWeaponControllers::advance_actor(…, dt_ms)` | weapon execution cursor (burst steps), projectile spawns, fire sounds | **mixed**: `self.tick` / `full_ticks` tick-count for step timing, dt_ms for resources | no | med |
| Animation switch | `apply::should_switch_animation`, `launch::airborne_clip`, `clip_clock::redrawn`, travel variant resolve | `MeshComponent.animation` state request, `launch.airborne_presented` | none | no | high |

### 7. Host camera callback / traps
`post_movement` (player only); `traps::run_trap_stage` (trap entities, not enemies). No per-enemy work. Conf high.

### 8. Agent steering — `agent_steering::tick_with_mover_pins` → `tick_impl` (`crates/sim/src/agent_steering.rs`)

Runs for **every non-quiescent, non-held Agent**, brain or not, independent of AI.

| Item | Symbol | State | Time | Stride-skipped? | Conf |
|---|---|---|---|---|---|
| Pin escape planning | `pin_escape::plan_escapes`, `pin_escape::escape_velocity` | escape walk for pinned agents | dt-param | no | med |
| Agent snapshot | `snapshot` vec (positions, radii) | — | — | no | high |
| Replan admission | `admit_replans` — 3 tiers (gate change, drift > `REPLAN_DEST_THRESHOLD` 0.5, staleness) under `REPLAN_BUDGET_PER_TICK = 4` (global, not per agent) | which agents path-find this tick | **tick-count** (`replan_cooldown_ticks`, `REPLAN_STALENESS_TICKS = 30`, `drift_wait_ticks`) | no — own budget, unrelated to think stride | high |
| Knockback decay | `movement::knockback::decay(dt)` | `knockback_velocity`, `velocity` | dt-param | no | high |
| Gate generation settle | `gate_change_owes_replan` | `gate_generation` | none | no | high |
| Lift ride | `mover_link::advance` (`RideWorld{dt}`) → `move_without_path` | ride state | dt-param | no | med |
| **Idle (no destination)** | `collide_and_slide` with zero steer, gravity + knockback, `report_static_overlap` | `velocity`, `ground`, `Transform.position` | dt-param | no — idle agents still gravity/ground-stick every tick | high |
| Path follow + separation + stuck/unstick | `tick_impl` body; `separation` (O(N) over snapshot per moving agent ⇒ O(N²)); `STUCK_TICKS_THRESHOLD = 20`, `UNSTICK_WINDOW = 10`; `collide_and_slide` | path cursor, `steer_velocity`, `stuck_ticks`, position | move: dt-param; stuck/unstick: tick-count | no | high (separation cost med) |
| Corpses | `corpse::tick_corpses(dt)` | quiescent bodies ride movers/fall | dt-param | n/a (dead) | med |
| Corpse yields | `mover_tick_states.yield_agent_as_corpse` | mover state | none | no | high |

Pose sub-stage (sim mod.rs), every tick:

| Item | Symbol | State | Time | Skipped? | Conf |
|---|---|---|---|---|---|
| Locomotion playback rate | `update_brain_animation_playback_rates` | `MeshAnimation` rate rebase (only on change) | **anim_time** for rebase | no | high |
| Pose inputs | `update_presentation_pose_inputs` → `update_pose_inputs` | mesh heading/aim-at-target fields for every animated mesh (reads `brain.acquired_target`) | none | no | high |
| Foot ground probes | `update_foot_ground_probes` → `write_foot_ground_probes` / `probe_foot` | foot IK contacts (raycasts per foot-probe candidate) | anim_time (pose sampling) | no | high (that enemies are candidates depends on model: `is_foot_probe_candidate`) |
| Auto-close timers / mover blocking pass | `auto_close_timers.tick`, `kinematic_mover::run_mover_blocking_pass` | judges agents at settled positions; may call `on_impact` | dt-param | no | med |

### 9–10. Weapons / projectile launch finalization
Player weapons only (`weapon_stage::*`) plus `finish_spawn_tick` closing enemy projectile spawn grace. Conf high.

### 11. Death sweep
`run_death_sweep` → `scripting_systems::health::sweep_deaths` (`health.rs`): iterates **every Health component** each tick, latches deaths at ≤0 HP, cancels weapon activations, frozen credit. No time. Not stride-gated. Death animation / despawn delay are authored impact policies → deferred effects (stage 0 row). Conf high.

### 12. Assist accumulator
`assist_state.run_host_pass` — players only (NV for whether it reads enemies; doc says pair values over players). Conf med.

### After the sim body (frame loop, per fixed tick)

| Item | Symbol | Time | Conf |
|---|---|---|---|
| AI emissions → reactions/sounds | `TickEvents::ai` drained post-tick; `on_enter` addresses and attack events dispatch at frame-end drain; `publish_observer_weapon_cues` | per tick list | med |
| Script `wait` tails / timed reactions | reaction scheduler `evaluate` once per fixed tick ("Each call is one fixed tick of the scheduler's tick count") | **tick-count** (global, not per enemy) | med |
| Runtime spawn registration / light enrollment | `host_register_descriptor_entities_after_fixed_sim_tick`, dynamic-light enroll after tick | none | med |
| Emitter / light / fog bridges | scripting bridges walk configuration components (entity_model §5 last para) — per **frame**, not per tick | frame dt | low (NV) |
| Mesh animation sampling | `scripting_systems::mesh_anim` — per-instance sample times on `anim_time` (presentation clock) | anim_time | med |

### (1) Idle / un-alerted enemy in an empty arena

Two cases.

**Aggro armed (default), in its initial `hold`/idle activity, no hostile in range.** Per tick it still runs: stage 0 transform snapshot, deferred-effect aging (if any), defense-layer recharge (if any); AI prelude snapshot clone + `admit`; ledger aging; **`target_offers` scan over every player and peer brain** (always, to price the stride); stride counter; on a due tick candidacy filter + one LOS ray per offered candidate (`raw_target_perception`) — if a player exists anywhere, due every 1/4/12 ticks by distance; **if there is no hostile candidate at all, `acquisition_due(None)` returns true, so it is due every tick** (but the scan is empty, so cheap); cooldown/recency/activity/clip clocks; guard fact refresh twice; full transition evaluation; motion resolve (→ `Clear` or patrol `MoveTo`); heading/launch; clip settle; engaged-motion plan (clears memory); combat slots early-outs if nobody engaged; arbitration over an empty request set; apply: brain clone publish, `clear_destination` (or patrol `set_destination`), animation latch check. Steering: idle settle `collide_and_slide` (gravity, ground stick) every tick, or full path-follow if patrolling. Pose: playback-rate check, pose inputs, foot probes (raycasts) if the model is a foot-probe candidate. Death sweep visits its Health. No perception LOS ray for a selected target (no target), no route query, no attack gates beyond the early `target` check.

**Aggro gate closed** (`enabled_on_spawn = 0`, or `setNpcState {aggro:false}`). Compute skips d–f, m's ray (no target), p, r–v entirely: no target scan, no guards. Still runs: snapshot/admit, ledger aging, cooldowns, recency, activity timers, clip clock, stride counter, `time_since_target_visible`, reseat-to-initial check, heading/launch, clip settle, engaged plan, apply (publish + `clear_destination` + anim latch), and **all of steering's idle settle, pose and death-sweep work**.

### (2) Existing dormancy / activation concepts

- **Aggro gate** (`BrainComponent::aggro_armed`, default true via `default_aggro_armed`): seeded from map KVP `enabled_on_spawn` by `ai_aggro_armed_on_spawn` (`crates/sim/src/scripting/builtins/data_archetype.rs`), toggled by the `setNpcState { aggro }` reaction (`crates/sim/src/scripting/reactions/npc_state.rs`), typically from a trigger (test in `trigger_system.rs` shows a "closet" reveal opening the gate). This is the only brain-level "asleep" state; it suppresses targeting and guard evaluation only — the entity still ticks clocks, steering, pose, collision.
- **Not existing until spawned**: `entity_spawner` + `spawnFromSpawner` reaction (`crates/sim/src/spawner.rs`) materialize enemies on demand (trigger- or event-driven), with a fixed post-spawn attack windup. This is the de-facto "wave/encounter" construct; enemies before spawn cost nothing.
- **Attack pools** (`ai_attack_pool`, `crates/sim/src/attack_pools`) group enemies by tag and start/stop by events, but only for the attack budget — not ticking.
- **Held enemies** (inventory-owned, e.g. picked up) leave the AI and steering sets entirely (`registry.is_inventory_owned` filters in both snapshots).
- **Quiescence** (zero HP or queued despawn, `host.is_quiescent` / `health::is_quiescent`) removes from AI and live steering; corpses take `tick_corpses` only.
- No distance/PVS/perception-based dormancy, sleep, or activation radius exists. grep for dormant/asleep/sleep in entities/ai/sim found nothing relevant. Conf high.

### (3) How the think stride is priced

- `crates/ai/src/engine_floor.rs`: `STRIDE_NEAR_DISTANCE = 12.0`, `STRIDE_MID_DISTANCE = 30.0` (XZ world units); `STRIDE_NEAR = 1`, `STRIDE_MID = 4`, `STRIDE_FAR = 12` ticks. `think_stride_for_distance(d)`: ≤12 → 1, ≤30 → 4, else 12.
- `crates/ai/src/targeting.rs::acquisition_due(brain, distance)`: due when stride ≤ 1 or `(think_stride_counter + 1) % stride == 0`; **`None` distance ⇒ due every tick**.
- Price input (`compute::evaluate`): the retained target's raw XZ distance if a target is retained (`engages_active`), else `target_offers(...).nearest` — the raw nearest **sentiment-hostile** candidate (players + brain peers), unfiltered by authored candidacy or LOS (doc §7c invariant). Retaliation-admitted non-hostiles never price it.
- Counter: `BrainComponent::think_stride_counter`, incremented every tick in compute (persisted/serialized on the component). Phase is per-enemy (counter starts 0 at spawn) — natural staggering by spawn tick only; no explicit stagger.
- What the stride gates: only fresh acquisition (candidacy, candidate LOS rays, ranking/retaliation/hysteresis), the `targetReachable` route query, and the `@brain.acquisitionDue` guard fact. Everything else in the table above is every-tick.

### Takeaways for sim LOD (observations, not decisions)

1. Already-strided work is small; the per-tick floor (offer scan, guards, clocks, steering idle settle, pose/foot probes, brain clone) is what LOD would save.
2. Things that would silently mis-time if called every N ticks with dt×N: `admit` turn clearing (breaks on any skip), `LOS_GRACE_TICKS`, `COMBAT_SLOT_HOLD_TICKS`, `launch.air_ticks` vs `FALL_DELAY_TICKS`, arbitration `since_tick` seniority, weapon-controller `full_ticks` step timing, steering `replan_cooldown_ticks`/`STUCK_TICKS_THRESHOLD`/`UNSTICK_WINDOW`/`drift_wait_ticks`, think stride counter, defense-layer `TICK_DURATION`/host-tick recharge.
3. dt-scaled and safe to batch arithmetically: cooldowns, recency, activity timers, clip clock (moment window widens, fire tick quantizes), sight-memory timer, ledger aging, facing slew, knockback decay, collide_and_slide (stability under 2×dt NV), deferred effects, turn `now_ms`.
4. Doc §7c explicitly commits clip accumulator + moment check to every tick ("Decision cadence work keeps the accumulator and the moment check on every tick"), and "Guards evaluate every tick" is a stated invariant — a 30 Hz guard cadence would be a contract change.

---

## Report: Arena relevance research: baked cell visibility, door state, cell location, enemy perception

Read-only research, 2026-11-17. Repo: /home/user/postretro. Confidence: high / med / low. "Not verified" means I did not open that code.

### Headline findings

1. **id 46 `perceivable` does not cull in a connected level.** It shipped as *portal-graph connected-component reachability*, not the sightline/anti-penumbra PVS the research doc first specified. Two cells are "perceivable" iff they share a component id. In a sealed, single-hull map almost every non-solid cell is in one component, so `perceivable(a, b)` is true for nearly every pair. Arena relevance gets no culling from `perceivable` alone. The only discriminating data is the graded `distance` (portal-path metres, top-32 nearest per source) and `aperture` (widest-path bottleneck).
2. **There is no runtime consumer of `LevelWorld::perceivable` / `LevelWorld::coupling` today.** Only diagnostics and dumps read id 46. The compiler's id-51 bake also uses id-46 fixed-point constants.
3. **Door state never enters id 46, by contract.** Closed-door blocking exists only as a render-only per-frame `blocked_portals: Vec<bool>`, rebuilt in the app frame loop. Its inputs, mover phase and `sealed_portal_ids`, are plain CPU ECS data, so a CPU-only consumer could derive the same mask.
4. **No sim/AI/net code locates cells today.** `locate_cell` is cheap (a plane-tree descent with no allocation), but no per-tick pawn→cell table exists anywhere.
5. **No hearing model exists.** Enemy perception is: an aggro gate, nearest-hostile candidacy, a static-world LOS ray (no sight cone), and damage/retaliation. Waking from sound happens only through authored `updateNpcState { aggro }` reactions.
6. **No per-player perceivable-cell or relevance set exists for any consumer.** `recipients_for_world_point` was retired. Recipients are `Local | Seats | Everyone | Skip`.
7. **Prior rejection to respect:** `context/plans/drafts/perf-ai-tick-cost/` rejected a cell-visibility broad phase *for target selection*. Its reasons: the stride is priced on raw distance, and LOS is about 0% of AI cost. The roadmap's Arena relevance bullet (`context/plans/roadmap.md:37`) already says arena relevance is distinct from that, and that the cost of an un-alerted enemy per tick is *unverified*.

---

### Q1. What the baked relation is (PRL id 46 `CellVisibility`)

**Format.** `crates/level-format/src/cell_visibility.rs`: `CellVisibilitySection { cell_count, component_ids: Vec<u32>, coupled_pairs: Vec<CoupledPairRecord> }`, with `CoupledPairRecord { cell_a, cell_b, distance: u32, aperture: u32 }`. Fixed point is 1024 units per metre (`CELL_VISIBILITY_DISTANCE_FIXED_POINT_SCALE`, `..._APERTURE_...`). Constants: `CELL_VISIBILITY_FANOUT_K = 32`, `CELL_VISIBILITY_DISTANCE_CAP` = 16 km. Version 1. (high)

**What a "row" means.** There is no per-cell row bitset. There are two parts:
- **`component_ids[cell]`** is the cull gate. It comes from a BFS over every non-solid portal edge (`component_ids()` in `crates/level-compiler/src/cell_visibility_bake.rs:234`). A and B are "perceivable" iff they are in the same component. (high)
- **`coupled_pairs`** is a sparse graded side table. For each source cell the bake runs Dijkstra over a portal-centroid "hub" graph (`portal_hub_graph`, `dijkstra`). It keeps the **32 nearest targets by path distance** in the same component (`source_top_k`, `insert_top_k`), takes the union over sources, and collapses each pair to a canonical unordered record (`collapse_directed_candidates`). Each pair's `aperture` is the bottleneck of the min in-plane portal width along the **maximum-spanning-tree** widest path (`maximum_spanning_tree`, `widest_paths_from`). That is *not* necessarily the same path as the distance path. (high)

**Properties.**
- **View-independent and cell-to-cell.** Yes. (high)
- **Conservative or exact?** Conservative to the extreme. It is reachability, a superset of any sightline set. The research doc records that the sightline tightening is deferred and unbuilt, and that if it lands it is an *additive* axis, not a redefinition (`context/research/cell-visibility-substrate.md`, "One correction to the record", "Sightline construction"). (high)
- **Missing section.** `LevelWorld::perceivable` returns `true` for everything. Pairs outside the top-K, or past the cap, have `distance` / `aperture` = `None` while staying perceivable. (high)
- **Sight or hearing?** Neither specifically. It is portal-graph coupling. The doc calls `perceivable` the PVS and its one-hop dilation the PHS, but with component reachability the distinction collapses. The doc says audio would use the graded axes, since "sound bends around corners". (high)
- **What is outside the graph.** Teleporters, ghost links and doors are all excluded. Teleporters do not create portals, so their arrival rooms are not coupled. Ghost links are "owed when runtime-masked couplings land" (`build_pipeline.md:397`, `networking.md:1261`). Door-sealed portals are still ordinary portals, because mover brushes are not static geometry (`build_pipeline.md:587`), so the relation is always the doors-open state. (high for doors and ghosts per the docs; teleporters: med, inferred from the bake reading only portals)
- **Symmetric.** Yes, stored upper-triangular. (high)

**Size per cell.** 4 B for the component id, plus 16 B per pair record. The pair count is at most `N × 32` (enforced at load through `max_encoded_len`). A cell's final undirected degree can exceed 32 because of the union. Upper bound is about 4 + 512 B per cell. (high)

**Parse and load.** `crates/level-loader/src/prl_loader/spatial.rs:509–545` validates the size bound and calls `CellVisibilitySection::from_bytes(data, cells.len())` then `convert_cell_visibility_section` → `Option<CellVisibility>` on `LevelWorld.cell_visibility`. The cell ids are runtime Cells ids; the bake runs over `runtime.cells.tree` (`crates/level-compiler/src/pipeline.rs:628–640`). (high)

**Runtime API.**
- `crates/level-loader/src/prl.rs:96–150`: `CellId = usize`, `CouplingTuple { perceivable, distance: Option<u32>, aperture: Option<u32> }`, `CellVisibility::{component_ids(), coupled_pairs()}`. Both `perceivable` and `coupled_pair` are `pub(crate)`; `coupled_pair` is a binary search.
- `crates/level-loader/src/prl_queries.rs:72–96`: the public entry points `LevelWorld::perceivable(a, b)` and `LevelWorld::coupling(a, b)`.
- There is no "all cells perceivable from A" iterator. A consumer would build one from `coupled_pairs()` (sorted by `(cell_a, cell_b)`, so B-side lookups need a scan or a derived index) or from component equality. (high)

**Consumers today.** None at runtime. A grep for `.perceivable(` / `.coupling(` outside level-loader finds nothing. Readers:
- `crates/postretro/src/observability/document.rs` (the `cell_visibility` dump)
- `crates/postretro/src/session/lightmap_residency/walk_measurement/world_poses/structure.rs:235` (stats print)
- the renderer debug UI grid (`crates/renderer/src/render/debug_ui/mod.rs:1124`)
- the compiler's lightmap-residency dry run and id-51 bake, which use the constants and `CoupledPairRecord`
- `crates/postretro/src/streaming/cell_demand.rs`, which uses only the scale constant (high)

**Related but different: id 51 `CellResidencySet`** (`build_pipeline.md` around line 503). For each camera cell it lists the cells the lightmap controller must hold, *sampled* from the real portal walk at a 3×3×3 eye lattice per cell, dilated by one portal hop, plus cells within a 32 m portal-path lead. It includes teleporter arrivals. It is the closest existing thing to a tight "cells visible from cell A" set. Its limits:
- it is explicitly *not* conservative ("id 46 stays conservative, and this set is sampled")
- it is streaming-owned
- it is emitted only when the level has usable portals

Accessor: `world.cell_residency_set` → `entries_for(cell)`. (med; I read the doc and the struct usage, not the loader body)

---

### Q2. Runtime modification (doors, movers)

- **id 46 is never modified at runtime.** `KinematicGeometry` portal associations "drive camera portal occlusion … neither enters `CellVisibility`, non-camera coupling, or multiplayer content parity" (`build_pipeline.md:587`). The research doc plans destructible/door masking as a future `(a, b, portal-state)` query widening, "paid then". (high)
- **Blocked-portal state** comes from `rebuild_blocked_portals` (`crates/postretro/src/main.rs:435–460`). It clears and refills a `Vec<bool>` of length `world.portals.len()`. A portal is blocked when any `KinematicMover` with `postretro_physics::kinematic_mover::mover_is_docked_closed(mover)` (`crates/physics/src/kinematic_mover.rs:336`) lists it in `mover.sealed_portal_ids` (`crates/entities/src/components/kinematic_mover.rs:95`). It is called each frame after the tick in `crates/postretro/src/frame_loop/mod.rs:1862` and in `crates/postretro/src/app/world_less_frame.rs:162`. It is consumed by the `postretro-visibility` walks (`visibility.rs:210`, `region_walk/entries.rs`, `mirror_visibility.rs`), and closed doors block in both directions (`rendering_pipeline.md` §2, step 2). (high)
- **Can a CPU-only consumer apply it?** Yes, technically:
  - the inputs are ECS components plus a physics-crate predicate
  - mover phase replicates, so clients derive the same mask (`client_predicted_mover_phase_matches_host_door_occlusion_phase`, `crates/netcode/src/client.rs:3922`)
  - `postretro-visibility` depends only on glam, log, level-loader and stage-timing, with no wgpu (`crates/visibility/Cargo.toml`)

  But the buffer is app-owned and labelled "render-only", and the existing walk is a **view-dependent frustum walk** that needs a view-projection. An omnidirectional "cells perceivable from A with doors applied" query would be new code: a BFS over `cell_portal_refs` / `world.portals` that skips blocked portals, giving door-aware connected reachability, or a 6-face walk. Neither exists today. (high on what exists; the design implication is my inference)

---

### Q3. Which cell a point or pawn is in

- **API.** `LevelWorld::locate_cell(position: Vec3) -> usize` (`crates/level-loader/src/prl_queries.rs:15`). It descends `cell_locator_nodes` with plane tests; on-plane points go front. Cost is O(tree depth), with no allocation (I did not measure the depth). `trace_locate_cell` is the dev-tools diagnostic twin. Physics keeps a private copy of the same descent for point-in-solid (`SolidCells::contains`, `crates/physics/src/collision/solid_cells.rs`) that does not expose the cell id. (high)
- **Who calls it.** Only the client and presentation side:
  - camera cell in the walk (`crates/visibility/src/visibility.rs:225`)
  - mesh instances (`crates/postretro/src/scripting/frame_systems/mesh_render.rs:151`, `crates/render-cpu/src/mesh_pass.rs:30`)
  - particles (`particle_render.rs:91`)
  - mover origin cells (`crates/postretro/src/runtime_movers.rs:376,444`)
  - lightmap walk-measurement tooling
  - SH diagnostics

  **No call in `crates/ai`, `crates/sim`, `crates/net`, `crates/netcode` or `crates/audio`.** Grepping for `CellId`, `locate_cell` or `cell_locator` there finds nothing. (high)
- **Is every pawn's cell computed each tick?** No. Mesh render locates *drawn* instances per frame on the viewing peer, not per sim tick and not on the host for remote players. The host has no per-client cell tracking; the roadmap lists "host-side per-client cell tracking" as owed work (`roadmap.md:143`). (high)
- **Can the AI tick reach it?** The AI tick gets `NavGraph` and `CollisionWorld` (`crates/ai/src/lib.rs:452`), not `LevelWorld`. `postretro-ai` does not depend on `postretro-level-loader`; `postretro-sim` does (`crates/sim/Cargo.toml:30`). So a relevance pass belongs in sim or the app, or it needs a new input threaded into the AI. (high)

---

### Q4. How enemies perceive players today

Reference: `context/lib/entity_model.md` §7c.

- **Aggro gate.** `BrainComponent::aggro_armed` (`crates/entities/src/components/brain.rs:267`). While closed, the brain does no target selection or guard evaluation and holds (`crates/ai/src/compute.rs:92,339`). It is set by:
  - spawn seeding, `ai_aggro_armed_on_spawn` (`crates/sim/src/scripting/builtins/data_archetype.rs:570`)
  - trigger commands (`crates/sim/src/trigger_system.rs:1784`)
  - the authored reaction `updateNpcState { aggro }` (`crates/sim/src/scripting/reactions/npc_state.rs`)

  (high)
- **Candidacy.** `target_offers` / `target_candidates` (`crates/ai/src/targeting.rs:140–200`) walk every `PlayerMovement` holder and then brain-only holders, with **no spatial filter**. Hostility comes from `is_hostile` (faction sentiment). An optional authored candidacy predicate narrows the set. Acquisition is time-sliced by `think_stride_for_distance`: XZ ≤12 m every tick, ≤30 m every 4th tick, else every 12th (`crates/ai/src/engine_floor.rs`). Retained-target reads and attack checks are not strided. (high)
- **Sight.** There is a static-world LOS ray and **no view cone or FOV**. `raw_target_perception` and `perceive_target` (`crates/ai/src/perception.rs`) call `collision::line_of_sight(enemy_eye, target_aim, world)`. Eye and aim points come from `enemy_eye` / `target_aim` (the hitbox top, or `EYE_FACTOR = 0.9` × agent height). A 3-tick grace (`LOS_GRACE_TICKS`) debounces a retained target only. Fresh acquisition requires `perception.visible` (`targeting.rs` around line 316). `fire_gate` gates attacks on the same verdict. A missing collision world means clear. (high)
- **Damage and retaliation.** A recent-attacker ledger plus per-pair tolerance admits an over-tolerance attacker, even a non-hostile one, and ranks it above the nearest (`select_target_with_attacker_ledger`, `RetaliationRank`). Brain-scope facts exposed to guards include damage and sight recency, `distanceToLastKnown`, `damageBearing` and provenance (`context/lib/scripting.md:653`). `last_known_target_pos` is written in `compute.rs:284`, and the motion verb `MoveToLastKnown` uses it. (high)
- **Hearing.** None. A grep for hear, noise or alert in ai/sim finds no stimulus model; "alert" is just an authored state name in tests. `context/research/enemy-aggro-model.md` lists "sound (gunfire/footstep noise events)" and "alert propagation / pack aggro" as uncommitted menu items, with "none (per-enemy)" today. Peer waking is possible only through authored reactions or triggers that flip `aggro_armed`. (high)
- **Prior decision.** `context/plans/drafts/perf-ai-tick-cost/index.md:23` and `research.md` §"Rejected: cell-visibility broad phase" rejected it for *selection* cost: the stride needs raw distance, and LOS is about 0% of AI cost, while the candidate walk dominates. (high)

---

### Q5. Any existing per-player perceivable or relevance set?

**None.** (high)
- **Network.** Replication has no relevance filter; nothing filters replication by relevance (`networking.md:1261`). Presentation `Recipients` is `Local | Seats(Vec<Seat>) | Everyone | Skip` (`crates/entities/src/reactions/system_commands.rs:223`). `transport.rs` uses `participating_clients()`.
- **VFX.** `recipients_for_world_point` was **retired** with the splash presentation route (`roadmap.md:74,143`) and no longer exists in `crates/`. The future body is described as `locate_cell(P)` + `perceivable` per client cell, plus host-side per-client cell tracking.
- **Audio.** No occlusion and no cell id at the boundary ("A runtime cell id is not yet part of the boundary; reverb zone lookup will add it", `context/lib/audio.md:24`). The potentially-audible set is roadmap E12 step 5, speculative (`roadmap.md:75`).
- **Input "relevance"** (`crates/postretro/src/input/relevance.rs`) is unrelated: it is about command relevance.
- The only per-viewer cell set that exists is the client's own camera visible set from the per-frame portal walk. It is a conservative superset that no consumer may read as proof of line of sight (`rendering_pipeline.md` §2), and it exists only for the local camera.

---

### Implications for arena relevance (inference, not verified by measurement)

- A consumer-agnostic `perceivable_from(player_cells) -> cell bitset` built on id-46 `component_ids` would mark essentially the whole level relevant. The design needs one of:
  - (a) door-aware runtime reachability, a BFS over portals skipping `blocked_portals`, which does cut sealed arenas behind closed doors
  - (b) a `distance` threshold from the graded table. The substrate invariants forbid using a scalar as a hard *cull* for presentation, but sim-LOD throttling is "modulation", which the doc allows ("AI → think-rate").
  - (c) id 51, or the deferred sightline axis
- Whatever is built needs a host-side per-player cell tracker, `locate_cell` on each pawn per tick. It is cheap, it does not exist yet, and E15 Phase 4 and VFX culling would share it. Home it outside `crates/net` and `crates/ai`, per the generalizability contract. The substrate "knows only Cells": the per-player set is consumer-side policy, while a generic "cells coupled to a cell set" helper can live in level-loader or visibility.
- Simulation authority: the substrate doc says consumers "must NOT cull authoritative simulation". A low-detail tick *is* a simulation change, so the arena-relevance brief should state explicitly how it squares with that invariant: for example, a deterministic host-only cost tier that never skips state an observer could detect.
- The roadmap requires measuring what an un-alerted enemy costs per tick before building.

---

## Report: Sim LOD (staggered half-rate enemy ticks): networking, presentation, events, tests

Research only, nothing edited. Repo read at HEAD on 2026-11-17. Confidence: **H** = I read the code or the contract line, **M** = inferred from code I read in part, **L** = not opened or only reasoned. "Not verified" means I did not open it.

### 0. Prior art: the roadmap already plans this

The roadmap already splits this work into two briefs, and the split constrains the design (`context/plans/roadmap.md`, Epic 10, "AI at scale"). **H**

- **Decision cadence and reaction time.** "Deciding (target choice, graph evaluation) runs for one pool of enemies per tick, about 15 Hz … **Acting (steering, facing, animation) and attack resolution stay every tick**, including Clip-timed attacks' clip time and its `at` check. Pools are assigned by slot, so co-op and replays stay deterministic." Not drafted.
- **Arena relevance.** "Enemies in cells no player can perceive drop to a low-detail tick … consumes the shared cell-visibility relation, built … with E15 Phase 4 network relevance." Not drafted.
- `entity_model.md` §7c (Clip-timed attacks) already commits: "**Decision cadence work keeps the accumulator and the moment check on every tick.**" **H**
- `drafts/perf-ai-tick-cost` acceptance: "Every enemy selects the same target, on the same tick, as today." Its non-goals exclude time-slicing perception or LOS because "it would change reaction latency." That brief lands first, and this one breaks its same-tick property on purpose. **H**
- A think stride already exists for target acquisition only: `think_stride_for_distance` (`crates/ai/src/engine_floor.rs`) uses bands of 1, 4 and 12 ticks at ≤12 m, ≤30 m and beyond. It is gated by `acquisition_due` (`crates/ai/src/targeting.rs`) on `brain.think_stride_counter`, which `compute.rs:240` advances every tick. The counter starts at 0 for every brain (`crates/entities/src/components/brain.rs:373`). Brains spawned on the same tick, such as every map-placed enemy at load, therefore acquire on the same ticks, with no stagger. Retained-target reads and attack range and cooldown checks are explicitly not strided. **H**

Several §7c invariants would need rewording, because each one says "every tick" or "next tick": "Guards evaluate every tick", "Newly entered activities do not evaluate transitions until the next tick", and arbitration "sees every request and release of the tick". **H** (doc)

---

### 1. Enemy replication, cadence, client presentation, prediction

**What replicates.** For a networked descriptor enemy, `collect_payloads` (`crates/netcode/src/replication.rs`) sends `Transform` and `MeshAnimationState { current_state }`, which is the state name only, with no clip time or phase. The record also carries the `entity_class` metadata and any light, fog or emitter presentation overrides. Health, Brain, Agent and Weapon never cross the wire. Clients build presentation-only display copies (`networking.md` §Current contract, "Connected-client spawn suppression"). Enemy health and shield reach clients only through the damaged-enemy `WireOverlayRoster`, which publishes at most 10 Hz and repairs every 250 ms, keyed on script time rather than ticks. **H**

**Cadence.** The cadence logic lives in `SNAPSHOT_TICK_INTERVAL = 2` and `complete_host_fixed_tick` (`crates/netcode/src/host.rs`). **H**

- `host_replicate` runs once per frame, after the tick loop. It ingests the registry, which feeds the dirty check, and encodes and sends only if `snapshot_due` (a completed tick crossed an even number). Each snapshot is stamped with the frame's last tick, so it carries end-of-frame state.
- With one tick per frame, snapshot ticks are the even ticks. In catch-up frames a snapshot can be stamped on an odd tick (ticks 3→5 stamps 5). **H**
- The dirty check is per entity. An entity whose wire mirrors equal its tracked state keeps its baseline id and is omitted (`crates/net/src/replication.rs` ~L375–403). An enemy that did not move or change state since its last sample sends no `Transform` that snapshot. **H** for the mechanism, **M** for exact omission when an acked baseline exists.

**Client presentation.** The client buffer is `RemoteInterpolationBuffer` (`crates/netcode/src/interpolation.rs`). `ClientReplication::apply_*` records each received `Transform` as a `TransformSample` stamped with the snapshot's `server_tick` (`client.rs` ~L2199–2235). `sample_into_registry` (`client.rs` ~L2453) samples at `render_server_tick` and writes the result through `set_presentation_transform`, which sets previous equal to current so the render alpha does not blend it again. **H**

- **Delay law.** The delay is the larger of two terms, each rounded up to whole ticks and slewed at 0.75–1.25× playback:
  - RTT/2 + clamp(50 ms + 2×jitter + starvation margin, 50 ms, 250 ms);
  - lag peak + `SNAPSHOT_TICK_INTERVAL`.

  On a LAN this is a 50 ms floor, about 3 ticks. Starvation extrapolates for at most 100 ms, then holds. **H**
- **Starvation feedback.** `held_newest_needs_feedback` raises the global delay for every remote, pawns included. It fires only if the entity's newest sample is at or after the latest accepted snapshot tick and the entity is moving. An enemy omitted because it did not change has an older newest sample, so it does not raise the delay. **H**
- **Walk rate.** `update_remote_enemy_walk_playback_rate` derives the client's walk-clip rate from the interpolated `speed_xz` of the bracketing sample pair. Irregular step spacing in the samples therefore becomes walk-rate jitter on clients. **H**
- **Animation timing.** Enemy animation states apply on receipt (`apply_mesh_animation_state` in the `MeshAnimationState` arm, `client.rs` ~L2266–2280), not at the presented tick. Only remote players queue through `queue_correction`. So an enemy's clip on a client already leads its drawn pose by the interpolation delay. **H**

**Prediction.** No part of an enemy is predicted on clients. **H**

- Clients never run `simulate_tick`, and display copies carry no Brain or Agent (`enemy_replication_harness_test.rs` header: "never predicted"). Graph evaluation is host-only (§7c last line).
- Movers differ: their phase is replicated and clients re-simulate it with the deterministic driver (`networking.md` §Current contract). A mover's block reaction against enemies is decided by the host, and clients reconcile only its result.
- `drafts/coop--predicted-combat-feedback` keeps enemies unpredicted. **H** It adds:
  - a cosmetic client-side push "guess" on display copies, handed over "as the render tick passes the host tick the shot's first verdict names", with `ShotVerdict` gaining a host resolution tick;
  - enemy animation states that queue with their server tick and apply at the presented render tick, as remote players' states do;
  - a hit jolt on drawn poses only.

  None of these predicts AI. The handover keys on the host tick on which the shot resolved, so a coarser enemy step changes only what host pose the guess converges to, not when the handover happens. **M**

**What half-rate stepping would show on clients.**

- **2-tick stepping, all paths:** an enemy whose position moves only every second tick is effectively a 30 Hz signal.
- **Steady 60 fps host:** every 30 Hz snapshot (even tick) contains exactly one step. Clients interpolate between 30 Hz samples today, so this is visually unchanged; odd-pool enemies are sampled one tick stale. **M**
- **Host running catch-up frames, below 60 fps:** snapshot stamps fall on irregular ticks, so segments carry 0 or 2 steps. That shows as speed pulsing and walk-rate jitter on clients. **M**
- **Low-rate tier (4 or more ticks), e.g. 4-tick:** half the snapshots omit the enemy, so samples are about 4 ticks apart. With a 3-tick render delay on a LAN, the render target can pass the newest sample, and the enemy extrapolates, then holds. Feedback is not raised (see above), so the hitches stay local to that enemy. **M**
  - This matters only if an "unperceived" enemy can still be seen by someone. Relevance must cover every player, not just the host. `networking.md` §Current contract, "Ghosts constrain future relevance", requires any relevance filter to treat an armed ghost link as coupling the cells under its two volumes, through cell-visibility id 46, "or ghosts silently vanish." **H** (doc)

### 2. Listen host: how enemies are drawn

The host draws enemies from the live registry at render rate, interpolating between the previous and current fixed tick. Stage 0 (`EntityRegistry::snapshot_transforms`, `crates/entities/src/registry.rs` ~L1855) copies current into previous every tick. `mesh_render.rs:377` reads `interpolated_transform(id, alpha)`, which lerps and slerps between them by the frame's sub-tick alpha. **H**

The host's delayed `RemoteInterpolationBuffer` presentation applies only to connected-client pawns in `MovementOwners` (`crates/netcode/src/host_presentation.rs` header; `networking.md` §Host-side remote-pawn presentation). Enemies are not delayed on the host. **H**

**Yes, the host would show 30 Hz stepping if enemy position advanced only every second tick.**

- On a tick where the enemy did not move, previous equals current, so it is frozen for that tick's render frames.
- On a tick where it moved, it covers two ticks of distance in one tick's frames.
- The result reads as move, stop, move at 30 Hz, very visible at the host's measured 250–330 fps render rate (the `host_presentation.rs` header comment). **H**

The same mechanism is why host-side remote-pawn presentation uses a fractional target. Integer stepping "reproduce[s] the choppiness" (`host_presentation.rs` comment). **H**

**Secondary host effects if locomotion itself is strided. M: they depend on whether the steering velocity persists on idle ticks.**

- **Animation flicker.** Locomotion-state animation, the moving-versus-stopped decision and facing all read the per-tick steering velocity against `MOVE_SPEED_EPSILON`:
  - `LocomotionIntent::from_velocity` (`crates/ai/src/lib.rs`);
  - `clip_clock.rs:350`;
  - `update_brain_animation_playback_rates` (`crates/sim/src/sim/mod.rs` ~L1380).

  A zero velocity on idle ticks would flip the clip between the travel cycle and the rest pose, and flip facing between travel and target. That `current_state` then replicates, so clients would also see flicker, at whatever phase the 30 Hz sampling catches.
- **Walk rate.** The host's walk playback rate would alternate between 2× and 0.

Mitigation consistent with the roadmap: keep steering, facing and animation every tick and stride only decisions. Alternatively, if locomotion must stride, render-interpolate across the stride (keep previous at the last step, with alpha over the stride length).

### 3. Events and timing

**Draining.** Movement, AI and weapon events are collected across all catch-up ticks in stage buckets and drained after the tick loop (`entity_model.md` §5 Events). Emissions are stamped with the emitter's position at the tick, not with a tick number. **H** (doc)

**Wire events carry no tick stamp.** **H**

- **Weapon fire and impact, enemy weapon attacks included.** They ride `WeaponCue { shot_id, kind, sound, additional_sound, alias, anchor }` (`crates/net/src/wire.rs` ~L255) on the reliable Input channel, "independently of snapshot cadence". There is no tick, and receivers play on receipt.
- **Presentation commands.** `ServerPresentationPayload::{Spawn, OverlayRoster, Command}` carries damage numbers, routed `PlaySound` and the like on the unreliable Presentation channel, with no tick. The roster carries script time.
- **Enemy behavior-entry sounds and contact (melee) sounds.** Today these are host-only. `drafts/E10--coop-enemy-telegraph-sounds` routes them as `PresentationCommand::PlaySound` at a host-captured origin: unreliable, no tick, played on receipt, so it leads the drawn pose by the interpolation delay.
- **Enemy animation state.** Snapshot state stamped with `server_tick`, but for enemies it applies on receipt (§1).

**Tick precision that clients actually get:**

- remote-player states and samples, keyed to server ticks;
- `levelLoad` `wait` tails, which use the host level tick on each snapshot;
- timed weapon meters;
- the planned `ShotVerdict` host tick for push handover.

None of these is enemy AI timing. **H**

**What depends on an enemy event landing on an exact tick (host side).**

- **Clip-timed attacks.** The brain accumulates clip time per AI tick and checks the moment once per entry, after transitions (`crates/ai/src/clip_clock.rs`, `Moment::{Untimed, Reached, Pending}`, with `CLIP_TIME_SLACK` 1e-4). The doc requires decision-cadence work to keep both every tick. The test `clip_timed_attacks_fire_on_the_same_ticks_at_any_frame_split_and_animation_time_scale` pins the brute jab's first damage at tick 17 (see §4). **H**
- **Attack-turn arbitration.** It is host-only and deterministic and sees every request and release of the tick. The longest wait wins, with brain slot index as the tie-break. A grant on tick N reads held on N+1 (`arbitrate_attack_turns`, `TurnFactReader::read`, per the in-progress stances brief). Staggered thinking moves requests to alternating ticks and changes who wins ties or waits. **H** (doc), **M** (impact)
- **Stance holds (in-progress brief).** A hold "ends on the first tick at or past" its hidden draw, and a release's own rows stay closed until its moment's tick. The co-op tell floor is the 250 ms interpolation ceiling plus a reaction budget. Half-rate thinking adds up to 1 tick, about 17 ms, of quantization; a slow tier adds more. A related test, `spawn_windup_covers_interpolation_ceiling_without_equality_pin` (`enemy_replication_harness_test.rs:1094`), pins the analogous margin for spawn windup. **H** (doc and test name)
- **Client-declared hits** do not depend on enemy tick timing. The host validates only static-world LOS from the live eye and range (`networking.md` §World-LOS-only validation; `crates/netcode/src/lib.rs` ~L2440–2510). It trusts the declared target, point and zone (`zone: record.zone.clone()`). There is no rewind and no lag-compensation window (§Non-goals). **H**
- **Damage** is applied host-side through the Health chokepoint at the tick where it is resolved: projectile stage 5, weapon stage 9, AI attacks in stage 6. The player's health reaches clients as owner-private slots at snapshot cadence. **H** (doc)

### 4. Tests that constrain a change

1. **`crates/sim/src/sim/determinism_tests.rs`.** **H**
   - Covers `simulate_tick_determinism_harness_matches_run_to_run_and_spawn_order` and the proptest `simulate_tick_is_deterministic_for_random_command_stream` (600 ticks, 32 cases).
   - `assert_runs_match` compares the stage-grouped events of every tick exactly, the final `enemy_state`, and the health ledgers, across a rerun and with player spawn order reversed.
   - In the fixture the single enemy is always spawned after both players (L335), so its slot does not change with the reversal. A slot-parity pool would pass here, but only by accident of the fixture.
   - It does not pin AI ticks to absolute numbers. It requires that the pool assignment be a deterministic function of sim state, not of frames, render or the camera.
2. **`clip_timed_attacks_fire_on_the_same_ticks_at_any_frame_split_and_animation_time_scale`** (same file, ~L4684). **H**
   - Asserts the brute jab first hurts on **tick 17**, and that damage lands on identical ticks at 1, 2 or 4 ticks per frame and at animation time scale 1.0 or 0.25.
   - If the moment check moved to think ticks, a pool phase could move 17 to 18. The roadmap says to keep it every tick.
   - Any "perceived" signal sampled per frame would break the frame-split invariance.
3. **AI crate suites, about 18.8k lines of test code** (`crates/ai/src/ai_tests.rs`, `clip_timed_shipped_tests.rs`, `attack_turns/tests/tick/{budget,refresh,tokens,reload}.rs`, `engaged_motion_tests.rs`, `held_world_participation_tests.rs`, `replan_stress_tests.rs`).
   - Many tests set `brain.think_stride_counter` by hand and assert acquisition on specific due ticks:
     - `off_stride_retained_target_does_not_switch_to_meaningfully_closer_pawn`;
     - `retained_target_prices_stride_off_stride_holds_and_due_tick_allows_hysteresis`;
     - `distant_enemy_strides_detection_but_attack_still_fires`.
   - `clip_timed_shipped_tests.rs` pins the brute and limitator timing.
   - Attack-turn tests pin grant on N and held on N+1.
   - **H** for existence, **M** for how many would break; I opened only the names.
4. **E18 tick-ordering rows** (`determinism_tests/e18_task7_tick_ordering.rs`) pin timed-reaction landings across multi-tick frames, not AI. **H** (header only)
5. **`crates/postretro/tests/headless_byte_identity.rs`** runs `--headless` twice on `spawner-test` for 30 ticks, with and without CPU timing, and asserts identical stdout. This is run-to-run only. It breaks if pool assignment or "perceived" depends on wall clock, timing or render state. **H**
6. **E20 scripted run capture** (`drafts/E20--scripted-run-capture/research.md`) is pre-draft research. It assumes "Sim is already byte-identical (batch runner)", and its capture runs N fixed ticks then renders. A tick-deterministic LOD is fine. One driven by the renderer's visible set is not, because headless has no renderer. **H** (doc)
7. **Netcode harnesses.**
   - `enemy_replication_harness_test.rs` moves the enemy every tick and asserts `carried >= 110` of 120 snapshots carry its pose, and that every carried pose becomes the newest sample. The test sets transforms directly rather than through AI, so it does not constrain the LOD. **M**
   - `remote_enemy_interpolation_is_deterministic_under_seed_0x1502` pins drop and sample counts under a fixed seed. **H** (name)
   - The interpolation unit tests use `SNAPSHOT_TICK_INTERVAL` stepping. **H**
8. **Not verified:**
   - whether `BrainComponent::think_stride_counter` serializes; save and restore of pool phase;
   - CI wiring of these suites.

### 5. Hit detection against enemies

**Host-resolved hits use the live tick pose.** These are the host player's own hitscan (weapon stage 9) and host-flown projectiles (stage 5).

- Narrow phase: `crates/sim/src/scripting_systems/hit_zones.rs` (`nearest_entity_hit`, `nearest_zone_hit`, `pose_world_joints`) reads `registry.get_component::<Transform>(id)`, which is the current tick transform, not interpolated.
- The skeleton is posed from the replicated or host `MeshAnimation.current_state` at the game-layer `anim_time` passed through `simulate_tick`, which is the frame's animation clock.
- So hits are evaluated at the host tick, and coarser enemy movement changes what host shots hit.
- With 2-tick stepping, an odd-phase enemy's live pose trails its continuous path by up to 1 tick (about 8 cm at 5 m/s). The gap between the host player's drawn pose (lerp of previous and current) and the tested pose roughly doubles, to up to 2 ticks of travel on step ticks.
- Small for body shots; it can matter at hit-zone edges such as head capsules. **H** (code path), **M** (magnitudes)

**Client-declared hits use the client's presented pose.**

- The client resolves against its display copy at the interpolated presentation pose. The test `client_fire_resolves_remote_enemy_at_presentation_pose_without_health` (`crates/sim/src/weapon/mod.rs` ~L3196) asserts exactly that. Zones are posed by the client's animation clock and the state applied on receipt.
- The host accepts the declared target and zone without any pose check (world LOS and range only).
- So host-side enemy LOD does not change the validity of client hits. It changes only the client's drawn pose, which at 30 Hz sampling is mostly unchanged (§1). **H**

**Gaps already noted in the docs.**

- Clips and models are outside every digest, so peers can pose zones differently (`networking.md` §What gates).
- On clients, animation state leads pose by the interpolation delay (the predicted-combat-feedback draft would fix this). **H**

### Summary of implications

- **Safe:** stride only thinking (target choice and graph evaluation), as the roadmap already decides.
  - Keep transform snapshots, steering and locomotion, facing, animation rate, clip clock and moment checks, and attack resolution every tick.
  - Assign pools from sim-deterministic data: slot, or a spawn-order-independent per-instance id like the behavior-draw seed.
  - Clients see no difference beyond about one tick of extra reaction latency.
- **Visible on the listen host:** halving locomotion produces 30 Hz move-stop judder in `mesh_render`'s alpha interpolation.
  - It may also flip walk and rest clips and facing through `LocomotionIntent`.
  - Clients would see walk-rate jitter and pulsing when the host drops below 60 fps.
- **The "unperceived" tier must be decided over every player's perception**, from the sim, using cell-visibility id 46. It must also honour the ghost-link coupling rule, never the host camera's per-frame visible set. That keeps determinism, frame-split invariance and headless runs intact.
- **Doc invariants to amend in §7c:** guards evaluate every tick; next-tick entry; arbitration per tick. Also amend `perf-ai-tick-cost`'s same-tick target-selection acceptance.

---

## Report: Sim-LOD / arena relevance: what the repo already decided or planned

Read at `3366eeeb7`, 2026-11-17. Read-only. Confidence: **H** means verified in doc text and code. **M** means read in docs or partly in code. **L** means inferred.

---

### 0. Findings that change the brief's premise (read these first)

1. **The baked `perceivable` bit barely culls in a connected level.** Shipped `perceivable(a,b)` is `component_ids[a] == component_ids[b]`, which is portal-graph reachability (`crates/level-loader/src/prl.rs` `CellVisibility::perceivable`). Every cell of a connected multi-arena map is in one component, so "no player can perceive this arena" is never true on the binary gate. The discrimination lives in the graded side table: `distance` is the shortest portal-path length and `aperture` is openness. That table is sparse. Each source cell keeps only its `CELL_VISIBILITY_FANOUT_K = 32` nearest partners under a 16 km cap (`crates/level-format/src/cell_visibility.rs`), and an omitted pair "stays perceivable" with `distance: None` (`prl_queries.rs::coupling`). A sim-LOD consumer must therefore define its own policy over `distance` (an absent pair counts as "far"), or wait for the deferred sightline axis. Doors are not in the relation: E17 doors-as-occluders explicitly left id 46 untouched, and a blocker-mask `(a,b,portal-state)` query is deferred. Sources: `context/research/cell-visibility-substrate.md` (Status note, "Bake-side contract — One correction to the record"); `context/plans/done/cell-visibility-relation/index.md` (Alternatives rejected (b): "the weak connections score low coupling"); `context/plans/done/E17--doors-as-occluders/index.md` (lines 11, 33–34). **H**
2. **The substrate's correctness invariant forbids culling authoritative simulation.** Quote: "Presentation / relevance only — never simulation authority. ... They must NOT cull authoritative simulation: the server still simulates the distant projectile / enemy ... Conflating 'don't show' with 'don't simulate' causes desync." The same doc also lists "AI perception broad-phase / sim-LOD ... a throttle for enemies in imperceptible Cells" as an intended consumer. So the existing commitments allow a *low-detail tick* (a throttle) but not a *no tick* (a cull). The brief should say which side of that line it is on, or amend the invariant. Source: `cell-visibility-substrate.md` §Correctness invariants, §Intended consumers. **H**
3. **The roadmap already splits this work into separate briefs, with a different rate.** "Decision cadence and reaction time" is its own roadmap brief. It sets deciding at about **15 Hz** in slot-assigned pools, while acting (steering, facing, animation) and attack resolution stay **every tick**. "Arena relevance" is a separate later brief. The proposed "perceived enemies advance non-predicted work at 30 Hz staggered" merges those two briefs and changes both the rate and the acting/deciding split. That needs explicit owner sign-off, plus a roadmap edit. Source: `context/plans/roadmap.md` Epic 10 "AI at scale", lines 34–38. **H**
4. **Clip time and the moment check are already committed to every tick.** Quote: "Decision cadence work keeps the accumulator and the moment check on every tick." Source: `context/lib/entity_model.md` §7c, Clip-timed attacks, "Clip time is game time". **H**

---

### 1. `context/plans/roadmap.md`

**AI at scale block** (Epic 10, line 34). Status `[ ]`. Owner request 2026-11-10. Quote: "enemy AI that holds up in a horde arena and on a map with several arenas … On a Windows desktop, the median AI tick for a converged wave was 1.4 ms at 64 strafers and 3.1 ms at 64 chasers, and 4.0 ms and 8.8 ms at 128. Cost grows superlinearly. The AI runs single-threaded on the host. Four briefs, in order, each changing a different contract." **H**

Sibling items, all `[ ]` and quoted verbatim:
- **Exact cost cuts** — "`drafts/perf-ai-tick-cost/`: a faction-bucketed target-candidate index that removes the every-enemy-against-every-pawn scan, an indexed nav point-to-region lookup, and `sim_ai` sub-stages. Selections stay identical. At draft refresh, adopt the stress row as its measurement and add the global 4-per-tick replan budget, now served longest waiter first, whose fair-share wait grows with crowd size (0.4 s at 128 chasers)."
- **Decision cadence and reaction time** — "staggered think pools. Deciding (target choice, graph evaluation) runs for one pool of enemies per tick, about 15 Hz, as Quake monsters thought at 10 Hz. Acting (steering, facing, animation) and attack resolution stay every tick, including Clip-timed attacks' clip time and its `at` check. Pools are assigned by slot, so co-op and replays stay deterministic. Owner decides the feel, including whether reaction time becomes an authored per-archetype value (`reactionMs`). It changes reaction latency, which `perf-ai-tick-cost` excludes, so it is its own brief. Not yet drafted."
- **Arena relevance** — "enemies in cells no player can perceive drop to a low-detail tick, so a dormant arena costs little. It consumes the shared cell-visibility relation, built consumer-agnostically with E15 Phase 4 network relevance. It is distinct from target selection, where `perf-ai-tick-cost` rejected a visibility broad phase. First piece: a multi-arena stress map, which no map represents today (the stress-map tiers are deferred). It must also measure what an un-alerted enemy costs per tick, which is unverified. Not yet drafted."
- **Parallel path replans** — "only if profiling after the three above shows the remaining cost is compute. Follows `context/lib/ai_pathfinding_mt_readiness.md`: snapshot registry-derived requests first, then fan out route queries. Not yet drafted."

Claims and confidence:
- The roadmap orders the briefs as exact cuts, then cadence, then arena relevance, then parallel. A brief that bundles cadence with relevance reorders that sequence. **H**
- The roadmap text says the relation is "built consumer-agnostically *with* E15 Phase 4". The relation is in fact already shipped (id 46), and no consumer is wired, so the line is stale. Whichever consumer lands first owns the per-player query shape. **H**

**E15 Phase 4** (line 98), `[ ]`. Quote: "Validate 16 players within a host-upstream byte budget. If the budget needs interest management, consume the shipped view-independent cell-visibility relation in two stages: binary relevance first, then graded throttling only if measured pressure warrants it. Resolve the host-play execution model before this phase…" It is gated on measured bandwidth pressure. Note that "binary relevance first" runs into finding 0.1: the binary gate is reachability and culls almost nothing. **H**

**Combat / projectile VFX relevance culling** (line 143), `[ ]`, "later / additive". Quote: "wire the shared cell-visibility relation into recipient selection for presentation events and observer weapon cues (broadcast today; the `recipients_for_world_point(P)` seam retired with the splash presentation route) … `locate_cell(P)` + `perceivable` per client cell, plus host-side per-client cell tracking, with no wire change. Shares the relation with E15 Phase 4 and the audio potentially-audible-set. Presentation-only — never culls authoritative simulation. Sequenced on measured co-op VFX-bandwidth need." It names "host-side per-client cell tracking", which is the per-player perception state this brief would build. **H**

**Audio PAS** (Epic 12 item 5, line 75): "The coarse potentially-audible-set is a consumer of the shared cell-visibility relation … whichever epic builds it first does so consumer-agnostically." This is a third future consumer of the per-player query. **H**

**Stress-map tiers.** The only mention is the Arena relevance item's "(the stress-map tiers are deferred)". Grep finds no tier plan anywhere in `context/`. It is an unnamed deferral. **M**

---

### 2. `context/plans/drafts/perf-ai-tick-cost/` (compact brief, not promoted)

What it decides (`index.md` §Decisions):
- **Faction-first pawn index, built once per AI pass.** Hostility resolves per bucket pair. The due-tick walk goes in (distance, group, slot) order and stops at the first eligible hostile. A grid inside a bucket is added only once that bucket is large. **H**
- **Amends `entity_model.md` §9** with a named exception to "Spatial partitioning for entity-entity queries": the AI target-candidate index. "Every other entity-entity query stays unindexed." Amended at promotion, so not yet. **H**
- **"No cell-visibility broad phase."** Rejected for target selection because "the stride is priced on raw distance, which visibility cannot answer, and line of sight measures about 0% of AI cost." **H**
- **The nav point-to-region lookup is indexed at load** (cell to candidate-regions over the existing grid; exact equality with the linear scans; no PRL change). **H**
- **`sim_ai` gains sub-stages** for targeting, perception, graph evaluation and apply, on `SimStage`. The AI crate receives `SimStage` scopes through `AiTickInputs`. **H**
- **Non-goals:** "Time-slicing perception or LOS: measured at about 0% of AI cost, and it would change reaction latency." Also "Changing think-stride bands." Also parallel AI ("the cost is a cache-hostile walk, not compute"). **H**
- **Landing order:** particle pool, then nav index, then pawn index, then registry occupancy index (its own brief). **H**

**Budgets** (§Acceptance Manual):
- Baseline `sim_ai` 5.8 ms/tick and `sim_steer_agents` 2.5 ms/tick at 64 agents with about 10k particles, on `stress-env-volumes` on an Intel Mac.
- Target, re-confirmed on the pool: `sim_ai` ≤ 1.0 ms/tick and `sim_steer_agents` ≤ 0.8 ms/tick.
- Per-agent `sim_ai` cost at 256 agents is at most 1.5× its cost at 64, in both the spread-patrol pose and the converged-horde pose.
- Per-enemy targeting cost does not change with 60k non-pawn registry entities.

**H**

**Research facts** (`research.md`):
- Before the pool, `target_candidates` made up 84% of `sim_ai`, and `region_at` about 80% of `sim_steer_agents`.
- About 90 µs per agent per tick at 11.2k slots; about 40 µs per moving agent in steering.
- **Idle (no held targets) cost about the same as mixed**, roughly 5.5–7 ms. So un-alerted enemies were not cheap, because `target_offers` runs every tick for every armed brain regardless of the stride.
- The engaged measurement never ran ("Not measured: genuinely engaged agents").

**H** for the numbers as reported; **M** for whether they still hold after the particle pool, which has landed (`context/plans/done/perf-particle-sim-cost`).

**Relationship to arena relevance.** The draft does not mention arena relevance by name. The roadmap places it before arena relevance and says arena relevance "is distinct from target selection". Its rejection covers only a visibility broad phase *inside target selection*, not tick-rate LOD. **H**

**Built?** No.
- `crates/ai/src/targeting.rs::target_candidates` still chains `registry.iter_with_kind(PlayerMovement)` and `iter_with_kind(Brain)`.
- `crates/sim/src/nav/mod.rs` `region_at` and `resolve_region_at` are still linear `for (i, region) in self.regions.iter()` scans.
- `SimStage` (`crates/sim/src/sim/cpu_stages.rs`) has `Ai` (`sim_ai`) with no AI sub-stages, only `SteerAgents`, `SteerPose` and `SteerMovers`.
- `git log --all --grep` finds no commit for a pawn index or region index. The last related commits are `2095e74c6` (stages plus drafts) and `16b90e4b2` (amendments).

**H**

**Open question that blocks build of the pawn index:** re-confirm the targets on the post-pool baseline (owner). The new draft-session rule (commit `45f055ee2`: "never gate the build on measuring current content") is in tension with this measurement-gated question. **M**

---

### 3. E10 target-selection seam, MT readiness, entity_model §9

**`context/plans/done/E10--enemy-mp-target-selection/index.md`:**
- It lands `select_target` as "a named, extensible chokepoint so a future view-independent cell-visibility broad-phase … can plug in without re-touching the FSM". The signature admits `impl Fn(EntityId) -> bool`, and the doc comment names `cell-visibility-substrate.md` (AC 5). **H**
- Decision "`visible` may widen bool → weight (anticipated)". **H**
- Risk "Selection cost": "If enemy counts ever make it hot, the `visible`/selection seam is exactly where a cell-visibility broad-phase drops in." This seam was later explicitly rejected *for selection cost* by perf-ai-tick-cost. It is still the reserved plug for perception gating. **H**

**Think stride: time-slicing already exists as engine policy.**
- `crates/ai/src/engine_floor.rs`: `STRIDE_NEAR` is 1 within 12 m, `STRIDE_MID` is 4 within 30 m, and `STRIDE_FAR` is 12. Acquisition only. "Retained-target reads and attack-in-range/cooldown checks are NOT strided."
- `entity_model.md` §7c: "The think stride is cost machinery and shares no data path with authored relevance rules." The engine owns "think-stride time-slicing of acquisition".
- Modders see it. `brain.acquisitionDue` is a documented guard input ("Detection is time-sliced; conjoin this onto detection edges"), in `docs/scripting-reference.md` lines 1937 and 2164.

**H**

**Stride phase is not staggered.** `think_stride_counter` starts at 0 for every brain (`crates/entities/src/components/brain.rs:373`), and every brain increments it each tick (`compute.rs:240`). Brains spawned on the same tick therefore come due on the same tick, so there is no slot stagger today. **M**: read in code, not run.

**`context/lib/ai_pathfinding_mt_readiness.md`.**
- `NavGraph` and `CollisionWorld` are `Send + Sync`.
- "The blocker is scheduling … snapshot all registry-derived request data on the simulation thread, then execute an ordered batch of navigation-only requests in a fork-join window."
- Per-tick mover state has no `Send + Sync` assertion yet.
- Non-goal: parallel entity-registry access.

This bears on a sim-LOD scheduler only if it batches work. **H**

**`entity_model.md` §9 Non-Goals** that this work touches:
- "Spatial partitioning for entity-entity queries (octree, grid)". Per-player perception keyed by cell is entity-to-cell, not entity-entity, but bucketing enemies by cell would read as partitioning. perf-ai-tick-cost's amendment style ("a named exception") is the precedent.
- "Full ECS (… system scheduler)": a per-entity tick-rate scheduler must not grow into a general system scheduler.

**H**

**`entity_model.md` §6 Cell Linkage.** Quote: "The entity registry stores no cell index and runs no per-entity cell update." Arena relevance needs enemy-to-cell, either per tick or amortized. That is a direct amendment to this contract. **H**

---

### 4. Testing conventions, stress fixtures, CPU stages

**`testing_guide.md` §Resource bounds.** Quote: "Manual stress and performance runs pin the fixture, inputs, machine class, worker count, cache mode, metric, baseline, and cleanup. Name upstream limits that could stop the run before the target stage. … A performance ceiling is the chosen design's measured cost plus a stated margin (default 25%), citing the run that measured it. A build over its ceiling stops for the owner; it never starts a tuning loop." Zero-allocation proof uses `postretro_sim::alloc_probe`. **H**

**`testing_guide.md` §Slow suites.** Large maps (`stress-warren*`, `campaign-test`) are `#[ignore]`-gated only, and routine tests use small synthetic fixtures. **H**

**Stress fixtures.**
- `content/dev/maps/stress-*` holds `stress-env-volumes(-baseline)`, `stress-street`, `stress-warren`, `-lit`, `-crates`, `-maze-crates`, `-mini`, `-overflow`, `-showcase`, `-hallway-inspection(-mini)`, and `teleporters-stress(-baseline)`. **H**
- They are generated by `tools/gen_stress_map.py`, which already has `--arenas N`, `--enemies N` (places `reference_enemy`, tag `enemy`), closets and `--preset warren` (`content/dev/maps/stress-warren.README.md` §Gameplay content). So a multi-arena map with hundreds of enemies is a generator invocation plus perhaps a new preset, not new tooling. The roadmap still says no committed map represents it. **M**
- The convention is that variants generated for measurement are "deleted after measurement" (perf-ai-tick-cost research §Setup). **H**

**Headless AI stress rows already exist.**
- `crates/ai/src/replan_stress_tests.rs::replan_stress_row` (`#[ignore]`) runs 0–128 wave enemies plus 8 chasers around a moving player for 600 ticks.
- An arbitration stress row and an alloc probe exist in `crates/ai/src/attack_turns/tests/tick/arbitration_stress.rs`.
- The results are in `context/plans/done/E10--enemy-combat-movement/plan.md` §Review loop. The fair 4-per-tick replan budget gives about an N/4-tick wait, 24 ticks at 128.

This harness shape fits measuring un-alerted versus engaged versus dormant cost. **H**

**`rendering_pipeline.md` §12 CPU Stage Timing.** Labels are "engine-closed and hierarchical". Each crate owns its `StageSet`, and the binary places roots by label. The tree has `sim_tick` with `sim_ai` and `sim_steering` (holding `sim_steer_agents`, `sim_steer_pose` and `sim_steer_movers`) under `fixed_step`. "Divide by `ticks` for the per-tick cost." Scopes are allocation-free. The `SimStage` enum is in `crates/sim/src/sim/cpu_stages.rs`. Any new LOD sub-stages, for example dormant versus perceived, would join this set and be documented in that table. **H**

---

### 5. Designer-facing activation, encounter and arena constructs

- **The aggro gate is the existing designer wake/sleep.**
  - `BrainComponent::aggro_armed` is host-authoritative, has no snapshot field, and defaults to open.
  - The FGD key `enabled_on_spawn` on AI enemies sets it (`data_archetype.rs::ai_aggro_armed_on_spawn`). A missing key warns and defaults open.
  - Scripts flip it with `npcs({ tag }).update({ aggro: true })` (the `updateNpcState` primitive; `scripting.md` §12 entity addressing, and `docs/scripting-reference.md` lines 2440 and 2670 for the closet pattern).
  - A closed gate skips target selection and graph evaluation, reseats to `initial` and clears steering (`compute.rs:92,339`; scripting-reference "The aggro gate skips evaluation").
  - A closed brain is therefore already a cheap form of dormancy. It still goes through steering (idle settle), pose and cooldown ticking (`compute.rs:206` says cooldowns tick "before the aggro gate").

  Design note: perception-driven dormancy must not collide with designer-driven aggro. The two are orthogonal axes, perception against authored intent. **H**
- **Spawners and monster closets.**
  - The `Spawner` component and `getMapEntities("spawner")` `.fire()` are host-only, in-tick and deterministic. Spawned enemies carry `spawned_tags` and "may take its first AI pass" on the spawn tick (`context/plans/done/E18--spawner-and-closet-containment/index.md`).
  - Closets are a first-class product pillar (`index.md` §1: "Monster closets and scripted reveals are first-class set-pieces").
  - A low-detail tick must not delay a closet reveal's first reaction. **H**
- **`ai_attack_pool`.** A point entity naming a `budget_profile` for `member_tag`. Started and stopped by trigger `command` or script. Pools run side by side, with membership by events (`build_pipeline.md` §Custom FGD line 114; `entity_model.md` §7c Attack budget). It is the closest existing "arena/encounter" grouping, but it is about combat pacing, not activation. **H**
- **"Activation" in E18 coop-activation-policy is not enemy activation.** It is per-seat player status (`loading`, `inPlay`, `down`, `reconnecting`), `session.playerCounts`, and the per-tag `occupancy.tag(tag)` family for co-op puzzles and room gates (`context/plans/done/E18--coop-activation-policy/index.md`). It is useful as a designer-side "players are in this room" signal, but it is trigger-volume based, not cell-visibility based. **H**
- **Grep found no** encounter, arena-volume or "dormant" construct for designers, and no script verb to put an enemy to sleep for cost (as opposed to aggro). "Arena" exists only as a stress-generator concept. **M**: negative result by grep.

---

### 6. Constraints that conflict with enemies ticking below 60 Hz

- **The "every tick" rule.** `entity_model.md` §3 Update says "All entities participate in applicable fixed-timestep game logic stages each tick." The one carve-out is quiescent enemies, excluded from AI and steering. A low-detail tick is a second carve-out and needs the doc amended. **H**
- **Guards evaluate every tick.** `entity_model.md` §7c invariant: "Guards evaluate every tick." Also `brain.timeInActivityMs` gives commitment windows in milliseconds, and "Newly entered activities do not evaluate transitions until the next tick". Evaluating guards at 30 Hz changes authored timing granularity, which modders can see. **H**
- **Clip time and moment checks.** These are pinned to every tick even under "Decision cadence work" (finding 0.4). `animationDone` is "true the tick after" a clip ends. **H**
- **Stride semantics are a modder contract.** `brain.acquisitionDue` is documented, so changing which ticks are "due" for LOD enemies is observable. **H**
- **Host-only AI keeps co-op prediction safe.** `networking.md` §Current contract says "`Brain`, `Agent`, `Health`, `Weapon`, and `Touchable` are never attached on the client", and §7c says "Graph evaluation is host-only". So enemy rate changes never touch client prediction or reconciliation. The visible effects are pose and transform replication (snapshots about 30 Hz, per §What gates, "about 30 times a second"), and host hit validation against the live enemy pose (§Combat authority / World-LOS-only validation, around line 1044). **H** for host-only; **M** for the snapshot rate.
- **Render interpolation.**
  - Stage 0 copies current to previous for every live entity every tick (`entity_model.md` §5 Update Order).
  - An enemy that moves only on alternate ticks interpolates as move/hold steps unless the LOD tick integrates a two-tick step or the snapshot is skipped.

  **L/M**: inferred from the stage contract.
- **Movers and pins read living agents every tick.** The mover tick's host pin pretest is over "player pawns and living agents" (§5 row 1). Steering walks pinned agents clear, and the mover blocking pass judges agents at settled positions (`movement.md` §6 "Pinned agents step clear", "Corpses ride, never block"). Knockback and release velocity land in the agent's knockback layer and decay each tick (`movement.md` line 227). These are tick-precise interactions with deterministic mover phase. **M**
- **Determinism.**
  - The M15 harness `simulate_tick_determinism_harness_matches_run_to_run_and_spawn_order` (`crates/sim/src/sim/determinism_tests.rs`) requires run-to-run and pawn-spawn-order invariance. Its design goal: "catches iteration-order / HashMap-order nondeterminism" (`context/plans/done/M15--p0-headless-sim-seam/index.md`). A stagger keyed by slot, which the roadmap proposes for cadence, is deterministic for replays but depends on slot history.
  - Enemy behavior draws are seeded "independent of spawn order" (`networking.md` §Current contract, decided, not yet built).
  - Rate assignment that depends on *player* positions (perception) is host-only, so no cross-machine determinism is needed. Replay determinism still holds if perception is computed from tick state.
  - Simulation is "deterministic at any refresh rate" (`rendering_pipeline.md` §1).

  **H/M**
- **No lockstep.** Non-goals (`index.md` §4, `networking.md` §Non-goals) rule out lockstep and rollback, so no cross-peer bit-exact enemy simulation is required. **H**
- **The relation's presentation-only invariant** (finding 0.2). **H**
- **Ghost links widen relevance.** `networking.md` §Current contract, "Ghosts constrain future relevance": "A relevance, cue-recipient or audible-set filter added later must treat an armed link as coupling the cells under its two volumes, through the cell-visibility relation … or ghosts silently vanish." A per-player perception query must union armed ghost-link couplings, and an enemy seen through a ghost must not go dormant. The id-46 table also says this coupling is "owed when runtime-masked couplings land". **H**
- **Teleporters.** Seamless teleports move a player's cell discontinuously, and id 51 already folds teleporters into its residency relation. The id-46 reachability does not mention them. **L**: check before relying on component ids around teleporters.

---

### 7. Adjacent assets the brief can reuse

- **Runtime query surface.** `LevelWorld::locate_cell`, `perceivable(a,b)` and `coupling(a,b) -> CouplingTuple { perceivable, distance: Option<u32>, aperture: Option<u32> }` are in `crates/level-loader/src/prl_queries.rs`. No runtime gameplay consumer uses them yet: grep shows only the `CELL_VISIBILITY_DISTANCE_FIXED_POINT_SCALE` constant reused in `streaming/cell_demand.rs`. A missing section falls back to all-perceivable. **H**
- **Substrate generalizability contract.** The substrate knows only `CellId`. Consumer policy (radius, hysteresis, grace period, per-client keying) "stays in the consumer". Smell tests reject APIs taking `Player`, `Entity` or `ClientId`. A "validate against a SECOND consumer on paper before merge" gate applies. The per-player perception query this brief builds is consumer-side policy and should live outside the substrate crate, keyed by player, then reused by net and VFX (`cell-visibility-substrate.md` §Generalizability contract). **H**
- **CellResidencySet (id 51).** A per-camera-cell baked set of cells the runtime portal walk may need within a movement lead (≤ 32 m, sampled through the runtime walk, teleporters included). It is closer to "can see" than id-46 reachability, but it is owned by lightmap streaming (`build_pipeline.md` §Cell residency set stage, id 51 row). Using it would cross a consumer boundary. **M**
- **Draft `registry-column-storage`.** This stub covers the registry storage shape that makes column walks O(slots), and is relevant if dormant enemies are still walked. **M**

---

### Source index

- `context/plans/roadmap.md`: lines 34–38, 75, 98, 143
- `context/plans/drafts/perf-ai-tick-cost/index.md`, `research.md`
- `context/research/cell-visibility-substrate.md`
- `context/plans/done/cell-visibility-relation/index.md`
- `context/plans/done/E10--enemy-mp-target-selection/index.md`
- `context/plans/done/E10--enemy-combat-movement/plan.md` §Review loop
- `context/plans/done/E18--coop-activation-policy/index.md`
- `context/plans/done/E18--spawner-and-closet-containment/index.md`
- `context/plans/done/E17--doors-as-occluders/index.md`
- `context/plans/done/M15--p0-headless-sim-seam/index.md`
- `context/lib/entity_model.md` §3 Update, §5, §6 Cell Linkage, §7c, §9
- `context/lib/networking.md` §Current contract
- `context/lib/ai_pathfinding_mt_readiness.md`
- `context/lib/testing_guide.md` §Resource bounds
- `context/lib/rendering_pipeline.md` §1, §12
- `context/lib/build_pipeline.md`: FGD table, id 46 and id 51 rows
- `docs/scripting-reference.md`: aggro gate, `brain.acquisitionDue`
- Code:
  - `crates/ai/src/engine_floor.rs`
  - `crates/ai/src/targeting.rs`
  - `crates/ai/src/compute.rs`
  - `crates/entities/src/components/brain.rs`
  - `crates/sim/src/nav/mod.rs`
  - `crates/sim/src/sim/cpu_stages.rs`
  - `crates/sim/src/sim/determinism_tests.rs`
  - `crates/sim/src/scripting/builtins/data_archetype.rs`
  - `crates/level-loader/src/prl.rs`, `prl_queries.rs`
  - `crates/level-format/src/cell_visibility.rs`
  - `crates/ai/src/replan_stress_tests.rs`
  - `tools/gen_stress_map.py`

## Stances and telegraphs follow-ups (folded in 2026-11-17)

Read from the stances branch before it merged, rechecked on `main` at 5dd62342e, and folded into the brief. Kept for the evidence:

- **Owner decision (2026-11-17), void once the 30 Hz cap was dropped (visible NPCs act at 60 Hz): a leaping NPC steers at 30 Hz until it lands.** `leap::pace` rewrites `arrive_in_s` every tick from `clip_clock::seconds_to_moment`, and steering's `paced_speed` lands the NPC in whole steps. At the Engaged tier's 15 Hz steering it would land a step late and whiff. `admit_replans` already sorts the drift tier by `(paced, wait)`, so within a replan tier paced agents go first, ahead of NPC-tier order. Amend the replan Decision and P13. The leap never course-corrects: `leap::steer` fixes `leap_spot` on entry and never re-aims, so a target that moves off the spot is missed (owner, 2026-11-17). The 30 Hz rule therefore serves arrival timing only, landing on the step that checks the moment, not tracking. It adds no per-step target read.
- **Homing projectiles (future, not built):** projectiles stay at 60 Hz in this brief, so a later homing projectile can re-read its target every tick without a tier rule. Perception and tiers never steer projectiles.
- **Pool key:** the branch adds `BehaviorDrawSalt::Placement(MapOrdinal)` and `Spawned{spawner, count}` (`spawner::next_spawn_count`). Derive phase from the salt word stored on the NPC, not the hashed `behavior_draws.seed`; replace the `Entity(id.to_raw())` fallback, which depends on spawn order. Extend the role-keyed reversed-`SpawnOrder` determinism rows (`role_activity_rolls`, `role_hold_releases`).
- **New tick-counted timing to convert:** `HeldTurn::granted_tick` (`graph_eval::granted_last_tick` checks `granted_tick + 1 == seen_tick`): it passes on the NPC's next run after its grant. Already time-based: combo `hold_release_ms`, `Stagger` elapsed, release exits, whiff cooldown, leap `arrive_in_s`. Queued rows reuse `since_tick`.
- **Clip clock** (30 Hz and tick-pose clauses void with the cap; kept: a dormant 2 Hz step resolves each moment inside it once, and a repeating release's passes inside one step whiff): restate "at most one moment a tick" and `warn_repeating_releases(…, tick_dt)` per 30 Hz step; the "none fired twice" row needs a repeating-release exception (several passes inside one step whiff). The tick pose honours `MeshAnimation::loop_phase` (zero phase for repeating releases). Clip time integrates `animation.rate`, which `playbackRate`/`maxPlaybackRate` can push to 2.5×.
- **attack_turns:** P1 and P2 name the queued grant: a skipped run keeps a queued grant; it opens on the next run.
- **Netcode** (void with the cap; snapshots are unchanged): `server_message_vocabulary_is_pinned_with_its_version_constants` moves with the per-step snapshot wire. Clients derive stride rate from displayed speed (`remote_enemy_stride_reference`), which fits interpolated presentation. Per-step stamps carry a repeating release's entry tick.
- **Tests pinning ticks** to add: `shipped_brute_combo_lands_three_jabs_320_ms_apart_after_a_2000_ms_hold`, `release_repeat_tests`, `release_exit_tests`, `activity_speed_tests`, `enemy_walk_rate_loopback_tests`.
- **`entity_model.md` §7c per-tick clauses** for the amendment list: leap "whole ticks left, recomputed every tick"; activity speed "governs steering from the tick … through the tick before"; release exit "at most one moment a tick"; and "Clip time is game time" still says Decision cadence keeps the accumulator on every tick.
- **Nav:** `leap::walkable_spot` adds up to 512 `resolve_region_at` calls on leap entry; `perf-ai-tick-cost`'s index serves them.

## Superseded: 30 Hz cap for visible NPCs (2026-11-17)

The brief at commit 635edd536 capped every NPC at 30 Hz, stepped Engaged NPCs' steering at 15 Hz, and paid for it with a tick-pose contract (every damage check against an interpolated pose), host drawing one step behind, a per-step snapshot wire, a raised client delay floor, client-side rebasing for NPC riders, a Close tier, a corpse-from-tick-pose rule and a 30 Hz leap exception. Pins P10 (in its 15 Hz form), P19 and P22–P26 belonged to it and are retired. The visible-rate spike (`spike-visible-rates.md`) measured the saving at 0.3–0.9 ms per tick for 64–128 engaged NPCs once the nav index lands, against 17–75 ms from the dormant tier and about three quarters of the AI tick from decide pools. The owner dropped the cap. If a later measurement shows visible acting cost matters, that commit is the starting design.

## Tests pinning ticks

`determinism_tests.rs` (extend its role-keyed reversed-`SpawnOrder` rows, `role_activity_rolls` and `role_hold_releases`, to phase), the clip-timed jab's tick pin, `shipped_brute_combo_lands_three_jabs_320_ms_apart_after_a_2000_ms_hold`, `release_repeat_tests`, `release_exit_tests`, `activity_speed_tests`, many AI tests set the stride counter by hand, `headless_byte_identity.rs`.

## Pool key

registry slots follow spawn order through the free list, and the AI's per-tick index is a snapshot position, so neither works. `BehaviorDrawSalt` carries the identity, but only its hashed seed reaches the brain; store the salt word on the NPC at seeding and hash it for phase (a Fibonacci-style multiply spreads strided ordinals). Hash the identity for phase with a mixing hash; balance stays statistical, since exact balance would need phases that depend on which other NPCs exist. The `Entity` fallback keeps the registry index (Decision).

P40, P42, P45, P46 and P48–P52 moved to `drafts/E10--stealth-opener/research.md` with the stealth split.

## Arguments moved from Decisions

- Tactical range: a slot around a target that far away is stale before the NPC arrives, and choosing one costs a long route search per candidate slot (P28).
- No-PVS ground rule: runtime value comes from metadata the bake already produces, even after its source such as the BSP is dropped.

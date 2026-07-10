# Co-op Triggers, Interaction Events, and Semi-Random Trap Pools

> **Read this when:** drafting Epic 18 (Co-op Set-Pieces) specs — buttons, pressure plates, trigger→event fan-out, monster-closet spawners, or randomized trap arming.
> **Product goal:** co-op PVE players do lightweight puzzle solving; the solving sets the stage for jump-scare monster-closet traps. A per-level script defines pools of authored traps; the engine semi-randomly arms N per pool at level load, so players learn layouts but runs stay unpredictable.
> **Key finding:** the trigger substrate is ~90% shipped (E17-C). The spec work is fan-out (what a trigger can *do*), co-op activation policy, an engine-owned spawner, and a seeded host-side arming pass. No new trigger detection machinery is needed.
> **Related:** `context/plans/done/E17--trigger-command-surface/` · `context/plans/done/E17--kinematic-platform-foundation/` · `context/lib/networking.md` · `context/lib/scripting.md` §10–11 · `context/lib/entity_model.md` §7 · roadmap Epic 17/18.

---

## 1. What already exists (reuse, don't rebuild)

| Capability | Status | Where |
|---|---|---|
| Touch trigger volume (rising-edge entry, per-player edge tracking) | Shipped | `trigger_volume` brush entity → PRL section 44 → `TriggerVolumeBridge` → `crates/postretro/src/trigger_system.rs` |
| Use trigger (overlap + Use press, per-player) | Shipped | Same system; `use_pressed` input bit replicated, host evaluates |
| Fire policy: `once` / `multiple` + `rearm_ms`, `enabled_on_spawn` | Shipped | `TriggerVolumeComponent` |
| Single activation gate receiving the activator `PlayerId` | Shipped — **deliberate E18 seam** | `evaluate_trigger_activation`; E17-C designed it as a policy swap point |
| Closed mover command vocabulary (`start`/`stop`/`reverse`/`goToPathNode`) | Shipped | Dual entry: KVP trigger and script reaction converge on one applier |
| Tag-based linking (`target_tag` → entity `_tags`) | Shipped | The engine's de-facto entity-linking currency; also the script query namespace |
| Named events → reaction fan-out (`levelLoad`, `playerDied`, kill-progress, crossings) | Shipped | Reaction registry + `setupLevel`/`ModManifest` composition |
| Host-authoritative gameplay; clients never evaluate triggers | Shipped | `networking.md` — triggers are baked map data, not replicated state |
| Mid-session server entity spawn + replication to clients (incl. late join) | Shipped mechanism | Pawn accept path (`netcode/lifecycle.rs`); enemies replicate as presentation-only remotes (E10 baseline) |
| Shared/global replicated state slots for set-piece progress | Shipped | M15 Phase 3.5 `sharedGlobal` scope |
| Per-level declarative script entry point | Shipped | `setupLevel(ctx)` data script; VM drops after load |

**Playtestable today, before any spec ships:** a `trigger_volume` (touch or use) commanding a `kinematic_mover` "door" with live enemies sealed behind it is a working monster closet end-to-end, in co-op, right now. Level-design playtesting of closet placement and pacing does not need to wait.

## 2. Gaps (the actual spec work)

| Gap | Severity | Notes |
|---|---|---|
| Triggers can only command movers | **Central** | Fire path is hardwired to the mover applier. No trigger→light/sound/damage/spawn/script-event. |
| No script hook on activation | Central | No `on_touch`/`on_use`; activation is engine-internal. The fix is trigger→named-reaction, not a callback (VM is dead at tick time). |
| No runtime entity spawn | **Central** | `spawnEntity` is deliberately absent from the script surface; no spawner entity class exists. Monster closets that spawn (vs. reveal pre-placed) enemies have no home. |
| No seeded gameplay RNG | **Central** | Only dev-harness and client-local particle RNG exist. Command-buffer IR forbids RNG by contract. Semi-random arming needs a new, host-only facility. |
| No runtime trigger enable/disable | High | `enabled_on_spawn` seeds `armed`; runtime arming was explicitly deferred to E18. Trap-pool arming and disarm-on-solve both need it. |
| No pressure-plate semantics | High | Only rising-edge entry. No leave-edge event, no while-held/occupancy state, no multi-activator counting. |
| No co-op activation policy | High | Gate discards the activator today. Co-op puzzles want any-player / N-players-simultaneous (two-plate doors) policies. |
| Enemies spawned mid-session aren't auto-registered for replication | Medium | E10 registers the replicable enemy set at level load only. Spawner work must register at spawn time — mechanism exists (pawn path), wiring is net-new. |
| Trigger/mover *latched* state has no wire mirror | Low | Mover phase replicates; trigger latch state is host-only. Matters for late-join edge cases; E17-C pointed at the standard component-mirror path. |
| Kill-progress totals are fixed at load | Low | `ProgressTracker` counts at install; runtime-spawned enemies don't raise totals. Interacts with spawner + "clear the ambush" reactions. |

## 3. Architectural constraints that shape the design

These are settled invariants; the specs must fit them, not relitigate them.

- **Scripts declare; Rust executes.** No live VM at tick time, no per-entity callbacks, no `on_touch` closures. Richer trigger behavior means triggers *fire named reactions* — declarative data bound at load — and the reaction vocabulary grows. This is the same shape as `levelLoad`/`playerDied`.
- **Host-authoritative, state-sync (not lockstep).** Only the host evaluates triggers, runs AI, spawns enemies. Clients observe results via replication. Therefore: **the arming decision is made once, host-side, and its *consequences* replicate.** A shared-seed client re-simulation model is wrong for this engine.
- **RNG posture.** Command-buffer IR is pure/total/bounded — no RNG there, ever. The right home for randomness is a host-only, engine-owned selection pass at level install, using a seeded PRNG whose seed is logged (and dev-overridable) for playtest reproducibility. Scripts declare *pools and counts*; they never observe or perform the roll.
- **Closed vocabularies over open scripting.** The mover command enum, reaction primitives, and the E17-C "later extension" note (`goToNearestNode(tag)` as a Rust-evaluated verb, not a live query) all point one way: spawner behavior and arming policy are closed, engine-evaluated verbs configured by data.
- **Tags are the linking currency.** Trap pools, trigger targets, and spawner groups should all select by `_tags`, consistent with `target_tag`, `world.query`, and reaction targeting. Do not introduce a parallel Quake `targetname` scheme.
- **Load-order seam.** Host-side arming naturally runs at level install after descriptor placements materialize (the same post-dispatch point E10 uses for enemy replication registration), gated to host/single-player roles exactly as enemy spawns and the HUD publisher already are.

## 4. Design direction per gap

### 4.1 Trigger → named-reaction fan-out (the one generalization that pays for everything)

Add a trigger fire action that dispatches a **named reaction** through the existing registry, alongside the direct mover command. One new KVP (e.g. `on_fire` = reaction name) connects the shipped trigger substrate to the entire existing reaction vocabulary — movers, lights, fog, emitters, `applyDamage`, sounds, future spawn verbs — with no new event machinery. FGD-only authors keep the direct mover path for the simple case; everything richer routes through a reaction the level script defines. Consider a leave-edge counterpart (`on_exit`) in the same pass — it is the other half of pressure-plate semantics and shares all plumbing.

### 4.2 Pressure plates and buttons — authoring patterns, not new entity classes

- **Pressure plate** = `trigger_volume` + new semantics: leave-edge firing and an occupancy count maintained per trigger (the per-player overlap map already exists). "While held" behavior falls out of enter/exit reaction pairs; no per-tick script needed.
- **Button** = composite authoring pattern: a `use`-activation `trigger_volume` co-located with visible geometry (a small `kinematic_mover` gives a free depress animation; `prop_mesh` for static buttons). Document the pattern; defer a `func_button` sugar class until authoring friction proves it's needed (FGD-from-registry is already on the Future list).

### 4.3 Co-op activation policy (fill the E17-C seam)

`evaluate_trigger_activation` already receives and discards the activator. Grow it into a small closed policy vocabulary on the trigger: any-player (default, today's behavior), and N-players-occupying / all-players-occupying for simultaneous-plate co-op puzzles. Occupancy counting comes from 4.2. This is the "trigger ownership" line item in the E18 charter, and it is what makes the *puzzle* half of the product goal co-op-native rather than single-player-with-spectators.

### 4.4 Arm/disarm at runtime

Two entry points over one mechanism: reaction primitives (`armTrigger`/`disarmTrigger`, tag-targeted like every other entity reaction) and the engine arming pass (4.6). Semantics: writing `armed`, honoring existing `once`/`rearm` state. Also the disarm-on-puzzle-solve verb level designers will want.

### 4.5 Engine-owned spawner entity (the monster-closet payload)

A point entity (working name `entity_spawner`) placed in maps: archetype `canonicalName`, count, spawn transform(s), and a firing hook (armed + triggered via reaction or `target_tag`). Spawn executes host-side through the existing registry spawn path; each spawned enemy is stamped with a `NetworkId` and registered into the replicable set **at spawn time** (extending E10's load-time-only registration). Two closet flavors both become authorable:

- **Reveal** (Doom-classic): pre-placed live enemies behind a door; trigger opens the mover. Works today; spawner not required.
- **Spawn**: empty closet until triggered; spawner materializes enemies on activation. Cheaper maps, true surprise, supports respawning waves later.

Script surface stays declarative: scripts may *reference* spawners by tag in reactions (`spawnFromSpawner` reaction primitive); there is still no `world.spawn`.

### 4.6 Trap pools + seeded arming at level load

The headline feature. Shape:

- **Declaration:** `setupLevel` returns pool definitions (working name `trapPools`): each pool = a tag selector + an arm count (and later, weights). Pure manifest data, same as reactions/crossings. Members are triggers and/or spawners authored `enabled_on_spawn = false`, tagged into pools in TrenchBroom.
- **Selection:** at level install, host-only, after placements materialize: engine resolves each pool's member set by tag, seeds a PRNG (map + session derived; seed logged, dev-overridable via CLI for reproducible playtests), picks N per pool, arms them via 4.4.
- **Replication:** none required for correctness — clients never evaluate triggers, and armed traps manifest to clients only through their consequences (mover motion, enemy spawns), which already replicate. Optionally project the armed set into a `sharedGlobal` state slot for dev overlay / spectator UI.
- **Not** command-buffer IR (RNG forbidden there), **not** a script-visible roll, **not** a shared-seed client computation.

### 4.7 Dev/playtest support

A dev-tools overlay showing trigger AABBs, armed/disarmed state, occupancy, and selected pool members — the E10 agent-diagnostics precedent says land the instrument before the feel tuning. Seed echo in logs so a playtest bug reproduces.

## 5. Proposed spec sequence (opens Epic 18)

Epic 18's charter already names this work ("trigger ownership, reveal/spawn fan-out, shared progress, late-join restoration, … one playable encounter"). Proposed detail-on-open sequence, each its own `/draft-plan` → `/orchestrate` cycle:

| # | Spec (working name) | Contents | Depends on |
|---|---|---|---|
| E18-A | **Trigger event fan-out + plate semantics** | Trigger→named-reaction fire path (`on_fire`), leave-edge (`on_exit`), occupancy state, arm/disarm reaction primitives + runtime enable, FGD KVPs, dev overlay | E17-A/C (shipped) |
| E18-B | **Co-op activation policy** | Policy vocabulary in the activation gate (any / N-simultaneous / all), consuming A's occupancy | E18-A |
| E18-C | **Spawner entity + reveal/spawn fan-out** | `entity_spawner` point class end-to-end (FGD → compiler → PRL → runtime), host-side spawn, spawn-time replication registration, `spawnFromSpawner` reaction, kill-progress interaction decision | E18-A; E10 baseline (shipped) |
| E18-D | **Trap pools + seeded arming** | `trapPools` manifest surface, engine seeded-RNG selection pass at install (host-gated), seed logging/override, optional `sharedGlobal` armed-set projection | E18-A, E18-C |
| E18-E | **Playable co-op encounter (capstone)** | One authored level: plate/button puzzle → staged reveal → semi-random closets; co-op playtest incl. late join; folds in trigger-state wire mirror if playtest demands | E18-A–D |

Sizing intuition: A and B are small (extensions of shipped seams); C is medium (one new entity class across the full pipeline — the `trigger_volume`/`kinematic_mover` compiler/loader/bridge pattern is the template); D is small-medium once C exists; E is a level + playtest + fixes wave.

**Wave guidance:** A can land alone quickly. B and D are candidates to pair with their prerequisites in one wave. C should not share a wave with A (different pipeline layers, but C is the integration-heavy one — same reasoning as E17's "A lands alone").

### Roadmap placement and cross-epic interactions

- **Epic 18 becomes the active epic for this work**; the sequence above is its opening spec set. The Future/Speculative "World Entities" entry (trigger volumes, doors-as-base-scripts, "scripted ambush set piece") is partially absorbed by E18-A/C and should be annotated accordingly when the roadmap updates.
- **E17-E (doors/blocking movers)** is the natural companion, not a prerequisite: closets work with displace-only movers today. Pull E17-E forward if playtests show door crush/blocking/interruption mattering to trap feel. A combined E17-E + E18-E playtest wave matches E17's own "later C + E wave" guidance.
- **E17-B (kinematic visual parity)** raises closet-door presentation quality; independent, schedule on visual demand.
- **Epic 12 (spatial audio)** is the biggest jump-scare force multiplier (positional monster sounds behind doors) but is not a blocker; the E18-E capstone gets better whenever Epic 12's spatial/sound-event specs land.
- **Epic 15 Phase 4** (late-join at scale) and the runtime-level-lifecycle × co-op interaction remain out of scope here; E18-E should note late-join behavior as observed, not re-architect it.

## 6. Risks and caveats

- **Aggro through closed doors.** Doors don't block portal visibility (E17-F territory), and enemy target selection is PVS-coarse today. Pre-placed closet enemies may aggro the moment the closet's portal is baked open. Mitigations, cheapest first: author closets as spawner-based (E18-C) so nothing exists to aggro; or a dormant-until-event brain gate; or the E10 line-of-sight spec. Decide in E18-C's draft; don't assume reveal-closets are free.
- **Kill-progress totals.** Spawned enemies don't raise `ProgressTracker` totals. "Clear the ambush" reactions over spawner output need either total-raising on spawn or a spawner-scoped kill event. Settle in E18-C.
- **Runtime level reload × co-op** is unproven ground (declared out of scope by runtime-level-lifecycle). Trap arming must key off the install seam so a reload re-rolls cleanly in single-player; co-op reload behavior is observed-only for now.
- **Determinism discipline.** The mover driver's determinism contract is load-bearing for client prediction. The RNG pass must stay strictly at install time and host-side; never introduce randomness into per-tick mover/trigger evaluation.

## 7. Non-goals

- Live-VM script callbacks (`on_touch(fn)`) — contradicts the scripting invariant.
- Shared-seed client-side random re-simulation — contradicts the state-sync posture.
- Quake-style `targetname`/`target` string pairing — tags are the linking currency.
- Line-of-sight / aim-ray triggers — separate concern, tracked with E10 LOS work.
- General logic-gate entity graphs (Source-style I/O) — reaction composition covers current needs; revisit only on authoring evidence.
- Mid-session pool re-rolls or wave-director systems — arming is a load-time decision in this pass; a runtime encounter director is future work with its own design questions.

# Co-op Triggers, Interaction Events, and Semi-Random Trap Pools

> **Read this when:** drafting Epic 18 (Co-op Set-Pieces) specs — buttons, pressure plates, trigger→event fan-out, monster-closet spawners, or randomized trap arming.
> **Product goal:** co-op PVE players do lightweight puzzle solving; the solving sets the stage for jump-scare monster-closet traps. A per-level script defines pools of authored traps; the engine semi-randomly arms N per pool at level load, so players learn layouts but runs stay unpredictable.
> **Key finding:** trigger *detection* is ~90% shipped (E17-C). The spec work is fan-out (what a trigger can do, and whether clients see it), co-op activation policy, an engine-owned spawner, respawn policy, and a seeded host-side arming pass. Two traps for spec writers, verified against source: most reaction verbs are host-local and silently no-op on connected clients, and reveal-style closets are broken today (enemies aggro through walls and path through closed doors).
> **Adversarial review:** this doc's claims were adversarially reviewed against source (2026-07); §1/§4.1/§6 reflect corrections from that pass.
> **Related:** `context/plans/done/E17--trigger-command-surface/` · `context/plans/done/E17--kinematic-platform-foundation/` · `context/lib/networking.md` · `context/lib/scripting.md` §10–11 · `context/lib/entity_model.md` §7 · roadmap Epic 17/18.

---

## 1. What already exists (reuse, don't rebuild)

| Capability | Status | Where |
|---|---|---|
| Touch trigger volume (rising-edge entry, per-player edge tracking) | Shipped | `trigger_volume` brush entity → PRL section 44 → `TriggerVolumeBridge` → `crates/postretro/src/trigger_system.rs` |
| Use trigger (overlap + Use press, per-player) | Shipped | Same system; `use_pressed` input bit replicated, host evaluates |
| Fire policy: `once` / `multiple` + `rearm_ms`, `enabled_on_spawn` | Shipped | `TriggerVolumeComponent` |
| Single activation gate receiving the activator `PlayerId` | Shipped — **deliberate E18 seam** | `evaluate_trigger_activation`; E17-C designed it as a policy swap point |
| Trigger evaluation inside the fixed tick / headless sim seam | Shipped | Trigger system runs in `simulate_tick` between movement and AI; the mover applier fires in-tick — the precedent E18-A's consequential dispatch extends |
| Closed mover command vocabulary (`start`/`stop`/`reverse`/`goToPathNode`) | Shipped | Dual entry: KVP trigger and script reaction converge on one applier |
| Tag-based linking (`target_tag` → entity `_tags`) | Shipped | The engine's de-facto entity-linking currency; also the script query namespace |
| Named events → reaction fan-out (`levelLoad`, `playerDied`, kill-progress, crossings) | Shipped | Reaction registry + `setupLevel`/`ModManifest` composition; dispatch is app-level, outside the sim tick |
| Per-peer crossing detection over replicated slots | Shipped | `CrossingDetector` runs on every peer with no role gate; client slot tables are fed by M15 P3.5 replication — the sanctioned host→client presentation channel |
| Host-authoritative gameplay; clients never evaluate triggers | Shipped | `networking.md` — triggers are baked map data, not replicated state |
| Mid-session server entity spawn + replication to clients (incl. late join) | Shipped mechanism | Pawn accept path (`netcode/lifecycle.rs`); enemies replicate as presentation-only remotes (E10 baseline); mesh animation state is on the wire |
| Respawn-as-teleport reconciliation | Shipped (netcode half) | M15 P3 correction classes; game-flow respawn *policy* is unowned — see E18-R |
| Shared/global replicated state slots for set-piece progress | Shipped | M15 Phase 3.5 `sharedGlobal` scope |
| Per-level declarative script entry point | Shipped | `setupLevel(ctx)` data script; VM drops after load |

**Playtestable today, before any spec ships:** touch/use triggers commanding movers, **any-player activation** — plates and buttons opening doors work in co-op right now, so puzzle-layout playtesting can start immediately. Simultaneous-plate puzzles need E18-A+B. Sealed reveal-closets are **not** playtestable today — see §6: enemies aggro through walls and path through closed doors.

## 2. Gaps (the actual spec work)

| Gap | Severity | Notes |
|---|---|---|
| Triggers can only command movers | **Central** | Fire path is hardwired to the mover applier. No trigger→light/sound/damage/spawn/script-event. |
| Most reaction verbs don't reach co-op clients | **Central** | Fog/light/emitter state and system reactions (`playSound`, `flashScreen`, …) are deliberately off the wire; a host-side trigger firing them mutates host-local state only. Works in single-player, silently no-ops on clients. See §4.1. |
| Reaction dispatch lives outside the sim tick | High | Named-event fan-out drains at the app level; trigger consequences (spawn, damage, arm) must execute inside the headless `simulate` seam or the dedicated-server north star and determinism harness lose them. See §4.1 bind-at-install. |
| No runtime entity spawn | **Central** | `spawnEntity` is deliberately absent from the script surface; no spawner entity class exists. |
| Enemies have no visibility check and ignore mover geometry | **Central** | Target acquisition is XZ distance only (the injectable visibility predicate ships unfilled — every call site passes none); movers are absent from navmesh input and the static BVH, and nav/steering never consult them. Reveal-closets self-open: the enemy aggros through the wall and walks through the closed door. |
| No seeded gameplay RNG | **Central** | Only dev-harness and client-local particle RNG exist. Command-buffer IR forbids RNG by contract. |
| No runtime trigger enable/disable | High | `enabled_on_spawn` seeds `armed`; runtime arming was explicitly deferred to E18. |
| No pressure-plate semantics | High | Only rising-edge entry. No leave-edge event, no while-held/occupancy state, no multi-activator counting. |
| No co-op activation policy | High | Gate discards the activator today. Co-op puzzles want any-player / N-players-simultaneous policies. |
| No co-op respawn / player-leave policy | High | Charter-named. `playerDied` fires a mod-bound game-flow verb; there is no respawn path. A lethal-trap capstone playtest hits this immediately. |
| Enemies spawned mid-session aren't auto-registered for replication | Medium | E10 registers the replicable enemy set at level load only. Mechanism exists (pawn accept path); wiring is net-new. |
| Trigger/mover *latched* state has no wire mirror | Low | Mover phase replicates; trigger latch state is host-only. Matters for late-join edge cases; E17-C pointed at the standard component-mirror path. |
| Kill-progress totals are fixed at load | Low | `ProgressTracker` counts at install; runtime-spawned enemies don't raise totals. Interacts with spawner + "clear the ambush" reactions. |

## 3. Architectural constraints that shape the design

These are settled invariants; the specs must fit them, not relitigate them.

- **Scripts declare; Rust executes.** No live VM at tick time, no per-entity callbacks, no `on_touch` closures. Richer trigger behavior means triggers *fire named reactions* — declarative data bound at load — and the reaction vocabulary grows.
- **Host-authoritative, state-sync (not lockstep).** Only the host evaluates triggers, runs AI, spawns enemies. Clients observe results via replication. Therefore: **the arming decision is made once, host-side, and its *consequences* replicate.** A shared-seed client re-simulation model is wrong for this engine.
- **State-sync converges values, not edges.** Slot replication repairs toward the current value; a pulse (0→1→0) can be missed entirely under loss. Anything clients must reliably converge on is encoded as *state* written by idempotent setters; transient events are best-effort unless a replicated event stream is built (it isn't, and shouldn't be until demanded).
- **RNG posture.** Command-buffer IR is pure/total/bounded — no RNG there, ever. The home for randomness is a host-only, engine-owned selection pass at level install, seeded PRNG, seed logged and dev-overridable. Scripts declare *pools and counts*; they never observe or perform the roll.
- **Closed vocabularies over open scripting.** Spawner behavior and arming policy are closed, engine-evaluated verbs configured by data — the same shape as mover commands and E17-C's deferred `goToNearestNode(tag)` note.
- **Tags are the linking currency.** Trap pools, trigger targets, and spawner groups all select by `_tags`. No parallel Quake `targetname` scheme.
- **Load-order seam.** Host-side arming runs at level install after descriptor placements materialize (the same post-dispatch point E10 uses for enemy replication registration), gated to host/single-player roles exactly as enemy spawns and the HUD publisher already are.

## 4. Design direction per gap

### 4.1 Trigger → named-reaction fan-out, with an effect-based dispatch split

Add a trigger fire action that dispatches a **named reaction**, alongside the direct mover command. One new KVP (`on_fire` = reaction name) connects the trigger substrate to the reaction vocabulary. But two verified facts mean this is *not* "free fan-out to everything":

**Classify every reaction verb by effect, not by mechanical family:**

| Class | Definition | Examples | Dispatch | Client visibility |
|---|---|---|---|---|
| **Consequential** | Writes authoritative or replicating state | mover commands, `applyDamage`, arm/disarm, `spawnFromSpawner`, `setState` (writes slots — replicated), `setAnimationState` (wire payload) | In-tick, inside the headless sim seam | Via existing replication, by construction |
| **Presentation** | Mutates client-local render/audio state | fog/light/emitter mutations, `playSound`, `flashScreen`, `rumble` | App-level drain (today's path) | **Host-local in v1** — does not reach clients |
| **Lifecycle request** | App-level flow control | `loadLevel`, `restartLevel`, `returnToFrontend` | Queued app-level; host decides | Co-op semantics deferred (reload×co-op is unproven ground) |

`setState` is the straddler that breaks any family-based rule: it files under system reactions today but writes replicated slots — it must be consequential, because it is the very verb the client-visibility channel below rides on.

**Dispatch: bind at install.** Resolve each trigger's `on_fire` name at level install into a validated, pre-bound list of consequential command descriptors (the E14 bind/eval split shape). In-tick execution then touches only the entity registry — no `ScriptCtx`, data registry, or reaction machinery threads into the sim seam — and presentation residue queues to the existing app drain. The trigger system already executes mover commands in-tick, so this extends a shipped precedent rather than inventing architecture, and it keeps the deterministic-harness story trivial.

**Client visibility for presentation (the co-op jump-scare channel):**

- **Persistent atmosphere** (lights out, fog surge, door-open ambience): host writes a `sharedGlobal` slot; each peer's crossing detector fires the presentation reaction locally. Verified: crossing detection runs on every peer with no role gate, over the P3.5-fed slot table. Required discipline: encode *state not pulses*, reactions must be idempotent setters, and E18-A must name the detector-init-before-baseline-apply ordering as a load-bearing assumption (late-join restoration of persistent state currently works because init precedes baseline apply; a latched one-shot slot also *replays* its crossing on join — design slots accordingly).
- **Transient stings** (sound sting, screen flash): structurally lossy over state-sync — a missed snapshot loses the pulse and repair converges the already-reset value. Accepted host-local in v1. If playtests demand reliable co-op stings, that is a replicated event broadcast earning its place as its own scoped item, with its own late-join story.

**Also in this spec:** leave-edge firing (`on_exit`) with **paired gating** — an exit fires iff its matching enter fired, independent of `once` latching and the rearm window (unpaired gating strands doors open; this is new semantics, not shared plumbing) — and FGD/compiler validation churn: `target_tag`/`command` become optional when `on_fire` is present.

### 4.2 Pressure plates and buttons — authoring patterns, not new entity classes

- **Pressure plate** = `trigger_volume` + leave-edge and an occupancy count per trigger (the per-player overlap map already exists). "While held" falls out of enter/exit reaction pairs; no per-tick script.
- **Button** = composite authoring pattern: a `use`-activation `trigger_volume` co-located with visible geometry (a small `kinematic_mover` gives a free depress animation; `prop_mesh` for static buttons). Document the pattern; defer a `func_button` sugar class until authoring friction proves the need.

### 4.3 Co-op activation policy (fill the E17-C seam)

Grow `evaluate_trigger_activation` into a small closed policy vocabulary: any-player (default, today's behavior), N-players-occupying, all-players-occupying — the simultaneous-plate co-op puzzle. Occupancy comes from 4.2. **Policy must define whether dead pawns count:** the trigger system iterates all player pawns with no alive check, so a corpse on a plate holds it down. Decide here, gate in E18-R.

### 4.4 Arm/disarm at runtime

Two entry points over one mechanism: reaction primitives (`armTrigger`/`disarmTrigger`, tag-targeted) and the engine arming pass (4.6). Also the disarm-on-puzzle-solve verb level designers will want, and the arming input for the dormant-brain gate (4.5).

### 4.5 Engine-owned spawner entity (the monster-closet payload)

A point entity (working name `entity_spawner`): archetype `canonicalName`, count, spawn transform(s), fired via reaction or `target_tag`. Spawn executes host-side through the existing registry path; each spawned enemy is stamped with a `NetworkId` and registered into the replicable set **at spawn time** (extending E10's load-time-only registration; the pawn accept path is the template). Script surface stays declarative (`spawnFromSpawner` reaction primitive); there is still no `world.spawn`.

**Spawn-flavor closets are the critical path for the capstone.** The reveal flavor (pre-placed live enemies behind a door) is broken today — no visibility check, and agents neither see nor collide with mover geometry, so a pre-placed enemy aggros through the wall and paths through the closed door (§6). E18-C therefore also owns **closet containment** for the reveal flavor: a dormant-until-armed Brain gate (closed-vocabulary, driven by the same arm/disarm verbs as 4.4) as the cheap fix, with agent-vs-mover awareness noted as a broader unowned gap (agents interpenetrate any mover, not just doors).

**Remote-visibility windup:** a spawned enemy appears on clients one interpolation delay after it exists on the host. The delay is adaptive but hard-clamped (≤ 250 ms interpolation component; baseline delivery adds a loss-dependent tail). Specable AC: spawner enforces a minimum pre-attack windup ≥ the interpolation clamp + the standard net profile's delivery budget, harness-asserted at the E15 reference `LinkConfig` — asserted at the standard profile, not claimed universally. The windup doubles as jump-scare animation time.

**Kill-progress decision:** spawned enemies don't raise `ProgressTracker` totals (counted at install). "Clear the ambush" reactions need either total-raising on spawn or a spawner-scoped kill event. Settle in this spec.

### 4.6 Trap pools + seeded arming at level load

The headline feature. Shape:

- **Declaration:** `setupLevel` returns pool definitions (working name `trapPools`): each pool = a tag selector + an arm count (later, weights). Pure manifest data, same as reactions/crossings. Members are triggers and/or spawners authored `enabled_on_spawn = false`, tagged into pools in TrenchBroom.
- **Selection:** at level install, host-only, after placements materialize: engine resolves each pool's member set by tag, seeds a PRNG, picks N per pool, arms them via 4.4.
- **Seed policy (decided):** **fresh roll per level install, including same-session restarts.** Rationale: N per pool is constant, so there is no count to scum toward; a re-rolled retry preserves the "on their toes" goal while layout knowledge (where traps *can* be) still accrues. Residual placement-scumming (restarting until a trap vacates the critical path) is priced by restart cost — revisit only if playtests observe it. Seed is logged every roll and CLI-overridable for reproducible playtests. The alternative (sticky per-campaign-run seed) was considered and rejected for retry tension; reopen only with playtest evidence.
- **Replication:** none required for correctness — clients never evaluate triggers, and armed traps manifest to clients only through consequences (mover motion, enemy spawns), which already replicate. The armed set feeds the **host-local dev overlay** only. If a spectator/UI need ever materializes, the stable shape is one Array slot per *declared pool* (pool names are declared, so dotted names are stable; members are ordinals in PRL record order) — sketched here so it isn't reinvented, not scoped.
- **Not** command-buffer IR (RNG forbidden there), **not** a script-visible roll, **not** a shared-seed client computation.

### 4.7 Dev/playtest support

A dev-tools overlay showing trigger AABBs, armed/disarmed state, occupancy counts, and selected pool members — the E10 agent-diagnostics precedent says land the instrument before the feel tuning. Seed echo in logs so a playtest bug reproduces.

## 5. Proposed spec sequence (opens Epic 18)

Epic 18's charter already names this work ("trigger ownership, reveal/spawn fan-out, shared progress, late-join restoration, respawn and player-leave policy, … one playable encounter"). Proposed detail-on-open sequence, each its own `/draft-plan` → `/orchestrate` cycle:

| # | Spec (working name) | Contents | Depends on |
|---|---|---|---|
| E18-A | **Trigger event fan-out + plate semantics** | `on_fire`/`on_exit` with paired gating, effect-based dispatch split (consequential in-tick via bind-at-install / presentation app-drain / lifecycle), persistent-atmosphere channel (slot + crossing, idempotent-setter discipline), occupancy state, arm/disarm primitives + runtime enable, FGD KVP validation churn, dev overlay | E17-A/C (shipped) |
| E18-B | **Co-op activation policy** | Policy vocabulary in the activation gate (any / N-simultaneous / all), consuming A's occupancy; defines dead-pawn occupancy semantics | E18-A |
| E18-C | **Spawner entity + closet containment** | `entity_spawner` end-to-end (FGD → compiler → PRL → runtime), host-side spawn, spawn-time replication registration, pre-attack windup AC, `spawnFromSpawner` reaction, dormant-brain gate for reveal closets, kill-progress decision | E18-A; E10 baseline (shipped) |
| E18-D | **Trap pools + seeded arming** | `trapPools` manifest surface, engine seeded-RNG selection pass at install (host-gated), fresh-roll-per-install seed policy + CLI override, dev-overlay armed-set | E18-A, E18-C |
| E18-R | **Minimal co-op respawn policy** | Dead pawn respawns at a placement after a delay (netcode half shipped: M15 P3 respawn-as-teleport). Owns: `playerDied` one-shot latch re-arms on respawn (second death must fire again); dead-state gating of trigger/occupancy interaction (with B) | E15 P3 (shipped); coordinates with E18-B |
| E18-E | **Playable co-op encounter (capstone)** | One authored level: plate/button puzzle → staged reveal → semi-random spawn-closets; co-op playtest incl. late join and player death; folds in trigger-state wire mirror if playtest demands | E18-A–D, E18-R |

Sizing: **A is medium** (the dispatch split and bind-at-install are real design work, not a KVP addition); B small; C medium (one new entity class across the full pipeline — the `trigger_volume`/`kinematic_mover` compiler/loader/bridge pattern is the template — plus containment); D small-medium; R small but not free (two named edges above). R is disjoint from D (health/game-flow vs manifest/RNG) and can run parallel if wave capacity allows.

**Wave guidance:** A lands alone (it is the integration surface everything else binds to). B and D pair naturally with their prerequisites. C should not share a wave with A. R∥D is a reasonable wave. E is a level + playtest + fixes wave.

### Roadmap placement and cross-epic interactions

- **Epic 18 becomes the active epic for this work**; the sequence above is its opening spec set. The Future/Speculative "World Entities" entry (trigger volumes, doors-as-base-scripts, "scripted ambush set piece") is partially absorbed by E18-A/C and should be annotated when the roadmap updates.
- **E17-E (doors/blocking movers)** is a companion, not a prerequisite: closets work with displace-only movers. Pull it forward if playtests show door crush/blocking/interruption mattering to trap feel. A combined E17-E + E18-E playtest wave matches E17's own "later C + E wave" guidance.
- **E17-F (doors as occluders) is not a prerequisite for any of these specs — the dependency runs the other way.** The closed door renders as opaque geometry, so depth testing hides the closet interior visually even though the portal is baked open; the real first-playtest spoilers are the aggro/pathing gaps, which E17-F would not fix (and which 4.5 owns). The E18-E capstone is the kind of concrete set-piece / profiled evidence E17-F says it is waiting for — E18 produces F's motivating consumer, not the reverse.
- **E17-B (kinematic visual parity)** raises closet-door presentation; independent, schedule on visual demand.
- **Epic 12 (spatial audio)** is the biggest jump-scare force multiplier but not a blocker; note that baked audio occlusion cannot model door state (movers are outside static bakes), one more reason spawn-flavor closets are the default.
- **Epic 15 Phase 4** (late-join at scale) and runtime-level-lifecycle × co-op remain out of scope; E18-E observes late-join behavior, it does not re-architect it.

## 6. Risks and caveats

- **Reveal-closets are broken today, twice over.** Enemy target acquisition has **no visibility check** — the injectable predicate ships unfilled, acquisition is XZ distance — so a pre-placed closet enemy aggros the approaching player through the wall. It then **paths through the closed door**: movers are excluded from navmesh input and the static BVH, and nav/steering never consult mover geometry, so the doorway is nav-connected and the enemy interpenetrates the door mesh. The roadmapped E10 line-of-sight spec would **not** fix this — it raycasts the world BVH, which the door is not in, so LOS through the baked-open doorway reads clear. Real mitigations: spawn-flavor closets (nothing exists to aggro) and the dormant-brain gate (4.5). Do not draft E18-E assuming reveal closets work.
- **Presentation reactions are host-local.** Any co-op set-piece whose payoff is atmospheric (lights, fog, sound) must route through the persistent-atmosphere channel (4.1) or accept host-only playback. Spec writers should treat "does this reach the client?" as a per-verb checklist item.
- **Crossing-channel ordering assumption.** Late-join restoration of persistent atmosphere works because detector init precedes baseline apply in the current boot→load→connect flow. Name it in E18-A; any future in-session transition that reorders them silently drops the restore edge.
- **Kill-progress totals** are fixed at load; spawner output needs the 4.5 decision.
- **Runtime level reload × co-op** is unproven ground (declared out of scope by runtime-level-lifecycle). Arming keys off the install seam so single-player reload re-rolls cleanly; co-op reload behavior is observed-only for now.
- **Determinism discipline.** The mover driver's determinism contract is load-bearing for client prediction. The RNG pass stays strictly at install time and host-side; never introduce randomness into per-tick mover/trigger evaluation.

## 7. Non-goals

- Live-VM script callbacks (`on_touch(fn)`) — contradicts the scripting invariant.
- Shared-seed client-side random re-simulation — contradicts the state-sync posture.
- Activator parameterization of reactions ("damage whoever pressed") — reaction args are load-time-fixed; if demanded, the closed extension is an activator-target selector resolved engine-side, not arbitrary context. Deferred until a real design needs it.
- A replicated transient-event broadcast — transient stings are accepted host-local in v1; build the broadcast only on playtest demand, with its own late-join story.
- Quake-style `targetname`/`target` string pairing — tags are the linking currency.
- Line-of-sight / aim-ray triggers — separate concern, tracked with E10 LOS work.
- General logic-gate entity graphs (Source-style I/O) — reaction composition covers current needs; revisit only on authoring evidence.
- Mid-session pool re-rolls or wave-director systems — arming is a load-time decision in this pass; a runtime encounter director is future work with its own design questions.

# gameplay-stack--ai-and-physics-crates — research

> Source-grounded derivation behind the brief. Not the contract. Point-in-time
> snapshot, read at 66e5caa (= origin/codex/gameplay-stack--sim-and-netcode-crates,
> M2). Every landed extraction moves files — re-ground against the live tree before
> building. Symbols, not line numbers.

## Crate sizes at read-at (src .rs LOC)

| Crate | LOC | Note |
|---|---|---|
| postretro-sim | ~116,826 | the central mass after M2; ~2x the binary |
| postretro (binary) | ~59,718 | main.rs ~13.6K; ~10K frame-path bridges; startup, capture |
| postretro-netcode | ~45,478 | |
| scripting-core | ~45,098 | already owns reaction_dispatch + store_bridge |
| entities | ~17,700 | net ~11,622 · foundation ~8,871 · combat-model 262 |
| AI cluster (`scripting_systems/ai/`) | ~6,500 | inside sim |
| physics candidate (collision+nav+combat_positioning) | ~5,500 | of which collision ~1.6K, movement ~6.3K, kinematic_mover ~5.4K separately |

The AI carve removes ~19K from sim (ai/ + ai_tests.rs). The physics leaf removes ~13K
(collision + movement + kinematic_mover). Sim shrinks from both ends.

## The premise the epic committed is stale

Epic hub + `development_guide.md` §Target shape: "Enemy AI remains fused to the scripting
VM runtime (script store and reaction dispatch) and sim … until a later scripting-VM
decomposition can carve out `postretro-ai`." At read-at:

- `scripting_systems/ai/` has **zero** non-test references to `scripting::primitives::store`
  (read/write_store_slot) or to reaction dispatch. Its only VM-host reach is
  `scripting::builtins::data_archetype::find_descriptor` (in `brain_programs.rs`), a pure
  lookup over a `&[EntityTypeDescriptor]` slice already injected via `AiTickInputs`.
- `scripting-core` already contains `reaction_dispatch.rs` and `store_bridge.rs`; sim's
  `scripting/primitives/store.rs` is a re-export shim. The "sink to scripting-core" work
  is done.
- No host→systems registration indirection exists. sim calls ai directly (`sim::simulate_tick`
  → `run_ai_tick_with_navigation_and_impact`). The scripting host in fact depends *downward*
  on ai constants (`data_archetype`, `impact_policy` import `FACTION_STATE_FIELD`).
- `AiRuntime` is plain data: warn-once latches, a derived `BrainPrograms`, an entity LOS-grace
  map. No mlua/rquickjs handle, no store, no dispatch state.

So M3 is not a VM decomposition. The one real seam is the `sim ↔ ai` tick edge.

## The sim ↔ ai coupling (production)

**sim → ai** (edges the inversion + foundation sink must remove):
- `sim::simulate_tick` / `simulate_tick_with_presentation_aim` — param `&mut AiRuntime`,
  call `run_ai_tick_with_navigation_and_impact` with `AiTickInputs { nav_graph,
  collision_world, descriptors, descriptor_generation, factions, faction_sentiment }`.
  simulate_tick is called only from the binary (`main.rs`, `observability/driver.rs`) — not netcode.
- `sim::update_brain_animation_playback_rates` → `ai::locomotion_animation(&graph)`.
- `impact_effects::resume_recovered_brain_presentation` → `ai::rest_animation(&graph)`.
- `impact_policy` → const `FACTION_STATE_FIELD`; `scripting/builtins/data_archetype` → consts
  `FACTION_STATE_FIELD`, `ARCHETYPE_TOLERANCE_STATE_FIELD`.
- All other sim→ai references (`spawner`, `weapon_stage`, `predict_reconcile`, the sim/mod and
  determinism/divergence harnesses) are `#[cfg(test)]` or `dev-tools`-gated.

`locomotion_animation` / `rest_animation` are `fn(&BehaviorGraphDescriptor) -> Option<&str>`;
`BehaviorGraphDescriptor` is a `postretro_foundation` type. The two functions and the two
constants are foundation-shaped — they sink cleanly.

**ai → sim / lower** (become down-edges once ai is on top):
- `crate::nav::{NavGraph, find_path, distance_xz}`, `crate::collision::CollisionWorld`,
  `crate::combat_positioning::*`, `crate::agent_steering::*` — hot reads, stay concrete.
- `crate::weapon::ProjectileLaunch`, `crate::sim::{spawn_projectile, EnemyProjectilePresentationSpawn}`
  — sim-core; `spawn_projectile` becomes an `AiHost` method, `EnemyProjectilePresentationSpawn`
  its return shape.
- `crate::scripting_systems::health::{is_quiescent, is_damage_target_eligible}` — become `AiHost`
  methods (effects/observation).
- workspace: `postretro_entities`, `postretro_foundation` (heavy — graph/IR/verb types),
  `postretro_scripting_core::data_descriptors::EntityTypeDescriptor`, glam.

**The act-then-observe constraint.** In `ai/apply.rs` the attack path spawns a projectile and
branches on the returned id (commits the fire / pushes the presentation spawn only on Some), and
applies melee damage then reads `is_quiescent` before `on_impact`. A deferred end-of-tick intent
batch cannot express this; the `AiHost` seam is synchronous.

## netcode → ai

One production edge: `netcode` client-apply path calls `scripting_systems::ai::locomotion_animation`
to derive a remote enemy's walk animation from its behavior graph. Two further `AiRuntime::new`
uses are `#[cfg(test)]` harness code. netcode re-exports sim's `scripting_systems` wholesale. Once
`locomotion_animation` lives in foundation, the production edge is gone and netcode depends only on
sim + foundation — no `postretro-ai` dependency. The new `layering_invariants_hold` assertion locks
that in.

## The physics leaf is a closed production subtree

- `collision/` — zero intra-sim `crate::` dependencies. Pure leaf over parry3d + level types.
- `movement/` — depends only on `crate::collision` (production). No scripting reach. One
  `crate::alloc_probe` reference is test-only (a `#[cfg(test)]` zero-alloc test) — crosses the
  new boundary behind `test-support`, same pattern as the kinematic_mover health ref.
- `kinematic_mover/` — depends on `crate::collision` + `crate::movement`; one
  `crate::scripting_systems::health::sweep_deaths` reference sits inside a `#[cfg(test)]` module
  (`blocking.rs`). Re-verify it is still test-only before the move.
- `nav` depends on `collision` (production edges are minimal — the bake-contract test imports a
  `SKIN_DISTANCE` const; re-verify the production coupling). `nav` **stays in sim**; its
  down-edge to the physics leaf for collision is clean either way.
- `weapon` is not part of the leaf — it reaches *up* into `scripting_systems` (several refs) and
  is sim-core.

## Why not a spatial-substrate crate (the rejected spine)

The earlier proposal made a `collision + nav (+ combat_positioning)` substrate the spine, to
"enable" multi-threaded pathfinding. Rejected:

- The MT-readiness properties already hold: `CollisionWorld` is `{TriMesh, Isometry}`, `NavGraph`
  and the path code are Rc/RefCell/registry-free, every query is `&self`, and `simulate_tick`
  already receives both by shared ref. `rayon::scope` over `&NavGraph` compiles today. A crate
  boundary only *enforces* a property that is already true — cheaply replaced by `Send + Sync`
  static asserts.
- The real MT blocker is the call site: the AI tick interleaves `find_path` with
  `perception::enemy_eye` reads of the `Rc<RefCell<EntityRegistry>>`. That is a scheduling problem
  (batch requests / fork-join, get registry reads out of the path loop) and lives in ai/sim
  regardless of where nav's files sit.
- Extracting `nav` as a *leaf* is backwards for compile cost: a leaf edit rebuilds every crate
  above it, and `nav` is about to become the least-stable module (the MT rewrite). Keep churny
  code high.

## Physics-leaf compile trade-off (owner-accepted)

Extracting a leaf pays only when the leaf is stable. `movement` / `kinematic_mover` have queued
drafts (`E22--kinematic-assemblies`, `E17--mover-relative-rider-presentation`,
`movement--sustained-fov-feel`, `movement-tick-component-clone-alloc`), but the owner classifies
these as tail-end bug fixes on an already-shipped kinematic-mover sprint — convergent, not
sustained churn — so leaf placement is acceptable. The compile win (a smaller sim for the frequent
scripting-host / weapon / combat edits) dominates; the rare movement/kinematic edit pays a small
extra rebuild. Timings are reported at build (per the epic's measure-don't-gate philosophy), not a
promotion gate.

## Runtime safety

Release `lto = "thin"` optimizes across crate boundaries; M2 crossed the same kind of boundary
(sim↔netcode) with no measured regression. Both extractions are plain module moves with concrete
types — no trait object on a hot path. The sim→physics and sim→ai crossings are per-tick step calls,
not per-entity inner loops. AI's per-enemy raycasts and `find_path` calls stay concrete shared-ref
calls (not through `&mut dyn AiHost`) — for borrow shape as much as dispatch cost. Frame-time check
guards against surprise.

## Enforcement

`layering_invariants_hold` (`xtask/src/crate_graph.rs`) today asserts binary-has-no-dependents,
foundation-is-a-leaf, entities-depends-only-on-foundation, net-has-no-internal-deps. It encodes
nothing about sim/netcode/ai. Add: the dependents of `postretro-ai` are exactly `[postretro]` —
one assertion that locks out both sim→ai and netcode→ai (the existing test already has a
reachable-dependents query).

## Ordering pins

Backing the synchronous-seam Acceptance rows. Symbols at read-at; re-verify against the live tree.
The tests that guard these live in `scripting_systems/ai_tests.rs` today and move into
`postretro-ai`; they call the real `run_ai_tick_with_navigation_and_impact` with a live registry,
the production `spawn_projectile` / `is_quiescent`, and an `on_impact` closure — so they only keep
proving these orderings if sim exports its concrete `AiHost` under `test-support` (Decision).

| id | scenario | ordering | expected outcome |
|---|---|---|---|
| ORD-1 | `spawn_projectile` returns None mid-attack (registry exhausted) | spawn attempt → observe returned Option → decide | no cooldown commit, no presentation spawn, no attack event; `[Weapon] entity registry exhausted` warns once (`weapon_stage/commands.rs`; `ai/apply.rs`) |
| ORD-2 | two enemies attack one tick; the earlier kills/despawns/recovers the later | publish all brain snapshots → per-outcome `is_quiescent` / invalidation guard → later actor suppressed | later actor does no observable work; its published snapshot is retained (`ai/apply.rs`) |
| ORD-3 | melee contact then impact policy | apply_damage → `is_quiescent` (invalidate) → `on_impact` | damage result observed before impact policy can synchronously recover the target (`ai/apply.rs`) |
| ORD-4 | impact policy writes sentiment overlay during outcome application | decay (mut) ends → AI live view (immut, compute only) drops → effects may `borrow_mut` | no `RefCell` panic; decay visible to AI's read; write reaches all brains next tick (`sim/mod.rs`; `ai/mod.rs`) |

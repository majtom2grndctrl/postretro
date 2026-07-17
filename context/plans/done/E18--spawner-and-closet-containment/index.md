# E18-C — Spawner Entity + Closet Containment

## Goal

Give co-op set-pieces the monster-closet payload: an engine-owned `entity_spawner` that a
trigger reaction fires to spawn enemies mid-session, replicated to clients including late
joiners. Reveal-closet containment consumes the **enemy aggro gate** from the co-designed
foundation (`E18--enemy-group-handle`) so pre-placed enemies do not aggro through walls or path
through closed doors before the reveal. Spawn-flavor is the capstone's critical path;
reveal-flavor containment is authoring over the foundation gate.

**Depends on:** `context/plans/done/E18--enemy-group-handle/` (the `enemies({ tag })` handle,
the `updateEnemyState` primitive, and the `BrainComponent` gate) — merged. E18-C is that
foundation's first consumer; the two are co-designed.

## Scope

### In scope

- `entity_spawner` point entity via the **generic classname-dispatch path** (like `prop_mesh` /
  `player_spawn`): a built-in classname handler, an engine-owned `SpawnerComponent`, an FGD
  entry. No new PRL section, no compiler branch, no loader change.
- `spawnFromSpawner` consequential reaction primitive (tag-targeted), executed VM-free in the
  fixed-tick seam. Each fire spawns `count` enemies of a named archetype at the spawner origin,
  facing the spawner's authored `angles`, offset by a fixed per-index delta (Task 2 pins it) so they do not overlap.
  The spawner is stateless — it never self-re-triggers; repeated spawns come only from repeated
  firing events, and one-shot behavior is owned by the firing trigger's policy.
- Spawn-time replication registration: a new `DescriptorSpawnPath::RuntimeSpawn`, a broadened
  networked-AI predicate, and a host-gated post-tick registration sweep so spawned enemies enter
  the `ReplicableSet` and stamp a `NetworkId`. Clients materialize them from the snapshot's
  `entity_class`.
- Spawner-archetype presentation: an archetype referenced only by a spawner (no pre-placed
  instance) has its mesh model uploaded and animation clips resolved on host and client, so a
  spawned enemy renders as its model rather than a debug capsule.
- Closet containment by **consuming the foundation aggro gate**: reveal-closet enemies are authored
  gate-closed with `enabled_on_spawn = false` (the foundation reads the KVP), and a reveal releases
  them via `enemies({ tag }).update({ aggro: true })`. The reveal composes at the trigger fan-out — an
  `openDoor` mover reaction and a `releaseCloset` reaction fired together — **not** one reaction body
  (a body is a single primitive; tag-targeted primitives ride the Primitive path, never a
  `sequence`). Spawn-flavor enemies spawn with the gate open (the foundation default): they appear
  after the reveal, already able to aggro.
- Pre-attack windup: a spawned enemy cannot attack until the netcode interpolation clamp has
  elapsed since it spawned — the clamp is the seed floor for the windup, and its known magnitude
  under jitter is ≤ 250 ms — so it is delivered and drawn on remote clients before it can hit. The
  windup doubles as jump-scare animation time.
- Full primitive-contract surface for the one new verb `spawnFromSpawner` (SDK TS + Luau builders,
  typedef regeneration + drift snapshot + parity fixtures, validators), authored through a
  `spawner({ tag }).fire()` handle — an object-filter selector in the foundation's fire-time-tag
  handle family, sibling to `enemies({ tag })`. No arm/disarm SDK work: enemy aggro authoring is the
  foundation's `enemies({ tag })` handle.

### Out of scope

- **Area/encounter-scoped kill progress** ("clear the ambush" reactions that count spawned
  kills) — its own spec; it carries deep questions (regional cells, regional BVH) that do not
  belong here. C only ensures spawned kills do not corrupt install-scoped `ProgressTracker`
  totals.
- Trap pools / seeded arming and any gameplay RNG — E18-D. Spawn order here is deterministic.
- Agent-vs-mover awareness (steering ignoring movers) — a broader unowned gap; the aggro gate
  covers only the pre-reveal window. Noted, not owned.
- Multi-point spawn patterns, per-fire spawn arguments, spawning non-AI archetypes.
- A wire field for the aggro gate — a gated enemy replicates as a stationary Idle enemy via its
  existing Transform; no new wire surface.
- Spawn-at-activator targeting (`@activators`) — not needed in C. The `@activators` sentinel
  exists now (`E18--trigger-event-params` landed); E18-C just does not use it.
- A new `world.query` filter term for spawners or enemies — both authoring handles are in the
  fire-time-tag family (`spawner({ tag })`, `enemies({ tag })`), not the setup-id `world.query` path.
- Runtime spawn-point validation (clear-of-geometry, on-navmesh) — spawn validity is an authoring
  responsibility, exactly as for map-placed enemies; the runtime trusts the authored placement. A
  spawner in a wall is an authoring bug for playtest and the dev overlay, not a runtime guard.

## Acceptance criteria

- [ ] An `entity_spawner` placed in a `.map` with an archetype name and count compiles and loads
      adding no new PRL `SectionId`; it rides the existing map-entities section (section inventory
      unchanged — this clause is a grep/review gate, not a runnable test).
- [ ] Firing a reaction containing `spawnFromSpawner` targeting the spawner's tag creates `count`
      live enemies of the named archetype at the spawner, on the host, inside the fixed tick.
- [ ] A spawner with an absent/empty archetype KVP, an unknown archetype name, or an archetype
      that is not an AI enemy is flagged `resolved = false` by the install pass (surfaced through
      its diagnostics tally) and spawns nothing at fire time.
- [ ] Firing `spawnFromSpawner` against the same spawner twice spawns `2 × count` enemies — the
      spawner is stateless and never self-exhausts.
- [ ] Firing `spawnFromSpawner` with a tag matching no `entity_spawner` spawns nothing and warns,
      deduped per distinct tag for the level (asserted via the warn set on the spawn-context handle;
      Task 2 owns the semantics and rationale).
- [ ] Spawned enemies face the spawner's authored `angles` direction (asserted at the spawn instant,
      before the AI FSM may reorient).
- [ ] A `count` of 0 (or malformed) spawns nothing — the directly-assertable fact (no entities
      created); it also warns at load (a `log::warn!`, so the assertion is the spawns-nothing
      outcome, not the log line). Registry exhaustion mid-batch spawns what fits, warns, and does not
      panic — asserted via a new low-capacity `#[cfg(any(test, feature = "test-support"))]` registry seam
      (Task 2 adds it to the `entities` crate) that forces `try_spawn` to return `None`, since
      filling to `u16::MAX` is impractical.
- [ ] In a two-peer co-op session, a host-spawned enemy appears and renders its mesh on the
      connected client and on a client that joins after the spawn, driven by its replicated
      Transform.
- [ ] A spawner-only archetype (no pre-placed instance) has its `mesh.model` in both the host and
      client upload sets and its clips resolved before the first spawn — the observable proxy for
      "renders as its model, not a debug capsule" (headless; no pixel check).
- [ ] A spawned enemy deals no damage until the windup floor (≤ 250 ms) has elapsed after spawn; at
      the E15 reference `LinkConfig` (`mandated_link`) it is delivered to the remote client (present
      in its snapshot, its archetype in the upload set per the prior AC) before its first attack
      lands. The harness asserts this delivered-before-hit property robustly (across the jitter
      envelope, not one lucky seed), so an insufficient floor fails rather than passing by chance.
- [ ] A reveal fan-out fires an `openDoor` mover reaction and a `releaseCloset`
      (`enemies({ tag }).update({ aggro: true })`) reaction together, opening the door and releasing the
      contained enemies on one plate edge. (Gate mechanics — containment, killable-while-gated,
      empty-resolution no-op, fire-time resolution — are the foundation's ACs; C asserts only the reveal
      composition and that spawn-flavor enemies arrive gate-open.)
- [ ] A spawned enemy carries no progress tags, so killing it changes no install-scoped
      kill-progress total (no over-count, no underflow).
- [ ] SDK TS and Luau emit byte-identical descriptors for `spawnFromSpawner`, authored via
      `spawner({ tag }).fire()`; the typedef drift check passes after regeneration.

## Tasks

### Task 1: `entity_spawner` entity class + `SpawnerComponent`

Add a built-in classname handler for `entity_spawner` mirroring `prop_mesh`
(`crates/postretro/src/scripting/builtins/prop_mesh.rs`): a `CLASSNAME` const, a handler
registered into `ClassnameDispatch` via `register_builtins`, riding the generic
`MapEntityRecord` point-entity path — **no** dedicated PRL section, compiler branch, or loader
change. The handler reads the `archetype` KVP (enemy archetype `canonical_name`) and the integer
`count` KVP (absent/empty `archetype` → `[Loader]` warning, spawner spawns nothing; `count` is
parsed to an integer at load — a missing, malformed, **or zero** value warns and yields a count of
0, so the spawner spawns nothing, satisfying AC 7's count-of-0 warn at load) and attaches an
engine-owned
`SpawnerComponent { archetype_name: String, count: u32, resolved: bool }` — all serde-friendly
primitives. **Do not store an `EntityTypeDescriptor` on the component:** every engine component is
a `ComponentValue` variant and `ComponentValue` derives serde, but `EntityTypeDescriptor`
(`crates/entities/src/data_descriptors/types/entity.rs`) has none — carrying it breaks the derive.
Registering the component is the usual ritual (mirror `Brain`): a new `ComponentKind::Spawner`
added to the `VARIANTS`/`COUNT` array (`crates/entities/src/registry.rs`), a `ComponentValue::Spawner`
arm + `kind()` arm, and a `Component` impl. The handler spawns via `try_spawn(transform, &entity.tags)`
so the spawner carries its `_tags` (the KVP bag is applied uniformly by `apply_classname_dispatch`;
tags attach through `try_spawn`, as `prop_mesh` does).

Resolution cannot happen in the handler — `ClassnameHandler` is a bare
`fn(&MapEntity, &mut EntityRegistry) -> Option<EntityId>` with no descriptor list in scope — so run
a **separate post-dispatch install pass** in `crates/postretro/src/startup/lifecycle.rs` (~`:1256`,
where `descriptors` and the baked `agent_params` are in scope). For each `SpawnerComponent`,
`find_descriptor` its `archetype_name` (matches `canonical_name`); warn via the pass's diagnostics
tally — a test-observable count or returned struct AC 3 asserts on (none exists yet; build it) — and
set `resolved = false` if the archetype is missing or not an AI enemy
(`descriptor_materializes_ai_enemy`), else `resolved = true`. Populate the **spawn context**
(`SpawnContext`) — a session-built, interior-mutable shared handle (mirror `MoverCommandDiagnostics`'s
`Rc<RefCell<…>>` shape, `crates/postretro/src/kinematic_mover.rs:24-28`; NOT an ECS component, so no
serde constraint) — whose interior holds the resolved enemy descriptors keyed by `canonical_name`
plus the level's baked `NavAgentParams` (`nav_graph.map(|g| g.agent_params())`,
`lifecycle.rs:1260`). Thread the handle exactly as `command_diagnostics` rides: create it at session
build (`session/mod.rs:~484`, beside `command_diagnostics`), store it on `ScriptingCore`, and pass it
through `build_trigger_bindings` (`lifecycle.rs:1019`) into the trigger table (Task 2 makes it a table
field). It is also `.clone()`d into the executor paths (Task 2); this install pass fills its interior
each level (replacing the prior level's on reload) so the fixed-tick executor resolves the descriptor
VM-free without touching the data registry or a `ScriptCtx`. Add the
FGD `@PointClass` entry (`entity_spawner`, archetype + count keys, plus the standard `angles` key
for facing). Enemy archetypes are any descriptor with an `ai` block; do not invent a modder-facing
enemy component.

### Task 2: `spawnFromSpawner` verb + in-tick spawn executor

Add `spawnFromSpawner` to `CONSEQUENTIAL_PRIMITIVES` in `crates/postretro/src/trigger_bindings.rs`,
and a `Spawn { target: BoundTarget }` variant to `BoundTriggerCommand` (and the matching `Spawn`
kind to the `#[cfg(test)] BoundTriggerCommandKind`, re-exported from `trigger_bindings.rs`) in
`crates/postretro/src/trigger_commands.rs`, updating `BoundTriggerCommand::kind()` to match. The
single primitive→command mapping point remains `bind_command`'s match in `trigger_bindings.rs`
(`partition_direct_reaction` and `bind_sequence_step` route into it); add the `Spawn` arm there.
The target is a `BoundTarget::Tag` (config lives on `SpawnerComponent`, no
per-fire args). Also add `spawnFromSpawner` to `is_trigger_consequential_primitive`
(`crates/scripting-core/src/reaction_dispatch.rs`), the hardcoded mirror of the fixed-tick command
set that backs the double-execution debug asserts. Put the spawn logic in a **new**
`crates/postretro/src/spawner.rs` module (mirror `kinematic_mover.rs`'s
`apply_mover_command_to_targets` + `register_mover_reaction_primitives` shape). The spawn context
is threaded exactly as `MoverCommandDiagnostics` is — and the load-bearing half is that it is a
**field** on `TriggerBindingTable` (`trigger_bindings.rs:79`, beside `command_diagnostics`), set by
`TriggerBindingTable::build_with_script_ctx_and_diagnostics` (`:171`) which `build_trigger_bindings`
(`lifecycle.rs:1019`) feeds from Task 1's session handle. So `execute`/`execute_with_script_ctx`/`execute_non_store`
(`trigger_commands.rs:108/130/162`) each gain a `&SpawnContext` parameter alongside the existing
`command_diagnostics: &MoverCommandDiagnostics`, and the internal `command.execute(...)` sites
(`trigger_bindings.rs:371/423`) pass `&self.spawn_context` just as they pass `&self.command_diagnostics`. `trigger_commands.rs` therefore gains the `Spawn` variant, a delegating
match arm in `execute_non_store`, **and** this threaded parameter (a mechanical threading edit, not
new logic); `trigger_bindings.rs` gains the `CONSEQUENTIAL_PRIMITIVES` entry and the `bind_command`
arm. Because the same handle is `.clone()`d into the `register_spawner_reaction_primitives` closure,
`spawnFromSpawner` fired from a **non-trigger** named reaction (crossing / named event / `levelLoad`,
which drain through the app-side registry with Transform-resolved targets) spawns identically —
there is no silent-no-op path. Both paths call the same `spawner.rs` helper.

The executor resolves spawner entities by a `ComponentKind::Spawner` query (mirroring
`updateEnemyState`'s direct `ComponentKind::Brain` query in `enemy_state.rs`, NOT the generic
Transform `resolve` — a non-spawner sharing the tag must never be treated as a spawn target); the
app-drain path filters its Transform-resolved targets to `SpawnerComponent`-bearing the same way
`register_enemy_state_reaction_primitives` filters to Brain-bearing. A tag matching **no** spawner
**warns**, deduped per distinct zero-match tag for the level — its dedup set lives on the
`SpawnContext` handle (mirror `MoverCommandDiagnostics.warn_*_once`, cleared on the per-level
interior refill), which AC 5 asserts on — and spawns nothing. A pre-placed spawner's zero-match tag
is a likely authoring typo, distinct from the foundation's fire-time-tag enemy case (a `debug`
no-op, since an enemy group is legitimately empty when unspawned or killed). It skips any spawner
with `resolved == false` (spawns nothing), and for each resolved spawner spawns `count` enemies via
`try_spawn(transform, &[])` (empty tags — spawned enemies are untagged; `registry.rs:732`, returns
`Option`, so `None` is the exhaustion stop-signal — NOT `spawn` at `:744`, which panics at
capacity) + `attach_descriptor_components` (module-private today — bump to `pub(crate)`; it takes an
`&MapEntity`, so synthesize a minimal one as the `defaultWeapon` literal path does, and it takes
`agent_params` — pass the context's baked `NavAgentParams`, NOT `None`, or spawned enemies get a
default-sized capsule that diverges from map-placed instances of the same archetype), stamping
`DescriptorProvenance.spawn_path = DescriptorSpawnPath::RuntimeSpawn` (add this variant to
`crates/entities/src/provenance.rs`).
Spawned enemies are **untagged** — this is deliberate (it keeps them out of tag-keyed kill-progress
counting, Task 6). Place the `count` enemies at the spawner origin plus a fixed per-index offset
of `index × (2 × agent capsule radius)` along the spawner's local right axis (`rotation_quat() * +X`)
so they do not perfectly overlap (do not rely on the steering separation pass, which may not run
for idle agents). Spawn is host-side, in-tick, in deterministic order, with no RNG. Register the
primitive through a `register_spawner_reaction_primitives` entry point (mirroring
`register_mover_reaction_primitives`'s call site in `session/mod.rs:523`, which is handed the shared
handle) so the verb is accepted by the descriptor/validation path **and** its app-drain closure —
holding the cloned spawn context — spawns for non-trigger firings.

The spawner is **stateless**: each fire spawns `count`, it holds no fired/exhausted flag, and it
never re-triggers itself — re-firing is an ordinary event outcome, and one-shot behavior (a closet
that springs once) is owned by the firing trigger's `once`/rearm policy, not the spawner. Spawned
enemies take the spawner entity's Transform rotation (its authored `angles`, via `rotation_quat()`)
so they face as placed. Degrade gracefully, matching `prop_mesh`: a `count` of 0 (parsed and defaulted at load, Task 1)
spawns nothing; if `try_spawn` returns `None` mid-batch (the registry at `u16::MAX`), spawn what
fits, warn once, and stop — never panic. Testing that exhaustion path needs a seam that does not
exist yet: add a `#[cfg(any(test, feature = "test-support"))]` capacity cap to
`crates/entities/src/registry.rs` (the existing `#[cfg(test)]` block at `:1046` manipulates
generation, not slot count, and `try_spawn` (`:732`)
otherwise only returns `None` at `slots.len() >= u16::MAX`) — a test-only cap so `try_spawn` returns
`None` at a low bound. This is a cross-crate `entities` edit this task owns (AC 7). Do not validate spawn
positions against geometry or the navmesh — a spawner inside a wall is an authoring bug for
playtest/dev-overlay, consistent with map-placed enemies. Because the trigger system runs between
movement and AI in the tick, a spawned enemy exists before this tick's AI pass and may take its
first steering step the frame it appears; the windup gates only its first attack, not its first
step.

### Task 3: Spawn-time replication registration + client materialization

In `crates/postretro/src/netcode/descriptor_class.rs`, broaden `is_networked_ai_map_enemy` to
accept `spawn_path ∈ {MapPlacement, RuntimeSpawn}` (Brain+Agent still required) and rename it to
`is_networked_ai_enemy`, updating the call sites (`descriptor_entity_class` and the host
registration sweep in `replication.rs:248/261`) and the **netcode-side** classifier agreement test
(`classifier_agrees_with_live_predicate_one_source_of_truth` in `descriptor_class.rs`). The rename
is **grep-complete** — do not trust an enumerated list; sweep every reference including non-code doc
comments (`replication.rs:16/224`, `enemy_replication_harness_test.rs:196`, and
`data_archetype.rs:280/2387`, which the build will not catch). This makes
`descriptor_entity_class` stamp a spawned enemy's `canonical_name` so the client materializes its
presentation from a Transform-only record (else it is an invisible/untyped ghost). **Re-invoke** the (now broadened)
`host_register_map_enemies` sweep post-tick — it is already a full idempotent stale-prune+register
sweep (`crates/postretro/src/netcode/replication.rs:239`, idempotency test at `:487-493`;
re-registration keeps the `NetworkId`), so no new registration function is needed. It sweeps for
networked-AI enemies not yet in the `ReplicableSet`, `allocator.stamp` + `ReplicableSet::register`
each, and tracks them in the same `map_enemies` set so level reload unregisters them. Run it every
tick — an unconditional per-tick call is correct; a dirty-flag optimization is a later concern, not
a correctness requirement, and no spawn-happened signal is threaded out of the executor today. Gate
it on host/single-player role exactly as `host_register_map_enemies_after_install` (`main.rs:5026`)
does, and call it after `host_advance_fixed_sim_tick()` in the host loop (`main.rs:2013`) via a thin
host-role-gated wrapper like that after-install method; the after-install call
(`lifecycle.rs:788`) stays as-is. Single-player spawns work with no registration (sweep is a no-op
off-host).

### Task 4: Closet containment authoring (consumes the foundation aggro gate)

Nearly no engine work — the gate, its FSM/steering guard, the `enabled_on_spawn` seed, and the
`updateEnemyState` primitive all land in the foundation (`E18--enemy-group-handle`). C is the
consumer. Author the set-piece into a **new dev map** `content/dev/maps/closet-reveal.map` (follow
the `content/dev/maps/trigger-event-spike-trap.map` set-piece precedent from the merged
`E18--trigger-event-params` work) plus its companion reaction script. The one engine artifact is AC
10's composition assertion — it lives as a test in `crates/postretro/src/trigger_system.rs`, where
trigger fan-out / `onTriggerEvent` dispatch tests already live, asserting the `openDoor` and
`releaseCloset` reactions dispatch together on one `enter` edge.

- **Reveal-flavor containment.** Author reveal-closet enemies gate-closed with
  `enabled_on_spawn = false` (the foundation's Task 1 reads the KVP on the AI-enemy placement). A
  reveal opens the door and releases the enemies as a **trigger fan-out of two reactions** — an
  `openDoor` mover reaction and a `releaseCloset` reaction (`enemies({ tag }).update({ aggro: true })`) — not
  one reaction body: a body is a single primitive, and a tag-targeted primitive rides the Primitive
  path, so the two cannot share a `sequence`.
- **Spawn-flavor.** Spawned enemies inherit the foundation gate default (open), so a
  `spawnFromSpawner` fire produces enemies that appear and immediately aggro — no gating step. The
  windup (Task 6), not the gate, holds their first attack.

The foundation gate is one aggro condition among several; later perception specs add LOS and sound
gates that compose additively (acquisition requires the gate open; new conditions narrow, never
replace, it).

### Task 5: SDK `spawner({ tag }).fire()` handle, typedefs, validators

Add the spawner selector and handle under `sdk/lib/` — `spawner(filter: { tag: string })` returning
a handle whose `fire()` emits a single `PrimitiveReactionDescriptor`
(`{ primitive: "spawnFromSpawner", tag }`, no args — `count` lives on the spawner component). `tag` is
required: a missing or empty tag is a compile-time/validation error, not a runtime drop — the SDK
typedefs and the JS/Lua reaction-argument validators reject it. This is
the fire-time-tag handle family the foundation establishes with `enemies({ tag })`: object-filter
selector, tag-keyed descriptor, `damage(tag)` builder template (`sdk/lib/data_script.ts:204`). Mirror
in Luau. Extend the typedef templates (`crates/scripting-core/src/typedef/templates/`), regenerate
`postretro.d.ts` / `.d.luau`, update the drift snapshot, add TS/Luau parity fixtures. Update the
reaction-argument validators to accept `spawnFromSpawner` and reject a malformed target — including a
missing or empty `tag`. (The
fire-time zero-match warn-once — a pre-placed spawner's empty tag is a likely typo, distinct from
the foundation's fire-time-tag enemy `debug` no-op — is the executor's, specified in Task 2.) No
arm/disarm SDK work — enemy aggro authoring is the foundation's `enemies({ tag })` handle. No
`world.query` filter change (see out of scope).

### Task 6: Pre-attack windup + kill-progress guard

Enforce a minimum pre-attack windup on a spawned enemy by seeding its existing
`attack_cooldown_remaining_ms` (`crates/entities/src/components/brain.rs`, an `f32` in
**milliseconds**, counted down in `ai.rs`) at spawn time in `spawner.rs` — **after**
`attach_descriptor_components` runs, since `attach_brain` inits the field to `0.0` and would
overwrite an earlier seed — to at least the delay clamp `MAX_DELAY_MICROS` (`crates/postretro/src/netcode/interpolation.rs`, currently module-private
— bump to `pub(crate)`; it is `250_000` **microseconds**, so convert ÷1000 to the ms field, never a
hardcoded literal) — the seed floor for the windup, not a ceiling on it — so a spawned enemy cannot
land its first attack before it is delivered and drawn on remote clients. Harness-assert
the property at the E15 reference `LinkConfig` (`mandated_link`): a freshly spawned enemy is present
and drawable on the client before it deals damage — assert at that profile, not universally. There is
no separate "delivery budget" constant; the interpolation clamp is the pinned floor and the harness
observes the delivered-before-hit property. Observe it robustly (across the jitter envelope, not one
seed): delivery-to-drawable is `link_delay + interp_delay`, so if the property fails at
`mandated_link` the clamp alone is short by the one-way link term and the floor must rise to cover
it — an impl-time observation the harness surfaces, consistent with the clamp being the pinned floor.
Kill-progress needs no counting change here: spawned
enemies are untagged (Task 2), and `ProgressTracker::on_entity_killed` matches only by tag, so a
spawned kill can never touch an install-scoped total — add a test asserting this and do not build
area-scoped progress (deferred sibling spec). (The spawner *entity* itself, like any tagged non-enemy
such as a `prop_mesh`, is counted by `count_entities_with_tag` if it shares a `progress`-tracked tag
— a pre-existing authoring concern, not a spawned-kill path: keep a spawner's fire tag distinct from
any progress tag.)

### Task 7: Spawner-archetype model upload + clip resolve (host + client)

Runtime-spawned enemies must render, not fall back to a debug capsule. Today the renderer model
cache and animation-clip indices are built by **level-load-only** passes scoped to the archetypes
the map references *by placement*: the host model sweep (`distinct_mesh_models`,
`crates/postretro/src/main.rs`) and clip resolve (`resolve_mesh_entity_clips`, same file), and on
the connected client the suppressed-AI upload (`suppressed_ai_enemy_mesh_models`,
`crates/postretro/src/scripting/builtins/data_archetype.rs`, scoped to classes the map references).
An archetype referenced only by an `entity_spawner` KVP matches none of them, so its mesh never
uploads and its clips never resolve. At install, collect every spawner-referenced archetype (from
the resolved `SpawnerComponent`s of Task 1) and union its `mesh.model` handle into both upload
sweeps (host and client) and resolve its clips, so a spawner-only archetype is drawable before its
first spawn. The install-time model union satisfies AC 8's capsule-vs-model proxy (a debug capsule
appears only when the model is absent from the cache), but the **host** clip *indices* need a second
step: `distinct_mesh_models`/`resolve_mesh_entity_clips` iterate already-spawned `Mesh` components at
load, so a host enemy spawned after install keeps pending clip indices and would animate as a static
first frame. Give host-spawned enemies a per-spawn clip resolve (mirror the client's
`materialize_net_remote_enemy_presentation` clip-resolve shape, host-side, at spawn or in the
post-tick sweep) so they animate — the archetype's clip table is already built by the install union,
so this is an index fill-in, not a rebuild. The client **has** `SpawnerComponent`s to read: the `entity_spawner` classname handler
runs on the shared install path (`apply_classname_dispatch`, `lifecycle.rs:1195`) *before* the
AI-enemy filter (`filter_out_client_ai_enemies`, `lifecycle.rs:1275`), and a spawner is not an AI
enemy, so it survives on the client — the client runs the same archetype-collection over its
client-side spawner components to build its upload union ("host-local" in the boundary inventory
means not-replicated, not host-only-existent). The client already has a post-materialization
resolve precedent for net-remote enemies (`materialize_net_remote_enemy_presentation`, defined at
`crates/postretro/src/scripting/builtins/net_descriptor.rs:211`; its production caller is
`materialize_armed_remote_enemy`, `crates/postretro/src/netcode/remote_materialize.rs:52-64` — the
`main.rs` occurrence is `#[cfg(test)]`); reuse its shape rather than inventing a per-spawn upload
path.

## Sequencing

**Phase 1 (sequential):** Task 1 — the spawner entity + component + install-time archetype resolution; everything spawns through it.
**Phase 2 (sequential):** Task 2 — the verb, executor, and `RuntimeSpawn` provenance variant; consumes Task 1's resolved component and edits `trigger_bindings.rs`.
**Phase 3 (concurrent):** Task 3 (replication) and Task 6 (windup + progress) — disjoint files: 3 = netcode (`descriptor_class.rs`/`replication.rs`), 6 = `spawner.rs` + harness; both consume Task 2's spawn path.
**Phase 4 (concurrent):** Task 5 (SDK `spawner({ tag }).fire()` handle + typedefs; consumes the verb from Task 2) and Task 7 (presentation upload) — disjoint files: 5 = `sdk/` + typedef templates, 7 = `main.rs` presentation sweeps + the client upload pass in `data_archetype.rs`.
**Task 4 (containment authoring)** carries no engine files — map placements + reveal reactions gated on the foundation gate landing. It can land any time after the foundation, alongside Phase 4.

**Prerequisite:** the foundation (`E18--enemy-group-handle`) — its `BrainComponent` gate, `updateEnemyState` primitive, and `enemies({ tag })` handle — lands before Task 4's containment authoring and before the capstone consumes the reveal loop.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Spawner class | `entity_spawner` handler | n/a (no PRL section) | n/a | n/a | `entity_spawner` (`@PointClass`) |
| Spawn verb | `BoundTriggerCommand::Spawn` | reaction descriptor `"spawnFromSpawner"` | `spawner({ tag }).fire()` | same | n/a |
| Spawner component | `SpawnerComponent` | not replicated (exists on host **and** client) | n/a | n/a | n/a |
| Runtime spawn path | `DescriptorSpawnPath::RuntimeSpawn` | `"runtime_spawn"` (serde, local only) | n/a | n/a | n/a |
| Aggro gate (consumed) | foundation `BrainComponent` gate | not on the wire | `enemies({ tag }).update({ aggro: true })` | same | `enabled_on_spawn` (authored here, read by foundation) |
| Archetype ref | `SpawnerComponent.archetype_name` | n/a | n/a | n/a | `archetype` key (value = `canonical_name`) |
| Spawn count | `SpawnerComponent.count` | n/a | n/a | n/a | integer `count` key |

## Rough sketch

- **Spawner entity (Task 1).** `prop_mesh.rs` is the exact template — `CLASSNAME`, a handler
  returning a spawned id with a component attached, registered in `ClassnameDispatch::register_builtins`;
  KVPs applied by `apply_classname_dispatch`, tags via `try_spawn(transform, &entity.tags)`
  (`prop_mesh.rs:65`). Archetype resolution is a post-dispatch install pass, not the handler:
  `find_descriptor(descriptors, name)` (`data_archetype.rs:249`); `descriptor_materializes_ai_enemy(d)
  == d.ai.is_some()` (`:293`) validates it is an enemy.
- **In-tick spawn (Task 2).** `bind_command`'s match (`trigger_bindings.rs:677` fn, match at `:691`) is
  the single primitive→`BoundTriggerCommand` mapping point. `execute_non_store`
  (`trigger_commands.rs:162`) gains a `&SpawnContext` param alongside `command_diagnostics`; its
  `Spawn` arm resolves spawners by a `ComponentKind::Spawner` query (mirroring `updateEnemyState`'s
  `ComponentKind::Brain` query — NOT the generic `BoundTarget::resolve` at `:246`, whose
  `query_by_component_and_tag(ComponentKind::Transform, tag)` call at `:254` returns every
  Transform-tagged entity) and delegates to `spawner::spawn_from_spawner_targets`. Spawn =
  `try_spawn(transform, &[])` (`registry.rs:732`, returns `Option`; `spawn` at `:744` panics at
  capacity) + `attach_descriptor_components` (`data_archetype.rs:389`, bump to `pub(crate)`;
  synthesize its `&MapEntity` as the `defaultWeapon` literal does, `:788-794`) with
  `spawn_path = RuntimeSpawn`, untagged.
  Keep the descriptor reachable without `ScriptCtx` via the install-time spawn context (descriptor
  index + baked `NavAgentParams`) threaded into the executor — NOT stored on the component
  (`EntityTypeDescriptor` has no serde; `ComponentValue` requires it).
- **Presentation (Task 7).** Model upload + clip resolve are level-load-only and placement-scoped
  (`distinct_mesh_models` / `resolve_mesh_entity_clips` in `main.rs`; client
  `suppressed_ai_enemy_mesh_models`, `data_archetype.rs:354`). Union spawner-referenced archetypes
  into both, else the spawned enemy draws as a debug capsule (the E10 AC#3 regression documented at
  `data_archetype.rs:343-349`).
- **Replication (Task 3).** `is_networked_ai_map_enemy` (`descriptor_class.rs:31`) currently gates
  on `MapPlacement`; broaden to `{MapPlacement, RuntimeSpawn}` + rename to `is_networked_ai_enemy`;
  its caller `descriptor_entity_class` (`:61`) then stamps the class for spawned enemies. Post-tick sweep mirrors
  `host_register_map_enemies` (`replication.rs:239`), which is idempotent
  (`replication.rs:487-493`); register via `ReplicableSet::register` (`:47`) + `allocator.stamp`.
  Pawn-accept template: `on_slot_accepted` (`netcode/lifecycle.rs:132`).
- **Containment (Task 4).** No engine work in C — the gate, its FSM guard
  (`run_ai_tick_with_navigation` before `evaluate_transition`), the `enabled_on_spawn` seed, and the
  `updateEnemyState` primitive are the foundation's. C authors the closed placements and the `openDoor` +
  `releaseCloset` fan-out.
- **Contract tax.** One new verb, `spawnFromSpawner`, touches `CONSEQUENTIAL_PRIMITIVES`,
  `BoundTriggerCommand` (+Kind), `partition_direct_reaction`/`bind_sequence_step`, a
  `register_*_reaction_primitives` call, SDK TS+Luau (the `spawner({ tag }).fire()` handle), typedef
  templates + drift + parity, validators. The `updateEnemyState` verb and `enemies({ tag })` handle carry
  their own tax in the foundation.
- **Oversized-file note (soft):** `ai.rs`, `registry.rs`, `reaction_dispatch.rs` exceed ~800 lines.
  `trigger_bindings.rs` was already split by the merged `E18--trigger-event-params` work —
  trigger-command types and execution now live in `crates/postretro/src/trigger_commands.rs`. E18-C's
  additions are localized: the `Spawn` variant + execution arm in `trigger_commands.rs`, and the
  `CONSEQUENTIAL_PRIMITIVES` entry + `bind_command` arm in `trigger_bindings.rs`. Spawn logic lands
  in a new `spawner.rs`. (The Brain gate field + tick guard are the foundation's, not C's.)

## Script syntax examples

```ts
// Spawn-flavor closet: a plate fires a reaction that spawns the ambush.
// A reaction body is ONE primitive; the spawner handle emits it.
defineReaction("springAmbush", spawner({ tag: "closet_a" }).fire());

// Reveal-flavor closet: a body holds one primitive, so the door and the enemy
// release are two reactions, fired together by the trigger fan-out.
export const openClosetDoor = defineReaction("openClosetDoor",
  { primitive: "moverStart", tag: "closet_door" });
export const releaseCloset = defineReaction("releaseCloset",
  enemies({ tag: "closet_a" }).update({ aggro: true }));   // foundation handle

onTriggerEvent({ tag: "reveal_plate" }, "enter", [openClosetDoor, releaseCloset]);
```

```
// TrenchBroom: the spawner point entity and a pre-placed, gate-closed enemy.
{ "classname" "entity_spawner" "archetype" "grunt" "count" "3" "_tags" "closet_a" }
{ "classname" "grunt" "enabled_on_spawn" "0" "_tags" "closet_a" }
```

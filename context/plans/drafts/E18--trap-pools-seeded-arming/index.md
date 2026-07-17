# E18-D — Trap Pools + Seeded Arming

## Goal

Close the Epic 18 semi-random loop: a per-level script declares pools of authored closet traps;
at level install a host-only seeded roll arms a declared count per pool, so players learn where
traps *can* be while each run varies which are live. Scripts declare pools and counts; they never
observe or perform the roll. Design intent: `context/research/co-op-triggers-trap-pools.md` §3
(RNG posture), §4.6. Consumes the shipped arm/disarm substrate (`done/E18--trigger-event-fanout`)
and composes with spawn-closets (`done/E18--spawner-and-closet-containment`).

## Scope

### In scope

- **`trapPools` manifest key** on the `setupLevel` return bundle — level-local declarative data
  beside `reactions`/`crossings`/`triggerEvents`. Each entry: `{ name, tag, arm }` — a unique pool
  name (identity for logs/overlay), a tag selecting member trigger volumes, and the count to arm.
  Parsed warn-and-skip per entry in both runtimes, mirroring the `triggerEvents` drains.
- **`defineTrapPool({ name, tag, arm })` SDK builder** — a pure identity/validation helper (the
  `defineMapCatalog` shape): no FFI, returns the descriptor for the manifest array. TS + Luau,
  typedefs, drift snapshot, parity fixture.
- **Pool membership = trigger volumes carrying the pool tag.** The trap's payload (spawner, mover,
  enemy release) hangs off each member trigger's `on_fire` reactions — shipped composition, nothing
  new. Non-trigger entities sharing the tag (a spawner sharing its closet tag) are simply not
  members; no warning.
- **Host-only seeded arming pass at level install**, after placements and trigger bindings
  materialize and before the `levelLoad` fire: resolve each pool's member set by tag, sort members
  by entity id (stable spawn order), pick `arm` distinct members with a seeded PRNG, arm the picks
  and disarm the rest via the shipped full-re-arm/disarm helpers. The pass force-sets both states,
  so an authored `enabled_on_spawn` value never leaks into a pool member (authoring convention is
  `false`; `true` warns as a likely mistake and is overridden).
- **Seed policy (pinned by research §4.6):** fresh roll per level install, including same-session
  restarts. Seed sources: `--trap-seed=<u64>` CLI override, else entropy (windowed), else the fixed
  default `0` in headless mode so batch-runner byte-identical output survives. Seed logged at info
  on every roll; each pool's armed member set logged with it.
- **Engine-owned PRNG:** SplitMix64 (integer-only, cross-platform deterministic), copied from the
  in-tree netcode-harness implementation. No `rand` dependency. RNG lives only in this install
  pass — never in per-tick evaluation, never in command-buffer IR.
- **Install report** (`TrapPoolInstallReport`): seed + per-pool member/selected sets, retained
  host-side for tests and the dev overlay. Host-local only.
- **Replication: none.** Clients never run the pass; armed traps manifest to clients only through
  consequences (mover phase, spawned enemies) that already replicate. Two-endpoint QA proves it.
- **Runtime composition:** the pass runs once, before `levelLoad`; script `armTrigger`/
  `disarmTrigger` fired afterwards (puzzle disarms, resets) compose normally and win.
- **Dev-tools Triggers tab:** a Pool column per member row — pool name + whether the roll selected
  it.
- Fixture map + script, headless determinism coverage, two-endpoint net QA.

### Out of scope

- A mod-global `ModManifest.trapPools` tier with a `levels` selector — pools are per-level by
  nature; add the tier only on authoring demand (the `ScopedTriggerEvent` machinery is the template
  when it comes).
- Weighted selection, per-member weights, guaranteed/forbidden members — `arm` count only in v1
  (research names weights as a later axis).
- Any script-visible roll: no primitive returns armed state, no pool query handle, no `@`-input.
  `world.query({ component: "trigger_volume" })` keeps its shipped snapshot (no armed state).
- Replicating the armed set. If a spectator/UI need materializes, the stable shape is one Array
  slot per declared pool (research §4.6 sketch) — sketched there so it isn't reinvented, not built.
- Mid-session re-rolls, wave directors, runtime pool mutation — arming is a load-time decision.
- Spawner-side arming state — spawners stay stateless (E18-C); pools gate the *triggers* that fire
  them.
- Sticky per-campaign-run seeds — considered and rejected in research §4.6 (retry tension); reopen
  only with playtest evidence.
- FGD, compiler, PRL, or wire-format changes — none needed; membership rides existing `_tags`.

## Acceptance criteria

- [ ] A `setupLevel` returning `trapPools: [defineTrapPool({ name, tag, arm })]` parses in a TS mod
      and a Luau mod; both runtimes emit byte-identical descriptor data; the typedef drift check
      passes after regeneration.
- [ ] A level declaring one pool (`arm: 2`) over 4 member triggers installs on the host with
      exactly 2 members armed and 2 disarmed, regardless of each member's authored
      `enabled_on_spawn` (headless install test).
- [ ] Two installs with the same seed produce identical armed sets; two pinned differing seeds
      (chosen by the test) produce different armed sets for the same fixture.
- [ ] With `--trap-seed` pinned, a same-session `restartLevel` re-runs the pass and reproduces the
      identical armed set; the seed is logged on every roll.
- [ ] Without an override, a headless run uses the fixed default seed and repeated identical runs
      stay byte-identical (batch-runner guarantee preserved).
- [ ] An unselected member never fires on player entry; a later `armTrigger` reaction targeting it
      re-arms it and it then fires normally (runtime arming composes after the roll).
- [ ] A selected member behaves as an ordinarily armed trigger: enter executes its bound
      consequential steps in the same sim tick.
- [ ] Degradation: `arm` ≥ member count arms all members and warns; a pool tag matching zero
      trigger volumes warns and the pool is inert; a malformed entry (missing `name`/`tag`,
      negative or non-integer `arm`) warns and is skipped without aborting the manifest; a
      duplicate pool name warns and the later entry is skipped; `arm: 0` is valid and disarms
      every member.
- [ ] A member matched by two pools warns once and the later pool's decision wins (declaration
      order), deterministically.
- [ ] A pool member authored `enabled_on_spawn = true` logs a warning naming the entity and is
      still processed (roll outcome overrides the authored value).
- [ ] Two-endpoint: the arming pass runs only on the host; a connected client's trigger components
      receive no roll (client-side armed state stays as authored), while the consequences of a
      host-armed trap firing (mover motion or a spawned enemy) reach the client via existing
      replication. No new wire surface (grep/review gate over the wire structs).
- [ ] Determinism: two identical headless runs with scripted inputs and a fixed seed produce
      identical trigger fire sequences and post-tick registry/slot state with a trap-pool level
      active.
- [ ] With `--features dev-tools`, the Triggers tab shows each member's pool name and whether the
      roll selected it; without the feature, no pool-overlay code compiles in.

## Tasks

### Task 1: `TrapPoolDescriptor` + manifest drains + registry storage

Add `TrapPoolDescriptor { name: String, tag: String, arm: u32 }` beside `TriggerEventDescriptor`
in `crates/entities/src/data_descriptors/types/reactions.rs`. Widen `LevelManifest`
(`crates/scripting-core/src/data_descriptors/runtime_manifest.rs`) with
`trap_pools: Vec<TrapPoolDescriptor>`. Parse the `trapPools` key in both converter paths by
mirroring the trigger-event drains exactly: a `drain_trap_pools_js(&obj, "setupLevel")` sibling to
`drain_trigger_events_js` (`crates/scripting-core/src/data_descriptors/js/manifest.rs:62`) called
from `LevelManifest::from_js_value`, and a `drain_trap_pools_lua` sibling to
`drain_trigger_events_lua` (`.../lua/manifest.rs:66`) called from `from_lua_table`. Per-entry
validation, warn-and-skip (never abort the manifest): `name` and `tag` required non-empty strings;
`arm` a required non-negative integer (reject fractional and negative numbers); reject a duplicate
`name` within the array (warn, skip the later entry). Storage: `DataRegistry`
(`crates/entities/src/data_registry.rs`) gains a level-scoped `level_trap_pools:
Vec<TrapPoolDescriptor>` with a `set_level_trap_pools(...)` setter called from the same
`install_world_cpu` block that calls `populate_level_with_trigger_events`
(`crates/postretro/src/startup/lifecycle.rs:1353-1361` — one added call), a read accessor
`trap_pools()`, and clearing on the same level-unload path that clears level reactions/crossings.
Level-local only: no `ModManifest` tier, no `staged_manifest.rs` edits.

### Task 2: Seeded arming pass + CLI seed override

New module `crates/postretro/src/trap_pools.rs` owning the whole pass. **PRNG:** copy the
SplitMix64 from `crates/net/src/harness.rs:33-65` (private there; copy, don't export — module-local
`pub(crate)` struct) — integer-only, no floats in selection. **Seed resolution:** `--trap-seed
<u64>` / `--trap-seed=<u64>` parsed in `crates/postretro/src/startup/session.rs` beside the
existing manual `--content-root`/`--headless` scanners, stored on the session boot config;
absent → headless sessions use fixed seed `0`, windowed sessions derive entropy from
`SystemTime::now()` nanos scrambled through SplitMix64. Thread the resolved
`Option<u64>` override into `WorldInstallHandles` (`lifecycle.rs:1191`) as a new field, alongside a
gate reusing the already-threaded `suppress_ai_enemies` flag: the pass runs iff
`!suppress_ai_enemies` (that flag is set only on a connected client — the same host/single-player
gating E18-C's spawner registration uses). **Call site:** inside `install_world_cpu`, after
`resolve_spawners_for_level` (`lifecycle.rs:1466-1469`) and before the `levelLoad` fire
(`:1531`), so `levelLoad` reactions observe — and may override — the final armed set. **Pass:**
read `data_registry.trap_pools()`; log `[TrapPools] seed=<n>` once per roll; for each pool in
declaration order, resolve members via
`registry.query_by_component_and_tag(ComponentKind::TriggerVolume, Some(tag))`
(`crates/entities/src/registry.rs:709`), collect and sort `EntityId`s ascending (stable spawn
order), warn+skip an empty member set, warn+clamp `arm > len` to all, pick `arm` distinct members
by partial Fisher–Yates over one PRNG stream shared across pools, then apply through the shipped
helpers `arm_trigger_targets` / `disarm_trigger_targets`
(`crates/postretro/src/trigger_system.rs:451/:461` — pass the `command_diagnostics` handle already
in scope in `install_world_cpu`). Warn once per member whose component has
`enabled_on_spawn == true`, and once per member matched by more than one pool (track ids across
pools; later pool wins). Log each pool's selected set. **Report:** the pass returns
`TrapPoolInstallReport { seed: u64, pools: Vec<TrapPoolOutcome> }` (`TrapPoolOutcome`: pool name,
tag, member ids, selected ids); return it through a new field on `WorldInstallProducts`
(`lifecycle.rs:1160`), unpacked onto `App` where the other products land, defaulting empty on a
connected client. Unit tests live in the module; install-integration tests drive
`install_world_cpu` headless.

### Task 3: SDK surface — `defineTrapPool`, `trapPools` key, typedefs, parity

Add `defineTrapPool(pool: { name: string; tag: string; arm: number }): TrapPoolDescriptor` to
`sdk/lib/data_script.ts` (a pure identity helper, the `defineMapCatalog` shape — no FFI, no
freezing beyond what siblings do) and the `TrapPoolDescriptor` type; widen the exported
`LevelManifest` type (`data_script.ts:110-119`) with `trapPools?: TrapPoolDescriptor[]`. Mirror
both in `sdk/lib/data_script.luau`, and add `defineTrapPool` to the Luau global allowlist
`DATA_SCRIPT_FIELDS` (`crates/scripting-core/src/luau_prelude.rs:127-140`) — an unlisted global is
never lifted after the data script evaluates. Extend the typedef templates
(`crates/scripting-core/src/typedef/templates/sdk_lib.d.ts` `LevelManifest` at `:284-287`, and
`sdk_lib.luau` twin) with the builder and the manifest key; regenerate `sdk/types/postretro.d.ts` /
`.d.luau` via `gen-script-types`; update the committed drift snapshot
(`crates/postretro/src/scripting/typedef/tests/`). Add a TS/Luau parity fixture asserting both
runtimes produce byte-identical `trapPools` manifest data for the same authored pool. SDK builders
do not pre-reject values beyond shape (required keys present, `arm` a number); the engine drains
(Task 1) own warn-and-degrade, matching the `edge`-value precedent.

### Task 4: Dev-tools Triggers tab — Pool column

Extend `TriggerDiagnosticsRow` (`crates/renderer/src/render/debug_ui/mod.rs:98`) with
`pool: String` (empty when the trigger is in no pool) and `pool_selected: bool`; widen
`draw_triggers_tab`'s `num_columns` from 10 to 11 (`:720`) and render the pool cell as name +
selected/unselected mark. The renderer cannot see trap-pool types: `collect_trigger_diagnostics_rows`
(`crates/postretro/src/trigger_diagnostics.rs:21`) gains a `&TrapPoolInstallReport` parameter
(empty report off-host) fed at the render call site in `main.rs` from the `App`-stored report
(Task 2), joining member ids to rows; update the function's existing test callers. Everything
compiles out without `--features dev-tools`.

### Task 5: Fixture + determinism and net QA

Author `content/dev/maps/trap-pools.map` plus its companion script (follow the
`closet-reveal.map` set-piece precedent): four spawn-flavor closet traps — each a
`trigger_volume` member tagged into one pool, firing a `spawnFromSpawner` reaction at its
co-placed `entity_spawner` — declared `defineTrapPool({ name: "closets", tag: "closet_trap",
arm: 2 })`. Headless tests: exact armed count; same-seed reproducibility and pinned
differing-seed divergence; restart-with-pinned-seed identity; unselected-member silence then
runtime `armTrigger` re-arm-and-fire; the degradation matrix (empty tag, over-count clamp,
`arm: 0`, malformed entry skip, duplicate name skip, overlap warning + later-pool-wins,
`enabled_on_spawn = true` warning). Two-endpoint (loopback harness, E18 net-QA precedent): host
installs with a pinned seed; assert the client ran no pass (client trigger armed state as
authored), and a host-armed trap firing spawns an enemy that reaches the client via existing
replication. Extend the determinism harness green-and-stays-green gate with a fixed-seed
trap-pool tick sequence.

## Sequencing

**Phase 1 (sequential):** Task 1 — descriptor + drains + storage; everything reads it.
**Phase 2 (concurrent):** Task 2 (engine pass + CLI, `postretro`/`entities` runtime files) and
Task 3 (SDK + typedefs) — disjoint files; both consume Task 1's descriptor shape.
**Phase 3 (concurrent):** Task 4 (overlay; consumes Task 2's report) and Task 5 (fixture + tests;
consumes Task 2's pass and Task 3's builder).

## Rough sketch

- **Selection is engine-evaluated data, exactly like mover commands and spawner config** — scripts
  declare `{ name, tag, arm }`, the engine owns the roll. Not command-buffer IR (RNG forbidden
  there), not a script-visible value, never a shared-seed client computation.
- **One PRNG stream, declaration order.** Deterministic given (seed, declaration order, member id
  order). Member ids sort ascending; identical installs of the same `.prl` spawn identical ids.
- **Arm/disarm mechanics are already right:** `arm_trigger` (full re-arm: clears latch, zeroes
  rearm, re-admits standing occupants) and `disarm_trigger` (enter-spring only) are the shipped
  `pub(crate)` helpers; the pass adds no new trigger-state semantics.
- **Why before `levelLoad`:** the fire order makes script-authored overrides (a `levelLoad`
  reaction disarming a tutorial trap) deterministically win over the roll, and the report reflects
  only the roll.
- **Client side:** clients populate trigger components but never evaluate triggers; skipping the
  pass keeps client state as authored and costs nothing. The report defaults empty off-host, so
  the overlay shows no pool data on a connected client.
- **Oversized-file note (soft):** `lifecycle.rs` (3524) and `trigger_system.rs` (2292) are already
  past the split threshold, but this spec adds only a call site to the former and calls existing
  helpers in the latter — the feature lands in a new `trap_pools.rs`. No split-before-extend task
  is warranted; do not grow either file with pass logic.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| pool manifest key | `LevelManifest.trap_pools` / `DataRegistry::level_trap_pools` | `"trapPools"` (setupLevel return key) | `trapPools?: TrapPoolDescriptor[]` | same | n/a |
| pool descriptor | `TrapPoolDescriptor { name, tag, arm }` | object keys `"name"`, `"tag"`, `"arm"` | `defineTrapPool({ name, tag, arm })` | same | n/a |
| seed override | session boot config `Option<u64>` | n/a (CLI `--trap-seed=<u64>`) | n/a | n/a | n/a |
| install report | `TrapPoolInstallReport` (host-local, not replicated) | none — no wire surface | n/a | n/a | n/a |

No FGD KVP, no PRL/binary section, no new wire surface. Membership rides existing entity `_tags`.

## Script syntax examples

```ts
// setupLevel — four authored closet traps, two live per run.
import { defineReaction, defineTrapPool, onTriggerEvent, spawner } from "postretro";

export function setupLevel() {
  return {
    // Each closet: a trigger_volume tagged "closet_trap" (authored
    // enabled_on_spawn = false) whose on_fire springs its own spawner.
    reactions: [
      defineReaction("springClosetA", spawner({ tag: "closet_a" }).fire()),
      defineReaction("springClosetB", spawner({ tag: "closet_b" }).fire()),
      defineReaction("springClosetC", spawner({ tag: "closet_c" }).fire()),
      defineReaction("springClosetD", spawner({ tag: "closet_d" }).fire()),
    ],
    trapPools: [
      // Engine rolls at install (host-only): 2 of the 4 tagged triggers arm.
      defineTrapPool({ name: "hallClosets", tag: "closet_trap", arm: 2 }),
    ],
  };
}
```

```luau
-- Luau parity: same declarative shape, same wire.
function setupLevel(_ctx)
  return {
    trapPools = {
      defineTrapPool({ name = "hallClosets", tag = "closet_trap", arm = 2 }),
    },
  }
end
```

```
// TrenchBroom: one closet trap — pool member trigger + its spawner payload.
{ "classname" "trigger_volume" "enabled_on_spawn" "0" "on_fire" "springClosetA" "_tags" "closet_trap" }
{ "classname" "entity_spawner" "archetype" "grunt" "count" "3" "_tags" "closet_a" }
```

## Open questions

- **Headless default seed.** Pinned here: fixed `0` when no `--trap-seed` is given, preserving the
  batch runner's byte-identical guarantee; windowed default is entropy. If reviewers prefer
  entropy-everywhere with an explicit runspec seed field instead, that is a one-line policy swap —
  flag for human taste before promotion.
- **`arm: 0` silence.** Pinned as valid-and-silent (an "all traps off" debug configuration; the
  report shows it). Warn instead if reviewers judge it a likelier authoring mistake than a debug
  tool.
- **Mod-global pool tier.** Deferred (out of scope) — confirm nobody wants campaign-wide pools
  with `levels` selectors in v1 before promotion locks the level-local shape.

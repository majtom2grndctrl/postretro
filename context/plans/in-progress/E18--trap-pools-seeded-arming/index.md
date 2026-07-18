# E18-D — Trap Pools + Seeded Arming

## Goal

Close the Epic 18 semi-random loop: a per-level script — or a mod manifest scoped by level tags —
declares pools of authored closet traps; at level install a host-only seeded roll arms a declared
count or percentage per pool, so players learn where traps *can* be while each run varies which are
live. Scripts declare pools and arming targets; they never observe or perform the roll. Design
intent: `context/research/co-op-triggers-trap-pools.md` §3 (RNG posture), §4.6. Consumes the shipped arm/disarm substrate (`done/E18--trigger-event-fanout`)
and composes with spawn-closets (`done/E18--spawner-and-closet-containment`).

## Scope

### In scope

- **`triggerPools` manifest key** on the `setupLevel` return bundle — level-local declarative data
  beside `reactions`/`crossings`/`triggerEvents`. Each entry: `{ tag, arm | armPercentage, levels? }`
  — a tag selecting member trigger volumes (also the pool's identity for logs/overlay — the pool IS
  "the trigger volumes carrying tag X") plus exactly one arming form: `arm` (integer count) or
  `armPercentage` (percentage in [0, 100], resolved per install as
  `floor(percentage / 100 × member count)`, evaluated in the pinned order
  `(percentage / 100.0) * member_count` (no fma contraction) then floor —
  bit-identical cross-platform). Parsed warn-and-skip
  per entry in both runtimes, mirroring the `triggerEvents` drains. `levels?` rides the descriptor
  at both tiers (the `TriggerEventDescriptor` precedent); it matters at mod scope.
- **Mod-global `ModManifest.triggerPools` tier** — the same descriptor, selecting levels via
  `levels` (empty/omitted = every level; else exact case-sensitive intersection with the level's
  map-catalog tags — `TriggerEventDescriptor.levels` semantics). Drained in mod-init in both
  runtimes, committed whole at boot and on staged dev reload (the `replace_global_trigger_events`
  pattern), and composed before level-local pools at each level install. When a level-local pool
  and a matched mod-global pool declare the same tag, the level-local one replaces it for that
  level (logged at info — the per-level override idiom; one roll per tag). Rolling stays per-level-install and
  host-only — each matched level rolls independently at its own install; replication posture
  unchanged. A staged reload swaps definitions only, never re-rolls the live level — the next
  install rolls the new set.
- **`defineTriggerPool({ tag, arm | armPercentage, levels? })` SDK builder** — a pure
  identity/validation helper (the `defineMapCatalog` shape): no FFI, returns the descriptor for
  the manifest array. TS + Luau,
  typedefs, drift snapshot, parity fixture. Trap pools are this primitive's motivating consumer;
  the same tagged-pool arm/disarm mechanism generalizes to randomized pickups, ambush points, or
  encounter variants.
- **Pool membership = trigger volumes carrying the pool tag.** The trap's payload (spawner, mover,
  enemy release) hangs off each member trigger's `on_fire` reactions — shipped composition, nothing
  new. Non-trigger entities sharing the tag (a spawner sharing its closet tag) are simply not
  members; no warning.
- **Host-only seeded arming pass at level install**, after placements and trigger bindings
  materialize and before the `levelLoad` fire: resolve each pool's member set by tag, sort members
  by entity id (stable spawn order), resolve the target count (`arm` clamped to the member set,
  `armPercentage` floored against it), pick that many distinct members with a seeded PRNG, arm the
  picks and disarm the rest via the shipped full-re-arm/disarm helpers. The helpers set each
  member's runtime `armed` state directly, overriding whatever `enabled_on_spawn` seeded at spawn,
  so an authored `enabled_on_spawn` value never survives into a pool member's live arming
  (authoring convention is `false`; `true` warns as a likely mistake and is overridden).
- **Seed policy (pinned by research §4.6):** fresh roll per level install, including same-session
  restarts. An explicit `--pool-seed=<u64>` performs the seeded roll in either mode; absent that,
  windowed sessions derive an entropy seed, and headless sessions bypass the roll entirely — every
  pool member arms, per-pool counts ignored. Arm-all is deterministic and keeps the batch runner's
  byte-identical guarantee without baking a seed-dependent subset into unpinned runs; headless
  tests that exercise the roll pin a seed. Seed logged at info on every roll (headless no-seed logs
  the arm-all bypass instead); each pool's armed member set logged with it.
- **Engine-owned PRNG:** SplitMix64 (integer-only, cross-platform deterministic), copied from the
  in-tree netcode-harness implementation. No `rand` dependency. RNG lives only in this install
  pass — never in per-tick evaluation, never in command-buffer IR.
- **Install report** (`TriggerPoolInstallReport`): seed (`Option<u64>`, `None` when the arm-all
  bypass ran — no roll, so no seed) + per-pool member/selected sets, retained host-side for tests
  and the dev overlay. Host-local only.
- **Replication: none.** Clients never run the pass; armed traps manifest to clients only through
  consequences (mover phase, spawned enemies) that already replicate. Two-endpoint QA proves it.
- **Runtime composition:** the pass runs once, before `levelLoad`; script `armTrigger`/
  `disarmTrigger` fired afterwards (puzzle disarms, resets) compose normally and win.
- **Dev-tools Triggers tab:** a Pool column per member row — pool tag + whether the roll selected
  it.
- Fixture map + scripts (level and mod tiers), headless determinism coverage, two-endpoint net QA.

### Out of scope

- Weighted selection, per-member weights, guaranteed/forbidden members — count and percentage only
  in v1 (research names weights as a later axis).
- Any script-visible roll: no primitive returns armed state, no pool query handle, no `@`-input.
  `world.query({ component: "trigger_volume" })` keeps its shipped snapshot (no armed state).
- Replicating the armed set. If a spectator/UI need materializes, the stable shape is one Array
  slot per declared pool (research §4.6 sketch) — sketched there so it isn't reinvented, not built.
- Mid-session re-rolls, wave directors, runtime pool mutation — arming is a load-time decision.
  A later spec may add pool-scoped story-event arming: an event arms previously-unarmed members,
  so an authored space turns from peaceful to dangerous mid-level. v1 already composes per-trigger
  `armTrigger` reactions after the roll (an AC below), so the substrate exists — deferred is only
  the pool-scoped verb, shape unpinned.
- Spawner-side arming state — spawners stay stateless (E18-C); pools gate the *triggers* that fire
  them.
- Sticky per-campaign-run seeds — considered and rejected in research §4.6 (retry tension); reopen
  only with playtest evidence.
- FGD, compiler, PRL, or wire-format changes — none needed; membership rides existing `_tags`.

## Acceptance criteria

- [ ] A `setupLevel` returning `triggerPools: [defineTriggerPool({ tag, arm })]` parses in a TS mod
      and a Luau mod; both runtimes emit byte-identical descriptor data; the typedef drift check
      passes after regeneration.
- [ ] A level declaring one pool (`arm: 2`) over 4 member triggers installs on the host with
      exactly 2 members armed and 2 disarmed, regardless of each member's authored
      `enabled_on_spawn` (headless install test with a pinned seed).
- [ ] Two installs with the same seed produce identical armed sets; two pinned differing seeds
      (chosen by the test) produce different armed sets for the same fixture.
- [ ] With `--pool-seed` pinned, a same-session `restartLevel` re-runs the pass and reproduces the
      identical armed set; the seed is logged on every roll.
- [ ] Without an override, a headless run bypasses the roll and arms every pool member (per-pool
      counts ignored); repeated identical runs stay byte-identical (batch-runner guarantee
      preserved). An explicit `--pool-seed` restores the seeded roll headlessly.
- [ ] An unselected member never fires on player entry; a later `armTrigger` reaction targeting it
      re-arms it and it then fires normally (runtime arming composes after the roll).
- [ ] A selected member behaves as an ordinarily armed trigger: enter executes its bound
      consequential steps in the same sim tick.
- [ ] Degradation (headless, pinned seed): `arm` > member count arms all members and warns; `arm`
      == member count arms all silently; a pool
      tag matching zero trigger volumes warns and the pool is inert; a malformed entry (missing
      `tag`, negative or non-integer `arm`, `armPercentage` outside [0, 100] or non-finite, both or
      neither arming form present) warns and is skipped without aborting the manifest; a duplicate
      pool tag within one tier warns and the later entry is skipped; `arm: 0` — or a percentage
      flooring to 0 — is valid and silently disarms every member (all-traps-off; the report
      records it).
- [ ] A member matched by two pools warns once and the later pool's decision wins (composed
      declaration order: matching mod-global pools first, then level-local), deterministically.
- [ ] A `ModManifest.triggerPools` entry parses in a TS mod and a Luau mod (byte-identical
      descriptor data) and composes by its `levels` selector: it rolls on a level whose catalog
      tags intersect the selector, is absent from a non-matching level, and an empty/omitted
      selector matches every level — including direct `.prl` path loads, whose tag set is empty.
- [ ] `armPercentage: 50` over 4 members arms exactly 2
      (`floor(percentage / 100 × member count)`), resolved against each matched level's own
      member set at its own install.
- [ ] Cross-tier precedence: a level-local pool declaring the same tag as a matched mod-global
      pool replaces it for that level (logged at info; one roll for the tag).
- [ ] A staged dev reload of a mod's `triggerPools` replaces the composed pool set without
      re-rolling the live level; the next level install rolls the new set.
- [ ] A pool member authored `enabled_on_spawn = true` logs a warning naming the entity and is
      still processed (roll outcome overrides the authored value).
- [ ] Two-endpoint: the arming pass runs only on the host; a connected client's trigger components
      receive no roll (client-side armed state stays as authored), while the consequences of a
      host-armed trap firing (mover motion or a spawned enemy) reach the client via existing
      replication. No new wire surface (grep/review gate over the wire structs).
- [ ] Determinism: two identical headless runs with scripted inputs and a fixed seed produce
      identical trigger fire sequences and post-tick registry/slot state with a trap-pool level
      active.
- [ ] With `--features dev-tools`, the Triggers tab shows each member's pool tag and whether the
      roll selected it; without the feature, no pool-overlay code compiles in (compile/review gate,
      not a runtime assertion).

## Tasks

### Task 1: `TriggerPoolDescriptor` + manifest drains + registry tiers

Add `TriggerPoolDescriptor { tag: String, arm: TriggerPoolArm, levels: Vec<String> }` with
`TriggerPoolArm { Count(u32), Percentage(f64) }` beside `TriggerEventDescriptor`
in `crates/entities/src/data_descriptors/types/reactions.rs` — derive only `Debug, Clone, PartialEq`
on both (not `Eq`/`Hash`: `Percentage(f64)` isn't hashable/`Eq`, so a literal mirror of
`TriggerEventDescriptor`'s derive line won't compile). Wire keys: `"arm"` XOR
`"armPercentage"` (exactly one), `"levels"` optional (absent → empty — the `TriggerEventDescriptor`
precedent). Widen `LevelManifest`
(`crates/scripting-core/src/data_descriptors/runtime_manifest.rs`) with
`trigger_pools: Vec<TriggerPoolDescriptor>`. Parse the `triggerPools` key in both converter paths by
mirroring the trigger-event drains exactly: a `drain_trigger_pools_js(&obj, scope)` sibling to
`drain_trigger_events_js` (`crates/scripting-core/src/data_descriptors/js/manifest.rs:62`) called
from `LevelManifest::from_js_value`, and a `drain_trigger_pools_lua` sibling to
`drain_trigger_events_lua` (`.../lua/manifest.rs:66`) called from `from_lua_value` (both reused by
Task 6's mod-init drains — the `scope` label distinguishes diagnostics). Per-entry validation,
warn-and-skip (never abort the manifest): `tag` required non-empty string; exactly one of `arm`
(non-negative integer — reject fractional and negative numbers, and reject a value exceeding
`u32::MAX` by the same warn-and-skip path) and `armPercentage` (finite, in
[0, 100]); `levels` an optional string array. Separately, across the `triggerPools` entries in this
drain, keep a seen-pool-tags set (the `seen_ids` precedent in the trigger-event drains) and skip a
later entry whose pool `tag` was already seen (warn) — pool-tag dedup, distinct from the `levels`
array; the set is keyed by tag `String`, never a `HashSet<TriggerPoolDescriptor>` (the descriptor
isn't `Hash`). Storage parallels the trigger-event tiers (with one deliberate divergence, noted
below) in `DataRegistry`
(`crates/entities/src/data_registry.rs`): retained `level_trigger_pools`, durable
`global_trigger_pools` with `replace_global_trigger_pools` (alias `ScopedTriggerPool =
TriggerPoolDescriptor`, the `ScopedTriggerEvent` pattern at `data_registry.rs:32`), and a composed
active `trigger_pools` rebuilt in `recompose_active_sets`: globals matching the level tags
(`levels_match`) in declaration order, then all level-locals appended unfiltered — their `levels`
field is ignored at level scope, `levels_match` gates the mod-global tier only; a level-local entry whose tag a
matched global also declares drops the global entry (log at info — the override idiom, not a
mistake). Level-locals ride the existing populate call: widen
`populate_level_with_trigger_events` (`data_registry.rs:96`) with a `trigger_pools` parameter fed
from the manifest at `crates/postretro/src/startup/lifecycle.rs:1353-1361` (compiler flags the
test callers); cleared in `DataRegistry::clear` with the other level-local sets. Mod-global
entries arrive via Task 6.

### Task 2: Seeded arming pass + CLI seed override

New module `crates/postretro/src/trigger_pools.rs` owning the whole pass. **PRNG:** copy the
SplitMix64 from `crates/net/src/harness.rs:33-65` (private there; copy, don't export — module-local
`pub(crate)` struct) — integer-only, no floats in selection. **Seed resolution:** `--pool-seed
<u64>` / `--pool-seed=<u64>` parsed once in `crates/postretro/src/startup/session.rs`'s
`build_session` (where argv is already collected) beside the existing manual
`--content-root`/`--headless` scanners, stored on the session boot config shared by both the
windowed and headless entry paths; absent → windowed sessions derive an entropy seed from
`SystemTime::now()` nanos scrambled through SplitMix64, headless sessions resolve to an arm-all
bypass (no roll: every member of every pool arms, per-pool counts ignored; log the bypass in place
of a seed) — unless `--pool-seed` was given, in which case the headless driver performs the seeded
roll too (closing AC5); a malformed `--pool-seed` (non-numeric or out-of-`u64`-range) warns and
falls through to the default policy (entropy seed windowed, arm-all headless). Thread the resolved
policy (pinned seed vs arm-all — a two-variant enum, not `Option<u64>`) into `WorldInstallHandles`
(`lifecycle.rs:1191`) as a new field: three construction literals set it, reading the resolved
policy from the boot config at both real entry points — the windowed site (`lifecycle.rs:744`) and
the headless driver site (`observability/driver.rs:149`, which today constructs its own
`WorldInstallHandles` and would otherwise hardcode the default) — plus the test literal at
`lifecycle.rs:3477` (default arm-all); the compiler flags all three, and the non-windowed sites
default to the arm-all bypass policy absent `--pool-seed`. Alongside a
gate reusing the already-threaded `suppress_ai_enemies` flag: the pass runs iff
`!suppress_ai_enemies` (that flag is set only on a connected client — the same host/single-player
gating E18-C's spawner registration uses). **Call site:** inside `install_world_cpu`, after
`resolve_spawners_for_level` (`lifecycle.rs:1466-1469`) and before the `levelLoad` fire
(`:1531`), so `levelLoad` reactions observe — and may override — the final armed set. **Pass:**
read the composed `data_registry.trigger_pools()` (Task 1 orders it mod-global first, level-local
after); in arm-all mode arm every member of every pool and skip the roll; otherwise log
`[TriggerPools] seed=<n>` once per roll and, for each pool in composed order, resolve members via
`registry.query_by_component_and_tag(ComponentKind::TriggerVolume, Some(tag))`
(`crates/entities/src/registry.rs:709`), collect and sort `EntityId`s ascending (`EntityId`'s `Ord`
is generation-major — deterministic and stable for a given install; restart-identity per AC assumes
members share a generation, true at a fresh install), warn+skip an empty member set, resolve the target count (`Count`: warn+clamp `arm > len`
to all; `Percentage`: `floor((percentage / 100.0) * len)` — evaluated in that pinned order, no fma
contraction, no PRNG), pick that
many distinct members by partial Fisher–Yates over one PRNG stream shared across pools, then apply
through the shipped helpers `arm_trigger_targets` / `disarm_trigger_targets`
(`crates/postretro/src/trigger_system.rs:451/:461` — pass the `command_diagnostics` handle already
in scope in `install_world_cpu`). Warn once per member whose component has
`enabled_on_spawn == true`, and once per member matched by more than one pool (track ids across
pools; later pool wins). Log each pool's selected set, keyed by tag. **Report:** the pass returns
`TriggerPoolInstallReport { seed: Option<u64>, pools: Vec<TriggerPoolOutcome> }` (`seed` is `None`
when the arm-all bypass ran — no roll, no seed to report; `TriggerPoolOutcome`:
tag, member ids, selected ids); return it through a new field on `WorldInstallProducts`
(`lifecycle.rs:1160`), unpacked onto `App` as `trigger_pool_report`, where the other products land,
defaulting empty on a connected client. Unit tests live in the module; install-integration tests drive
`install_world_cpu` headless.

### Task 3: SDK surface — `defineTriggerPool`, `triggerPools` key, typedefs, parity

Add `defineTriggerPool(pool: { tag: string; arm?: number; armPercentage?: number; levels?: string[]
}): TriggerPoolDescriptor` to `sdk/lib/data_script.ts` (a pure identity helper, the
`defineMapCatalog` shape — no FFI, no freezing beyond what siblings do) and the
`TriggerPoolDescriptor` type; widen the exported `LevelManifest` type (`data_script.ts:110-119`)
with `triggerPools?: TriggerPoolDescriptor[]`. Mirror both in `sdk/lib/data_script.luau`, and add
`defineTriggerPool` to the Luau global allowlist `DATA_SCRIPT_FIELDS`
(`crates/scripting-core/src/luau_prelude.rs:127-140`) — an unlisted global is never lifted after
the data script evaluates — and, mirroring `defineMapCatalog` in full, to
`POSTRETRO_ROOT_MODULE_EXPORTS` (`crates/scripting-core/src/luau_prelude.rs:254`, the
`require("postretro")` virtual-module exports) and the `virtual_module.luau` typedef template
(`crates/scripting-core/src/typedef/templates/virtual_module.luau:108-110`), so a mod-tier Luau
author using `require("postretro")` gets and type-checks the builder too. Extend the typedef templates
(`crates/scripting-core/src/typedef/templates/sdk_lib.d.ts` `LevelManifest` at `:284-287`, and
`sdk_lib.luau` twin) with the builder and the manifest key, and add a `triggerPools?` field to the
generated `ModManifest` typedef — a `.field(...)` on the `register_type("ModManifest")` builder
(`crates/postretro/src/scripting/primitives/mod.rs:539`); regenerate `sdk/types/postretro.d.ts` /
`.d.luau` via `gen-script-types`; update the committed drift snapshot
(`crates/postretro/src/scripting/typedef/tests/fixtures/expected.d.ts` + `.d.luau`). Add a TS/Luau parity fixture asserting both
runtimes produce byte-identical `triggerPools` manifest data for the same authored pools —
level-local count form and mod-global percentage form with a `levels` selector, the percentage form
using exactly-representable values (e.g. 50, 25 — not 33) so the byte-identical f64 assert holds
across QuickJS and Luau. SDK builders do not
pre-reject values beyond shape (required keys present, arming fields numbers); the engine drains
(Task 1) own warn-and-degrade, matching the `edge`-value precedent.

### Task 4: Dev-tools Triggers tab — Pool column

Extend `TriggerDiagnosticsRow` (`crates/renderer/src/render/debug_ui/mod.rs:98`) with
`pool: String` (empty when the trigger is in no pool) and `pool_selected: bool`; widen
`draw_triggers_tab`'s `num_columns` from 10 to 11 (`:720`) and render the pool cell as tag +
selected/unselected mark — for a member matched by more than one pool, the row shows the deciding
(later/winning) pool, and `pool_selected` reflects that pool's outcome. The renderer cannot see trap-pool types: `collect_trigger_diagnostics_rows`
(`crates/postretro/src/trigger_diagnostics.rs:21`) gains a `&TriggerPoolInstallReport` parameter
(empty report off-host) fed at the render call site in `main.rs` from the `App`-stored
`trigger_pool_report` (Task 2), joining member ids to rows; update the function's existing test callers. Everything
compiles out without `--features dev-tools`.

### Task 5: Fixture + determinism and net QA

Author `content/dev/maps/trap-pools.map` plus its companion script (follow the
`closet-reveal.map` set-piece precedent): four spawn-flavor closet traps — each a
`trigger_volume` member tagged into one pool, firing a `spawnFromSpawner` reaction at its
co-placed `entity_spawner` — declared `defineTriggerPool({ tag: "closet_trap", arm: 2 })`. Add a
second quartet of members tagged `ambush_trap` for the mod tier: the fixture mod's start script
declares `defineTriggerPool({ tag: "ambush_trap", armPercentage: 50, levels: ["trap-pools"] })`,
with `trap-pools` a tag on the fixture's map-catalog entry. Headless tests (roll-exercising cases
pin `--pool-seed` — headless no-seed arms all): exact armed count; the no-seed arm-all default
(every member of both pools armed); same-seed reproducibility and pinned differing-seed
divergence; restart-with-pinned-seed identity; unselected-member silence then runtime `armTrigger`
re-arm-and-fire; percentage resolution (`armPercentage: 50` over 4 arms 2); mod-global composition
(the `ambush_trap` pool rolls on the matched fixture level, stays inert on a level without the
catalog tag, and a level-local same-tag declaration replaces it). Blocker: the headless driver
(`observability/driver.rs:145`) hardcodes `active_level_tags = Vec::new()`, so a `levels`-scoped
mod-global pool (`ambush_trap`, `levels: ["trap-pools"]`) never composes headless (empty tags → no
`levels_match`), making both the arm-all-default and seeded mod-global-match coverage for that pool
unreachable — extend the headless driver to source `active_level_tags` from the target level's
map-catalog entry (mirroring `retain_active_level_tags_for_install`, `lifecycle.rs:294`,
`entry.tags.clone()`); pin seeds for install-integration tests via the `WorldInstallHandles` policy
field directly, not the CLI, so they don't depend on argv threading. The degradation matrix covers
empty tag, over-count clamp, `arm: 0` and percentage-to-zero, malformed entry skip incl.
both/neither arming form, duplicate tag skip, overlap warning + later-pool-wins,
`enabled_on_spawn = true` warning. Two-endpoint (loopback harness, E18 net-QA precedent): host
installs with a pinned seed; assert the client ran no pass (client trigger armed state as
authored), and a host-armed trap firing spawns an enemy that reaches the client via existing
replication. Extend the determinism harness green-and-stays-green gate with a fixed-seed
trap-pool tick sequence. Note: warn/info-log assertions (Degradation's warns, the cross-tier and
two-pool ACs' info/warn logs, the `enabled_on_spawn = true` warn) live at this install / mod-init
test level via the `log_capture` harness — scripting-core drain unit tests (Task 1) assert result
shape only (skip/inert/clamp/zero), with no log capture there.

### Task 6: Mod-global tier — mod-init drains + boot/staged commit

Drain `ModManifest.triggerPools` in both mod-init paths by calling Task 1's drains beside the
trigger-event ones: `drain_trigger_pools_js` in the QuickJS manifest conversion
(`crates/scripting-core/src/runtime/mod_init_exec.rs:262`) and the Luau twin (`:441`); widen
`ModManifestResult` (`crates/scripting-core/src/runtime/types.rs:73`) with `trigger_pools:
Vec<TriggerPoolDescriptor>`. Boot commit: drain into
`DataRegistry::replace_global_trigger_pools` beside `replace_global_trigger_events`
(`crates/postretro/src/session/mod.rs:610`). Staged dev reload: thread the field through the
staged-manifest commit exactly as `next_global_trigger_events`
(`crates/scripting-core/src/runtime/core.rs:199/:226/:365`) — every destructured arm: the `Built`
arm, the `NoStartScript` arm (with its `Vec::new()` padding), the `Failed` early-return, and the
`#[cfg(not(debug_assertions))]` block, easy to miss; the existing post-commit
`recompose_active_sets` call (`main.rs:3700`) then refreshes the composed pool set for free.
Definitions only — a staged reload never re-rolls the live level; the next install rolls the new
set (the arming pass runs only in `install_world_cpu`). Tests: mod-init parse in both runtimes
(malformed entry warn-and-skip), staged-commit replacement visible in the composed set, and
`levels` selection against catalog tags (match, non-match, empty selector, and the empty-tag
direct-`.prl` case).

## Sequencing

**Phase 1 (sequential):** Task 1 — descriptor + drains + registry tiers; everything reads it.
**Phase 2 (concurrent):** Task 2 (engine pass + CLI, `postretro`/`entities` runtime files),
Task 3 (SDK + typedefs), and Task 6 (mod-init drains + commits: scripting-core runtime files +
`session/mod.rs`) — disjoint files; all consume Task 1's descriptor shape and registry tiers.
**Phase 3 (concurrent):** Task 4 (overlay; consumes Task 2's report) and Task 5 (fixture + tests;
consumes Task 2's pass, Task 3's builder, and Task 6's mod-global commit).

## Rough sketch

- **Selection is engine-evaluated data, exactly like mover commands and spawner config** — scripts
  declare `{ tag, arm | armPercentage, levels? }`, the engine owns the roll. Not command-buffer IR (RNG forbidden
  there), not a script-visible value, never a shared-seed client computation.
- **One PRNG stream, composed declaration order** (matching mod-global pools first, then
  level-local). Deterministic given (seed, composed order, member id order). Member ids sort
  ascending; identical installs of the same `.prl` spawn identical ids.
- **Percentage resolution is arithmetic, not RNG:** `floor(percentage / 100 × member count)`,
  evaluated in the pinned order `(percentage / 100.0) * member_count` (no fma contraction) then
  floor. Bit-identical across platforms; the PRNG touches only member selection. `floor` matches the script-side `Math.floor` idiom in the
  `world.query` example below.
- **Headless arm-all over a fixed default seed:** a fixed seed is byte-identical too, but bakes a
  seed-and-declaration-order-dependent subset into every unpinned batch run; arm-all is
  order-independent and exercises every authored trap. Subset-sensitive tests pin a seed.
- **Cross-tier layering mirrors reactions/trigger events** (globals compose first, locals after) —
  but the same-tag rule is replace, not merge: two pools rolling one tag would arm/disarm the same
  member set twice, making declaration distance decide state. One declaration per tag per level;
  the local one wins.
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
  helpers in the latter — the feature lands in a new `trigger_pools.rs`. No split-before-extend task
  is warranted; do not grow either file with pass logic.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| pool manifest key | `LevelManifest.trigger_pools` / `ModManifestResult.trigger_pools` / `DataRegistry` level + global tiers | `"triggerPools"` (setupLevel return key and `ModManifest` key) | `triggerPools?: TriggerPoolDescriptor[]` (both manifests) | same | n/a |
| pool descriptor | `TriggerPoolDescriptor { tag, arm: TriggerPoolArm, levels }` | keys `"tag"`, `"arm"` XOR `"armPercentage"`, optional `"levels"` | `defineTriggerPool({ tag, arm?, armPercentage?, levels? })` | same | n/a |
| seed override | session boot config (pinned seed vs arm-all) | n/a (CLI `--pool-seed=<u64>`) | n/a | n/a | n/a |
| install report | `TriggerPoolInstallReport` (host-local, not replicated) | none — no wire surface | n/a | n/a | n/a |

No FGD KVP, no PRL/binary section, no new wire surface. Membership rides existing entity `_tags`.

## Script syntax examples

The pool has no `name` field — its identity (for logs, the install report, and the dev-tools Pool
column) is its `tag`. Authors still get a friendly handle: bind the descriptor to a `const`/`local`
and reference that binding in the returned `triggerPools` array. That binding is author-side only
and is never reflected into runtime data.

```ts
// setupLevel — four authored closet traps, two live per run.
import { defineReaction, defineTriggerPool, spawner } from "postretro";

export function setupLevel() {
  // Named only on the author's side, by this const binding — the pool's
  // runtime identity (logs, install report, dev-tools Pool column) is its tag.
  const hallClosets = defineTriggerPool({ tag: "closet_trap", arm: 2 });

  return {
    // Each closet: a trigger_volume tagged "closet_trap" (authored
    // enabled_on_spawn = false) whose on_fire springs its own spawner.
    reactions: [
      defineReaction("springClosetA", spawner({ tag: "closet_a" }).fire()),
      defineReaction("springClosetB", spawner({ tag: "closet_b" }).fire()),
      defineReaction("springClosetC", spawner({ tag: "closet_c" }).fire()),
      defineReaction("springClosetD", spawner({ tag: "closet_d" }).fire()),
    ],
    // Engine rolls at install (host-only): 2 of the 4 tagged triggers arm.
    triggerPools: [hallClosets],
  };
}
```

```luau
-- Luau parity: same declarative shape, same wire.
local Postretro = require("postretro")

function setupLevel(_ctx)
  -- Named only on the author's side, by this local binding — the pool's
  -- runtime identity (logs, install report, dev-tools Pool column) is its tag.
  local hallClosets = Postretro.defineTriggerPool({ tag = "closet_trap", arm = 2 })

  return {
    reactions = {
      Postretro.defineReaction("springClosetA", Postretro.spawner({ tag = "closet_a" }):fire()),
      Postretro.defineReaction("springClosetB", Postretro.spawner({ tag = "closet_b" }):fire()),
      Postretro.defineReaction("springClosetC", Postretro.spawner({ tag = "closet_c" }):fire()),
      Postretro.defineReaction("springClosetD", Postretro.spawner({ tag = "closet_d" }):fire()),
    },
    -- Engine rolls at install (host-only): 2 of the 4 tagged triggers arm.
    triggerPools = { hallClosets },
  }
end
```

```
// TrenchBroom: one closet trap — pool member trigger + its spawner payload.
{ "classname" "trigger_volume" "enabled_on_spawn" "0" "on_fire" "springClosetA" "_tags" "closet_trap" }
{ "classname" "entity_spawner" "archetype" "grunt" "count" "3" "_tags" "closet_a" }
```

For a plain proportion, `armPercentage` says it declaratively — `armPercentage: 50` arms half of the
members, floored, engine-resolved per install. When the count is computed — half plus one, a
minimum, a difficulty scale — query the authored trigger volumes by tag with `world.query` (the
same primitive `world.query({ component: "trigger_volume" })` uses elsewhere in this spec) and
derive `arm` from the count. This reads only the pre-roll authored
member set, never armed state, so it does not touch the "no script-visible roll" rule (§Out of
scope): the count `world.query(...).length` returned here equals the member count the engine's
arming pass itself resolves via `query_by_component_and_tag` for the same tag, since no trigger
volumes are added between the data script running and the pass.

```ts
// setupLevel — arm ~50% of a tagged pool, computed from the authored member count.
import { defineTriggerPool, world } from "postretro";

export function setupLevel() {
  // Pre-roll authored data, not a roll observation: counting members by tag
  // is unaffected by, and precedes, the host's seeded arm/disarm pass.
  const ambushMembers = world.query({ component: "trigger_volume", tag: "ambush_trap" });
  const halfArmed = defineTriggerPool({
    tag: "ambush_trap",
    arm: Math.floor(ambushMembers.length / 2),
  });

  return {
    reactions: [],
    triggerPools: [halfArmed],
  };
}
```

```luau
-- Luau parity: arm ~50% of a tagged pool, computed from the authored member count.
local Postretro = require("postretro")

function setupLevel(_ctx)
  -- Pre-roll authored data, not a roll observation: counting members by tag
  -- is unaffected by, and precedes, the host's seeded arm/disarm pass.
  local ambushMembers = Postretro.world:query({ component = "trigger_volume", tag = "ambush_trap" })
  local halfArmed = Postretro.defineTriggerPool({
    tag = "ambush_trap",
    arm = math.floor(#ambushMembers / 2),
  })

  return {
    reactions = {},
    triggerPools = { halfArmed },
  }
end
```

A mod-global pool covers many levels, where member counts differ and `setupLevel`'s `world.query`
never runs — so it arms by percentage (or a fixed per-level count) and selects levels by catalog
tags. Each matched level rolls independently at its own install.

```ts
// start-script — one campaign-wide pool: half of each matched level's
// "vent_trap" triggers arm, rolled at that level's own install.
import { defineMod, defineTriggerPool } from "postretro";

export default defineMod({
  name: "campaign",
  triggerPools: [
    defineTriggerPool({ tag: "vent_trap", armPercentage: 50, levels: ["campaign"] }),
  ],
});
```

```luau
-- Luau parity: campaign-wide pool, percentage-armed per matched level.
local Postretro = require("postretro")

return Postretro.defineMod({
  name = "campaign",
  triggerPools = {
    Postretro.defineTriggerPool({ tag = "vent_trap", armPercentage = 50, levels = { "campaign" } }),
  },
})
```

## Resolved decisions

- **Count-vs-percentage surface (settled).** Two mutually exclusive fields: `arm` (integer count)
  XOR `armPercentage` ([0, 100]), valid at both tiers. The rejected alternative — one `arm` field
  reading values below 1 as fractions — saves a field but makes `arm: 1` vs `arm: 0.99` a silent
  semantics cliff. Percentage resolves with `floor` (not round-half-up), matching the script-side
  `Math.floor` idiom. No open questions remain.

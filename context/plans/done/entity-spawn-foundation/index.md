# Entity spawn foundation

## Goal

First step toward the entity-definition vision: rename `classname` to `canonicalName` on script-side descriptors so absence-of-name structurally means "not directly placeable from a map," and introduce a `@BaseClass = EntitySpawn` taxonomy in the FGD with `player_spawn` as the first concrete subclass. Fixes the duplicate-player bug in `campaign-test.map` and puts the engine in a shape where the eventual `mob_spawn` / `item_spawn` siblings slot in without further refactoring.

## Scope

### In scope

- Rename `EntityTypeDescriptor.classname: String` → `canonical_name: Option<String>` (Rust). Script-side property becomes `canonicalName?: string` (TS) / `canonicalName: string?` (Luau).
- Update `apply_data_archetype_dispatch` to look up descriptors by `canonical_name`. Descriptors with `None` no longer match any map placement — the absence is the signal, no flag check.
- Add a warn-once-per-classname-per-sweep when a map placement names a classname with no registered descriptor (excluding engine-special classnames already handled separately: `worldspawn`, `player_spawn`, and built-in FGD-only entities). The warn names the classname and the placement's `diagnostic_origin()`, directing authors toward the correct placement path.
- FGD: add `@BaseClass = EntitySpawn` carrying `_tags`. Rename `info_player_start` → `player_spawn` as `@PointClass base(EntitySpawn) ... = player_spawn`. Keep the existing `entity_class` and `angles` properties. Inheriting from `EntitySpawn` adds `_tags` to `player_spawn` — the current `info_player_start` body has no `_tags` field, so nothing is removed.
- Engine: update `PLAYER_START_CLASSNAME` value from `"info_player_start"` to `"player_spawn"`. `spawn_from_player_starts` continues to be the hardcoded marker-routing path; this is a name change, not a structural one.
- Update `content/dev/scripts/player.ts` (and `.js` sibling) to omit `canonicalName`. The player archetype becomes marker-spawn-only structurally; direct `"classname" "player"` placements no longer match.
- Update other `defineEntity` call sites in `content/dev/scripts/` to use `canonicalName:` instead of `classname:`.
- Update `campaign-test.map`: rename the marker entity from `info_player_start` to `player_spawn`, set its `origin` to `1808 2592 72`, delete the stray `player` entity block. Angle `90` is unchanged.
- Regenerate `sdk/types/postretro.d.ts` and `sdk/types/postretro.d.luau` so the renamed property is visible.

### Out of scope

- Script-side `@BaseClass` mixin support (composable archetypes — script does not yet inherit; it stays flat). The FGD inheritance is hand-authored for now.
- FGD generation from script registry — FGD remains hand-edited this round.
- `mob_spawn` / `item_spawn` concrete subclasses — wait for the gameplay need.
- `world.spawn(canonicalName, transform)` script primitive — needed for `mob_spawn`, not for this work.
- `_tags` multi-tag query semantics review (any vs all matching) — separate concern.
- Generalizing `spawn_from_player_starts` into a tag-driven marker-routing system. The hardcoded path stays; only the classname it scans for changes.
- Renaming other `info_*` entities. Player is the one with a live bug.
- Removing the `entity_class` KVP from `player_spawn`. Mod-overridable spawn archetype is still useful.

## Acceptance criteria

- [ ] Loading `content/dev/maps/campaign-test.map` spawns exactly one entity carrying `PlayerMovementComponent`. Its `Transform.position` equals `(1808, 2592, 72)` and its yaw matches angle `90`.
- [ ] Given the post-rename `player` descriptor (no `canonicalName`), loading a map containing a direct `"classname" "player"` entity at any position spawns zero entities carrying `PlayerMovementComponent` from that placement.
- [ ] In the same scenario, a `warn!` fires exactly once per distinct unknown classname per dispatch sweep, naming the classname and the diagnostic origin of the first placement encountered. Multiple direct placements of the same classname produce one warn, not N.
- [ ] Loading a map with one `player_spawn` and no stray `player` placement spawns exactly one player at the marker — behavior unchanged from current `info_player_start` flow except for the classname.
- [ ] Existing script-registered archetypes that set `canonicalName` (e.g., `light`, `emitter`-bearing archetypes, `grunt`) continue to spawn from direct map placement.
- [ ] SDK type files declare `canonicalName?: string` (TS) and `canonicalName: string?` (Luau) on `EntityTypeDescriptor`; the drift test `committed_sdk_types_match_current_registry` passes.
- [ ] `sdk/TrenchBroom/postretro.fgd` contains `@BaseClass = EntitySpawn` carrying `_tags` and a `@PointClass base(EntitySpawn) ... = player_spawn`. The `info_player_start` block is gone.
- [ ] No source-tree reference to `"info_player_start"` remains in Rust, TS, Luau, FGD, or `.map` content under version control.

## Tasks

### Task 1: `canonicalName` rename in Rust core

Rename `EntityTypeDescriptor.classname: String` → `canonical_name: Option<String>` in `data_descriptors.rs`. Sweep every `EntityTypeDescriptor { ... }` literal across the crate — known sites: `data_archetype.rs` (`stub_descriptor` ~850, `light_descriptor` ~368, inline test literals at ~lines 369, 602, 641, 677, 710, 745, 798, 851), `data_registry.rs:108` (`grunt_descriptor`), `reaction_dispatch.rs:466` and `:476`. Grep `EntityTypeDescriptor {` to confirm none missed. Wrap existing string values in `Some(...)`.

Update `apply_data_archetype_dispatch` in `builtins/data_archetype.rs` so the descriptor lookup matches the map's `classname` KVP against each descriptor's `canonical_name`, skipping descriptors with `None`. The existing collision-warn dedup at `data_archetype.rs:228` is the pattern for the new unknown-classname warn.

Update both FFI walkers — `entity_descriptor_from_js` (`data_descriptors.rs:434`) and `entity_descriptor_from_lua` (`data_descriptors.rs:901`) — to read an optional `canonicalName` string from the script-side descriptor (currently they read a required `classname`). When absent, store `None`. The `FromJs`/`FromLua` impls at `conv.rs:859` and `:867` delegate to these walkers — no edit needed there.

Update `PLAYER_START_CLASSNAME` at `data_archetype.rs:283` from `"info_player_start"` to `"player_spawn"`. `spawn_from_player_starts` is otherwise unchanged.

When sweeping for `EntityTypeDescriptor { ... }` struct literals, also update embedded JS and Lua script strings in `crates/postretro/src/scripting/runtime.rs` (~lines 1353, 1367, 1387, 1403, 1418, 1432, 1653, 1664, 1720, 1731, 1786, 1808) that inline entity descriptor objects using `classname:`. These pass through the FFI walkers and must use `canonicalName:` after the rename.

The unknown-classname warn must also skip any classname already in the `handled` set passed to `apply_data_archetype_dispatch` (the same set used to avoid re-dispatching classnames already handled by `apply_classname_dispatch`). Skipping `handled` classnames first; then apply the inline exclusion list for `worldspawn` and `player_spawn`.

Add tests in `data_archetype.rs`:
- A descriptor with `canonical_name = None` and two direct `.map` placements of any classname in a single `apply_data_archetype_dispatch` call → zero entities spawned for that classname, exactly one unknown-classname warn emitted naming the classname and the diagnostic origin of the first placement encountered in iteration order.
- A descriptor with `canonical_name = Some("player")` (placed back temporarily for the test) and `canonical_name = None` for `player`, plus a `player_spawn` marker → marker-routed spawn still produces exactly one player entity.
- A descriptor with `canonical_name = Some("foo")` spawns as before from direct map placement (regression guard).

Add the unknown-classname warn dedup using a per-sweep `HashSet<String>` mirroring the `collision_warned` pattern. Exclude engine-special classnames from the warn: `worldspawn`, `player_spawn`, and FGD-only entity types that have no script descriptor by design. Maintain the exclusion list inline near the warn site.

Add a code comment near the dispatch lookup pointing at the roadmap entries (composable archetypes, FGD from registry, `mob_spawn`) so the next person sees where the abstractions go next.

### Task 2: SDK types regeneration

Edit `EntityTypeDescriptor` in `crates/postretro/src/scripting/typedef.rs` (TS block ~line 1114, Luau block ~line 1294): rename `classname: string` → `canonicalName?: string` (TS) and `classname: string` → `canonicalName: string?` (Luau). Regenerate via `cargo run -p postretro --bin gen-script-types` and commit the updated `sdk/types/postretro.d.ts` and `sdk/types/postretro.d.luau`. The drift test at `typedef.rs:1747` keeps them in sync.

### Task 3: Script content rename

Sweep `content/dev/scripts/` for every `defineEntity` call: rename the `classname:` property to `canonicalName:`. For the player descriptor in `content/dev/scripts/player.ts`, omit `canonicalName` entirely (the player is marker-spawn-only). Regenerate `player.js` and any other `.js` siblings using `scripts-build --in <entry.ts> --out <output.js>` (the project's TypeScript compiler). Debug builds auto-compile at startup, but the committed copies should be current.

Verify the actual mod-init entry file under `content/dev/scripts/` still includes `playerEntity` in the `entities` array returned from `setupMod()`.

### Task 4: FGD update

Edit `sdk/TrenchBroom/postretro.fgd`:

- Add `@BaseClass = EntitySpawn` carrying a single property: `_tags(string) : "Space-delimited tags for script queries" : ""`. Place it near the other `@BaseClass` declarations (close to `@BaseClass = Light` at line 25).
- Remove the existing `@PointClass = info_player_start` block (lines ~122–132).
- Add `@PointClass base(EntitySpawn) color(0 220 0) size(-16 -16 -24, 16 16 32) = player_spawn : "Player Spawn Point"` carrying the existing properties: `angles(angles)` and `entity_class(string)`. `_tags` arrives via `EntitySpawn` inheritance — the current `info_player_start` body has none to remove.
- Update the leading doc comment to name `player_spawn` instead of `info_player_start`.

### Task 5: Map fix

Edit `content/dev/maps/campaign-test.map`:

- Delete the entity block whose `classname` is `player` (origin `1808 2592 72`).
- In the surviving marker block, change the `classname` value from `info_player_start` to `player_spawn`, and replace its `origin` with `1808 2592 72`. The existing `angle` of `90` is unchanged.

### Task 6: Sweep remaining `info_player_start` references

Three groups of files outside the main task scope still reference `"info_player_start"` and must be updated to satisfy AC #8.

**Other `.map` files** — rename the `info_player_start` marker classname to `player_spawn` in each of:
- `content/dev/maps/occlusion-test.map`
- `content/dev/maps/test_animated_weight_maps_single.map`
- `content/dev/maps/test_animated_weight_maps_cap.map`
- `content/dev/maps/test_animated_weight_maps_mixed.map`
- `content/dev/maps/test_animated_weight_maps_occluded.map`

These maps don't require a live player spawn for their test purposes, but the classname string must match the renamed constant so the engine doesn't warn on load.

**`crates/postretro/src/main.rs`** — update any comment or log strings that mention `info_player_start`. These are textual; no behavioral change.

**`crates/level-compiler/src/parse.rs`** — update test fixture strings at ~lines 1163, 1167, 1208, 1212, 1213, 1280, 1281, 1285. These embed `info_player_start` as a hardcoded classname in parse tests; replace with `player_spawn`. Confirm the tests still compile and pass after the rename.

After all three groups: `grep -r "info_player_start" .` must return no hits under version control.

## Sequencing

**Phase 1 (sequential):** Task 1 — renames the field everywhere, retargets dispatch, updates `PLAYER_START_CLASSNAME`. Blocks all downstream work because the type signature changes.

**Phase 2 (concurrent):** Task 2, Task 3, Task 4, Task 6 — independent file groups (SDK types, script content, FGD, reference sweep). None depend on each other; all depend only on Phase 1's field rename being committed.

**Phase 3 (sequential):** Task 5 — map edit. Depends on Task 4 because the marker classname literal `player_spawn` must exist in the FGD before TrenchBroom (or any sanity check) recognizes the edited map.

**Phase 4 (sequential):** end-to-end verification — compile the map with `prl-build`, run the engine, confirm AC #1, #2, #3, #4 against the running build.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| canonical-name | `EntityTypeDescriptor.canonical_name: Option<String>` | hand-read in both FFI walkers as optional `canonicalName` (stored `None` when absent), matching the pattern used for `components` | `canonicalName?: string` on `EntityTypeDescriptor` | `canonicalName: string?` on `EntityTypeDescriptor` | n/a — FGD `@PointClass` names (e.g. `player_spawn`) are the wire string; the script-side `canonicalName` is what dispatch matches against |
| player-spawn marker | `PLAYER_START_CLASSNAME = "player_spawn"` | n/a | n/a | n/a | `@PointClass = player_spawn` inheriting `@BaseClass = EntitySpawn` |
| entity-spawn mixin | n/a (no Rust counterpart yet) | n/a | n/a (no script-side `@BaseClass` yet) | n/a | `@BaseClass = EntitySpawn` carrying `_tags` |

Wire casing matches the existing pattern: snake_case in Rust, camelCase on the wire and in script-facing types, snake_case for FGD `@PointClass`/`@SolidClass` identifiers, PascalCase for `@BaseClass` identifiers.

## Rough sketch

Files touched:

- `crates/postretro/src/scripting/data_descriptors.rs` — rename field on the struct (`classname: String` → `canonical_name: Option<String>`). Update both FFI walkers (`entity_descriptor_from_js` ~line 434, `entity_descriptor_from_lua` ~line 901) to read optional `canonicalName`. Sweep struct literals.
- `crates/postretro/src/scripting/builtins/data_archetype.rs` — dispatch lookup change in `apply_data_archetype_dispatch` (~line 220); per-sweep `HashSet<String>` for unknown-classname warn dedup mirroring `collision_warned` at line 228; exclusion list inline near the warn; `PLAYER_START_CLASSNAME` value update at line 283; new tests for the AC scenarios.
- `crates/postretro/src/scripting/data_registry.rs` — `grunt_descriptor` literal at line 108.
- `crates/postretro/src/scripting/reaction_dispatch.rs` — literals at lines 466 and 476.
- `crates/postretro/src/scripting/typedef.rs` — TS block ~1114, Luau block ~1294. `sdk/types/postretro.d.ts` and `sdk/types/postretro.d.luau` are generated; regenerate via `gen-script-types`.
- `content/dev/scripts/*.ts` and `.js` siblings — rename `classname:` → `canonicalName:` at every `defineEntity` site; omit entirely in `player.ts`.
- `sdk/TrenchBroom/postretro.fgd` — add `@BaseClass = EntitySpawn`; remove `info_player_start`; add `player_spawn` as `@PointClass base(EntitySpawn)`.
- `content/dev/maps/campaign-test.map` — marker classname + origin change, stray-entity deletion.
- `content/dev/maps/occlusion-test.map`, `test_animated_weight_maps_*.map` (5 files) — marker classname rename only.
- `crates/postretro/src/main.rs` — comment/log string sweep.
- `crates/level-compiler/src/parse.rs` — test fixture string sweep (~lines 1163, 1167, 1208, 1212, 1213, 1280, 1281, 1285).

## Open questions

None. The shape (canonicalName rename + EntitySpawn base + player_spawn rename) is locked. Implementation may discover edge cases in the FFI walker rename or in defining the exclusion list for the unknown-classname warn; capture those as task-time decisions rather than re-opening the spec.

# Fix duplicate player spawn

## Goal

`content/dev/maps/campaign-test.map` carries both an `info_player_start` marker and a stray `player` map entity that predates the marker system. Both spawn at level load — the stray wins the camera-follow race, leaving the marker dead. Resolve the visible bug by fixing the map, and close the structural footgun so any future map containing a direct `player` placement cannot bypass the marker pipeline.

## Scope

### In scope

- Remove the stray `player` map entity from `content/dev/maps/campaign-test.map`.
- Move the existing `info_player_start` to the location previously occupied by the stray entity (`1808 2592 72`, angle `90`).
- Prevent the data-archetype dispatch from spawning the `player` archetype when a map places it directly. The marker-routed path (`spawn_from_player_starts` reading `entity_class`) remains the only way the player descriptor is materialized.
- Diagnostic on direct placement attempt: warn with the placement's origin diagnostic, name the classname, point the author at `info_player_start`.

### Out of scope

- Renaming `classname` to `canonicalName` in the scripting API or PRL format.
- FGD generation from the script registry.
- `@BaseClass`-mixin / composable-archetype refactor.
- Generalizing the spawn-only mechanism to other archetypes — no current descriptor besides `player` needs it. The fix is shaped to extend cleanly, but additional spawn-only archetypes are not added here.
- Changes to `info_player_start`'s FGD entry, the `entity_class` KVP, or the marker-routing logic in `spawn_from_player_starts`.

## Acceptance criteria

- [ ] Loading `content/dev/maps/campaign-test.map` spawns exactly one entity carrying `PlayerMovementComponent`.
- [ ] That entity's `Transform.position` equals `(1808, 2592, 72)` and its yaw matches angle `90`.
- [ ] Loading a map that contains a direct `"classname" "player"` entity (at any position) spawns zero entities carrying `PlayerMovementComponent` from that placement. A `warn!` line names the offending placement origin and points the author at `info_player_start`.
- [ ] Loading a map with one `info_player_start` and no stray `player` placement spawns exactly one player at the marker position — unchanged from current behavior.
- [ ] Existing `spawn_from_player_starts` tests continue to pass: marker-routed spawn of the `player` archetype is unaffected by the dispatch guard.

## Tasks

### Task 1: Mark `player` as spawn-only and guard the dispatch

Introduce a mechanism that lets the `player` descriptor be registered (so `spawn_from_player_starts` can route to it via `entity_class`) while making direct-placement dispatch refuse to spawn it. Two viable shapes — pick at implementation time:

1. **Hardcoded exclusion**: a `const SPAWN_ONLY_CLASSNAMES: &[&str] = &["player"]` in `data_archetype.rs`; `apply_data_archetype_dispatch` checks membership before spawning. Simplest, narrowest, no API change.
2. **Descriptor flag**: add `spawn_only: bool` to `EntityTypeDescriptor` and the script-side registration shape (`registerEntity({ classname, spawnOnly: true, components })`); dispatch reads the flag. More extensible, but touches the SDK type files and the script registration path.

The hardcoded form is preferred for this scope — extensibility is a Future / Speculative concern (see roadmap: composable archetypes, `canonicalName` rename). The implementation should leave a short comment pointing at those roadmap entries so the next person knows where the generalization lives.

In either form, when dispatch detects a direct placement of a spawn-only classname, it must `warn!` once per classname per dispatch sweep (matching the existing collision-warn dedup pattern) with the placement's `diagnostic_origin()` and a message naming `info_player_start` as the supported path.

Update the data-archetype test suite to cover: a direct `player` placement is skipped; the warn fires; marker-routed `player` spawn through `spawn_from_player_starts` still lands.

### Task 2: Fix `campaign-test.map`

Edit `content/dev/maps/campaign-test.map`:

- Delete the entity block at lines 1142–1147 (`"classname" "player"`, `"origin" "1808 2592 72"`, `"angle" "90"`).
- Update the `info_player_start` block at lines 1137–1141 so its `origin` becomes `"1808 2592 72"` and its `angle` becomes `"90"`. The classname and the entity-block structure are unchanged.

Verify post-edit by running `cargo run -p postretro -- content/dev/maps/campaign-test.prl` (after recompiling the map via `prl-build`) and confirming the camera lands at the marker position.

## Sequencing

**Phase 1 (concurrent):** Task 1, Task 2 — independent. Task 1 changes Rust source under `crates/postretro/src/scripting/builtins/` and tests; Task 2 changes a single `.map` file. No shared files, no contract dependency.

**Phase 2 (sequential):** end-to-end verification — compile the map with `prl-build`, run the engine, confirm AC #1, #2, #4 against the running build. Cannot start until both Phase 1 tasks land because the end-to-end check exercises the combined effect.

## Rough sketch

Files touched:

- `crates/postretro/src/scripting/builtins/data_archetype.rs` — add the spawn-only guard inside `apply_data_archetype_dispatch`, near the `find_descriptor` branch. Use the existing `warn_parse`-style pattern (per-sweep `HashSet<String>` for dedup) for the warn-once behavior.
- `crates/postretro/src/scripting/builtins/data_archetype.rs` tests — add cases covering the new branch and pin AC #3.
- `content/dev/maps/campaign-test.map` — entity-block edit only.

If Task 1 takes the descriptor-flag form instead of hardcoded exclusion, also:

- `crates/postretro/src/scripting/data_descriptors.rs` — `spawn_only: bool` field on `EntityTypeDescriptor`, default `false`.
- `sdk/types/postretro.d.ts` + `sdk/types/postretro.d.luau` — `spawnOnly?: boolean` in the `EntityTypeDescriptor` type. Regenerate via `cargo run -p postretro --bin gen-script-types`.
- `content/dev/scripts/player.ts` (+ generated `.js` sibling) — add `spawnOnly: true` to the existing `registerEntity` call.
- The descriptor-drain path in `setupMod` handling — propagate the new field from script-side to Rust struct.

## Open questions

- **Mechanism choice (hardcoded vs. flag).** Recommended: hardcoded exclusion, because the only spawn-only archetype today is the player and the descriptor-flag approach pulls SDK regeneration into a bug-fix scope. The flag is the right shape if a second spawn-only archetype lands within the same milestone; otherwise defer to the broader entity-authoring refactor on the roadmap.
- **Existing test impact.** The unit tests in `data_archetype.rs` that use `stub_descriptor("player")` exercise `spawn_from_player_starts`, not `apply_data_archetype_dispatch`, so they should be unaffected by the guard. Confirm by inspection during implementation; if any test ends up registering `player` for the direct-dispatch path, update it to a different name (`spectator` is already used elsewhere in the file).

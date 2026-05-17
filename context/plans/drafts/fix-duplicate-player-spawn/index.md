# Fix duplicate player spawn

## Goal

`content/dev/maps/campaign-test.map` carries both an `info_player_start` marker and a stray `player` map entity that predates the marker system. Both spawn at level load — the stray wins the camera-follow race, leaving the marker dead. Resolve the visible bug by fixing the map, and close the structural footgun so any future map containing a direct `player` placement cannot bypass the marker pipeline. Expose the spawn-only distinction as a script-declared property so modders can apply the same pattern to their own marker-routed archetypes.

## Scope

### In scope

- Remove the stray `player` map entity from `content/dev/maps/campaign-test.map`.
- Move the existing `info_player_start` to the location previously occupied by the stray entity (`1808 2592 72`, angle `90`).
- Add a `spawn_only` boolean field to `EntityTypeDescriptor` (Rust) and the equivalent `spawnOnly` property to the script-side registration shape (TypeScript and Luau).
- Make `apply_data_archetype_dispatch` skip any descriptor with `spawn_only = true`, emitting a `warn!` once per offending classname per sweep that names the placement origin and points the author at `info_player_start`.
- Update `content/dev/scripts/player.ts` (and its compiled `.js` sibling) to set `spawnOnly: true`.
- Regenerate `sdk/types/postretro.d.ts` and `sdk/types/postretro.d.luau` so the property is visible to authoring.

### Out of scope

- Renaming `classname` to `canonicalName` in the scripting API or PRL format.
- FGD generation from the script registry.
- `@BaseClass`-mixin / composable-archetype refactor.
- Changes to `info_player_start`'s FGD entry, the `entity_class` KVP, or the marker-routing logic in `spawn_from_player_starts`.
- Generalizing absence-of-classname semantics ("no canonical name = spawn only") — the eventual rename will subsume `spawn_only`, but that's a Future / Speculative item on the roadmap.

## Acceptance criteria

- [ ] Loading `content/dev/maps/campaign-test.map` spawns exactly one entity carrying `PlayerMovementComponent`.
- [ ] That entity's `Transform.position` equals `(1808, 2592, 72)` and its yaw matches angle `90`.
- [ ] Loading a map that contains a direct `"classname" "player"` entity (at any position) spawns zero entities carrying `PlayerMovementComponent` from that placement. A `warn!` names the placement origin and points the author at `info_player_start`.
- [ ] The warn fires once per offending classname per dispatch sweep, regardless of how many direct placements of that classname appear.
- [ ] Loading a map with one `info_player_start` and no stray `player` placement spawns exactly one player at the marker position — unchanged from current behavior.
- [ ] A script registering an archetype without `spawnOnly` (or with `spawnOnly: false`) continues to spawn from direct map placement — existing `light`/`emitter`/`movement`-bearing archetypes are unaffected.
- [ ] SDK type files declare `spawnOnly?: boolean` on `EntityTypeDescriptor`; `cargo test` drift check passes.

## Tasks

### Task 1: `spawn_only` descriptor field and dispatch guard

Add `spawn_only: bool` to `EntityTypeDescriptor` (default `false`). Thread it through the script-side registration shape so authors write `spawnOnly: true` on the descriptor object. Wire it through the FFI drain that builds the Rust struct from the script-side object.

In `apply_data_archetype_dispatch`, check the flag before the `try_spawn` call. When `spawn_only` is true, drop the placement and `warn!` once per classname per sweep — match the dedup pattern used for the built-in / data-script collision warn (per-sweep `HashSet<String>`). The warn message names the placement's `diagnostic_origin()`, the classname, and directs the author at `info_player_start`.

Add test coverage in `data_archetype.rs`:
- A descriptor with `spawn_only = true` and a direct placement: zero entities spawned, dedup observed across multiple placements.
- A descriptor with `spawn_only = false` (the existing behavior) spawns as before.
- A descriptor with `spawn_only = true` routed via `spawn_from_player_starts` does spawn (the marker path ignores the flag).

Regenerate SDK types via `cargo run -p postretro --bin gen-script-types`. Commit the regenerated `.d.ts` and `.d.luau`.

Add a code comment near the dispatch guard pointing at the roadmap entries (`canonicalName` rename, composable archetypes) so the next person knows where the future generalization lives — absence of a canonical name will eventually replace this flag.

### Task 2: Fix `campaign-test.map`

Edit `content/dev/maps/campaign-test.map`:

- Delete the entity block at lines 1142–1147 (`"classname" "player"`, `"origin" "1808 2592 72"`, `"angle" "90"`).
- Update the `info_player_start` block at lines 1137–1141 so its `origin` becomes `"1808 2592 72"` and its `angle` becomes `"90"`. Classname and block structure unchanged.

### Task 3: Update `player.ts` to declare `spawnOnly`

Add `spawnOnly: true` to the existing `registerEntity` call in `content/dev/scripts/player.ts`. Regenerate the `.js` sibling (or hand-edit if the script-compiler step does not run automatically in this flow).

## Sequencing

**Phase 1 (sequential):** Task 1 — adds the field and dispatch behavior the other tasks depend on. SDK regeneration must complete inside this phase so downstream script edits can rely on the typed property.

**Phase 2 (concurrent):** Task 2, Task 3 — independent file edits. Task 2 touches a `.map` file; Task 3 touches a `.ts` file. Neither depends on the other.

**Phase 3 (sequential):** end-to-end verification — compile the map with `prl-build`, run the engine, confirm AC #1, #2, #5 against the running build. Cannot start until Phase 2 lands.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| spawn-only flag | `EntityTypeDescriptor.spawn_only: bool` (default `false`) | `"spawnOnly"` (camelCase, via existing `#[serde(rename_all = "camelCase")]`) | `spawnOnly?: boolean` on `EntityTypeDescriptor` | `spawnOnly: boolean?` on `EntityTypeDescriptor` | n/a — not surfaced on any FGD entity |

The wire casing matches the existing pattern: snake_case in Rust, camelCase on the wire and in script-facing types. No new serde plumbing — the existing `rename_all = "camelCase"` derive covers it.

## Rough sketch

Files touched:

- `crates/postretro/src/scripting/data_descriptors.rs` — add `spawn_only: bool` to `EntityTypeDescriptor`. Existing test helpers in `data_archetype.rs` (`stub_descriptor`, `light_descriptor`, inline literals) need the field added — mechanical sweep, default `false` everywhere.
- `crates/postretro/src/scripting/builtins/data_archetype.rs` — guard inside `apply_data_archetype_dispatch` between the `find_descriptor` branch and `try_spawn`. Per-sweep `HashSet<String>` for warn dedup mirrors the existing `collision_warned` pattern at line 228. New tests for AC #3, #4, #6.
- The FFI drain path that converts the script-side descriptor object into `EntityTypeDescriptor` — extend to read the optional `spawnOnly` property. Locate by following the `setupMod` manifest-drain code path; the drain is the only producer of descriptor instances from script side.
- `sdk/types/postretro.d.ts`, `sdk/types/postretro.d.luau` — regenerate; do not hand-edit. The `gen-script-types` binary is authoritative.
- `content/dev/scripts/player.ts` — add `spawnOnly: true` to the existing `registerEntity` call.
- `content/dev/scripts/player.js` — regenerate or hand-edit to match.
- `content/dev/maps/campaign-test.map` — entity-block edit (Task 2).

## Open questions

None. The mechanism (descriptor flag) is locked. Implementation may discover edge cases in the FFI drain path that warrant a follow-up; capture those as task-time decisions rather than re-opening the spec.

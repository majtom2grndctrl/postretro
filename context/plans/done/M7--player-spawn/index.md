# Player Spawn (M7)

> **Status:** ready
> **Depends on:** Mod Script Layer
> **Prerequisite for:** Movement Scripts (M7)
> **Related:** `context/lib/entity_model.md` · `context/lib/build_pipeline.md` · `context/lib/scripting.md §2` · `context/lib/boot_sequence.md`

---

## Goal

Add `info_player_start` as an FGD entity and wire level load to spawn player entities from it. Spawn point supports multiple instances and multiple player classnames, leaving the door open for co-op, character selection, and campaign multi-entrance maps.

---

## Tasks

### 1. `info_player_start` FGD entry

- Add `info_player_start` to `sdk/TrenchBroom/postretro.fgd` as a point entity.
- Optional KVP: `angles` (facing — pitch yaw roll, degrees; default `"0 0 0"`). Position is implicit from point entity placement; do not declare `origin` in the FGD.
- Optional KVP: `entity_class` (string, FGD default `"player"`) — the registered classname to spawn at this point.

### 2. Level load spawn logic

- Before either dispatch sweep runs, a pre-pass extracts all `info_player_start` records from the map entity list into a separate spawn-point list; no `info_player_start` entry reaches the built-in or script-registered dispatch paths. Partition the entity slice into `(spawn_points, others)` and pass `&others` to both dispatch sweeps.
- After both the built-in sweep (`apply_classname_dispatch`) and the script-registered sweep (`apply_data_archetype_dispatch`) complete, the spawn pass resolves each collected spawn point. Both sweeps are called from `main.rs`; the pre-pass and spawn pass slot in immediately before and after those calls.
- For each, spawn one entity of the classname specified by `entity_class` at the spawn point's origin and facing.
- Each `entity_class` is instantiated via the same script-registered descriptor path used by the data-archetype sweep.
- Position is read from `MapEntityRecord::origin`; facing from `MapEntityRecord::angles` (engine radians — `classname`, `origin`, `_tags`, `angle`, `angles`, and `mangle` are reserved KVP keys consumed at compile time and are not present in `key_values`). The compiler converts `angles` degrees to engine radians during MapEntity serialization — no runtime conversion is needed.
- `_tags` from `info_player_start` are copied to the spawned entity (replacing any tags the descriptor may have set initially). All non-reserved KVPs (excluding `classname`, `origin`, `angle`, `angles`, `mangle`, `_tags`, and `entity_class`) are forwarded to the spawned entity's KVP bag.
- If no `info_player_start` is present: log at `info!` and spawn no player entities. Engine continues loading.
- If `entity_class` names a classname not in `data_registry.entities`: log a warning and skip that spawn point.

---

## Acceptance criteria

- A map with one `info_player_start` spawns one entity of the named classname at that position and facing.
- A map with multiple `info_player_start` entries spawns one entity per entry.
- `entity_class` defaults to `"player"` — confirmed by a test that registers a stub `"player"` data archetype as a fixture.
- Spawned entity's facing (in engine radians) matches the degree-to-radian conversion of the `angles` KVP on its `info_player_start`.
- Map with two `info_player_start` entries carrying different `entity_class` values spawns one entity of each classname.
- Absent `info_player_start` logs at `info!` and loads cleanly.
- Unknown `entity_class` logs a warning and skips that spawn point.
- `_tags` on `info_player_start` are present on the spawned entity.
- An `info_player_start` carrying a non-reserved custom KVP (e.g. `loadout`) results in the spawned entity receiving that KVP via the same bag data-archetype-spawned entities use.

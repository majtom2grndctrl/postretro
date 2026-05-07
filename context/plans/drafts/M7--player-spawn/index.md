# Player Spawn (M7)

> **Status:** draft
> **Depends on:** Mod Script Layer
> **Prerequisite for:** Movement Scripts (M7)
> **Related:** `context/lib/entity_model.md` · `context/lib/build_pipeline.md`

---

## Goal

Add `info_player_start` as an FGD entity and wire level load to spawn player entities from it. Spawn point supports multiple instances and multiple player classnames, leaving the door open for co-op, character selection, and campaign multi-entrance maps.

---

## Tasks

### 1. `info_player_start` FGD entry

- Add `info_player_start` to `sdk/TrenchBroom/postretro.fgd` as a point entity.
- Required KVPs: `origin` (position), `angle` (facing). No other required KVPs.
- Optional KVP: `entity_class` (string) — the registered classname to spawn at this point. Defaults to `"player"` if absent.

### 2. Level load spawn logic

- After map entities are parsed and the mod data script has run (entity types registered), locate all `info_player_start` entries in the map entity list.
- For each, spawn one entity of the classname specified by `entity_class` at the spawn point's origin and angle.
- `info_player_start` is consumed by the spawn logic — no persistent map entity remains for it.
- If no `info_player_start` is present: log a warning and spawn no player entities. Engine continues loading.
- If `entity_class` names an unregistered classname: log a warning and skip that spawn point.

---

## Acceptance criteria

- A map with one `info_player_start` spawns one entity of the named classname at that position and facing.
- A map with multiple `info_player_start` entries spawns one entity per entry.
- `entity_class` defaulting to `"player"` is confirmed by test.
- Absent `info_player_start` logs a warning and loads cleanly.
- Unknown `entity_class` logs a warning and skips that spawn point.

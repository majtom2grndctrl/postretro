# Mod Script Layer

> **Status:** draft
> **Prerequisite for:** Player Spawn (M7)
> **Related:** `context/lib/scripting.md`

---

## Goal

Establish a mod-level script execution layer that runs before any level loads. Cross-level concerns — player entity type registration, update priority, game-wide setup — live here rather than in per-map scripts.

---

## Motivation

The existing scripting lifecycle is entirely level-scoped: data scripts run at level load, behavior scripts run during play, everything clears on level unload. Entity types registered via `registerEntity` do survive level unload (they go into the engine-global type registry), but there is no authoring surface for a mod to declare things at engine init time, before any map is loaded.

M7 needs: (1) player entity classname registered before any `info_player_start` spawn logic runs, (2) a mechanism for declaring entity update priority so the engine knows to tick player-class entities first.

---

## Scope

### In scope

- **Mod data script discovery** — path convention for the mod-level data script (e.g. `content/<mod>/scripts/mod-data.ts` / `mod-data.luau`). Engine loads it at init, before any level.
- **Mod data context lifecycle** — single short-lived VM context at engine init; same `registerEntity` API surface as the existing level-load data context. Registrations land in the engine-global type registry and persist for the engine's lifetime.
- **Update priority declaration** — mechanism for a classname to declare player-tier update priority (runs before all other entity ticks). Options: a field on `registerEntity`; a separate `declarePlayerClass` primitive; or a reserved classname convention. Decision required.
- **SDK and scripting reference** — document the mod data script lifecycle and the update priority field.

### Out of scope

- Campaign progression state
- Cross-map persistent entity state
- Networked session setup

---

## Open decisions

1. **Path convention** — how is the mod data script file discovered? Fixed path relative to content root? A KVP in a `postretro.mod` manifest? Something else?
2. **Update priority mechanism** — new field on `registerEntity` (e.g. `updatePriority: "player" | "default"`)? A dedicated primitive? A reserved classname?
3. **Multiple mod data scripts** — does a mod have one entry data script, or can multiple files contribute (lexicographic load order like behavior scripts)?

---

## Acceptance criteria

- Mod data script runs at engine init before any level loads.
- Entity types registered in the mod data script are available when the first level's `info_player_start` spawn logic fires.
- Player-class entities tick before all other entities each frame.
- Mod data script lifecycle documented in `docs/scripting-reference.md`.

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

- **Mod entry point** — `content/<mod>/start-script.{ts,luau}` at the mod root. Engine loads it at init, before any level. Domain scripts (`actors/`, `weapons/`, etc.) are pulled in via `import`/`require` from start-script; no auto-scanning of domain folders. If both `start-script.ts` and `start-script.luau` exist in the same mod, the engine errors at init.
- **Mod data context lifecycle** — single short-lived Definition-kind VM context at engine init. Same `registerEntity` API surface as the existing level-load data context, extended with the `updatePriority` field described below. `registerLevelManifest` is not invoked from this path. Registrations land in the engine-global type registry and persist for the engine's lifetime.
- **Update priority declaration** — `updatePriority` field on `registerEntity`. Accepted values for M7: `"player"` (ticks before all other entities) and `"default"` (normal tick order). Engine errors at `registerEntity` call time if an unknown value is given. The entity registry maintains two tick queues (`player_priority` and `default_priority`), updated at spawn and despawn. M7 ships exactly two buckets. Future `updatePriority` values are out of scope for this plan.
- **Absent start-script** — if a mod has no `start-script`, the engine continues with an empty mod-data context. No error.
- **SDK and scripting reference** — document the mod entry point lifecycle and the `updatePriority` field.

### Out of scope

- Campaign progression state
- Cross-map persistent entity state
- Networked session setup

---

## Acceptance criteria

- `start-script.{ts,luau}` runs at engine init before any level loads.
- Entity types registered in start-script (and domain scripts it imports) are available when the first level's `info_player_start` spawn logic fires.
- Player-class entities (`updatePriority: "player"`) tick before all other entities each frame.
- An entity type registered in a domain script imported by start-script (not directly in start-script itself) is available at first-level entity spawn.
- Registering an entity with an unrecognized `updatePriority` value produces an engine error.
- Mod entry point lifecycle and `updatePriority` field documented in `docs/scripting-reference.md`.
- `EntityTypeDescriptor.updatePriority` present in generated SDK type definitions (`postretro.d.ts` and `postretro.d.luau`).
- A mod with no `start-script.{ts,luau}` boots successfully with no error.

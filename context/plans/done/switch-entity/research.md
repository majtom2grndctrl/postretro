# Switch Entity — Research Notes

Code-grounded facts behind the spec. Ephemeral; identifiers/lines verified at draft
time and will drift.

## The gap: two disjoint spawn paths, neither produces "visible + pressable"

- **`trigger_volume`** — brush-only, **invisible** `@SolidClass`. Compiler branch at
  `crates/level-compiler/src/parse.rs:634` (`classname == "trigger_volume"` inside the
  `has_brushes` block, line 632) → `resolve_trigger_volume`
  (`crates/level-compiler/src/trigger_volumes.rs:13`) → PRL `TriggerVolumeRecord` →
  `TriggerVolumeBridge::populate_from_level` spawns a trigger entity at the brush AABB
  centre, stores the AABB, attaches `TriggerVolumeComponent`. The brush faces are
  **not** emitted as draw geometry — the volume is invisible and non-solid.
- **`prop_mesh` / point entities** — `@PointClass` → generic `MapEntityRecord`
  (`parse.rs:747`) → runtime `ClassnameDispatch` (`crates/postretro/src/scripting/builtins/mod.rs`,
  `register_builtins`, count-locked at 3 by a test) → spawns `Transform` +
  `MeshComponent`. No trigger, no AABB registration.

No path attaches **both** renderable/solid geometry **and** a use-trigger. That union
is the switch.

## Load-bearing finding: use-activation is containment-based

`trigger_system.rs` computes per-player edges with `capsule_overlaps_aabb(center,
radius, half_height, aabb_min, aabb_max)` (the exact upright-capsule-vs-AABB test),
and for `TriggerActivation::Use` fires the `Enter` edge only when the capsule overlaps
**and** `use_pressed` is set for that player. So a `use` trigger requires the player
capsule to **overlap the trigger AABB**.

Consequence for the switch: if the switch's faces become **solid** world geometry, the
player capsule cannot enter the brush volume — collision stops it. So the trigger AABB
must be **inflated past the solid surface** (the `use_reach` margin) into the open space
the player stands in, or the switch can never be pressed. This is why `use_reach` is a
required part of the design, not polish.

## The desugar reuses two existing operations

1. **Static world geometry:** `parse.rs:626` already does
   `world_brush_ids.extend(brush_ids)` for editor-group classnames — an entity's
   brushes becoming static world brushwork (rendered, lit, collided, partitioned). The
   switch reuses this verbatim.
2. **Use trigger:** `resolve_trigger_volume` reads `activation` from props (`"use"` →
   `1`, `trigger_volumes.rs:23-27`) and computes the brush-hull AABB into
   `MapTriggerVolume { name, tags, aabb_min:[f32;3], aabb_max:[f32;3], activation:u8, .. }`
   (`crates/level-compiler/src/map_data.rs:177-182`). The wire record
   `TriggerVolumeRecord` carries `aabb_min`/`aabb_max: [f32;3]`
   (`crates/level-format/src/trigger_volumes.rs:14-18`). Inflate those by `use_reach`.

So the switch branch = fold-to-world + emit-use-trigger-with-inflated-AABB. Everything
downstream (`TriggerVolumeBridge`, `trigger_system`, `TriggerVolumeComponent`,
`on_fire`/`on_exit` fan-out from E18-A) is untouched.

## `TriggerVolumeComponent` fields the switch inherits

`crates/entities/src/components/trigger_volume.rs`: `activation` (Touch/Use),
`target_tag`, `on_fire`, `on_exit`, `command`, `command_arg`, `fire_mode`
(Once/Multiple), `rearm_ms`, `enabled_on_spawn`, plus runtime latch state. The switch
forwards all of these unchanged except `activation`, which it forces to `Use`.

## Why not a point/model switch

A `@PointClass switch` (glTF model + use-radius) would need, all net-new:
- a new `ClassnameDispatch` builtin in `register_builtins` (and the count-lock test
  bump);
- point-origin → AABB ingestion in `TriggerVolumeBridge` (today it ingests only brush
  `TriggerVolumeRecord`s), since a use-radius around a point is not a brush hull.

That is strictly more code than the brush desugar (which adds **zero** runtime code),
for a model-placement workflow less natural to brush mappers than carving switch
brushwork. Rejected for v1; a model switch could return if content demands it.

## Naming / convention

No existing `switch` / `button` / `func_button` entity in the tree (grep hits are Rust
`match`/UI-`Button`/`use_pressed`). FGD classnames are snake_case with no `func_`
prefix — the engine deliberately rejects the Quake `func_`/`targetname` idiom (tags are
the linking currency, per co-op-triggers research §3/§7). `switch` fits; "button"
already means a UI node in this codebase, so `switch` is the less ambiguous name.

## No scripting-type-surface change

`sdk/types/postretro.d.{luau,ts}` declare the queryable **component** union
(`trigger_volume`, etc.), not entity classnames. A switch reusing
`TriggerVolumeComponent` is already queryable as `trigger_volume`; no `.d` change.

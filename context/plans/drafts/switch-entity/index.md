# Switch Entity

## Goal

A `switch` map entity: one authored brush that is **visible, solid, and pressable** —
the player presses `use` on it to fire an activation (a named reaction or a mover
command), over the already-shipped use-activation trigger + fan-out substrate. Today a
button is authored as two co-located entities (a `use` `trigger_volume` brush + a
`prop_mesh`/`kinematic_mover` for the visible part); `switch` collapses that into one.
The co-op-triggers research (§4.2) deferred this sugar class pending proven authoring
friction — we are now electing to build it.

## Scope

The whole feature is a **compile-time desugaring** into two mechanisms that already
ship: static world geometry (the switch's faces render, light, and collide like
worldspawn brushwork) and a `use` `TriggerVolumeRecord` (the press-to-activate volume).
No runtime, PRL, renderer, `ClassnameDispatch`, or `TriggerVolumeBridge` change.

### In scope

- A `switch` `@SolidClass` in `sdk/TrenchBroom/postretro.fgd` (snake_case, no `func_`
  prefix — the engine's convention; tags are the linking currency).
- A `switch` branch in the level compiler's brush-entity dispatch that: (a) folds the
  switch's brushes into the **static world geometry** (visible + collidable), and (b)
  emits a **`use`-forced** `TriggerVolumeRecord` whose AABB is the switch's brush AABB
  **inflated by a `use_reach` margin**, so a player standing in front of the now-solid
  switch overlaps the trigger (use-activation is capsule-vs-AABB overlap —
  `capsule_overlaps_aabb`, an intersection test, not containment).
- Reuse of the shipped trigger vocabulary: `on_fire` / `on_exit` named reactions,
  `target_tag` + `command` / `command_arg` mover control, `fire_mode` (once/multiple),
  `rearm_ms`, `enabled_on_spawn`, `_tags`.

### Out of scope

- **Depress / travel animation.** v1 switch geometry is static. A mapper who wants a
  depressing button still authors a `kinematic_mover` + `switch` pair, or a later
  enhancement lets `switch` optionally own a short mover throw. Not v1.
- **`use`-prompt / "Press USE" HUD hint.** No world-trigger interaction-prompt UI
  exists anywhere; adding one is net-new HUD work. A switch fires on `use` with no
  on-screen prompt, exactly as a `use` `trigger_volume` does today.
- **Point-entity (model) switch.** Considered and rejected (see Decisions). v1 is
  brush-authored.
- **New scripting-type surface.** `switch` reuses `TriggerVolumeComponent`; no
  `.d.luau` / `.d.ts` change (classnames are not declared in the scripting type
  surface — only the queryable component union is, and `trigger_volume` already covers
  it). Because a `switch` is emitted as a `TriggerVolumeComponent`,
  `world.query({ component: "trigger_volume", tag })` now also returns switches —
  indistinguishable at the component/query layer (intended), so script authors
  querying trigger volumes will include switches.
- **A new `activation` choice on the switch.** `switch` is press-to-activate by
  definition; `activation` is forced to `use` by the compiler and is not an authorable
  KVP.

## Acceptance criteria

- [ ] A `switch` brush entity compiles: its faces appear in the **static world
  geometry** (drawn + collidable, indistinguishable from worldspawn brushwork), and a
  single `use`-activation `TriggerVolumeRecord` is emitted covering it. Verified by a
  compiler unit test asserting both the world-geometry contribution and a
  `MapTriggerVolume` with `activation == 1` (use); a `TriggerVolumeRecord` only exists
  after `encode_trigger_volumes_section` runs, so a test wanting the wire record must
  also call the encoder.
- [ ] The emitted trigger's AABB is **larger than the raw switch brush AABB** by the
  `use_reach` margin on every axis (so a flush-standing player capsule can overlap it).
  Verified by the same test comparing `aabb_min`/`aabb_max` against the brush hull on
  each of x/y/z — `aabb_min` strictly decreased and `aabb_max` strictly increased on
  every axis.
- [ ] In-engine, pressing `use` while standing in front of a placed `switch` fires its
  `on_fire` reaction (and/or commands its `target_tag` mover) identically to an
  equivalent `use` `trigger_volume`. Verified on a dev-map fixture (manual gate — the
  switch reuses the shipped `trigger_system` evaluation unchanged).
- [ ] `fire_mode` (once vs multiple), `rearm_ms`, and `enabled_on_spawn` behave
  identically to `trigger_volume` — they map to the same `TriggerVolumeComponent`
  fields, so no re-verification of the mechanism is needed beyond confirming the
  compiler forwards them.
- [ ] A `switch` with none of `on_fire` / `on_exit` / `target_tag` compiles with the
  same "inert" warning `trigger_volume` emits.
- [ ] A `switch` classname does **not** fall through to a generic `MapEntityRecord`
  (it is handled by the brush-entity branch, like `trigger_volume`).
- [ ] The change touches only the FGD, the level compiler, and tests — **no** new PRL
  section, runtime system, render path, `ClassnameDispatch` builtin, or
  `TriggerVolumeBridge` code (review/grep gate).

## Tasks

### Task 1: `switch` FGD class
Add a `switch` `@SolidClass` to `sdk/TrenchBroom/postretro.fgd`, modelled on the
`trigger_volume` class but with the switch semantics: **no `activation` KVP** (the
compiler forces `use`), plus a `use_reach(float)` property (the AABB inflation margin, authored in map units;
its default is a concrete literal — e.g. 24 map units ≈ 0.61 m — that must equal the
compiler's fallback literal; not tied to the runtime capsule constant, which the
`level-compiler` crate cannot reach — see Rough sketch). Carry the shared trigger
authoring surface: `on_fire(string)`, `on_exit(string)`, `name(string)`, `target_tag(string)`,
`command(choices: start/stop/reverse/go_to_path_node)`, `command_arg(string)`,
`fire_mode(choices: once/multiple)`, `rearm_ms(float)`,
`enabled_on_spawn(choices: 0/1)`, `_tags(string)`. Help text states that a `switch` is
visible, solid geometry the player presses `use` on, and that unlike `trigger_volume`
its brushwork renders and collides. Disjoint file from Task 2.

### Task 2: compiler desugar + tests
In the level compiler's brush-entity dispatch (`crates/level-compiler/src/parse.rs`,
beside the `trigger_volume` branch at the `has_brushes` block), add an
`if classname == "switch"` branch that does two things with the switch's `brush_ids`:
(1) **folds them into static world geometry** — `world_brush_ids.extend(brush_ids.iter().copied())`,
the same operation editor-group classnames already use, so the faces flow through the
normal world partition / lighting / collision / draw pipeline; and (2) **emits a
`use`-forced trigger** — build a `MapTriggerVolume` from the same brushes with
`activation` set to `1` (`use`) regardless of any authored value (reuse
`resolve_trigger_volume` with an `activation`-forced props map, or call it then
overwrite `MapTriggerVolume.activation`), then **inflate `aabb_min`/`aabb_max` by
`use_reach * scale`** (parsed from props, default per Rough sketch; `scale =
format.units_to_meters()`, the same scale the AABB vertices already carry — the AABB is
in engine meters but `use_reach` is authored in map units, so the raw value must not be
added directly) on every axis, and push it
to the `trigger_volumes` list. The switch must **not** also produce a `MapEntityRecord`
(the brush branch `continue`s, like `trigger_volume`). This new branch must sit inside
the `has_brushes` block and `continue` before that block's terminal unconditional
`continue` (~parse.rs:675) — otherwise the switch falls through and is silently
dropped. All downstream consumers
(`TriggerVolumeBridge`, `trigger_system`, `TriggerVolumeComponent`) are unchanged.
Tests: (a) a `switch` brush contributes to world geometry (non-empty world brush set /
draw geometry) **and** emits exactly one `MapTriggerVolume` with `activation == 1`
and an AABB inflated past the raw brush hull (assert on `map.trigger_volumes`; call
`encode_trigger_volumes_section` first if the wire `TriggerVolumeRecord` is wanted); (b) the inert-warning parity; (c) the
switch classname does not appear as a `MapEntityRecord`; (d) a fully-populated `switch`
(fire_mode=multiple, rearm_ms, target_tag, command/command_arg, on_fire/on_exit,
enabled_on_spawn) round-trips every shared field onto the emitted `MapTriggerVolume` —
the forwarding AC-4 asks to confirm. Also add a **dev-map fixture**:
place a `switch` in a dev map wired via `on_fire` to a visible reaction (mover start or
a store-slot write), for the AC-3 manual in-engine gate (press `use`, confirm it fires).
`parse.rs` is large (~2000
lines) but this edit is a **localized new branch** mirroring the adjacent
`trigger_volume` branch — a cohesive addition, not a tangle across functions — so no
split-before-extend is warranted (dev guide: "soft smell, not a gate"). Note: because
the desugar reuses `resolve_trigger_volume`, its inert warning and error diagnostics
hardcode and name `trigger_volume`, not `switch`. Accepted for v1 (consistent with
wholesale reuse); parameterizing the classname in those messages is an optional
follow-up.

## Sequencing

**Phase 1 (concurrent):** Task 1 (FGD), Task 2 (compiler + tests). Disjoint files
(`sdk/` vs `crates/level-compiler/`); Task 2's tests reference only the `"switch"`
classname string, not the FGD file.

## Rough sketch

- **Geometry fold precedent:** `parse.rs:626` already does
  `world_brush_ids.extend(brush_ids)` for editor-group classnames — the switch reuses
  exactly this to make its faces static world geometry.
- **Trigger reuse:** `resolve_trigger_volume` (`crates/level-compiler/src/trigger_volumes.rs:13`)
  reads `activation` from props (`"use"` → `1`) and computes the brush-hull AABB into
  `MapTriggerVolume { aabb_min, aabb_max, activation, .. }` (`map_data.rs:177`); the
  record type `TriggerVolumeRecord` (`crates/level-format/src/trigger_volumes.rs:14`)
  carries `aabb_min`/`aabb_max: [f32; 3]`. That AABB is in engine meters — each vertex
  is `quake_to_engine(v) * scale` — so `use_reach` (authored in map units) must be
  scaled by `scale` before it is added; inflate those by `use_reach * scale`.
- **`use_reach` default:** use-activation is capsule-vs-AABB overlap (an intersection
  test, `trigger_system.rs` `capsule_overlaps_aabb` + `use_pressed`, not containment),
  so the trigger must extend past the solid switch face into the space the player
  stands in. The player capsule radius is an authored descriptor field
  (`CapsuleParams.radius`, ~0.4 m in fixtures), not an engine constant the compiler can
  read — the overlap test already effectively grants ~radius of reach. Pin the default
  at 24 map units (~0.61 m), which the compiler hardcodes as a literal (it cannot read
  the runtime descriptor) and which exceeds the ~0.4 m capsule radius. Uniform
  inflation on all axes is fine — for a wall-flush switch the rear/side margins fall
  inside solid wall and are unreachable; the front margin is the reachable one.
- **No depress:** static geometry means the switch does not move on press. That is the
  v1 boundary (Out of scope).

## Boundary inventory

| Name | FGD KVP | Rust (compiler) | Wire / PRL | Runtime component |
|---|---|---|---|---|
| `switch` | classname `switch` (`@SolidClass`) | new `parse.rs` brush branch → `MapTriggerVolume` (activation forced `use`) + `world_brush_ids` | existing `TriggerVolumeRecord` (no new section) | existing `TriggerVolumeComponent` (no new type) |
| `use_reach` | `use_reach(float)` | inflates `aabb_min`/`aabb_max` | folded into the record's AABB | n/a |

## Decisions

- **Brush `@SolidClass`, not a point/model entity.** A switch is wall/console brushwork
  in a Doom/Quake-lineage editor, and the research (§4.2) frames a button as brush
  geometry (a small mover "gives a free depress animation"). The brush variant desugars
  entirely into two shipped mechanisms (world-geometry fold + `use` trigger) with **no
  runtime code** — the lowest-risk path. The point-entity variant (`@PointClass`,
  glTF model + use-radius) was rejected: it needs a net-new `ClassnameDispatch` builtin
  (breaking the count-locked table at `builtins/mod.rs`) **and** net-new point-origin
  AABB ingestion in `TriggerVolumeBridge` (whose stored trigger geometry is AABB-only —
  no rotation/OBB — it ingests the full trigger record otherwise), for a
  workflow (place a model prop) less natural to brush mappers than carving a switch.
- **`activation` forced to `use`, not authorable.** A switch that could be a touch
  trigger is just a `trigger_volume`; forcing `use` is what makes `switch` a distinct,
  meaningful sugar.
- **Inflation is mandatory to the mechanism; `use_reach` is exposed as a tunable, not
  required.** Unlike an invisible `trigger_volume` the player can stand *inside*, a
  switch is solid — the trigger must be inflated to be reachable, so the compiler
  always applies `use_reach`. It ships with a default (Task 1), so it is not an
  author-required KVP; exposing it as a tunable lets mappers widen reach for large
  consoles or recessed switches.

## Open questions

- **Depress animation as a follow-up.** If authoring friction shows mappers routinely
  want a depress, a v2 could let `switch` optionally own a short mover throw (reusing
  the `kinematic_mover` geometry + command path) instead of static geometry — sequenced
  only if the need shows.

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
  **grown by a `use_reach` margin on each face whose adjacent space is open**, so a
  player standing in front of the now-solid switch overlaps the trigger while one behind
  the wall it is mounted on does not (use-activation is capsule-vs-AABB overlap —
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
  KVP. An authored `activation` is warned about and discarded rather than dropped
  silently — the realistic source is a `trigger_volume` converted by editing its
  classname, and TrenchBroom cannot surface the leftover key on a class that does not
  declare it.
- **A facing requirement on press.** Activation is overlap plus `use_pressed`, with no
  yaw, dot-product, or raycast test — a switch fires while the player faces away, or from
  above or below, anywhere the volume reaches. Adding a facing check is runtime work in
  `trigger_system`, which this feature does not touch.

## Acceptance criteria

- [ ] A `switch` brush entity compiles: its faces appear in the **static world
  geometry** (drawn + collidable, indistinguishable from worldspawn brushwork), and a
  single `use`-activation `TriggerVolumeRecord` is emitted covering it. Verified by a
  compiler unit test asserting both the world-geometry contribution and a
  `MapTriggerVolume` with `activation == 1` (use); a `TriggerVolumeRecord` only exists
  after `encode_trigger_volumes_section` runs, so a test wanting the wire record must
  also call the encoder.
- [ ] The emitted trigger's AABB is **larger than the raw switch brush AABB by the
  `use_reach` margin on each face whose adjacent space is open, and unchanged on faces
  that abut solid world geometry** — so a flush-standing player capsule overlaps it and a
  player on the far side of the wall behind it does not. Verified by two compiler tests
  comparing `aabb_min`/`aabb_max` against the brush hull per axis: one switch standing
  clear of other brushwork, where all six faces grow the full margin (guards against
  over-refusing); one wall-flush switch, where the walled face keeps the raw hull and
  stays short of the wall's far side while the other five grow.
- [ ] `use_reach` outside `(0, MAX_SWITCH_USE_REACH]` is a **compile error, not a clamp**
  — zero and negative alike, and above the bound. Verified by a compiler test over both
  ends plus the bound itself as a legal value.
- [ ] In-engine, pressing `use` while standing in front of a placed `switch` fires its
  `on_fire` reaction (and/or commands its `target_tag` mover) identically to an
  equivalent `use` `trigger_volume`. Verified on a dev-map fixture (manual gate — the
  switch reuses the shipped `trigger_system` evaluation unchanged). The fixture mounts the
  switch flush on a wall, so the same pass confirms `use` from the room on the far side of
  that wall does **not** fire it.
- [ ] `fire_mode` (once vs multiple), `rearm_ms`, and `enabled_on_spawn` behave
  identically to `trigger_volume` — they map to the same `TriggerVolumeComponent`
  fields, so no re-verification of the mechanism is needed beyond confirming the
  compiler forwards them.
- [ ] A `switch` with none of `on_fire` / `on_exit` / `target_tag` **compiles
  successfully** (inert, not an error), through the same `resolve_trigger_volume` inert
  path `trigger_volume` uses. The `log::warn!` text is not unit-capturable (the
  compiler's `CollectingLogger` is a process-global backend); warning parity is a
  review/grep gate on the shared `resolve_trigger_volume` call, not a log-capture test.
- [ ] A `switch` classname does **not** fall through to a generic `MapEntityRecord`
  (it is handled by the brush-entity branch, like `trigger_volume`).
- [ ] The change touches only the FGD, the level compiler, and tests — **no** new PRL
  section, runtime system, render path, `ClassnameDispatch` builtin, or
  `TriggerVolumeBridge` code (review/grep gate).

## Tasks

### Task 1: `switch` FGD class
Add a `switch` `@SolidClass` to `sdk/TrenchBroom/postretro.fgd`, modelled on the
`trigger_volume` class but with the switch semantics: **no `activation` KVP** (the
compiler forces `use`), plus a `use_reach(float)` property (the AABB inflation margin,
authored in map units; **default `24` map units ≈ 0.61 m** — this is the single source of
truth for the default, and Task 2's compiler fallback literal must match it. It is a
hardcoded literal, not tied to the runtime capsule constant: the `level-compiler` crate
cannot reach that constant). Carry the shared trigger
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
overwrite `MapTriggerVolume.activation`; warn when an `activation` was authored), then
**grow `aabb_min`/`aabb_max` by `use_reach * scale` on each face whose adjacent space is
open** (parsed from props, default `24.0` map units when `use_reach` is absent — must
equal Task 1's FGD default; rejected outside `(0, MAX_SWITCH_USE_REACH]`; `scale =
format.units_to_meters()`, the same scale the AABB vertices already carry — the AABB is
in engine meters but `use_reach` is authored in map units, so the raw value must not be
added directly), and push it
to the `trigger_volumes` list. The face-by-face test reads the finished static world
brush set, which the entity loop is still collecting, so the branch resolves the trigger
un-grown and **defers the margin to a pass after `build_brush_volumes`** — the
`pending_kinematic_movers` deferral precedent. The switch must **not** also produce a `MapEntityRecord`
(the brush branch `continue`s, like `trigger_volume`). This new branch must sit inside
the `has_brushes` block and `continue` before that block's terminal unconditional
`continue` (~parse.rs:675) — otherwise the switch falls through and is silently
dropped. All downstream consumers
(`TriggerVolumeBridge`, `trigger_system`, `TriggerVolumeComponent`) are unchanged.
Tests: (a) a `switch` brush contributes to world geometry (non-empty world brush set /
draw geometry) **and** emits exactly one `MapTriggerVolume` with `activation == 1`
and an AABB grown past the raw brush hull (assert on `map.trigger_volumes`; call
`encode_trigger_volumes_section` first if the wire `TriggerVolumeRecord` is wanted; get
the reference hull from the fixture's known brush extents `* scale`, or by compiling the
same brush as a `trigger_volume`, then compare per-axis) — paired with a wall-flush
fixture asserting the walled face keeps the raw hull and stops short of the wall's far
side; (a2) `use_reach` at zero, negative, non-numeric, and above the bound all fail to
compile, and the bound itself compiles; (b) inert-compile parity — a
`switch` with none of `on_fire`/`on_exit`/`target_tag` **compiles successfully** (inert,
not an error); the `log::warn!` text itself is not unit-assertable (the compiler's
`CollectingLogger` is a process-global backend that parallel tests cannot capture), so
warning parity rides on the reuse of `resolve_trigger_volume` and is a review/grep gate;
(c) the switch classname does not appear as a `MapEntityRecord`; (d) a fully-populated `switch`
(fire_mode=multiple, rearm_ms, target_tag, command/command_arg, on_fire/on_exit,
enabled_on_spawn) round-trips every shared field onto the emitted `MapTriggerVolume` —
the forwarding AC-4 asks to confirm. Also add a **dev-map fixture**:
place a `switch` in a dev map wired via `on_fire` to a visible reaction (mover start or
a store-slot write), for the AC-3 manual in-engine gate (press `use`, confirm it fires).
`parse.rs` is large (~2000
lines) but this edit is a **localized new branch** mirroring the adjacent
`trigger_volume` branch — a cohesive addition, not a tangle across functions — so no
split-before-extend is warranted (dev guide: "soft smell, not a gate"). Note:
`resolve_trigger_volume` takes the authored classname and names it in every diagnostic,
so a switch's warnings and errors say `switch` — an error attributed to `trigger_volume`
would send the author hunting for an entity their map may not contain.

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
  read — the overlap test already effectively grants ~radius of reach *on top of* the
  margin, which is why a margin on a face that abuts a wall reaches through it (see
  Decisions). Pin the default at 24 map units (~0.61 m), which the compiler hardcodes as
  a literal (it cannot read the runtime descriptor) and which exceeds the ~0.4 m capsule
  radius. Apply it per face, only where the space immediately outside that face is open.
- **No depress:** static geometry means the switch does not move on press. That is the
  v1 boundary (Out of scope).

## Boundary inventory

| Name | FGD KVP | Rust (compiler) | Wire / PRL | Runtime component |
|---|---|---|---|---|
| `switch` | classname `switch` (`@SolidClass`) | new `parse.rs` brush branch → `MapTriggerVolume` (activation forced `use`) + `world_brush_ids` | existing `TriggerVolumeRecord` (no new section) | existing `TriggerVolumeComponent` (no new type) |
| `use_reach` | `use_reach(float)` | grows `aabb_min`/`aabb_max` on open faces | folded into the record's AABB | n/a |

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
- **Growth is mandatory to the mechanism; `use_reach` is exposed as a tunable, not
  required.** Unlike an invisible `trigger_volume` the player can stand *inside*, a
  switch is solid — the trigger must reach past the switch face to be reachable, so the
  compiler always applies `use_reach`. It ships with a default (Task 1), so it is not an
  author-required KVP; exposing it as a tunable lets mappers widen reach for large
  consoles or recessed switches.
- **Revised during implementation: grow per face, not uniformly on all six.** This spec
  originally specified uniform inflation on every axis and defended it with "for a
  wall-flush switch the rear/side margins fall inside solid wall and are unreachable; the
  front margin is the reachable one." **That reasoning was wrong on a load-bearing point:
  it omitted the player capsule radius.** `capsule_overlaps_aabb` measures axis-to-AABB
  distance against `radius²`, so effective reach past a face is `use_reach * scale +
  radius` — about 31 map units at defaults, against first-party walls that are 16 units
  thick. A margin does not have to *contain* the player to be reachable; it only has to
  come within a capsule radius of them. The `switch-demo` console's rear margin landed 8
  units past its wall's far face, so a player in the next room, facing away, could fire
  the switch through the wall. Shipping that was rejected and revising this spec was
  authorized instead. **What ships:** a face grows only where the space immediately
  outside it is not solid world geometry. A wall-flush switch emits no rear margin, which
  makes press-through-wall structurally impossible rather than contingent on wall
  thickness — the property the original design could not offer at any margin. Cost: the
  solidity test needs the finished static world brush set, so the margin moves out of the
  entity loop into a pass after `build_brush_volumes`. **Accepted residual:** only the
  space immediately against each face is probed, so a switch floated off a wall with a gap
  wider than the probe still grows across that gap. Bounded by authoring guidance (mount
  console brushwork flush), not by the compiler.
- **`use_reach` is range-checked, not clamped.** Zero is rejected alongside negatives:
  the margin *is* the reach mechanism, so a zero-margin switch would compile clean and be
  unpressable. An upper bound (`MAX_SWITCH_USE_REACH`) rejects the authoring typo it
  exists for — a stray digit, a unit mix-up — before it becomes a press volume that
  swallows the room and fires on every `use` press in it. Both ends error rather than
  clamp: a silently-corrected value hides the mistake that produced it.

## Open questions

- **Depress animation as a follow-up.** If authoring friction shows mappers routinely
  want a depress, a v2 could let `switch` optionally own a short mover throw (reusing
  the `kinematic_mover` geometry + command path) instead of static geometry — sequenced
  only if the need shows.

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
  **grown per face by a `use_reach` margin, clamped to the free space in front of that
  face**, so a player standing in front of the now-solid switch overlaps the trigger while
  one behind the wall it is mounted on does not (use-activation is capsule-vs-AABB overlap —
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
- [ ] The emitted trigger's AABB grows past the raw switch brush AABB **per face, by
  `min(use_reach, free distance in front of that face)`** — holding the invariant that
  **no grown face extends past the near side of any occluder whose cross-section that
  face overlaps**. A flush-mounted face has zero free distance and does not grow at all;
  a face 4 units off its mount grows 4; a face fronting open room grows the full margin.
  Occluders are the static world brush hulls plus the `kinematic_mover` hulls at their
  authored position, minus the switch's own brushes. Verified by three compiler tests
  comparing `aabb_min`/`aabb_max` against the brush hull per axis: a switch standing
  clear of other brushwork, where all six faces grow the full margin (guards against
  over-refusing); a wall-flush switch, where the walled face keeps the raw hull; a
  corner-mounted switch with a 4-unit gap to the second wall, where that face grows
  exactly 4 while the four open faces still grow the full margin.
- [ ] `use_reach` outside `(0, 128]` map units (`MAX_SWITCH_USE_REACH`) is a **compile
  error, not a clamp** — zero and negative alike, non-numeric, non-finite, and above the
  bound. An **empty** value is the one exception: a field the author cleared in
  TrenchBroom arrives as `""` and falls back to the default rather than failing the
  compile (the `_lightmap_density` posture). Verified by compiler tests over both ends,
  the bound itself as a legal value, and the empty-value fallback.
- [ ] In-engine, pressing `use` while standing in front of a placed `switch` fires its
  `on_fire` reaction (and/or commands its `target_tag` mover) identically to an
  equivalent `use` `trigger_volume`. Verified on a dev-map fixture (manual gate — the
  switch reuses the shipped `trigger_system` evaluation unchanged). The manual pass covers
  only what a human alone can judge: that the press registers, that the indicator light is
  visible while looking at the console, and that the door actually moves. The far-side
  property — that the volume never reaches through the wall the switch is mounted on — is
  discharged by `switch_flush_against_a_wall_grows_only_into_open_space` and
  `switch_reach_clamps_each_face_to_the_free_space_in_front_of_it`, which check the
  geometry in both directions (no over-reach, no over-refusal) more precisely than one
  button press can. Adding a far-side closet to `switch-demo.map` was considered and
  **rejected**: the fixture is a single sealed room with no space behind that wall, the
  unit tests are the stronger guard, and a manual criterion nobody can execute is false
  assurance rather than coverage.
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
**grow `aabb_min`/`aabb_max` per face by `min(use_reach * scale, free distance in front of
that face)`** (parsed from props, default `24.0` map units when `use_reach` is absent — must
equal Task 1's FGD default; rejected outside `(0, MAX_SWITCH_USE_REACH]`; `scale =
format.units_to_meters()`, the same scale the AABB vertices already carry — the AABB is
in engine meters but `use_reach` is authored in map units, so the raw value must not be
added directly), and push it
to the `trigger_volumes` list. The clamp reads the finished static world brush set (plus
the `kinematic_mover` hulls), which the entity loop is still collecting, so the branch
resolves the trigger un-grown and **defers the margins to a pass after
`build_brush_volumes`** — the `pending_kinematic_movers` deferral precedent. The switch must **not** also produce a `MapEntityRecord`
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
  radius. Apply it per face, clamped to the free space in front of that face.
- **No depress:** static geometry means the switch does not move on press. That is the
  v1 boundary (Out of scope).

## Boundary inventory

| Name | FGD KVP | Rust (compiler) | Wire / PRL | Runtime component |
|---|---|---|---|---|
| `switch` | classname `switch` (`@SolidClass`) | new `parse.rs` brush branch → `MapTriggerVolume` (activation forced `use`) + `world_brush_ids` | existing `TriggerVolumeRecord` (no new section) | existing `TriggerVolumeComponent` (no new type) |
| `use_reach` | `use_reach(float)` | grows `aabb_min`/`aabb_max` per face, clamped to free space | folded into the record's AABB | n/a |

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
- **Revised twice during implementation: grow per face, clamped to the free space in
  front of it.** This spec originally specified uniform inflation on every axis and
  defended it with "for a wall-flush switch the rear/side margins fall inside solid wall
  and are unreachable; the front margin is the reachable one." **That reasoning was wrong
  on a load-bearing point: it omitted the player capsule radius.** `capsule_overlaps_aabb`
  measures axis-to-AABB distance against `radius²`, so effective reach past a face is
  `use_reach * scale + radius` — `24 * 0.0254 + 0.4` m = `1.0096` m, or **≈ 40 map units
  at defaults**, against first-party walls that are 16 units thick. (An earlier draft of
  this bullet said ~31 units; that figure carried a `− SKIN_DISTANCE` term that was
  correctly dropped without recomputing the number. The corrected figure makes the
  original defect **worse** than this plan recorded, not better: the rear *margin* alone
  landed 8 units past the `switch-demo` wall's far face, but effective reach put the
  pressable point ~24 units into the next room.) A margin does not have to *contain* the
  player to be reachable; it only has to come within a capsule radius of them. So a player
  in the next room, facing away, could fire the switch through the wall. Shipping that was
  rejected and revising this spec was authorized instead.
- **The first replacement was also wrong: a per-face probe was a sampling error.** The fix
  that superseded uniform inflation tested each face's adjacent space with a single probe
  point one map unit out, then grew that face by the full 24. This plan recorded that as
  making press-through-wall "structurally impossible rather than contingent on wall
  thickness." **That claim was an overclaim.** Any solid between 1 unit and 24 units out
  was sampled as open and then grown through: leak condition `clearance + thickness <
  use_reach`, which against 16-unit walls at default reach means any face 1–8 units off a
  wall. It had moved the threshold, not removed it. **The durable insight is distinct from
  the first lesson.** The first was a missing term; this one is a mismatch of distances: a
  test that samples at distance A must not license an action that reaches to distance B.
  The probe and the growth have to be the same question. A boolean "is it open?" cannot
  authorize a quantity.
- **What ships: growth clamped per face to the nearest occluder standing past it.** Each
  face grows by `min(margin, free distance)`, holding the invariant *no grown face extends
  past the near side of any occluder whose cross-section that face overlaps*. Flush
  mounting falls out as a zero gap rather than being a special case, and the floated-switch
  gap is no longer a residual — growth stops at the mount whatever the gap. `kinematic_mover`
  hulls are in the occluder set at their **authored** position, so a switch on a door or
  lift clamps against its mount; reach stays conservative for a mover that later moves
  away, which is the right trade against pressing through a closed door. Cross-section
  overlap must be positive-area within a 1 mm tolerance: trigger AABBs are `f32` on the
  wire while brush hulls are f64, and a strict comparison reported phantom overlaps at
  flush contact planes (a flush mount touches four of the console's faces edge-on, and
  counting that as overlap zeroed all four margins). Cost: the clamp needs the finished
  static world brush set, so the margins move out of the entity loop into a pass after
  `build_brush_volumes`. **Deliberately stated with what it does not cover** — the third
  claim in this sequence should not be the third overclaim. The runtime still measures from
  the capsule axis, so effective reach is the clamped distance *plus* the capsule radius
  (~15.75 map units); an occluder thinner than that radius remains pressable through.
  Clamping removes the margin from the leak, not the radius. The compiler docs and the FGD
  say so alongside the invariant.
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

# E22 — Group Recognition & Assembly Provenance — research

> Source-grounded current-state map for `index.md`. Findings, not decisions. Every
> identifier read from current source this session; line numbers drift on the next
> edit — cite by symbol. Epic-level grounding lives in
> `context/plans/drafts/E22--kinematic-assemblies/research.md`; this file adds only
> what spec 2 touches.

## 1. Recognition sites today (everything is discarded)

- **Adapter predicate.** `crates/level-compiler/src/format/quake_map.rs` —
  `is_editor_group_classname(classname)` returns `classname == "func_group"` and
  reads no KVPs. Doc comment: editor groups are "authoring containers, not runtime
  entities."
- **Flatten call site.** `crates/level-compiler/src/parse.rs`, in the entity-walk
  loop `for entity_id in geo_map.entities.iter()`: a `func_group`'s brushes are
  `world_brush_ids.extend(brush_ids); continue;`. `world_brush_ids` is seeded from
  worldspawn brushes before the loop, then consumed once into `MapData.brush_volumes`
  via `build_brush_volumes_with_ids(&geo_map, &world_brush_ids, scale)`, which returns
  `Vec<(BrushId, BrushVolume)>` — **the `BrushId` is currently dropped**
  (`.map(|(_, volume)| volume)`). The `switch` branch reuses the same
  `world_brush_ids.extend(...)` alongside pushing a trigger volume.
- **`_tb_*` strip.** `is_runtime_map_entity_key(key)` (in `parse.rs`) =
  `!RESERVED_MAP_ENTITY_KEYS.contains(&key) && !key.starts_with("_tb_")`. Applied only
  when building a point entity's generic `key_values` bag (the `props.filter(...)` at
  the `MapEntityRecord` tail). `RESERVED_MAP_ENTITY_KEYS` =
  `["classname", "origin", "_tags", "angle", "angles", "mangle"]`.
- **Nothing reads `_tb_group` / `_tb_id` / `_tb_name` / `_tb_linked_group_id` /
  `_tb_transformation` / `_tb_layer`.** Crate-wide grep hits only fixtures and the
  inline test-map builders. Group identity, names, and per-group transforms are all
  discarded.
- **No FGD change needed.** `func_group` is a TrenchBroom built-in classname emitted
  on save; `_tb_*` are TrenchBroom built-ins. `sdk/TrenchBroom/postretro.fgd` does not
  declare `func_group`. Recognition reads what TrenchBroom already writes.

The `GeoMap` (shambler) is not a unified entity struct — classname/origin/props via
`get_property`/`collect_entity_properties`, brushes via
`geo_map.entity_brushes.get(entity_id)`. The compiler's own per-entity types are
`EntityInfo { classname, origin }` and `MapEntityRecord`.

## 2. Two nesting forms, confirmed in fixtures

`content/dev/maps/{campaign-test,kinematic-platform,movement-feel}.map`.

1. **Brace-owned brushes.** One `func_group` entity whose `{ }` directly contains the
   brushes. Only `dome_ceiling` (`movement-feel.map`, `_tb_id "8"`) exercises this —
   the only `func_group` that owns brushwork anywhere in the repo.
2. **Sibling entities.** The `func_group` is an empty marker (`_tb_id`, `_tb_name`,
   `_tb_linked_group_id`, no brushes); member point/light entities are top-level
   siblings each carrying `_tb_group "<id>"`. All light groups use this form
   (`Phase Lights`, `pink_ambient_lights`, `Spot lights`, …).

Membership association therefore needs both: brace-form via `entity_brushes` on the
marker; sibling-form via matching a sibling's `_tb_group` to a marker's `_tb_id`.
Because a sibling may be walked before its marker, recognition wants a **marker
pre-pass** (collect `_tb_id → {name, linked_group_id, brushes}`) before the main walk
resolves membership.

## 3. Linked-group GUIDs — no instance to recognize

- **No shared `_tb_linked_group_id` exists** across any fixture — every GUID is
  distinct. There is **no true linked-instance pair** to dedupe. (Confirms deferring
  instance-of semantics to spec 3, per owner decision this session.)
- **Duplicate group *names*, distinct GUIDs.** `movement-feel.map` has
  `pink_ambient_lights` twice (`_tb_id 1` / `_tb_id 10`, distinct GUIDs) and
  `cyan_ambient_lights` twice. Name is **not** a unique key; `_tb_id` (per file) is.
  These same-named pairs are unlinked copies, **not** instances.
- **GUID spelling varies.** Some `_tb_linked_group_id` values are brace-wrapped
  (`"{e18d05b4-…}"`), others bare (`"3301ebda-…"`). A captured GUID must normalize
  braces so a later spec-3 comparison is spelling-insensitive.
- **`_tb_layer` absent from the entire repo.** No fixture exercises it.
- `_tb_transformation` (16-value row-major 4×4) appears on some groups; unused here
  (movers own their own motion; static groups flatten in place).

## 4. Carrier resolution to reuse (spec 1, unchanged)

`resolve_carried_light_links(lights: &mut [MapLight], movers: &[MapKinematicMover]) ->
Vec<CarriedLightLink>` (`parse.rs`). Per light with a non-empty `carrier`:
- dynamic bake-only → warn, clear carrier, skip; baked (non-dynamic) → warn, clear,
  skip.
- match movers by `mover.name == light.carrier`: `[]` → warn unbound; `[one]` → bind;
  `[many]` → warn duplicate, unbound.
- spot + `mover.spin_axis != [0;3]` → warn (position-only).
- `local_offset = (light.origin - mover.origin) as f32`, finite-check, else warn
  unbound.
- push `CarriedLightLink { source_light_index, mover_id, local_offset }`.

`MapLight.carrier: String` already exists (spec 1). **Consequence for spec 2:** a group
pass that sets `light.carrier = mover.name` for eligible grouped lights *before* this
runs makes grouped carry flow through the identical resolution — offset derivation,
baked/spinner/duplicate degradation, and `CarriedLightLink` construction stay in one
place. The group pass constructs no links itself.

**Eligibility for synthesis** (mirrors resolve's own gates, applied at the source so a
synthesized carrier is never spurious): `light.is_dynamic && !light.bake_only &&
light.carrier.is_empty()`. Duplicate mover *names* remain a warn-unbound case in
resolve (spec 1's posture; spec 1 explicitly did not add a unique-`name` check), so the
group path inherits it rather than working around it.

## 5. Provenance consumer — the live diagnostic surface

Diagnostics are unstructured `log::warn!` text with a hand-written `"[Compiler]"`
prefix — **no `CompileDiagnostics` type, no structured collector.** Provenance is
threaded as formatted context into a call site.

- **Watertightness (live, brush-resolved).** `pipeline.rs` (~`check_watertight`):
  `partition::check_watertight(&result.faces) -> WatertightReport { open_edge_count,
  samples }`; each sample carries `midpoint: DVec3` and `brush_index: usize`. The
  per-edge line already prints `near brush {brush_index}`. `brush_index` is the
  enumerate index into the BSP input brush slice (`partition/face_extract.rs`:
  `for (brush_index, brush) in brushes.iter().enumerate()`), which is
  `MapData.brush_volumes` in order. So a `Vec<Option<AssemblyId>>` aligned to
  `brush_volumes` indexes directly by `edge.brush_index`. **This is the thin slice's
  consumer.**
- **Switch use-reach clamp (live, but occluder not brush-resolved).**
  `parse.rs::apply_switch_use_reach` names the switch `name` and the clamp *plane*, not
  the occluder brush/entity — naming the occluder's group needs deeper plumbing.
  Out of scope; the same `brush_assembly` map is available to a later pass.
- **geometry-integrity-validation is an unbuilt draft** (`context/plans/drafts/`), not a
  live consumer. A future beneficiary of the same map.
- Portal-count-0 warning carries no positional context.

**Alignment plumbing.** `build_brush_volumes_with_ids` already returns
`(BrushId, BrushVolume)`; retaining the `BrushId` (instead of `.map(|(_, volume)| volume)`) plus a
`BrushId → AssemblyId` map built during the walk yields `brush_assembly[i]` aligned to
`brush_volumes[i]` even if a degenerate brush is dropped (keyed by id, not position).

## 6. Types touched (current shape)

- `MapData` (`map_data.rs`) fields include `brush_volumes: Vec<BrushVolume>` (doc: "…plus
  flattened editor groups"), `lights: Vec<MapLight>`, `carried_light_links:
  Vec<CarriedLightLink>`, `kinematic_movers: Vec<MapKinematicMover>`, `map_entities:
  Vec<MapEntityRecord>`. No `assemblies`, `provenance`, or `group` field exists.
- `MapKinematicMover { mover_id: u32, name: String, origin: DVec3, … }`.
- `MapLight { light_type, carrier: String, origin: DVec3, is_dynamic, bake_only, … }`.
- `MapEntityRecord { classname, origin, angles, key_values: Vec<(String,String)>, tags }`
  — `_tb_*` already stripped from `key_values`.
- No `MapAssembly`/`Assembly` type in the crate (`Assembly` hits in `sh_group.rs` are
  unrelated SH-bake prose).

## 7. Regression tests that lock current behavior (must survive)

`parse.rs`:
- `trenchbroom_func_group_brushes_are_flattened_into_static_world` — brace-form group's
  brushes land in `brush_volumes` (count includes them), no `func_group` in
  `map_entities`. Spec 2 keeps geometry byte-identical; may extend to also assert
  provenance recorded.
- `empty_trenchbroom_func_group_is_not_a_runtime_entity` — empty group adds no static
  brushes, no runtime entity. Spec 2 keeps this: an empty group has no members, emits no
  member-bearing assembly and no runtime entity.
- `map_entity_key_values_strip_trenchbroom_metadata` — `_tb_*` never reach a point
  entity's `key_values`. Spec 2 reads `_tb_*` from the *raw props* at the adapter into a
  separate canonical provenance field and **does not loosen `is_runtime_map_entity_key`**,
  so this passes unchanged.

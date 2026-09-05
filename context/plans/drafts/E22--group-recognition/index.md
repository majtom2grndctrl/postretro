# E22 — Group Recognition & Assembly Provenance

> **Epic 22, spec 2.** Foundation spec 1 (`done/E22--assembly-carried-members`) shipped
> the carried-light *linkage* via an explicit `carrier` KVP. This spec introduces the
> canonical **assembly** and recognizes TrenchBroom `func_group` grouping onto it —
> provenance for diagnostics, and a "just group them" carried-light path over spec 1's
> explicit binding. Shared-template *instance-of* semantics are deferred to spec 3
> (`E22--kinematic-instancing`); no shared-GUID fixture or consumer exists yet.
> **Related:** epic hub `context/plans/drafts/E22--kinematic-assemblies/` ·
> `build_pipeline.md` §Source-format neutrality, §Compiler pipeline · `research.md`
> (sibling).

## Goal

Stop discarding TrenchBroom group identity at the Quake adapter. Recognize both
`func_group` nesting forms and translate them into a canonical, engine-shaped
`MapAssembly` in the compiler's canonical layer, so (a) compiler diagnostics can name
the group a problem brush belongs to, and (b) a mapper can group a dynamic light with a
kinematic mover and have the light ride the mover — the same runtime linkage spec 1's
explicit `carrier` produces, without typing a per-light KVP. Static groups keep
flattening into worldspawn with geometry byte-identical to today.

## Scope

### In scope
- Adapter recognition (`format/quake_map.rs` + the `parse.rs` entity walk) of both
  `func_group` forms: **brace-owned brushes** and **sibling entities** back-referencing
  a marker via `_tb_group`↔`_tb_id`.
- A canonical `MapAssembly` in `map_data.rs`: engine-shaped identity + provenance name +
  the captured (normalized) linked-group GUID. Membership recorded as a
  `brush_assembly` index aligned to `brush_volumes` (brush members) and a transient
  group→member map consumed for carrier synthesis.
- **Provenance in diagnostics:** the live watertightness warning names the owning group
  of an open-edge brush.
- **Ergonomic carried-light:** a group containing exactly one named `kinematic_mover`
  synthesizes `light.carrier = mover.name` for eligible member dynamic lights, reusing
  spec 1's `resolve_carried_light_links` unchanged.
- Explicit compile warnings — each naming the group — for every ambiguous or
  contradictory grouped-carry case.

### Out of scope
- **Shared-template instance-of semantics / template dedup / instanced draw.** No
  shared-GUID fixture or consumer exists (`research.md §3`). The GUID is *captured and
  normalized* for spec 3 to consume; no instance-of relation is derived from it here.
  (Deferred to `E22--kinematic-instancing`.)
- **`_tb_layer` handling.** Absent from the entire repo (`research.md §3`); no fixture to
  exercise. Explicitly deferred — recognition ignores it, as today.
- **Naming the group in the switch use-reach clamp or geometry-integrity diagnostics.**
  The switch clamp identifies occluders by plane, not brush; geometry-integrity is an
  unbuilt draft (`research.md §5`). The `brush_assembly` map is available to a later
  pass; wiring those sites is not this spec.
- **New PRL section or FGD KVP.** Assemblies are compile-time only; brush provenance is a
  diagnostic aid, not emitted to PRL; grouped carry reuses spec 1's `CarriedLightLink`
  already carried in `KinematicGeometry` v5. No wire or FGD surface changes.
- **`_tb_transformation` application.** Static groups flatten in place; movers own their
  own motion. The matrix is unused here.
- **General entity parenting / scene graph.** Carried members follow one mover's
  transform, exactly as spec 1.

## Direction

**Problem.** The compiler has no canonical unit for "these authored things belong to one
group," so group identity is destroyed at the adapter — `func_group` brushes are
`extend`ed into a flat brush list and `_tb_*` is prefix-stripped — before any stage can
use it. The observable cause, not the symptom: a watertightness warning names only
`near brush 37`, never the group the brush belongs to; and a light grouped with a mover
in the editor compiles to a disconnected top-level light — spec 1's explicit `carrier`
is the only binding that works. Both payoffs are **anticipated, not yet observed
in-repo**: exactly one `func_group` owns brushwork today (`research.md §2`) and no fixture
groups a light with a mover, so this spec authors both proving fixtures (Task 3). Spec 1
shipped the carried capability and named this recognition its successor
(`done/E22--assembly-carried-members` Scope: "`MapAssembly` lands with spec 2").

**Prior commitments.**
- *Source-format neutrality* (`build_pipeline.md`: "`func_group`, `_tb_*` → Flattened
  into the world or dropped, before shared logic"). This spec **refines, not reverses**
  it. Recognition of `_tb_*` stays in the `format/`/adapter boundary; the canonical
  `MapAssembly` is defined by what the engine needs (a provenance-bearing group, a
  carrier the light rides), not by TrenchBroom's field shapes. "Dropped" stops being the
  *only* translation — grouping is now *translated* to a canonical assembly. The
  invariant holds: no `_tb_*` vocabulary reaches a shared stage; downstream sees
  `MapAssembly` and `CarriedLightLink`, engine terms. Stated because unstated divergence
  from a written invariant is the defect.
- *The `_tb_*` strip stays.* `is_runtime_map_entity_key` is **not** loosened. Recognition
  reads group markers from the *raw* property bag at the adapter and routes them to a
  separate `MapAssembly` field; the generic `MapEntityRecord.key_values` bag stays
  `_tb_*`-free. All three regression tests survive (`research.md §7`).
- *Static-group geometry is behavior-identical.* Grouped brushes still flatten into
  `brush_volumes`; recognition only *also* records which group each came from.
- *Spec 1's carrier resolution is the one place links are built.* The group pass
  synthesizes a `carrier` *name*; it constructs no `CarriedLightLink`. Offset derivation
  and baked/spinner/duplicate-name degradation stay in `resolve_carried_light_links`
  (`research.md §4`).

**Alternatives rejected.**
- *Keep explicit-`carrier`-only; never recognize `func_group`.* Rejected: leaves
  diagnostics unable to name groups (the common-case, static-brush payoff, independent of
  kinematics) and blocks spec 3, which consumes this recognition.
- *Build links directly in the group pass from co-membership.* Rejected: duplicates spec
  1's offset/degradation logic in a second place. Synthesizing the `carrier` name and
  letting the existing pass resolve keeps the fact stated once. Cost: a grouped mover with
  a duplicate `name` inherits spec 1's warn-unbound (the group knew which mover, the name
  round-trip loses it) — accepted as consistent with spec 1's stance that duplicate mover
  names are already a broken authoring state.
- *Recognize the shared-GUID instance-of relation now (as the epic hub brief sketched).*
  Rejected this session (owner decision): no shared-GUID fixture exists and its only
  consumer (spec 3) is deferred — it would ship untestable recognition for an absent
  consumer. The GUID is captured for spec 3; instance-of is not inferred.
- *A bare `brush→group` side-table instead of a `MapAssembly` type.* Rejected: groups own
  two member kinds in-repo (brace-owned brushes; sibling lights/entities), and specs 3–4
  build on the assembly, so the canonical container earns its place now. Provenance is its
  first consumer.
- *Split on the consumer axis: ship recognition + `MapAssembly` + provenance + GUID-capture
  now, defer the grouped-carry ergonomic to a later pass.* Considered and **decided against
  by the owner this session** — bundle both. Grouped carry is the epic spine's natural first
  kinematic projection of the assembly, it shares `parse.rs` and the marker recognition with
  the provenance half, and it reuses spec 1's `resolve_carried_light_links` unchanged, so the
  marginal cost is one synthesis pass plus its warning arms — accepted for delivering the
  epic's "just group them" headline now rather than in a second spec. The split stays the
  clean fallback if the synthesis pass proves larger than expected.

**Placement.** Recognition lives at the adapter/parse boundary (source vocabulary stops
there); `MapAssembly` is a canonical-layer type (`map_data.rs`), engine-shaped; the
provenance consumer is a shared-stage diagnostic reading a canonical field, never
`_tb_*`. This is the guard against the "canonical layer shaped by the source" failure
`build_pipeline.md` names — the assembly is shaped by diagnostics and carry, not by what
TrenchBroom happens to write. The hub sketched `MapAssembly { name, kind, instance_of,
local_transform }`; this spec ships `{ provenance, group_id, linked_group_id }` and
distinguishes static-vs-carried *behaviorally* (does the group contain a mover?) rather
than with a `kind` discriminant — the hub licensed the exact fields to the foundation
spec, and the discriminant is omitted until a second kind (spec 3 instancing) needs it.
The one place a source artifact sits in the canonical layer is `linked_group_id` — a
normalized source GUID with no consumer in *this* spec, held for spec 3. It is the closest
this spec comes to the failure mode above; accepted because re-parsing the GUID in spec 3
is worse, and the field carries no source *semantics* (no instance-of is derived from it
here), only the identifier spec 3 will key on.

**Foreclosures / one-way doors.** Nothing material. `MapAssembly` is compiler-internal
(`pub(crate)`), no wire contract; provenance is additive diagnostic text; static-group
geometry is unchanged. Reversible at the cost of deleting a type and a warning arm. The
captured-GUID field shapes what spec 3 reads, but capturing-without-semantics forecloses
no instance-of design.

## Acceptance criteria

- [ ] A brace-form `func_group` owning brushes → those brushes appear in
  `brush_volumes` exactly as they do today (the existing flatten assertions still hold),
  **and** each is associated to its group. An ungrouped worldspawn brush is associated to
  no group.
- [ ] A watertightness open edge on a brush belonging to a group → the per-edge warning
  names that group (its provenance name, disambiguated when names collide). An open edge
  on an ungrouped brush → the warning names no group (current wording).
- [ ] An empty `func_group` → no static brushes, no runtime entity, and no
  member-bearing assembly.
- [ ] No `_tb_*` key appears in any `MapEntityRecord.key_values` (strip unchanged).
- [ ] Two `func_group`s sharing a `_tb_name` but with distinct `_tb_linked_group_id` →
  two distinct assemblies, each independently nameable in a diagnostic.
- [ ] Both nesting forms recognized: a brace-owned brush and a sibling entity carrying
  `_tb_group "<id>"` each associate to the marker with matching `_tb_id`.
- [ ] A group with exactly one named `kinematic_mover` and a co-grouped `light_dynamic`
  that has **no** explicit `carrier` → a `CarriedLightLink` with the same `mover_id` and
  the same `local_offset` as the link produced when that light instead carries an
  explicit `carrier` naming the mover.
- [ ] A co-grouped member light with an explicit `carrier` → the explicit binding is used
  unchanged; when it names a mover other than the group's single mover, a warning names
  the group (contradiction) and the explicit binding wins.
- [ ] A group with a `light_dynamic` member and **more than one** `kinematic_mover` → a
  warning names the group; no carrier is synthesized (the light stays unbound at its
  authored origin).
- [ ] A group with a `light_dynamic` member and exactly one `kinematic_mover` whose
  `name` is **empty** → a warning names the group; no carrier is synthesized.
- [ ] A `_tb_linked_group_id` written `"{guid}"` and one written `"guid"` normalize to
  the same captured value; no instance-of relation, template, or dedup is produced from a
  shared GUID (deferred to spec 3).

## Tasks

### Task 1: Recognition, canonical `MapAssembly`, and provenance to the watertightness diagnostic (thin slice)

Introduce `pub(crate) struct MapAssembly` in `crates/level-compiler/src/map_data.rs`
carrying a provenance display name (`_tb_name` when non-empty, else a stable form of
`_tb_id`), the adapter-local group id (`_tb_id`), and the **normalized** captured
`_tb_linked_group_id: Option<String>` (strip surrounding braces so `"{g}"` and `"g"`
compare equal — captured only, no instance-of derived; §Scope out). Add
`MapData.assemblies: Vec<MapAssembly>` and `MapData.brush_assembly: Vec<Option<usize>>`
aligned index-for-index to `MapData.brush_volumes` (value = index into `assemblies`, or
`None` for worldspawn/ungrouped brushes). In `parse.rs`, run a **marker pre-pass** over
`geo_map.entities` collecting every `func_group` marker's `_tb_id`/`_tb_name`/
`_tb_linked_group_id` (read from the raw props, not through `is_runtime_map_entity_key`)
and its brace-owned brushes (`geo_map.entity_brushes`); build one `MapAssembly` per
marker. In the existing entity walk, keep the `func_group` flatten
(`world_brush_ids.extend(brush_ids); continue;`) geometry-identical but also record each
appended `BrushId → assembly index` in a side map; for sibling entities carrying
`_tb_group "<id>"`, associate them to the marker with matching `_tb_id`. Change
`build_brush_volumes_with_ids`'s consumption to **retain the returned `BrushId`** (today
`.map(|(_, v)| v)` drops it) so `brush_assembly` is built keyed by `BrushId` — robust to a
degenerate brush being dropped from `brush_volumes` — rather than by position. Do **not**
loosen `is_runtime_map_entity_key`; `_tb_*` must stay out of `MapEntityRecord.key_values`.
An empty marker (no brushes, no siblings) records no members and emits no runtime entity.
Duplicate `_tb_name`s across markers stay distinct assemblies (keyed by `_tb_id`). Then
wire the thin-slice consumer: thread `brush_assembly` and the assemblies' provenance names
from `MapData` to the watertightness reporting site in `pipeline.rs`
(`check_watertight`), and for each `WatertightReport` sample append the owning group's
provenance name when `brush_assembly[edge.brush_index]` is `Some` (confirm `brush_index`
indexes `brush_volumes` order — it is the `enumerate` index in
`partition/face_extract.rs`). Extend the existing
`trenchbroom_func_group_brushes_are_flattened_into_static_world` and
`empty_trenchbroom_func_group_is_not_a_runtime_entity` tests to assert the new provenance
without weakening their geometry/runtime-entity assertions, and keep
`map_entity_key_values_strip_trenchbroom_metadata` green.

### Task 2: Ergonomic carried-light synthesis via grouping

Add a group pass in `parse.rs` that runs after the entity walk (assemblies and their
transient group→{member `kinematic_mover` ids, member `MapLight` indices} map are built)
and **before** `resolve_carried_light_links`. For each assembly containing at least one
member `kinematic_mover`: if it has exactly one mover with a non-empty `name`, set
`light.carrier = mover.name` for each member light that is `is_dynamic && !bake_only &&
carrier.is_empty()`; leave any member light with a non-empty explicit `carrier` untouched
(explicit wins), and when that explicit `carrier` names a mover other than the group's
single mover, `log::warn!` naming the group (contradiction). If the assembly has more than
one member mover, `log::warn!` naming the group and listing the movers, and synthesize
nothing (ambiguous carrier). If it has exactly one member mover whose `name` is empty,
`log::warn!` naming the group (grouped carry needs a named mover) and synthesize nothing.
Assemblies with zero member movers synthesize nothing (light-only groups are normal).
Construct no `CarriedLightLink` here — synthesis only sets the `carrier` string, so the
unchanged `resolve_carried_light_links` (`research.md §4`) performs offset derivation,
the baked/bake-only/spinner/duplicate-name degradation, and link construction for grouped
and explicit bindings identically. Add unit tests for each warning arm (>1 mover, unnamed
mover, explicit-carrier contradiction) and for the positive synthesis producing a link.

### Task 3: Fixtures and cross-form / parity tests

Author the durable fixtures and the tests that exercise recognition and carry across both
nesting forms. (a) A crafted grouped-leak test map: a `func_group` (brace form) whose
single brush is deliberately non-watertight, asserting the watertightness warning names
that group and that an ungrouped leak in the same map names no group (both sides of AC 2).
(b) A light+mover group fixture — a `func_group` (sibling form) containing one named
`kinematic_mover` and one `light_dynamic` with no explicit `carrier` — asserting the
resulting `CarriedLightLink` (`mover_id`, `local_offset`) is identical to the link
produced by an otherwise-identical map where the light instead sets an explicit `carrier`
naming the mover (AC 7). (c) A cross-form recognition test: one map with a brace-owned
brush group and a sibling-entity group, asserting both associate correctly and that two
same-named/distinct-GUID markers stay distinct (AC 5, 6). (d) A GUID-normalization test:
braced and bare `_tb_linked_group_id` spellings compare equal, and no instance-of/template
is produced (AC 11).

## Sequencing

**Phase 1 (sequential):** Task 1 — thin slice; falsifies the assembly boundary
(recognition → `MapAssembly` → `brush_assembly` aligned to `brush_volumes` → watertightness
names the group) across every layer before either consumer piles on.
**Phase 2 (sequential):** Task 2 — consumes Task 1's assemblies and transient membership
to synthesize carriers.
**Phase 3 (sequential):** Task 3 — fixtures and tests consume Task 1 recognition and Task 2
synthesis end to end.

Phases are sequential because all three edit `parse.rs` and Task 2/3 read the types and
membership Task 1 establishes.

## Rough sketch

- `map_data.rs`: `pub(crate) struct MapAssembly { provenance: String, group_id: String,
  linked_group_id: Option<String> }`; `MapData.assemblies: Vec<MapAssembly>`,
  `MapData.brush_assembly: Vec<Option<usize>>`.
- `parse.rs`: marker pre-pass → `HashMap<group_id, assembly_index>` + brace-brush list;
  side map `BrushId → assembly_index` filled at the flatten; `brush_assembly` built from
  the retained `(BrushId, _)` pairs of `build_brush_volumes_with_ids`; transient
  `HashMap<assembly_index, (Vec<mover_id>, Vec<light_index>)>` for Task 2.
- `pipeline.rs`: watertightness loop indexes `brush_assembly[edge.brush_index]` →
  `assemblies[i].provenance` for the per-edge line.
- Provenance display when names collide: include `group_id` (e.g. `'pink_ambient_lights'
  (group 10)`); implementer picks exact format, AC 5 only requires distinct groups be
  distinguishable.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Static-group geometry unchanged from pre-change `brush_volumes` | Task 1 (flatten unchanged; recognition only records membership) | Task 1 must not alter brush volume construction; a reordering of the flatten breaks alignment | AC 1 |
| `_tb_*` never in `MapEntityRecord.key_values` | Task 1 (reads markers from raw props into `MapAssembly`; `is_runtime_map_entity_key` untouched) | Task 1 routing a `_tb_*` value through the generic bag | AC 4 |
| `CarriedLightLink` construction lives only in `resolve_carried_light_links` | Task 2 (synthesizes `carrier` name only) | Task 2 constructing a link directly, duplicating offset/degradation logic | AC 7 |
| Explicit `carrier` takes precedence over grouped synthesis | Task 2 (skips lights with non-empty `carrier`) | Task 2 overwriting a non-empty `carrier` | AC 8 |
| `brush_assembly` aligned index-for-index to `brush_volumes` | Task 1 (keyed by retained `BrushId`, not position) | a degenerate brush dropped from `brush_volumes`; `brush_index` mis-indexed | AC 1, 2 |

## Open questions

- **Provenance display format on name collision** — decided as an implementer choice
  (include `group_id`); AC 5 pins the requirement (distinct groups distinguishable), not
  the string.
- **`brush_index` basis** — grounded as the `enumerate` index in
  `partition/face_extract.rs` over the BSP input brush slice (= `brush_volumes`); Task 1
  confirms this holds at the `pipeline.rs` call site before relying on direct indexing.

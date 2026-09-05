# Epic 22 — Kinematic Assemblies & Instanced Groups (Epic hub)

> **Status:** draft — epic *shape*, not yet per-spec detail. **New epic (Epic 22)** per owner decision — not an E17 continuation — delivering the non-visibility-bearing kinematic-cluster capability roadmap E17-F had parked. Source-grounded findings in `research.md`.
> **Layout:** this folder is the epic hub (index + `research.md`). The four child specs get their own sibling `E22--*` folders, drafted **detail-on-open** in dependency order (the E10/E15/E19 pattern) — not written up front, because each reshapes what the next assumes.
> **Related:** `context/lib/build_pipeline.md` §Source-format neutrality, §Compiler pipeline, §PRL section IDs · `context/lib/entity_model.md` · `context/lib/rendering_pipeline.md` §4/§7.4 · `context/plans/roadmap.md` Epic 17 (kinematic substrate this builds on) + Epic 22 stub, "Chunk Primitive" / "Sector Graph" (Future/Infra) · working branch `claude/grouped-brushes-prl-build`.
> **Names not locked** (scope still settling): "assembly" and the child-spec folder names are working proposals; the epic number (22) and placement are decided.

## Goal

Make grouped and instanced brushwork first-class in `prl-build` by giving the compiler a canonical **assembly** — a named, transform-bearing unit that owns member content (brushes plus member entities such as a dynamic light) and moves or instances as one — and translating TrenchBroom `func_group` / linked-group authoring onto it, instead of discarding the grouping and flattening every group into worldspawn. The runtime substrate builds on the shipped Epic 17 kinematic tier, where geometry already lives outside the static bake; nothing named after Quake or TrenchBroom reaches runtime.

## The one decision that shapes the whole epic

**The canonical assembly is defined by engine need and authored two ways that both project onto it.** This is the spine; every child spec inherits it.

- **Primary authoring: explicit reference/tag binding** — a member entity names its assembly the way movers and triggers already bind (`name` / `target_tag` / `_tags`; `crates/level-compiler` `MapKinematicMover.name`, `MapTriggerVolume.target_tag`). Robust, front-end-agnostic, and independent of TrenchBroom metadata quirks.
- **Ergonomic authoring: TrenchBroom grouping, recognized in the adapter** — `func_group` membership (both nesting forms) and linked-group identity (`_tb_linked_group_id`) are read in `format/quake_map.rs` and *translated* into the same canonical assembly + instance-of relation. Grouping is source vocabulary; it stops at the adapter (per source-format neutrality) and reaches the canonical layer as an engine concept, exactly as "the FGD projects canonical semantics; a second front end reaches the same canonical vocabulary" (`build_pipeline.md`).

This resolves the tension the request creates: "func_group first-class" does **not** mean a TrenchBroom-shaped canonical layer (the "canonical layer shaped by the source" failure `build_pipeline.md` warns against). The canonical assembly is shaped by what the engine needs — a mover that carries members, a template that instances — and `func_group` is one projection onto it, not its definition.

## Scope

### In scope (across the four child specs)
- A canonical **assembly** concept in the compiler's canonical layer (`crates/level-compiler/src/map_data.rs`) — a named unit owning member brushes and member entities, with an optional instance-of (shared-template) relation.
- **Carried members:** a dynamic light (and, by the same mechanism, other member entities) bound to a kinematic mover, travelling with it at runtime — the foundation's first real consumer.
- **Adapter recognition:** stop discarding `_tb_*` group metadata; recognize both `func_group` nesting forms and linked-group GUIDs in `format/quake_map.rs`; translate to the canonical assembly. Provenance for diagnostics (name geometry-integrity / leak / switch-clamp warnings by group).
- **Kinematic-tier instancing:** recognize linked-group instances sharing a template, dedupe to one template payload + per-instance transforms, and draw instanced — the "exploit shared template" / geometry-efficiency ask, on the kinematic tier only.
- **Runtime-addressable assemblies:** an assembly referenceable at runtime by name/tag for scripting reactions/commands, on a named gameplay consumer.
- Explicit compile-time validation and warnings for every unresolved or contradictory binding (see `research.md §7–8`).

### Out of scope
- **Instancing or grouping of static world geometry as a runtime concept.** Static geometry is baked world-space (BSP / BVH / lightmaps); runtime-instancing it contradicts baked-over-computed (`index.md §2`). Static groups keep flattening into worldspawn — with provenance preserved, geometry unchanged.
- **Visibility-bearing assemblies** — dynamic portals, sector-graph updates, assemblies that rewrite the runtime cell/portal graph. Stays E17-F; gated on author-defined sector graphs (Future/Infra). Assemblies here move like today's movers: collide and render, never re-partition visibility.
- **Building the Chunk Primitive.** The instancing spec *forces the decision* whether the instanced record converges toward the deferred chunk primitive; it does not build the unified record (that waits on ≥2 consumers, per Future/Infra).
- **General entity parenting / a scene graph.** Members follow one kinematic parent's `Transform`; this is not a general N-level transform hierarchy.
- **`func_group` as a runtime classname.** No group survives compilation as a runtime entity; the assembly is a compile-time unit that emits mover/instance/provenance data, never a `func_group` runtime type.
- **TrenchBroom layers** (`_tb_layer`) as a distinct concept — no repo fixture exercises them; treat a layer as a group for recognition, or defer explicitly per spec.

## Direction

**Problem.** The compiler has no canonical unit for "these things move or repeat together," so authored grouping cannot become grouped runtime behavior. The observable cause, not the symptom: a mover-carried light is unbuildable (a grouped light compiles to a disconnected top-level `MapLight` at its authored origin — `research.md §1`, §4), and a reused prefab has no dedup (TrenchBroom bakes each linked instance to concrete geometry and the compiler flattens it — `research.md §2`). The discarded `_tb_*` metadata is the symptom; the missing canonical assembly is the cause.

**Prior commitments.**
- *Source-format neutrality* (`build_pipeline.md`): `func_group` / `_tb_*` "flattened into the world or dropped, before shared logic." This epic **refines, not reverses** it. Flattening was one valid translation (identity discarded); we add a richer translation (identity preserved, grouping projected onto a canonical assembly). The rule is "translated, not propagated" — recognition stays in `format/quake_map.rs`, and the canonical layer stays engine-defined, so the invariant holds. The divergence is only that "dropped" is no longer the *only* translation. Stated here because unstated divergence from a written invariant is the defect.
- *E17 charter* ("deterministic kinematic geometry, not rigid-body sim, not author-scripted per-tick motion"): assemblies stay deterministic movers with members — inside the charter.
- *E17-F* (kinematic clusters/sub-worlds, chunk-primitive consolidation) is deferred pending "a concrete set-piece or measured performance need." The carried-light consumer and the instancing efficiency case are those concrete needs for the *non-visibility-bearing* slice; the visibility-bearing remainder stays E17-F. This epic is the justification to open that slice — an owner call (below), not a unilateral one.
- *Multi-brush movers already exist* (`research.md §3`): a kinematic group of brushes reuses `MapKinematicMover.brush_volumes` and the existing fuse-to-one-blob geometry path — the foundation does not re-spec mover geometry.

**Alternatives rejected.**
- *Keep flattening; author moving-platform-with-lamp by hand.* Fails outright — the light cannot move with the mover; no mechanism exists (`research.md §4`). This is the capability gap, not a workaround.
- *Solve carried lights with a narrow parent-KVP and never touch func_group.* The strongest rival for the **first spec**, and partly adopted: the foundation's primary authoring *is* an explicit reference (above), and could ship the carried light alone. Rejected as the *epic* shape because it leaves instances and "groups first-class" unaddressed — the user's headline asks ("exploit shared template," runtime-addressable groups). Kept as the thin slice inside the foundation spec.
- *Instance static world geometry (template + transforms, instanced draw for worldspawn).* Rejected: contradicts baked-over-computed — per-instance lightmaps differ, BSP/BVH are world-space. Instancing lives on the kinematic tier, where geometry is already origin-relative and transform-drawn (`research.md §5–6`).
- *Reuse `LightComponent.follow_transform` for carried lights.* Rejected as-is: same-entity and projectile-scoped (`research.md §4`).
- *Invent a mover-specific parent link.* Rejected in favor of **generalizing the shipped E21 attachment relation** (`done/E21--bone-sockets-attachments/`, `research.md §6`), which already renders a child at `holder_interpolated_transform × socket_matrix`, follows the *rendered* pose, inherits the holder's visibility, and re-derives on the client with no new wire — the same problems `research.md §7` lists for the carried light. E21 poses a rigid *mesh* at a posed *skeleton joint*; a mover-carried member is the same composition with the source pose being the mover's `Transform` (a static `Rigid(Mat4)` in E21's own binding vocabulary) and the child a live *entity* — exactly the "entity-as-attachment" growth E21 explicitly reserved ("the relation can grow that later without changing the socket vocabulary"). Spec 1's member-representation question is therefore *whether to extend that relation*, not a bare child-entity-vs-offset binary. **Data-offset divergence from E21, resolved:** E21 deliberately carries no per-attachment data offset (the prop's mesh origin is the grip point). A light has no mesh, but the same stance holds — the light's authored world position expressed in the mover's local frame *is* the offset, derived by the same origin-subtraction `extract_kinematic_mover_geometry` already applies to mover brushes (`research.md §3`), so the offset stays out of authored data. Spec 1 must adopt this derivation explicitly rather than add an offset KVP, or argue why light-carrying needs data where attachment-carrying refused it.

## Canonical model (sketch — pinned per spec)

A compiler-side assembly (working shape; exact fields land in the foundation spec):

```rust
// Proposed design — canonical layer, crates/level-compiler/src/map_data.rs
struct MapAssembly {
    name: String,                 // stable id: explicit `name`/tag, or derived from _tb_id/_tb_name
    kind: AssemblyKind,           // Static (flatten, keep provenance) | Kinematic(mover binding) | Instanced(template)
    instance_of: Option<TemplateId>, // Some(..) for linked-group copies sharing a template
    local_transform: Option<Affine3>,// per-instance transform where an instance-of relation exists
    // brush membership + member-entity references resolved by kind
}
```

- **Static** assemblies flatten exactly as today (behavior-identical geometry) but carry provenance for diagnostics.
- **Kinematic** assemblies bind member content to a mover; members (dynamic lights) get a runtime parent-transform link.
- **Instanced** assemblies dedupe a shared template to one payload + per-instance transforms.

The exact wire representation is a per-spec **Wire format** decision, deliberately not fixed here (`context_style_guide.md`: state the constraint, not the layout). **The `MapAssembly` container is introduced with spec 2** (`func_group` recognition + heterogeneous members). Spec 1 ships only the **Kinematic** row's member link in minimal form — a carried-light linkage carried in `KinematicGeometry` id 43 (v5) — because that is all its consumer exercises; the container earns its place once a second member kind and grouping exist.

## Child-spec roster

| Folder (proposed) | Unit | Layer | Opens deferred infra? | Risk |
|---|---|---|---|---|
| `E22--assembly-carried-members` | Foundation: canonical assembly + a mover-carried dynamic light | adapter + canonical + kinematic runtime | E17-F non-visibility slice | medium |
| `E22--group-recognition` | Adapter recognition of `func_group` / linked-group; provenance & diagnostics | `format/` adapter + canonical | no | low–medium |
| `E22--kinematic-instancing` | Shared-template dedupe + instanced draw path | compiler + renderer | forces Chunk-Primitive decision | high |
| `E22--runtime-addressable-assemblies` | Assemblies addressable by scripts/reactions | runtime + scripting (E14/E18) | no | medium |

## Child-spec briefs

Each is one paragraph of intent, not a task breakdown; the deep spec is drafted when its turn comes.

**1 — `E22--assembly-carried-members` (foundation + first consumer) — ready (`context/plans/ready/E22--assembly-carried-members/`).** Ship the minimal member-of-a-mover **linkage** (the reusable relation) and prove it with one consumer: a dynamic light that travels with a kinematic mover. Resolved during drafting: authoring is an explicit `carrier` KVP on the `DynamicLight` FGD base naming the mover by its `name` (a 1:1 parent — not the trigger `_tag` fan-out vocabulary), resolved at compile with `local_offset = light.origin − mover.origin`; the carried light stays a normal `AlphaLights` record plus a small linkage in `KinematicGeometry` v5 (not peeled into the mover record, not a sibling section); the runtime **generalizes the E21/`follow_transform` hook** to compose the mover's *interpolated* pose ∘ offset at upload time (no per-tick system); baked-light bindings and 0/>1-match bindings **warn and degrade** (not hard errors); client parity needs no new wire. The full `MapAssembly` container is **deferred to spec 2** — spec 1's linkage is the reusable seam, and a container with no `func_group` consumer yet would be a stub. See the spec for tasks/AC.

**2 — `E17--group-recognition` (adapter ergonomics + provenance).** Stop discarding `_tb_*` in `format/quake_map.rs`; recognize both `func_group` nesting forms (brushes-in-braces and sibling entities carrying `_tb_group "<id>"`) and the `_tb_linked_group_id` GUID; translate editor grouping into the canonical assembly of spec 1 and the instance-of relation of spec 3. A `func_group` tagged/typed as kinematic derives the same assembly an explicit binding would, so a mapper can "just group them" as a convenience over spec 1's explicit path. Static groups still flatten (behavior-identical geometry) but now carry a provenance name, so geometry-integrity / leak / switch-clamp diagnostics can name the offending group. Pure adapter + canonical work; no new runtime wire beyond what spec 1 established. Recognition must handle the two GUID brace styles on disk and the distinct-GUID (unlinked copy) case, and decide layer (`_tb_layer`) handling explicitly.

**3 — `E17--kinematic-instancing` (shared template / geometry efficiency).** Recognize linked-group instances sharing a template (spec 2's GUID relation), dedupe the duplicated concrete geometry to one origin-local template payload plus per-instance transforms, and add the net-new instanced draw path — movers today draw one transform per draw; only billboards truly instance (`research.md §5`), so this builds on the E10 instance-friendly mesh foundation. Kinematic tier only. This spec **forces the Chunk-Primitive decision**: an instanced-template record is chunk-primitive-shaped, so the spec either designs the record as the first chunk-primitive consumer or explicitly defers consolidation with a stated reason (Future/Infra gates consolidation on ≥2 consumers). Needs a real shared-GUID fixture authored — none exists (`research.md §2`), so the efficiency win here is **anticipated, not yet observed**: no map has hit prefab-geometry bloat. That makes this the epic's most deferrable leg — its last-in-sequence position and the stated fallback reflect that. Fallback if the instanced draw path is cut: the existing one-blob-per-mover model gives correctness at N× geometry with no dedup. High risk — render path + wire format + deferred-infra decision.

**4 — `E17--runtime-addressable-assemblies` (scripting reach).** Make an assembly referenceable at runtime by name/tag so scripts/reactions can address it (toggle, target, batch), building on the E18 consequential-dispatch and E14 IR addressing surfaces (`enemies({ tag })` / `updateEnemyState` precedent). Gated on a concrete gameplay consumer — likely an E18 set-piece — and may land *in* E18 rather than E17 depending on that consumer. Held last because it is the least-grounded and most likely to reshape against whatever set-piece drives it.

## Sequencing (across specs)

Detail-on-open, one child spec per `/orchestrate` cycle, re-grounded against the live tree just before it is built (each landed spec moves the ground the next stands on):

**Spec 1 first, alone** — the foundation is the thin slice; it falsifies the assembly boundary across every layer before any breadth piles on. Do not pair it.
**Spec 2 after 1** — recognition translates onto the canonical assembly spec 1 defines; it consumes that shape.
**Spec 3 after 2** — instancing consumes the GUID/instance-of relation spec 2 recognizes; also the heaviest, kept until the foundation and recognition are stable.
**Spec 4 last / detail-on-open** — reshapes against its gameplay consumer; may migrate to E18.

Within each spec, its own phase-1 thin slice rule applies (adapter → canonical → PRL → runtime for spec 1; compiler → renderer for spec 3).

## Owner decisions (resolved)

1. **Open the E17-F non-visibility slice — YES.** The carried-light consumer and the instancing efficiency case are the "concrete set-piece / measured need" E17-F's deferral asked for. The *visibility-bearing* remainder (dynamic portals, sector-graph rewrite) stays E17-F.
2. **New epic — Epic 22.** Not an E17 continuation; a standalone epic, so finished epics can be archived independently. Builds on the shipped E17 kinematic substrate.
3. **Chunk-Primitive trigger — spec 3's call.** No owner steer; spec 3 forces "converge the instanced record toward the chunk primitive, or defer consolidation with a stated reason" when it is drafted. Default lean: defer consolidation (Future/Infra gates it on ≥2 consumers) and design the instanced record so a later chunk-primitive can subsume it, unless spec-3 grounding shows convergence is cheaper than a bespoke record.
4. **First-spec floor — spec 1 ships alone.** The carried light (spec 1) is the first visible outcome on its own capability-gap justification; recognition (spec 2) follows, not bundled.

## Open questions (resolved per spec, flagged here)

- **Member representation** — extend the E21 attachment relation (child-at-`holder_interpolated_transform × offset`, generalized to a mover source and a live-entity child) vs. a bespoke mover parent link; offset derived from authored position, not an offset KVP (`research.md §6`) — spec 1.
- **Wire home for members** (extend `KinematicGeometry` id 43 with a version bump vs. a sibling section) — spec 1's Wire format section.
- **Instanced wire shape and whether it is the chunk-primitive seed** — spec 3 + owner decision 3.
- **`_tb_layer` handling** — spec 2 (treat as group, or defer).
- **Non-light member kinds** (a `prop_mesh`, an emitter carried by a mover) — the foundation generalizes to member entities, but only the dynamic light is a spec-1 consumer; others are additive, named when a consumer appears.
- **A real shared-GUID linked-group fixture** must be authored for spec 3 — none exists in-repo.
- **Branch-uniform brush provenance** — spec 2 wires `brush_assembly` from the `func_group`
  flatten only, so a grouped `switch`'s brushes (routed through their own walk branch) gain no
  provenance and a leak on them names no group. Deferred, not a permanent boundary: for an
  engine other people author maps in, a leak diagnostic that names the group for some
  brush-entering branches but not others is an inconsistency a third-party mapper feels. The
  spec that next touches the `switch` branch, or the `geometry-integrity` diagnostic (another
  consumer of the same `brush_assembly` map), should feed provenance from that branch too. The
  seam is cheap because `brush_assembly` is keyed by `BrushId`, not position. `trigger_volume`
  is excluded — its brushes never enter `brush_volumes`.

# M10 — Navigation Representation (Baked)

## Goal

Resolve where walkable surfaces come from: an offline navmesh bake in prl-build, emitted as a new PRL section — kin to the baked BVH. Seeds the baked spatial-AI layer; the next plan (pathfinding + path following) consumes this section as its contract.

## Scope

### In scope

- A new `NavMesh` PRL section (id 36) in `postretro-level-format`: walkable-span voxel grid + the agent parameters it was baked with.
- A prl-build bake stage: voxelize walkable space from the geometry section's triangles, filter by slope / clearance / step, erode by agent radius. Deterministic; cached via the build-stage cache.
- Agent-parameter authoring: worldspawn KVPs with engine defaults.
- Runtime loader: section read into `LevelWorld`, absent section valid.
- Debug overlay (`dev-tools`): walkable cells rendered in-world, toggled by a diagnostic chord.

### Out of scope

- Runtime pathfinding, A*, steering, agent movement — the next plan.
- Polygonal navmesh / contour simplification — the span grid is the v1 representation; polygonization is a future additive change behind the same section id discipline (new section or version bump) if path quality demands it.
- Per-archetype navmeshes — one grid per map, baked for one canonical agent. Multiple agent sizes defer.
- Authored walkability overrides (FGD brush roles, "no-nav" brushes) — derivation is geometry-only in v1.
- Off-mesh links: jump links, cover points, hint nodes — later additive extensions of the spatial-AI layer.
- Dynamic obstacles / runtime navmesh updates.

## Representation decision

Walkable-span voxel grid (the heightfield half of Recast, hand-rolled; no contouring, no external nav crate):

- World XZ is divided into uniform square columns (`cell_size`, default 0.25 m). Each column holds zero or more **walkable spans**: a floor height plus vertical clearance, supporting stacked floors (bridges, multi-storey rooms).
- A span is walkable iff: supporting triangle slope ≤ `max_slope`; clearance ≥ `agent_height`; cell survives erosion by `agent_radius` (cells within the radius of a non-walkable boundary are removed).
- Source triangles are the geometry section's — the same triangle set the runtime collision trimesh is built from, so the navmesh can never claim ground collision would reject. Triangles in exterior or solid BSP leaves are excluded.
- Cell adjacency (4-neighbor, step-height rule: reachable iff floor-height delta ≤ `step_height` and both clearances admit the agent) is **derived at load time by the runtime**, not serialized — keeps the wire format minimal and lets the adjacency rule evolve without a format break.

Rationale: deterministic, dependency-free, handles multi-level topology, and small at boomer-shooter map scale. Path smoothing over grid paths belongs to the pathfinding plan.

## Acceptance criteria

- [ ] `prl-build` on `content/dev/maps/campaign-test`'s source map emits a NavMesh section; the build summary lists the navmesh stage and its timing.
- [ ] No walkable span lies inside solid brushes or exterior leaves; no span sits on a surface steeper than `max_slope`; no span exists where vertical clearance is below `agent_height`; spans within `agent_radius` of a wall are eroded (verified by unit tests on fixture geometry).
- [ ] A fixture with two stacked floors produces walkable spans on both levels in the same column.
- [ ] Two consecutive builds of the same map produce byte-identical NavMesh section bytes.
- [ ] An unchanged rebuild hits the stage cache; changing any nav worldspawn KVP misses it.
- [ ] Worldspawn KVPs override each agent parameter and the cell size; absent KVPs use engine defaults; the baked parameters are readable back from the loaded section.
- [ ] A map with no walkable surface emits no NavMesh section, and the build succeeds.
- [ ] The engine loads a `.prl` with the section into `LevelWorld`; a `.prl` without it loads with no error or warning beyond a debug-level note.
- [ ] With `dev-tools`, a diagnostic chord toggles an in-world overlay of walkable cells; cells visibly hug floors and stop at walls (manual check on campaign-test).
- [ ] Adjacency derived at load: two spans whose floor delta exceeds `step_height` are not neighbors; spans across a step at or under it are (unit test).

## Tasks

### Task 1: NavMesh section format

New `navmesh` module in `postretro-level-format`: `SectionId::NavMesh = 36` (enum + `from_u32`), a section struct with `to_bytes` / `from_bytes`, a section-internal `u16` version, and round-trip tests including the empty-grid and single-span cases. Carries grid metadata (origin, cell size, XZ dims), the agent parameters baked with, sparse occupied columns, and the flat span array (see Wire format).

### Task 2: Bake stage

New bake module in the level compiler, inserted after the BVH stage and before the lightmap stage in the build sequence. Inputs: the extracted geometry result (triangles + face metadata), the BSP tree, the exterior-leaf set, and the resolved nav parameters. Rasterize triangles into column spans, classify walkable by slope, compute clearance against overhead spans, apply the step-height floor merge, erode by agent radius, emit the section (or none when no span survives). Wrap in the build-stage cache (`cache.rs` `CacheKey::new` pattern): stage id `"navmesh"`, a stage-version `u32` const bumped on algorithm change, input hash over geometry bytes + nav parameters. Sequential, allocation-stable code — no parallel reductions — so output is byte-deterministic. Nav parameters: parse worldspawn KVPs in the map-data layer (the `lightmap_density` precedent), defaults `nav_agent_radius` 0.4, `nav_agent_height` 1.8, `nav_step_height` 0.3, `nav_max_slope` 45.0 (degrees), `nav_cell_size` 0.25 (meters). Wire the section into the pack call as a new optional parameter.

### Task 3: Runtime loader + debug overlay

Read the section into a new optional `LevelWorld` field following the existing optional-section pattern (warn-and-continue on a malformed section, silent-valid when absent). Derive the cell adjacency at load into a runtime-side nav structure (the query surface the pathfinding plan will extend) and unit-test the step-height neighbor rule. Add a `DiagnosticAction` variant + chord for a navmesh overlay; render walkable cells as translucent quads at span floor heights through a debug pass in the renderer module (the SH-diagnostics pass is the structural precedent; all wgpu stays renderer-side), feature-gated `dev-tools`.

## Sequencing

**Phase 1 (sequential):** Task 1 — the section type both sides consume.
**Phase 2 (concurrent):** Task 2, Task 3 — compiler and runtime sides are independent once the format exists (Task 3 tests against hand-constructed sections).

## Rough sketch

- Format: `crates/level-format/src/navmesh.rs`, registered in `lib.rs` (`NavMesh = 36`, `from_u32` arm). Mirror `bvh.rs` conventions.
- Bake: `crates/level-compiler/src/navmesh_bake.rs`; stage inserted in `main.rs` between the BVH and lightmap stages; section threaded through `pack::pack_and_write_portals` as `Option<&NavMeshSection>`. KVP parsing beside the existing worldspawn fields in `map_data.rs` / `parse.rs`.
- Rasterization: per triangle, clip to overlapped columns, record (min_y, max_y, walkable = face normal's Y ≥ cos(max_slope)) span fragments; merge fragments per column bottom-up; clearance = gap to the next span above (open sky = +∞). Exterior/solid exclusion via the face's leaf (the geometry result carries per-face leaf association; if only per-leaf face ranges exist, invert that mapping at bake start).
- Runtime: `prl.rs` gains `pub navmesh: Option<NavMeshSection>`; a new `nav` module owns the load-time adjacency derivation and the cell query surface. Overlay: `DiagnosticAction::ToggleNavOverlay` in `input/diagnostics.rs`; debug pass beside `render/sh_diagnostics.rs`.
- Oversized-file flag: compiler `main.rs` (~1000 lines of stage orchestration) gains one more stage block — acceptable, it is a linear stage table; do not refactor it in this plan.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| agent radius | `agent_radius` | binary field | n/a | n/a | `nav_agent_radius` |
| agent height | `agent_height` | binary field | n/a | n/a | `nav_agent_height` |
| step height | `step_height` | binary field | n/a | n/a | `nav_step_height` |
| max slope (deg) | `max_slope_deg` | binary field | n/a | n/a | `nav_max_slope` |
| cell size | `cell_size` | binary field | n/a | n/a | `nav_cell_size` |

No script surface in this plan.

## Wire format

Mirrors `BvhSection` conventions: little-endian throughout, `u32` counts, flat arrays, no padding between records.

Section body, in order:

1. `version: u16` — section-internal format version, starts at 1.
2. Grid header: `origin: [f32; 3]` (min corner), `cell_size: f32`, `dim_x: u32`, `dim_z: u32`.
3. Agent params (provenance, the values baked with): `agent_radius: f32`, `agent_height: f32`, `step_height: f32`, `max_slope_deg: f32`.
4. `column_count: u32`, then `column_count` records of `(column_index: u32, first_span: u32, span_count: u32)` — sparse occupied columns, sorted ascending by `column_index` (= `x + z * dim_x`). Unlisted columns have no spans.
5. `span_count: u32`, then `span_count` records of `(floor_y: f32, clearance: f32)` — flat array the column records index into; per column, spans sorted ascending by `floor_y`. `clearance` is `f32::INFINITY` for open sky.

Empty encoding: a map with zero walkable spans emits **no section** (the SDF-atlas precedent); a section, once present, always has `column_count ≥ 1`. Adjacency is never serialized.

## Open questions

- Erosion at step edges: naive radius erosion also eats cells beside a climbable step (a wall for half the body, a floor for the feet). v1 accepts the conservative loss; revisit if doorway-width paths vanish on real maps — implementer should log eroded-cell counts at `info`.
- Whether the geometry result exposes per-face leaf ids directly or only per-leaf face ranges decides a small bake-start inversion step — implementation detail, decided at code contact.

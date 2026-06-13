# M10 — Navigation Representation (Baked)

## Goal

Resolve where walkable surfaces come from: an offline navmesh bake in prl-build, emitted as a new PRL section — kin to the baked BVH. The baked shape is the permanent query contract — convex walkable regions joined by portals — so the next plan (pathfinding + path following) builds A* + funnel string-pulling once, and the baked spatial-AI layer (jump links, cover, hints) extends it additively.

## Scope

### In scope

- A new `NavMesh` PRL section (id 36) in `postretro-level-format`: walkable regions + portals + the agent parameters baked with.
- A prl-build bake stage: voxelize walkable space from the geometry section's triangles into a span grid (slope / clearance / step filters, agent-radius erosion), then greedy rectangular decomposition into regions and shared-edge portal extraction. Deterministic; cached via the build-stage cache.
- Agent-parameter authoring: worldspawn KVPs with engine defaults.
- Runtime loader: section read into `LevelWorld` and exposed as a region-graph query surface; absent section valid.
- Debug overlay (`dev-tools`): regions and portal edges rendered in-world, toggled by a diagnostic chord.

### Out of scope

- Runtime pathfinding, A*, funnel, steering, agent movement — the next plan, built against this plan's region/portal contract.
- Contour-traced polygonal regions (full Recast-style polygonization) — rectangular decomposition is the v1 convex-area generator; a contoured generator would be a bake-internal swap behind the same section semantics, taken up only against measured path-quality need.
- Per-archetype navmeshes — one graph per map, baked for one canonical agent. Multiple agent sizes defer.
- Authored walkability overrides (FGD brush roles, "no-nav" brushes) — derivation is geometry-only in v1.
- Off-mesh links (jump links, drop-downs) and hint data (cover points) — future **portal kinds / region attachments** on this format; the model accommodates them additively, none ship here.
- Dynamic obstacles / runtime navmesh updates.

## Representation decision

**Convex regions + portals** — the shape every mature nav system converges on, and the one the funnel algorithm requires. The bake reaches it in two stages:

1. **Span-grid rasterization** (the heightfield half of Recast, hand-rolled; no external nav crate). World XZ divides into uniform square columns (`cell_size`, default 0.25 m); each column holds walkable spans (floor height + vertical clearance), supporting stacked floors. A span is walkable iff: the supporting triangle's upward-facing normal passes the slope filter (`normal.y >= cos(max_slope_deg)`, evaluated in normal-Y space — no per-triangle angle is computed); clearance ≥ `agent_height`; the cell survives erosion by `agent_radius`. **Erosion is wall-aware:** a cell is eroded only when within `agent_radius` of a true non-walkable boundary (a non-walkable neighbor, or a floor delta to the nearest span exceeding `step_height` — a wall or unclimbable drop). A climbable-step neighbor (an adjacent walkable span within `step_height`) is **not** a boundary and does not erode its neighbors, so a path through a doorway beside a step stays open — the M10 north star requires enemies to flow through stepped arena geometry toward the player. Source triangles are the geometry section's — the same set the runtime collision trimesh is built from, so the navmesh never claims ground collision would reject. Triangles in exterior or solid BSP leaves are excluded. The grid cells and spans are a **bake-internal intermediate** — never serialized (only the grid *header* is, as decode provenance for cell-space region coords).
2. **Region + portal extraction.** Greedy rectangular decomposition merges walkable cells into disjoint axis-aligned rectangular regions; cells merge into one region only while every interior adjacent-cell floor delta stays ≤ `step_height` (a region is traversable everywhere inside itself). Portals are extracted where two regions share an edge whose floor delta along the shared run is ≤ `step_height`; each portal is a world-space segment (the funnel algorithm's input) joining two region indices. A shared edge exceeding `step_height` produces no portal (a ledge; one-way drop portals are a future portal kind).

The runtime consumes regions and portals directly — no load-time adjacency derivation, no cell data. Rationale: deterministic, dependency-free, multi-level capable, and the section + runtime contract already have the end-state shape: a later quality upgrade (contoured polys) or extension (jump-link portals, region hints) changes the bake or adds records — it never rewrites the pathfinding layer built on top.

## Acceptance criteria

- [ ] `prl-build` on `content/dev/maps/campaign-test`'s source map emits a NavMesh section; the build summary lists the navmesh stage and its timing.
- [ ] No region covers space inside solid brushes or exterior leaves, on surfaces failing the `max_slope_deg` normal filter, under clearance below `agent_height`, or within `agent_radius` of a true non-walkable boundary (a wall or unclimbable drop); a climbable step beside a walkable cell does not erode it, so a doorway-width path past a step stays connected (unit tests on fixture geometry, including a doorway-beside-a-step case).
- [ ] Regions are disjoint axis-aligned rectangles that exactly cover the surviving walkable cells; within any region, every interior adjacent-cell floor delta is ≤ `step_height` (unit test).
- [ ] Portals exist exactly where two regions share an edge run with floor delta ≤ `step_height`; a shared edge across a taller ledge yields no portal; each portal's segment endpoints lie on the shared edge in world space (unit tests).
- [ ] A fixture with two stacked floors produces distinct regions on both levels over the same footprint, with no portal between them.
- [ ] Two consecutive builds of the same map produce byte-identical NavMesh section bytes.
- [ ] An unchanged rebuild hits the stage cache; changing any nav worldspawn KVP misses it.
- [ ] Worldspawn KVPs override each agent parameter and the cell size; absent KVPs use engine defaults; the baked `cell_size` and all four agent parameters are readable back from the loaded section.
- [ ] A map with no walkable surface emits no NavMesh section, and the build succeeds.
- [ ] The engine loads a `.prl` with the section into `LevelWorld` and exposes a region-graph query surface (region lookup by point, portal iteration per region); a `.prl` without it loads with no error, the absent section logged at `info` (matching the SDF-atlas precedent).
- [ ] With `dev-tools`, a diagnostic chord toggles an in-world overlay of region rectangles and portal edges; regions visibly hug floors and stop at walls (manual check on campaign-test).

## Tasks

### Task 1: NavMesh section format

New `navmesh` module in `postretro-level-format`: `SectionId::NavMesh = 36` (enum + `from_u32` arm), a section struct with `to_bytes` / `from_bytes`, a section-internal `u16` version (an addition over the `BvhSection` layout, which carries no version field — the rest of the byte discipline mirrors `bvh.rs`), and round-trip tests including the single-region, no-portal, and stacked-region cases. Carries the grid header (origin, cell size, XZ dims — provenance for decoding cell-space region coords), the agent parameters baked with, the region records, and the portal records (see Wire format).

### Task 2: Bake stage

New bake module in the level compiler, inserted after the BVH stage and before the lightmap stage in the build sequence; register it with the build summary/timing report so its name and elapsed time appear in the summary (mirror an existing cached stage's summary entry). Inputs: the extracted geometry result — its `geometry` (vertices/indices/`faces`), with per-face leaf id available directly as `FaceMeta.leaf_index` — the BSP tree (`BspTree`), the exterior-leaf set (`&HashSet<usize>`), and the resolved nav parameters. Stage 1: rasterize triangles into column spans, classify walkable by the `max_slope_deg` normal filter, compute clearance against overhead spans, erode by agent radius against true non-walkable boundaries only (a climbable-step neighbor is not a boundary — see the Representation decision). Stage 2: greedy rectangular decomposition (deterministic scan order: ascending z then x, grow x-run then z) into regions honoring the interior step rule, then portal extraction over shared region edges with the step rule, emitting world-space portal segments. The stage returns `Option<NavMeshSection>` into the compiler's stage orchestration (none when no region survives); `main.rs` passes it to the pack call as the new trailing `Option<&NavMeshSection>` argument (the SDF-atlas parameter is the precedent). Wrap the stage body in the build-stage cache (`cache.rs` `CacheKey::new(stage_id, stage_version, input_hash)`): stage id `"navmesh"`, a stage-version `u32` const bumped on algorithm change, input hash over geometry bytes + nav parameters. Sequential, allocation-stable code — no parallel reductions — so output is byte-deterministic. Nav parameters: parse worldspawn KVPs in the map-data layer and resolve them onto the parsed map-data/worldspawn struct already threaded to compiler stages (the bake reads them from there; no CLI override). Defaults `nav_agent_radius` 0.4, `nav_agent_height` 1.8, `nav_step_height` 0.3, `nav_max_slope` 45.0 (degrees), `nav_cell_size` 0.25 (meters). The grid-header `cell_size` equals the resolved `nav_cell_size`; the bake does not re-derive it.

### Task 3: Runtime loader + debug overlay

Read the section into a new optional `LevelWorld` field following the existing optional-section pattern (warn-and-continue on a malformed section; an absent section is logged at `info`, matching the SDF-atlas loader). A new runtime `nav` module owns the region graph — region records, per-region portal lists (built once at load from the portal array), point → region lookup, and read-back of the grid header + agent parameters — as the query surface the pathfinding plan extends; unit-test the lookup and portal iteration against hand-built sections. Add a `DiagnosticAction` variant + chord for a navmesh overlay; render region rectangles and portal segments through a debug pass in the renderer module (the `render/sh_diagnostics.rs` line-overlay pass is the structural precedent; all wgpu stays renderer-side), feature-gated `dev-tools`.

## Sequencing

**Phase 1 (sequential):** Task 1 — the section type both sides consume.
**Phase 2 (concurrent):** Task 2, Task 3 — compiler and runtime sides are independent once the format exists (Task 3 tests against hand-constructed sections).

## Rough sketch

- Format: `crates/level-format/src/navmesh.rs`, registered in `lib.rs` (`NavMesh = 36`, `from_u32` arm). Byte discipline mirrors `bvh.rs`; the `u16` version is new (bvh.rs has none).
- Bake: `crates/level-compiler/src/navmesh_bake.rs`; stage inserted in `main.rs` between the BVH stage (`bvh_build::build_bvh`) and the lightmap stage; section threaded through `pack::pack_and_write_portals` as a new trailing `Option<&NavMeshSection>`. KVP parsing beside the existing worldspawn fields in `map_data.rs` / `parse.rs`.
- Rasterization: per triangle, clip to overlapped columns, record (min_y, max_y, walkable = `normal.y >= cos(max_slope_deg)`) span fragments; merge fragments per column bottom-up; clearance = gap to the next span above (open sky = +∞). Exterior/solid exclusion via `FaceMeta.leaf_index` against the exterior set and solid-leaf classification.
- Region floor heights: a region record carries `floor_y_min` / `floor_y_max` over its cells; exact ground height is the collision world's job (agents sweep against real collision; the navmesh routes).
- Runtime: `prl.rs` gains `pub navmesh: Option<NavMeshSection>`; new `nav` module owns the region graph + queries. Overlay: `DiagnosticAction::ToggleNavOverlay` in `input/diagnostics.rs`; debug pass beside `render/sh_diagnostics.rs`.
- Oversized-file flag: compiler `main.rs` (~2200 lines of stage orchestration) gains one more stage block — acceptable, it is a linear stage table; do not refactor it in this plan.

## Boundary inventory

| Name | Rust | Wire / serde | FGD KVP |
|---|---|---|---|
| agent radius | `agent_radius` | binary field | `nav_agent_radius` |
| agent height | `agent_height` | binary field | `nav_agent_height` |
| step height | `step_height` | binary field | `nav_step_height` |
| max slope (deg) | `max_slope_deg` | binary field | `nav_max_slope` |
| cell size | `cell_size` | binary field | `nav_cell_size` |

No script surface in this plan. Worldspawn KVP underscore use is inconsistent in-tree (`fog_pixel_scale`, `data_script` have none; `_lightmap_density`, `_tags` lead with one); the `nav_*` keys (no leading underscore) follow the `fog_pixel_scale` form, the majority for engine-authored worldspawn KVPs.

## Wire format

Mirrors `BvhSection` byte conventions (little-endian throughout, `u32` counts, flat arrays, no padding between records), plus a leading section-internal version (bvh.rs has none).

Section body, in order:

1. `version: u16` — section-internal format version, starts at 1.
2. Grid header (provenance for cell-space coords): `origin: [f32; 3]` (min corner), `cell_size: f32`, `dim_x: u32`, `dim_z: u32`.
3. Agent params (the values baked with): `agent_radius: f32`, `agent_height: f32`, `step_height: f32`, `max_slope_deg: f32`.
4. `region_count: u32`, then `region_count` records of `(x0: u32, z0: u32, x1: u32, z1: u32, floor_y_min: f32, floor_y_max: f32)` — cell-space rectangle, min inclusive / max exclusive; records sorted ascending by `(z0, x0, x1, z1, floor_y_min)`, a tuple the decomposition guarantees unique per region. Portal records index this final sorted region array.
5. `portal_count: u32`, then `portal_count` records of `(region_a: u32, region_b: u32, left: [f32; 3], right: [f32; 3])`. Endpoint construction: the shared cell-edge X/Z map to world via `origin + cell_size * index`; the segment Y is the minimum of the two regions' floor heights along the shared run. The segment is wound left-to-right as seen from `region_a` facing `region_b`; `region_a < region_b`; records sorted ascending by `(region_a, region_b)` then lexicographically by `left.x`, `left.y`, `left.z` under f32 total order (`to_bits` comparison — the bake produces no NaNs). `portal_count = 0` is valid (a single isolated region).

Empty encoding: a map with zero walkable regions emits **no section** (the SDF-atlas precedent); a section, once present, always has `region_count ≥ 1`. Cell and span data are never serialized; the grid header is.

## Open questions

- Greedy rect decomposition can fragment around slopes (floor variance splits runs). Acceptable at v1 scale; the region count is logged at bake so fragmentation is observable before it matters. Consumer-resolved: the next wave (pathfinding) builds A* over the regions and reveals whether fragmentation degrades paths or perf — the logging is that feedback's instrumentation. Safe to defer because regions+portals is the contract; a contour-traced generator is a bake-internal swap behind the same section semantics (Out of scope).

## Resolved during review

- Step-edge erosion: erosion is wall-aware — a climbable step (delta ≤ `step_height` between two walkable spans) is not a boundary and does not erode its neighbors. The north star ("walks toward the player without clipping" through stepped geometry) makes the wall-vs-step distinction a correctness requirement, not an optimization; the implementer still logs eroded-cell counts at `info` for the fragmentation feedback above.

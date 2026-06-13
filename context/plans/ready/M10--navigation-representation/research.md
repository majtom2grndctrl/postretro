# Research: Navigation representation (baked) — pre-spec brief

> Prep for `/draft-spec`. Roadmap task: M10 "Navigation representation (baked)" (`plans/roadmap.md:216`).
> Wave context: pairs with `M10--skeletal-hit-zones` in the same orchestration wave — disjoint subsystems
> (level-compiler/level-format vs. model/weapon/scripting). Pathfinding + path following is the next wave
> and consumes this spec's section; design the section format as its contract.

## Scope intent (from roadmap)

Resolve the expensive, hard-to-reverse question: where do walkable surfaces come from. Lead candidate:
offline bake in prl-build deriving a navmesh from world geometry (agent radius/height, slope filter),
emitted as a new PRL section — kin to the baked BVH and collision trimesh. Seed of a broader baked
spatial-AI layer (cover points, jump links, hint nodes extend the same section additively later).
Runtime A*/steering is explicitly **out of scope** (next spec); this spec should still ship a runtime
loader and a debug overlay so the bake is verifiable.

## Compiler insertion point

- prl-build stages run sequentially in `crates/level-compiler/src/main.rs:284–985`. Natural slot:
  **after BVH build (`:416–423`), before lightmap bake (`:425`)** — geometry, BSP tree, and exterior
  leaves are all final by then.
- New module: `crates/level-compiler/src/navmesh_bake.rs`; section serialized in
  `pack.rs::pack_and_write_portals()` as a new optional parameter (precedent: BVH at `pack.rs:43–72`).

## Available bake inputs

- `GeometryResult` (`level-compiler/src/geometry.rs:28–33`): vertices/indices, `FaceMeta[]`
  (texture index per face), per-face index ranges.
- `BspTree` (`partition.rs`): empty/solid leaf classification; exterior-leaf set from the visibility
  stage (`visibility/mod.rs`, `HashSet<usize>`). Navmesh must exclude exterior and solid regions.
- No per-face "walkable" flag exists in the Quake `.map` source; walkability must be derived
  (slope from face normals) and/or authored later via FGD extension.

## PRL section

- `SectionId` registry: `crates/level-format/src/lib.rs:67–235`. **Next free ID: 36** (`NavMesh = 36`).
- Pattern: new `level-format/src/navmesh.rs` with `NavMeshSection`, `to_bytes()/from_bytes()`, and a
  section-internal `u16` version. Update `from_u32()`.
- Runtime reader: `crates/postretro/src/prl.rs` — add `pub navmesh: Option<NavMeshSection>` to
  `LevelWorld`, warn-and-continue on read failure (existing optional-section pattern).

## Build-stage cache

`<workspace>/.build-caches/prl-cache/`, key = `blake3(stage_id || stage_version_u32 || input_hash)`,
`input_hash = blake3(postcard(inputs) || postcard(config))`; LRU eviction, 2 GiB default. Follow the
SDF-atlas pattern: `stage_id = "navmesh"`, `NAVMESH_STAGE_VERSION` const bumped on algorithm changes.
**Determinism (byte-identical output for identical inputs) is required** for the cache — constrains
algorithm choice (no nondeterministic parallel reductions).

## Agent parameters

- Player values live in descriptor data, materialized into `PlayerMovementComponent`
  (`scripting/components/player_movement.rs:147–256`): `CapsuleParams { radius, half_height, eye_height }`,
  `GroundParams { step_height, max_slope }` (degrees; `cos_walkable` precomputed). Test values:
  `step_height: 0.3`, `max_slope: 45.0` (`movement/mod.rs:1942–1943`).
- Bake needs its own agent params (radius/height/step/slope). Open: per-map worldspawn KVP vs.
  CLI/config default vs. per-archetype (multiple param sets ⇒ multiple meshes?). Start with one
  canonical agent param set, authored per map with engine defaults.

## Debug visualization (runtime verification)

- Diagnostic chord system: `crates/postretro/src/input/diagnostics.rs:29–150`
  (`DiagnosticAction::ToggleWireframe`, egui debug panel behind `dev-tools`). Precedent for a custom
  debug pass: `render/sh_diagnostics.rs` (SH probe visualization). A `ToggleNavmeshOverlay` action +
  overlay pass follows these patterns. Note: renderer owns all wgpu calls.

## Prior art in repo

None. No navmesh/recast/pathfinding code or crate deps anywhere; all mentions are roadmap text.
Greenfield, but the BVH bake (`level-compiler/src/bvh_build.rs:65–94`) is the structural template:
collect primitives → deterministic sort → build → flatten to dense arrays → serialize.

## Open design questions the spec must resolve

1. **Algorithm**: Recast-style voxelization vs. lightweight cell-and-portal/BSP-derived walkable
   polygons vs. hand-rolled voxel grid + simplification. Engine bias: lean, no heavy deps,
   deterministic. The BSP already classifies empty space — a BSP/face-derived approach may be far
   cheaper than full Recast at boomer-shooter geometry scale.
2. **Runtime data structure** (the contract the pathfinding spec consumes): polygon mesh + adjacency
   graph vs. grid. Whatever ships must support A* and "nearest point on navmesh" queries.
3. **Walkable derivation**: geometry-only (non-exterior, non-solid, slope ≤ max) for v1; FGD brush-role
   overrides deferred additively.
4. **Agent-param authoring** (above) and whether one mesh serves all archetypes in v1 (recommend: yes).
5. **Empty/absent handling**: section absent when no walkable surface (SDF-atlas precedent), never a
   build failure.
6. **Multi-level topology**: ramps, bridges, floating platforms — algorithm must handle stacked
   walkable layers (rules out naive 2D heightfield).

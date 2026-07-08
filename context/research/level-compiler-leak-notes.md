# Level-Compiler Leak & Missing-Geometry Notes

> **Read this when:** debugging missing world geometry from `prl-build` — holes you fall through, dropped triangles, suspected leaks or exterior-cull problems.
> **Status:** point-in-time research from a face-extraction bug hunt. Function/line references may drift; verify against source. Read `build_pipeline.md` §Compiler pipeline first.
> **Related:** [Build Pipeline](../lib/build_pipeline.md)

Distilled from tracing a "fall through the floor" bug in `movement-feel.map`. Root cause was **not** a leak — it was a convex-hull robustness failure in face extraction (fixed in `face_extract.rs::monotone_chain_hull`). The traps below are what made it look like a leak.

## Where the machinery lives

- **Exterior culling:** `crates/level-compiler/src/visibility/mod.rs` → `find_exterior_leaves`. Floods from a probe point *outside* the world AABB (`map_max + (1,1,1)`) through the portal graph. Every reachable non-solid leaf becomes "exterior" and gets `face_count = 0` — its geometry drops from both the geometry buffer and the BVH.
- **It is silent.** Only `warn!`s on a solid seed, or if *every* interior empty leaf vanishes. A *partial* leak just deletes geometry with an info log. Assume nothing is being reported.
- **Portals are BSP-derived:** `portals::generate_portals(&tree)`. Solid classification, portals, and cells all come from the BSP. The BVH is built from *extracted geometry*, not BSP nodes.
- **Pipeline order** (`main.rs`, BSP through BVH): parse → BSP partition → generate_portals → find_exterior_leaves → extract_geometry → build_bvh → *then* the slow lightmap/SH bakes.

## Traps (don't repeat these)

1. **A large culled *area* is not evidence of a leak.** A boxy sealed map legitimately culls ~40–50% of its surface area — the outward back-faces of the shell. Always check face **normals** first. Correctly-culled faces point *outward*: floor underside `(0,-1,0)`, ceiling topside `(0,+1,0)`, wall exteriors. A leak culls *inward* faces: walkable floor-top `(0,+1,0)`, visible wall interiors.
2. **"Void reaches player_spawn" can miss real problems.** The interior portal graph can be fragmented, so a geometry-owning leaf floods while the player's leaf does not. In this bug, 0 of 33 entities sat in flooded leaves yet geometry looked missing. Entity-based leak detection is necessary, not sufficient.
3. **"Missing triangles / fall through" is often not a leak.** Face-*extraction* drops mimic leaks. Discriminate: a leak culls inward faces via the exterior flood; an extraction drop means the face is absent from `result.faces` *before* culling. Compare input brush sides vs emitted faces; check whether the Pass-1 `visible_hull` is already incomplete.

## Diagnostics that worked

- Instrument `main.rs` right after `find_exterior_leaves`, then `std::process::exit(0)` to **skip the multi-minute bake**. Spatial stages run in seconds; the bake dominates and times out.
- **Floor-hole finder:** grid the XZ plane; for each standing point (empty air ~0.3 m above floor) check whether a `+Y` floor face exists beneath it. Finds walk-through holes with coordinates.
- **Normal audit** of culled-vs-kept faces (the leak discriminator above).
- **Pointfile:** BFS from the exterior seed to the suspect leaf over the portal graph; dump portal centroids in map units to see where the void connects in.
- Log noise: filter `texture_validation` / `surface-map` lines. A naive `grep face` matches "sur**face**".
- For a numerical bug, capture **full-precision** coordinates (`{}`, not `{:.2}`) into a unit test — rounding hides FP-sensitive bugs.

## Latent bug worth checking during a leak hunt

Portals are **not clipped to the world bounding box.** `make_node_portal` (`portals.rs`) clips only by ancestor planes, so a portal can extend to `WINDING_HALF_EXTENT` (±16384 m). Observed ±8192 m portal centroids in a ±41 m map. This deviates from id-Tech's `MakeHeadnodePortals` bbox clip and can manufacture spurious leaf adjacency the exterior flood walks through. It did not change this map's connectivity, but for a genuine leak it is a prime suspect. Fix: seed portal clipping with the six world-box half-spaces.

## Grounding facts

- Coordinate transform: engine = `(-qy, qz, -qx) x 0.0254` (Quake Z-up to engine Y-up, inch to metre). Inverse for map-space pointfiles: `q = (-ez, -ex, ey) / 0.0254`.
- 1 map unit = 0.0254 m. `SPLIT_EPSILON = 0.1` units (~2.5 mm) in `face_extract.rs`.
- Determinism invariant (`build_pipeline.md`): byte-identical output for identical inputs. Parse/BSP/portals/geometry/BVH run **uncached** — changes there need no cache stage-version bump.

## If the next bug is a genuine leak

The silent-cull is the footgun. The proper fix is the id-Tech fill — `exterior = void-reachable MINUS interior-reachable` (flood-protect entity-reachable leaves) — plus a hard leak error with a TrenchBroom-loadable pointfile, rather than today's pure void-flood.

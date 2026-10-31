# mover-light-flow--foundation — research notes

Read at e3b0772, 2026-10-30/31.

## Where this sits

- `context/research/gi-rtx-lite/index.md` build order: step 2 (door classification, influence graph, light → cell reach, compile report) sizes step 5 (door channel masks), step 6 (direct-SH door factor), step 7 (slab shadows); step 12 lists the mover rest-pose channel. This brief does step 2 per (mover, terminus) so step 12 is not a second mechanism.
- IP: Take-Two '688/'404 granted claims do not reach door channels (charted 2026-10-30, `ip-check.md` piece 7). Tencent '451's elements are indirect transfer × coefficient on a light change; a direct-only visibility analysis has none of them. This brief is compile-only, IP "None" in the research table.

## Grounding

- Door recognition and sealing read the waypoint-0 brushes only (`recognize_doors`, `sealed_portal_ids_for_mover`); no compiler code poses a mover elsewhere. Sealing runs for every mover, so a non-door (the rising room's lowered roof) can still seal portals.
- Per-(light, texel) visibility: `LightmapLayer { target_layer, texels: Vec<LayerTexel { idx, raw_visibility }> }`, zero-visibility records kept. SDF-shadow lights are filtered out of the layers in the lightmap stage. `build_pipeline.md` says no per-(light, texel) visibility is retained after the walk.
- Animated: id 25 ships per-texel `TexelLight { light_index, weight }` keyed to id-22 blocks.
- SH: base SH sums lights per probe; per-light deltas (27/41/45) are per affinity cell. Probe → cell via `find_leaf_for_point`.
- Existing reach-like data, none per light per cell measured: `light_portal_reach` (transient flood), id 21 LightInfluence spheres, id 23 ChunkLightList, id 18 AlphaLights `leaf_index`, id 46 CellVisibility, id 31 FogCellMasks, id 42 shadowmask owners per face.
- Runtime: promotion candidates pass a light-sphere vs `fog_reachable` cell-AABB test (`light_reaches_visible_cell`); `blocked_portals` shrinks `fog_reachable` only. No code floods from a light. Fog point lights ignore reach. A later runtime filter must account for entity receivers before dropping candidates.

## Counts on record

| Map | Cells | Lights | Source |
|---|---|---|---|
| stress-warren-hallway-inspection | 10021 (interior 3268, drawable 3184) | 345 | `plans/done/occluder-first-partition/measurement/9k/cap26/.../structure.out` |
| campaign-test | 879 (interior 165, drawable 155) | 34 | same series |

## Report totals

To be filled at build (Manual AC).

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
| stress-warren-hallway-inspection | 10021 (interior 3268, drawable 3184) | 345 | `context/plans/done/occluder-first-partition/measurement/9k/cap26/stress-warren-hallway-inspection/structure.out` |
| campaign-test | 879 (interior 165, drawable 155) | 34 | `context/plans/done/occluder-first-partition/measurement/9k/cap26/campaign-test/structure.out` |

## Pins

| id | Scenario | Ordering | Expected outcome |
|---|---|---|---|
| P1 | Sliding wall's open terminus fills a pocket whose faces the bake lit through the opening | The bake lights pocket texels with the wall absent; posed open, the wall covers their lifted centres; posed closed, their segments cross the wall | Covered texels count at the open terminus and are gated closed too, so they are shared |
| P2 | Movers present, no static lightmap lights, or only SDF-shadow ones | The fused walk returns before any partition consumer runs | The summary still prints, movers count as analysed, zero gated rows |
| P3 | Warm build after a mover-only edit | The lightmap-section and ShadowmaskAtlas memos hit inside the lightmap stage and skip reading partitions; the gating memo key changed | Static gating misses its memo and reads each needed partition from the per-light layer cache once, tracing no visibility; the report equals the cold report |
| P4 | Static and animated lights in one report | Static gating finishes inside the walk; animated gating after `AnimatedWeightMaps` | Lines sort by one light order across both tiers, never by arrival or worker |
| P5 | Last waypoint coincides with waypoint 0 (three or more waypoints; parse rejects only consecutive duplicates) | Both termini pose identically | Gated sets equal: all shared, differing-only estimate 0 B |
| P6 | Light inside a posed brush at one terminus only | Termini evaluated independently, inside-test per terminus | Flagged on that terminus's row only; texels lit and ungated at the other terminus are differing |

## Report totals

To be filled at build (Manual AC).

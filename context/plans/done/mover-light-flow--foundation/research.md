# mover-light-flow--foundation — research notes

Read at e3b0772, a6a40a1 and 56ec2e6, 2026-10-30/31.

## Where this sits

- `context/research/gi-rtx-lite/index.md` build order: step 2 (door classification, influence graph, light → cell reach, compile report) sizes step 5 (door channel masks), step 6 (direct-SH door factor), step 7 (slab shadows); step 12 lists the mover rest-pose channel. This brief does step 2 per (mover, terminus) so step 12 is not a second mechanism.
- IP: Take-Two '688/'404 granted claims do not reach door channels (charted 2026-10-30, `ip-check.md` piece 7). Tencent '451's elements are indirect transfer × coefficient on a light change; a direct-only visibility analysis has none of them. This brief is compile-only, IP "None" in the research table.

## Grounding

- Parse rejects waypoint cycles and non-spinning movers resolving to fewer than two waypoints (`resolve_kinematic_path`). A spin axis may be set with zero spin speed and spun later by `set_spin_rate`, which requires a non-zero axis.
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
| P1 | Sliding wall's open terminus fills a pocket whose faces the bake lit through the opening | The bake lights pocket texels with the wall absent, some only by off-centre footprint and emitter samples past the mouth's static corner; posed open, the wall covers their lifted centres; posed closed, most centre segments cross the wall | Covered texels count open. Those whose segment crosses the closed pose are shared. Open-only (differing) texels are those lit with the wall closed: penumbra texels at the mouth's static corner, and texels along the closed wall's shadow edge inside the pocket for lights that see into it obliquely |
| P2 | Movers present with no static lightmap lights, only SDF-shadow ones, or only animated ones | With no static lights the walk returns before any consumer; with only SDF-shadow static lights it runs with no layer lights | The summary still prints and movers count as analysed; animated and probe gating still run |
| P3 | Warm build after a mover-only edit | The lightmap-section and ShadowmaskAtlas memos hit inside the lightmap stage and skip reading partitions; the gating memo key changed | Gating misses its memo and reads each needed partition from the per-light layer cache once, tracing no visibility while those entries are present; a missing entry re-bakes as the ShadowmaskAtlas re-read does; the report equals the cold report |
| P4 | Static and animated lights in one report | Static gating finishes inside the walk; animated gating after `AnimatedWeightMaps` | Lines sort by one light order across both tiers, never by arrival or worker |
| P5 | Last waypoint coincides with waypoint 0 (three or more waypoints; parse rejects only consecutive duplicates) | Both termini pose identically | Gated sets equal: all shared, differing-only estimate 0 B |
| P6 | Light inside a posed brush at one terminus only | Termini evaluated independently, inside-test per terminus | Flagged on that terminus's row only; texels lit and ungated at the other terminus are differing |
| P7 | Edit touches only an animated light | Static gating results are unchanged; animated gating reruns after `AnimatedWeightMaps` | Lines merge in the one light order; report equals cold |
| P8 | Mover edit plus an owner-eligibility edit: section hit, ShadowmaskAtlas miss, gating miss | Both consumers need partitions after the section hit | Each partition is read once and feeds both; report equals cold |

## Centre-segment estimate at shadow edges

Measured on `mover-setpiece`, `setpiece_wall` open, texel pitch 0.025 m. The figures are the farthest open-only texel from the pocket's mouth plane.

| Light | Open-only texels | Farthest from mouth |
|---|---|---|
| (640, 320, 192), on the mouth plane | 464 | 1.20 pitches |
| spot (368, 560, 288) | 477 | 19.48 pitches |
| (240, 1376, 256), west back room | 504 | 44.48 pitches |

- **The mouth-plane light.** Its strip is the static-edge bias: the bake lit those texels only through off-centre samples.
- **The other two lights.** Their strips run along the closed wall's shadow edge inside the pocket: lit with the wall closed, covered with it open. They are real per-state differences.
- **Bias.** One centre segment per texel misclassifies texels at soft shadow edges. That's acceptable for sizing; the mask bake must use the bake's own samples per state (roadmap).

The owner dropped the brief's original one-texel-column bound on 2026-11-02. It encoded where this map's lights stand, not a property of the analysis.

## Report totals

The always-printed "Mover lighting:" block from cold (`--no-cache`) branch builds at 163570e72, 2026-11-02/03. Mask estimates are bytes of the id 22/25/42 lightmap family.

| Map | Movers analysed / not | Rows | Gated texels (shared / differing) | Gated probes (shared / differing) | Side count (carried dynamic / other) | Every-gated mask est. | Differing-only mask est. | Lightmap family |
|---|---|---|---|---|---|---|---|---|
| mover-setpiece | 2 / 0 | 14 | 3,373,746 (1,221,040 / 2,152,706) | 6,443 (1,000 / 5,443) | 1 / 0 | 3,374,282 B (10.58%) | 2,153,066 B (6.75%) | 31,879,968 B |
| stress-warren-showcase | 8 / 0 | 44 | 3,230,293 (9,718 / 3,220,575) | 4,176 (0 / 4,176) | 0 / 47 | 3,231,333 B (0.20%) | 3,221,239 B (0.20%) | 1,646,848,816 B |
| kinematic-platform | 5 / 1 | 25 | 478,392 (0 / 478,392) | 123 (0 / 123) | 3 / 3 | 478,704 B (0.21%) | 478,704 B (0.21%) | 233,443,492 B |
| campaign-test | 3 / 0 | 5 | 293,162 (50,226 / 242,936) | 621 (64 / 557) | 0 / 22 | 293,234 B (0.12%) | 242,992 B (0.10%) | 250,405,204 B |
| stress-warren-hallway-inspection | 11 / 0 | 78 | 18,032,411 (3,129,412 / 14,902,999) | 56,672 (11,040 / 45,632) | 1 / 0 | 18,034,491 B (0.68%) | 14,904,911 B (0.57%) | 2,636,264,704 B |

No map has a baked light left unanalysed. The hallway block is identical across its three branch builds (two timing runs and the gate-breakdown run).

**Step-5 read (per-state masks).** Only `mover-setpiece` is a material fraction of its lightmap family (6.75% differing-only, 10.58% every-gated); the large maps sit at or under 0.7%, so per-state masks are affordable if the estimate is sound. It is biased in both directions: one centre segment per texel runs low on soft-emitter penumbrae and can count edge texels lit only by off-centre samples (see "Centre-segment estimate at shadow edges"), so the differing-only figures are sizing guides, not bounds.

### Row 21: compile-time cost on `stress-warren-hallway-inspection`

Cold `--no-cache --no-tui`, default `--jobs`, `/usr/bin/time -l` under `caffeinate -dimsu`, release binaries, one run at a time, no `-v`. Machine: Intel Core i9-9980HK, 8 cores / 16 logical. Parent is `af1aab9ef` (the merge-base; source equals main), branch is 163570e72. Both used the same `scripts-build` beside the binary. Before each timed run, `pgrep` showed no cargo, rustc or prl-build process, and the one-minute load average was 1.9 to 2.0 (a settle gate of < 2.0 the driver enforced; the pilot, p1, started at 7.6 because it followed the failed first attempt's 34 s burst, so it is shown separately and left out of the means). Unit: seconds, peak RSS in GB (decimal, bytes from `time -l`).

| Run | Binary | Lightmap Bake | Total | Peak RSS |
|---|---|---|---|---|
| p1 (pilot) | parent | 565.17 | 796.28 | 8.83 |
| 1 | parent | 561.02 | 786.49 | 8.36 |
| 2 | branch | 572.27 | 802.94 | 8.08 |
| 3 | parent | 564.15 | 790.20 | 8.69 |
| 4 | branch | 571.97 | 803.27 | 7.85 |

- **Lightmap Bake.** Parent mean 562.59 s (spread 3.13 s, 0.56%; with the pilot, 563.45 s over three runs, spread 4.15 s). Branch mean 572.12 s (spread 0.30 s). Delta +9.53 s, +1.69%.
- **Outside Lightmap Bake.** The extra branch run with `RUST_LOG=info,prl_build::pipeline::lightmap_stage=debug,prl_build::pipeline::mover_light_stage=debug` (not a timing row; Lightmap Bake 575.64 s) printed: probe tier 491.0 ms over 2,212,901 probes, animated tier 5.9 ms, static tier 3,851.2 ms summed over workers (CPU time inside the walk, about 0.24 s of wall across 16 workers).
- **Whole cost.** 9.53 + 0.49 + 0.006 = 10.03 s, or 1.78% of parent Lightmap Bake against the 3% budget (16.88 s). Pairing the extremes (slowest branch against fastest parent) gives 11.75 s, 2.09%.
- **Reading.** Passes. The parent's run-to-run spread (0.56%) is well under the 3% budget, so the margin is real at n = 2 per binary. The static tier's wall share is far smaller than the observed delta, so most of the +9.5 s is not the gate's own passes; the debug lines do not say where it lands (partition retention, extra memo work in the walk, or drift).
- **Peak RSS.** No growth: parent mean 8.52 GB (8.36, 8.69; pilot 8.83), branch mean 7.96 GB (8.08, 7.85; gate run 8.22), a 0.56 GB drop, larger than the parent pair's 0.33 GB spread but with n = 2 and no mechanism for a saving. Read as "no growth observed", not as a saving.

# Research: spatial-residency--lightmap-bounded-pool

Re-anchored 2026-11-06 at `6a813bdcd`, after the SH brief landed; no lightmap, shadowmask or pool source changed between. Measured 2026-11-05 at `cb3934294` on the PRLs in `content/dev/maps` (hallway baked 2026-11-01: 1,840 blocks, 7,881 cells). Raw outputs were scratch and are not kept; rerun with the commands below.

## Pool anatomy

| Plane | Format | MiB per layer |
|---|---|---|
| Irradiance | BC6H, 2048² | 4 |
| Direction | Rg8, 1024² | 2 |
| Shadowmask | BC5, 4096 × 2048, groups side by side | 8 |

Metal trace on the hallway: lightmap pool 490 MiB, of 3,011 MiB steady before the SH brief and 2,170 after. Other runs ended at 546 MiB.

## Density

Commit `362bf5ef5` (2026-10-27) moved the default from 0.04 to 0.025 m/texel, 2.56x the texels. The hallway sets no `_lightmap_density`. Its worst mandatory set rose from 136.5 MiB (`done/spatial-residency--lightmap-cell-blocks/findings.md`) to 348 MiB, 2.55x. Density probably explains that, inferred from the ratio: `occluder-first-partition` also changed the cell count in that window.

Arithmetic, not measurement: at 0.04 the hallway would sit near 1,700 MiB steady and 1,940 MiB peak after the SH brief alone. The owner kept 0.025 on quality.

## Low tier

Low allows 256 MiB for lightmap-shaped data. Hallway at L = 16 m, exact fit, split pools: 220 MiB, plus animated atlas 36 and weight maps 35.4, about 291 MiB. At L = 4 m, about 269 MiB. The add-ons are 0.04 figures and likely understate. So the hallway is not Low-valid at this density at any measured lead.

## All-zero planes

Encoded plane bytes, read from id 42. All-zero BC5 decodes to zero, and all-zero blocks were the only constant blocks found.

| Map | Blocks | Id 22 MiB | Group A MiB (zero, blocks) | Group B MiB (zero, blocks) |
|---|---|---|---|---|
| Hallway | 1,840 | 1,038.4 | 692.3 (459.3, 1,151) | 692.3 (687.3, 1,833) |
| `campaign-test` | 137 | 65.4 | 43.6 (19.7, 69) | 43.6 (36.4, 127) |
| `occlusion-test` | 117 | 37.1 | 24.8 (17.3, 95) | 24.8 (24.8, 117) |

On the hallway, 83% of mask bytes are zero: about 1.1 GB of PRL, and nearly half of lightmap and mask reads.

## Worst mandatory demand

Every camera cell, id 51 entries within L, blocks packed tallest first with a port of the shelf allocator. "Today" is one pool at 14 MiB per layer plus the spare. "Split" is lightmap layers at 6 MiB plus mask planes at 4 MiB, each with a spare. Both are exact fits, before any margin.

| Map | L (m) | Content max MiB | Without zero planes | Layers p50 / p95 / max | Today MiB | Split MiB |
|---|---|---|---|---|---|---|
| Hallway | 4 | 308 | 156 | 8 / 17 / 24 | 350 | 198 |
| Hallway | 8 | 318 | 161 | 9 / 18 / 25 | 364 | 208 |
| Hallway | 16 | 348 | 178 | 12 / 20 / 27 | 392 | 220 |
| Hallway | 32 | 394 | 225 | 18 / 26 / 30 | 434 | 254 |
| `campaign-test` | 16 | 115 | 77 | 7 / 10 / 10 | 154 | 106 |
| `occlusion-test` | 16 | 86 | 44 | 7 / 7 / 7 | 112 | 64 |

- Lead barely moves the worst cell: most of it is what the camera already sees.
- MaxRects on the twelve worst cells matched the shelf allocator within a layer either way.
- 348 MiB is about 25 layers of content in 27: from scratch, packing is near 92%.

## Walks

`lightmap_residency_walks_from_prl`, hallway, 2,000 steps, `POSTRETRO_LIGHTMAP_WALK_READS=completed`, real `LightmapPoolModel`, no GPU. Cap was varied by editing the sweep list; a cap above today's default behaves as a pool sized once. Today's single-pool layout, so these bound the fragmentation problem and do not measure the split. Miss counts compare setups; they do not predict play.

Far-point tour, 97,610 frames, mandatory max 344.7 MiB:

| L (m) | First layers | Peak layers | Peak held MiB | Growths | Repacks | Reads per step MiB | Hit rate | Missed block-frames |
|---|---|---|---|---|---|---|---|---|
| 16 | 15 | 37 | 1,050 | 6 | 20 | 53.9 | 62.0% | 311 |
| 16 | 26 | 36 | 1,008 | 3 | 22 | 36.7 | 81.1% | 93 |
| 16 | 28 | 36 | 1,022 | 4 | 26 | 34.1 | 83.8% | 60 |
| 16 | 30 | 37 | 1,050 | 3 | 17 | 32.2 | 85.9% | 54 |
| 16 | 32 | 37 | 1,050 | 4 | 18 | 30.3 | 87.6% | 53 |
| 16 | 34 | 37 | 1,050 | 3 | 19 | 28.3 | 89.2% | 29 |
| 16 | 36 | 39 | 1,106 | 3 | 5 | 27.3 | 89.9% | 27 |
| 16 | 38 | 38 | 546 | 0 | 20 | 26.3 | 90.6% | 26 |
| 8 | 15 | 34 | 924 | 4 | 13 | 63.0 | 73.5% | 650 |
| 0 | 15 | 32 | 910 | 5 | 25 | 67.3 | 76.1% | 895 |
| 32 | 15 | 43 | 1,218 | 9 | 29 | 25.8 | 0% | 28 |

Random walk, 90,767 frames, mandatory max 288.0 MiB:

| L (m) | First layers | Peak layers | Peak held MiB | Growths | Reads per step MiB | Missed block-frames |
|---|---|---|---|---|---|---|
| 16 | 15 | 32 | 910 | 4 | 40.1 | 23 |
| 16 | 30 | 31 | 882 | 1 | 23.8 | 0 |
| 16 | 32 | 32 | 462 | 0 | 22.9 | 0 |
| 16 | 38 | 38 | 546 | 0 | 21.9 | 0 |
| 8 | 15 | 28 | 798 | 4 | 44.5 | 165 |

- At L = 32 m nothing is left in the band, so hit rate reads 0% by definition.
- Sizing once removes the peak and halves reads. Band retention, not mandatory demand, buys the read saving.
- 34 and 36 layers grew on the tour and 38 did not, and 36 ended higher than 38. Downward-only repack fails on route order, so a calibrated margin does not generalise. `GROWTH_HEADROOM` of 4/3 gives 36 from 27.
- The tour needs 37 to 39 layers for a set that packs into 27 to 28 from scratch: about 150 MiB of fragmentation at 14 MiB per layer.

## Margin

Compaction removes fragmentation from the margin's job. What remains is unmodelled drawn demand, planar mirrors included, and band retention. From the tour table: misses fall from 53 at 32 layers to 29 at 34, then 26 at 38, and reads flatten near 1 MiB per step per layer. 34 is 5/4 of the 27-layer bound. One route on today's single pool, with growth in those runs, so the split-pool walks confirm it.

- `campaign-test`: 5/4 of its 10-layer bound is 13, its all-resident size tallest first, so its capacity covers the whole level. Block-id order needs 15, so this rests on R24. Band pairs do not repack, so 13 layers can refuse 2 to 10 of its 137 blocks in arrival order (R33).
- The value is not tuned finely: the hallway is a ceiling test.

Block table, unverified: a plane address needs about 26 bits at 4-texel alignment (layer 8, offset 9 + 9). Offset, extent, layer and flags fit the other two words, so one `vec4` per block may still hold everything.

## Planar mirrors

Same harness, 2,000 steps, `POSTRETRO_LIGHTMAP_WALK_PLANAR` off and on, default levers.

| Map, walk | Mirror frames | Mirror-only cells, max per frame | Blocks outside id 51 (block-frames, max per frame) | Mandatory, resident, pool, growths |
|---|---|---|---|---|
| `reflection-lake-test`, random | 52,581 of 89,005 | 55 | 1,429, 3 | Unchanged by the mirror |
| `reflection-lake-test`, tour | 56,117 of 91,572 | 53 | 2,297, 3 | Unchanged |
| `campaign-test`, random | 8,541 of 93,326 | 30 | 23, 2 | Unchanged |
| `campaign-test`, tour | 19,355 of 97,308 | 31 | 75, 2 | Unchanged |

No drawn block was non-resident. These walks show very little. A mirror adds only Visible demand, so mandatory cannot move. At this brief's sizing both maps fit whole, so growth cannot fire on them either: `reflection-lake-test`'s worst cell is all 172 blocks, and `campaign-test` has bound 10 against all-resident 13. The reason mirrors stay out is the tier cost, not these numbers.

A large water map's mirror could reach well past id 51 at a planar tier. No such map exists, so that case is unmeasured. Adding mirror cells later needs the sweep (a crate move and a cached stage), and a second stored figure if the default tier is to stay free of them: id 58 holds one figure per pool. SH's bound also leaves mirrors out (`plan.md` on its branch).

Mirror cells also join SH visible targeting on a completed walk with the main view on a walk path (`sh_visible_cells`), a mandatory tier, so a mirror can raise SH demand too.

Crate edges, normal dependencies only: compiler → level-format, level-loader, visibility, render-data. Render-cpu → visibility, level-loader, level-format, entities, model, render-data. Visibility → level-loader, stage-timing. Grouping sits in render-cpu; the seeded walk in visibility; pinned copies of the fog band and clip bias in visibility.

## Threshold and figures

- 384 MiB: 2,048 target − 512 SH − 427 shadows − about 693 other (video memory, measured after the SH brief) ≈ 416, rounded down to a multiple of 64 MiB. Expect to revisit it as real content arrives.
- 532 MiB: 37 layers plus the spare at 14 MiB, the far-point tour at today's defaults. Metal runs ended at 490 and 546 MiB.

## After the SH brief

`done/spatial-residency--sh-bounded-pool/plan.md`, hallway, Metal Resource Events. Which column the spike read is unconfirmed.

| Figure | `resource-size` MiB | Video memory only MiB |
|---|---|---|
| Steady | 2,170 | 2,009 |
| Peak, at 27 s | 2,574 | 2,415 |

- SH pool 399, no SH growth. The peak is lightmap growth: planes 116/58/232 to 140/70/280 MiB, old and new alive together.
- Lightmap pool in that walk: 490 MiB. Everything else, shadows aside: about 854 in `resource-size`, about 693 in video memory, against the 650 first estimated.
- SH sizes at 25% headroom too, so 5/4 matches it.
- A whole-map rule landed for SH: a pool holding the whole map names no victims. Uniform slots leave no holes there. Lightmap blocks are not uniform (R33).
- A fixed all-resident layout for whole-map lightmap pools was considered and declined: levels that small are test fixtures, not realistic content, so R33's ordinary misses stand. Stress warrens are the tool for coping with scope, though they are generated without art direction, and no map yet represents real content.

## Projection

Estimates, not measurements.

| Item | MiB |
|---|---|
| Split pools, exact fit at L = 16 m | 220 |
| Split pools at 5/4: 34 lightmap and 15 mask layers, plus spares | 274 |
| Hallway total, steady and peak: 2,170 − 490 + 274 | 1,954 in `resource-size` |
| Same, video memory only: 2,009 − 490 + 274 | 1,793 |

Both assume no growth, and leave shadows (427) and everything else as measured.

## Limits of this evidence

No map represents real content yet. Warrens engineer for scope, but they do not model realistic spatial relationships: a mix of small, medium and large spaces, straight and L-shaped connectors, or empty void between rooms. So these findings hold for warrens and need rechecking on real content:

- Block-size mix: real rooms would give a wider spread, so packing near 92%, MaxRects matching shelf, and 5/4 as the knee may all shift.
- Lead: "lead barely moves the worst cell" may be a warren trait. Connectors and void could shorten sight lines, so lead may matter more.
- Zero planes: the 83% depends on how lights and owners fall.

The structure does not depend on any of this: the bound is computed per map, and compaction's guarantee holds for any block mix.

## Ordering pins

| Id | Scenario | Ordering | Expected |
|---|---|---|---|
| R1 | Pair needs both pools, one full; or mask pool has room for one of its two planes | Lightmap half and plane A placed, plane B fails | Whole pair rolled back, every rect free; retried whole |
| R2 | Mandatory pair evicts a band block | Only the mask pool lacked room | Victim leaves both pools; its table entry goes non-resident once |
| R3 | Block with both planes absent | Same drain as pairs with masks | Lightmap-only read, upload and placement; never counted as a mask miss |
| R4 | Drain fails after placements in both pools | Failure lands after either pool's writes | Abort: journal restores both pools and every table entry. Failed install: that pair leaves both pools; other moves and growth stand |
| R5 | New generation's first batch | Old residency in both pools | Both freed, not reported evicted; layers and retiring generations stand |
| R6 | Repack with up, down, same-layer moves and a swap | Destination occupied by a block yet to move | Blocker staged in the spare; each source read before it is overwritten |
| R7 | Lightmap pool repacks, mask pool does not | One drain | One submission; mask placements unchanged but for R15; table reflects both |
| R8 | Repack while a generation retires | Retiring copy not yet done | Only the live generation is touched |
| R9 | Both pools grow on one drain | Retirement completes at different times | Each retires alone; a later growth waits only on its own pool |
| R10 | Pair needs room | Downward repack fits the whole targeted demand, then one that does not | First drain moves nothing up; second compacts |
| R11 | Cell change raises demand past one drain's budget | Pairs ready across consecutive drains | First repack reserves for all targets; later drains place without repacking |
| R12 | Settling union exceeds the pool | Growth lands before the reveal | One growth, warned by name; pool keeps its size after Settling |
| R13 | Pair's lightmap half needs growth, mask pool's generation retires | Lightmap half placed past the layers, then mask half deferred | Pair deferred whole; neither pool grows for it |
| R14 | Mask pool repacks, lightmap pool does not | A plane moves while its block's lightmap placement stands | Entry rewritten for every moved plane, in the same submission; no other entry written |
| R15 | Lightmap repack drops a band block | Mask pool does not repack | Block's planes leave the mask pool too; entry goes non-resident once; every other mask placement stands |
| R16 | Pair evicts band victims it may not need | Victim's lightmap rect still free, a mask rect now taken | Victim returns only when all its rects are free; otherwise it leaves both pools. A victim that freed nothing the pair used goes back |
| R17 | Band pair refused, mask pool full, lightmap pool has room | Lightmap room grows, then mask room | Not read again on lightmap room alone; requestable once the pool that refused it has room |
| R18 | Downward repack fits this drain's pairs, not the whole targeted demand | First drain of the demand | Downward pass counts as failed; compaction runs this drain, once |
| R19 | Targeted demand past the pool even compacted; this drain's pairs fit by repack | Targets arrive before their pairs | Pairs place; no growth until a ready pair cannot be placed |
| R20 | Whole targeted demand fits the pool after one repack | Pairs arrive shortest first: (28,36), (8,28), then (4,44) in a 64² layer | Later pairs land in reserved room; no second repack, no growth |
| R21 | Target leaves targeted demand after a repack reserved its room: removed, or dropped to band | Before its pair is ready | Room returns that drain; band may use it; a pair arriving later places as a band pair; an aborted drain restores the reservation |
| R22 | Compaction moves have no order through one spare | Found while planning, before any copy is recorded | Model rolls back to the failed repack; demand takes growth and warns by name; no partial copy list |
| R23 | One pool repacks, the other grows | One drain | One submission; copies land in the repacking pool's live generation, uploads in each pool's live or new generation; only the grown pool retires |
| R24 | Level sized at the all-resident cap | Cap counted in block-id order, bound and repack tallest first; (56,16), (28,44), (24,20) in a 64² layer is 1 layer against 2 | Cap and bound count one order; the pool never sits below the bound, and holds every block with no growth |
| R25 | Controller replaced mid-level on a pool sized past the old default | New controller's first batch | Band limit stays the sized pool; no band block is evicted over a default |
| R26 | Second demand past an already grown pool | After the first generation retired | Pool grows again and warns again; "once" means once per demand |
| R27 | Id 42 present, every plane absent | Bound and all-resident count are zero mask layers | Both modes bind a valid mask texture, place nothing, and read zero, never the no-id-42 fallback |
| R28 | Room reserved, pair not ready; a band pair ready | Reservation made, then band placement and the next frame's draw, all before the pair lands | Block stays a miss: no table write, no copy, no eviction reported. Band never lands in reserved room. The pair uploads and turns resident in the drain it arrives |
| R29 | Room reserved; band reads gate on reported headroom | Reservation made, its pair in hand, then the room returns | Headroom leaves reserved room out, and a pair in hand comes off it once. Return raises headroom, so a band pair refused for that room is requestable |
| R30 | One pool grows alone, a plane or block placed in a grown layer | Growth lands, then the next draw | The draw samples through the new generation; the other pool's binding is unchanged |
| R31 | Both pools repack | One drain, a block moved in both | One submission; each pool's copies land in its own live generation; the block's entry is written once with both new addresses |
| R32 | Targeted demand past the pool even compacted, arriving over several drains | First drain's pairs fit by compaction, a later drain's pairs do not fit as the pool stands | One compaction; the later pair takes growth and warns by name |
| R33 | Level sized at the all-resident cap, band pairs arriving in an order that does not pack | Pool full by placement with texels free; band pairs do not repack | Those band pairs are refused, read again on room, and miss until mandatory; the pool does not grow, and mandatory demand still places |

## Reproduction

```text
# Walks: edit the cap list in walk_measurement/mod.rs (after this brief, the harness sets pool size directly), then
POSTRETRO_LIGHTMAP_WALK_PRL=$PWD/content/dev/maps/stress-warren-hallway-inspection.prl \
POSTRETRO_LIGHTMAP_WALK_STEPS=2000 POSTRETRO_LIGHTMAP_WALK_READS=completed \
  cargo test -p postretro --bin postretro lightmap_residency_walks_from_prl -- --ignored --nocapture
```

The plane scan and demand table came from a throwaway script over the PRL section table, ids 22, 42 and 51. Any rewrite should check it matches 348 MiB content and 1,833 zero group B blocks on this bake.

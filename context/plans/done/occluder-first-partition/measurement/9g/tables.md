
### M1: step-cap trips; poses with new drawn misses / reported culled / faceless

| Map | before | cap19 | cap26 |
|---|---|---|---|
| occlusion-test | 0 trips, 0 drawn / 0 culled / 0 faceless | 0 trips, 0 drawn / 0 culled / 0 faceless | 0 trips, 0 drawn / 0 culled / 0 faceless |
| hallway | 0 trips, 0 drawn / 1 culled / 2 faceless | 0 trips, 0 drawn / 0 culled / 0 faceless | 0 trips, 0 drawn / 0 culled / 0 faceless |
| campaign-test | 0 trips, 0 drawn / 0 culled / 0 faceless | 0 trips, 0 drawn / 0 culled / 0 faceless | 0 trips, 0 drawn / 0 culled / 0 faceless |
| arena | 0 trips, 0 drawn / 13 culled / 0 faceless | 0 trips, 0 drawn / 0 culled / 0 faceless | 0 trips, 0 drawn / 0 culled / 0 faceless |
| street | 518 trips, 0 drawn / 0 culled / 0 faceless | 29 trips, 0 drawn / 9 culled / 0 faceless | 19 trips, 0 drawn / 9 culled / 0 faceless |

### M2: deferred texels, random / far-point / pose tour

| Map | before | cap19 | cap26 |
|---|---|---|---|
| occlusion-test | 0 / 0 / 0 | 0 / 0 / 0 | 0 / 0 / 0 |
| hallway | 0 / 0 / 0 | 0 / 0 / 0 | 0 / 0 / 0 |
| campaign-test | 0 / 0 / 0 | 0 / 0 / 0 | 0 / 0 / 0 |
| arena | 0 / 0 / 0 | 0 / 0 / 0 | 0 / 0 / 0 |
| street | 0 / 0 / 0 | 0 / 0 / 0 | 0 / 0 / 0 |

### M2 beside: left-behind texels, random / far / tour

| Map | before | cap19 | cap26 |
|---|---|---|---|
| occlusion-test | 0.0 M / 0.0 M / 0.0 M | 0.0 M / 0.0 M / 1.2 M | 0.0 M / 0.0 M / 0.0 M |
| hallway | 28.7 M / 38.4 M / 23,233.0 M | 67.9 M / 236.5 M / 23,074.5 M | 219.2 M / 827.9 M / 30,705.1 M |
| campaign-test | 0.0 M / 0.0 M / 157.4 M | 0.0 M / 0.0 M / 121.8 M | 0.0 M / 0.0 M / 84.0 M |
| arena | 0.0 M / 0.0 M / 9.3 M | 0.0 M / 0.0 M / 25.1 M | 0.0 M / 0.0 M / 15.8 M |
| street | 2,020.8 M / 2,686.4 M / 82.5 M | 162.4 M / 221.6 M / 1.8 M | 108.0 M / 92.3 M / 1.8 M |

### M2 beside: trip frames, random / far / tour

| Map | before | cap19 | cap26 |
|---|---|---|---|
| occlusion-test | 0 / 0 / 0 | 0 / 0 / 0 | 0 / 0 / 0 |
| hallway | 0 / 0 / 0 | 0 / 0 / 0 | 0 / 0 / 0 |
| campaign-test | 0 / 0 / 0 | 0 / 0 / 0 | 0 / 0 / 0 |
| arena | 0 / 0 / 0 | 0 / 0 / 0 | 0 / 0 / 0 |
| street | 19,554 / 25,858 / 1,036 | 2,449 / 2,894 / 58 | 1,619 / 1,972 / 38 |

### M2 pool: peak layers (active MiB with the spare layer, 14.0 MiB per layer), random / far / tour

| Map | before | cap19 | cap26 |
|---|---|---|---|
| occlusion-test | 10 (154 MiB) / 10 (154 MiB) / 10 (154 MiB) | 10 (154 MiB) / 10 (154 MiB) / 10 (154 MiB) | 9 (140 MiB) / 9 (140 MiB) / 9 (140 MiB) |
| hallway | 34 (490 MiB) / 38 (546 MiB) / 34 (490 MiB) | 34 (490 MiB) / 36 (518 MiB) / 32 (462 MiB) | 37 (532 MiB) / 44 (630 MiB) / 30 (434 MiB) |
| campaign-test | 15 (224 MiB) / 16 (238 MiB) / 15 (224 MiB) | 15 (224 MiB) / 17 (252 MiB) / 15 (224 MiB) | 14 (210 MiB) / 15 (224 MiB) / 14 (210 MiB) |
| arena | 2 (42 MiB) / 2 (42 MiB) / 2 (42 MiB) | 2 (42 MiB) / 2 (42 MiB) / 2 (42 MiB) | 2 (42 MiB) / 2 (42 MiB) / 2 (42 MiB) |
| street | 2 (42 MiB) / 2 (42 MiB) / 2 (42 MiB) | 2 (42 MiB) / 3 (56 MiB) / 2 (42 MiB) | 2 (42 MiB) / 2 (42 MiB) / 2 (42 MiB) |

### M2 pool: headroom cost at peak, layers (MiB), random / far / tour

| Map | before | cap19 | cap26 |
|---|---|---|---|
| occlusion-test | 1 (14.0 MiB) / 1 (14.0 MiB) / 1 (14.0 MiB) | 1 (14.0 MiB) / 1 (14.0 MiB) / 1 (14.0 MiB) | 0 (0.0 MiB) / 0 (0.0 MiB) / 0 (0.0 MiB) |
| hallway | 0 (0.0 MiB) / 0 (0.0 MiB) / 7 (98.0 MiB) | 0 (0.0 MiB) / 0 (0.0 MiB) / 2 (28.0 MiB) | 0 (0.0 MiB) / 0 (0.0 MiB) / 2 (28.0 MiB) |
| campaign-test | 0 (0.0 MiB) / 0 (0.0 MiB) / 0 (0.0 MiB) | 0 (0.0 MiB) / 0 (0.0 MiB) / 0 (0.0 MiB) | 0 (0.0 MiB) / 0 (0.0 MiB) / 0 (0.0 MiB) |
| arena | 0 (0.0 MiB) / 0 (0.0 MiB) / 0 (0.0 MiB) | 0 (0.0 MiB) / 0 (0.0 MiB) / 1 (14.0 MiB) | 1 (14.0 MiB) / 0 (0.0 MiB) / 1 (14.0 MiB) |
| street | 1 (14.0 MiB) / 1 (14.0 MiB) / 1 (14.0 MiB) | 0 (0.0 MiB) / 0 (0.0 MiB) / 0 (0.0 MiB) | 0 (0.0 MiB) / 0 (0.0 MiB) / 0 (0.0 MiB) |

### M2 pool: first → peak layers, growths, random / far / tour

| Map | before | cap19 | cap26 |
|---|---|---|---|
| occlusion-test | 10→10, 0 / 10→10, 0 / 10→10, 0 | 10→10, 0 / 10→10, 0 / 10→10, 0 | 9→9, 0 / 9→9, 0 / 9→9, 0 |
| hallway | 15→34, 12 / 15→38, 6 / 15→34, 3 | 15→34, 6 / 15→36, 6 / 15→32, 2 | 15→37, 7 / 15→44, 10 / 15→30, 2 |
| campaign-test | 15→15, 0 / 15→16, 1 / 15→15, 0 | 15→15, 0 / 15→17, 2 / 15→15, 0 | 14→14, 0 / 14→15, 1 / 14→14, 0 |
| arena | 2→2, 0 / 2→2, 0 / 2→2, 0 | 2→2, 0 / 2→2, 0 / 2→2, 0 | 2→2, 0 / 2→2, 0 / 2→2, 0 |
| street | 2→2, 0 / 2→2, 0 / 2→2, 0 | 2→2, 0 / 2→3, 1 / 2→2, 0 | 2→2, 0 / 2→2, 0 / 2→2, 0 |

### M3: seal lines per mover

| Map | before | cap19 | cap26 |
|---|---|---|---|
| occlusion-test | none | none | none |
| hallway | m0 5, m1 4, m2 2, m3 2, m4 5, m5 2, m6 1, m7 1, m8 1 | m0 6, m1 4, m2 4, m3 4, m4 6, m5 4, m6 2, m7 2, m8 2 | m0 6, m1 4, m2 5, m3 4, m4 6, m5 4, m6 2, m7 2, m8 2 |
| campaign-test | m0 2 | m0 5, m1 5 | m0 5, m1 5 |
| arena | m0 1, m1 1, m2 4, m3 4, m4 1, m5 4 | m0 3, m1 3, m2 4, m3 4, m4 3, m5 5 | m0 3, m1 3, m2 4, m3 4, m4 3, m5 5 |
| street | none | none | none |

### M4: leak; exterior / interior empty leaves (full tree); open edges

| Map | before | cap19 | cap26 |
|---|---|---|---|
| occlusion-test | none; ext/int leaves 97 / 287; open edges 0 | none; ext/int leaves 122 / 438; open edges 0 | none; ext/int leaves 122 / 416; open edges 0 |
| hallway | none; ext/int leaves 411 / 2082; open edges 283 | none; ext/int leaves 416 / 3664; open edges 568 | none; ext/int leaves 416 / 3259; open edges 563 |
| campaign-test | none; ext/int leaves 114 / 207; open edges 9 | none; ext/int leaves 241 / 416; open edges 37 | none; ext/int leaves 241 / 343; open edges 43 |
| arena | none; ext/int leaves 228 / 560; open edges 24 | none; ext/int leaves 207 / 1014; open edges 51 | none; ext/int leaves 206 / 821; open edges 24 |
| street | none; ext/int leaves 177 / 1985; open edges 396 | none; ext/int leaves 307 / 3092; open edges 624 | none; ext/int leaves 307 / 2917; open edges 591 |

### M5: occluders (occlude alone; sections occluding)

| Map | before | cap19 | cap26 |
|---|---|---|---|
| occlusion-test | – | 29 alone; 5 of 11 sections (17 brushes); len 20 | 29 alone; 5 of 11 sections (17 brushes); len 20 |
| hallway | – | 738 alone; 12 of 99 sections (51 brushes); len 20 | 738 alone; 12 of 99 sections (51 brushes); len 20 |
| campaign-test | – | 40 alone; 7 of 15 sections (21 brushes); len 20 | 40 alone; 7 of 15 sections (21 brushes); len 20 |
| arena | – | 237 alone; 2 of 11 sections (10 brushes); len 20 | 237 alone; 2 of 11 sections (10 brushes); len 20 |
| street | – | 72 alone; 13 of 37 sections (1070 brushes); len 20 | 72 alone; 13 of 37 sections (1070 brushes); len 20 |

### M5: occluder-tier leaves

| Map | before | cap19 | cap26 |
|---|---|---|---|
| occlusion-test | – | 332 | 313 |
| hallway | – | 5,430 | 5,110 |
| campaign-test | – | 616 | 559 |
| arena | – | 1,296 | 1,131 |
| street | – | 2,067 | 1,896 |

### M5: cells (interior / solid / exterior)

| Map | before | cap19 | cap26 |
|---|---|---|---|
| occlusion-test | 555 (287/171/97) | 505 (126/257/122) | 484 (107/255/122) |
| hallway | 5,671 (2,082/3,178/411) | 7,846 (1,812/5,618/416) | 7,439 (1,490/5,533/416) |
| campaign-test | 487 (207/166/114) | 911 (186/484/241) | 832 (129/462/241) |
| arena | 1,479 (560/691/228) | 1,576 (556/813/207) | 1,389 (391/792/206) |
| street | 3,709 (1,985/1,547/177) | 4,223 (1,021/2,895/307) | 4,034 (850/2,877/307) |

### M5: runtime walkable portals

| Map | before | cap19 | cap26 |
|---|---|---|---|
| occlusion-test | 693 | 291 | 241 |
| hallway | 4,722 | 2,950 | 2,368 |
| campaign-test | 524 | 439 | 288 |
| arena | 1,333 | 912 | 608 |
| street | 6,288 | 4,248 | 3,755 |

### M5: PRL faces = lightmap charts / triangles (extra charts from atlas sub-chart cuts, already in the face count)

| Map | before | cap19 | cap26 |
|---|---|---|---|
| occlusion-test | 1,018 / 2,036 (0 cut) | 1,074 / 2,148 (0 cut) | 1,057 / 2,114 (0 cut) |
| hallway | 8,457 / 16,910 (12 cut) | 11,841 / 23,546 (0 cut) | 11,370 / 22,594 (0 cut) |
| campaign-test | 808 / 1,612 (0 cut) | 1,135 / 2,265 (0 cut) | 1,022 / 2,038 (0 cut) |
| arena | 2,239 / 4,478 (0 cut) | 3,294 / 6,587 (0 cut) | 2,899 / 5,798 (0 cut) |
| street | 7,657 / 15,315 (0 cut) | 10,591 / 21,181 (0 cut) | 10,441 / 20,881 (0 cut) |

### M5: interior cell largest extent p50 / p95 / max (m)

| Map | before | cap19 | cap26 |
|---|---|---|---|
| occlusion-test | 13.0 / 21.5 / 26.0 | 13.2 / 21.9 / 25.2 | 17.1 / 26.0 / 26.6 |
| hallway | 10.2 / 26.0 / 52.0 | 13.0 / 21.1 / 39.0 | 9.8 / 27.6 / 53.6 |
| campaign-test | 9.8 / 22.8 / 35.8 | 13.8 / 20.3 / 24.8 | 15.8 / 26.0 / 26.0 |
| arena | 16.3 / 29.3 / 94.3 | 16.3 / 22.8 / 39.0 | 26.0 / 26.0 / 58.5 |
| street | 5.8 / 24.4 / 88.0 | 17.4 / 23.0 / 49.0 | 16.0 / 27.7 / 62.0 |

### M5: interior cells with smallest extent 0.25-0.5 m

| Map | before | cap19 | cap26 |
|---|---|---|---|
| occlusion-test | 22 | 48 | 42 |
| hallway | 49 | 2 | 2 |
| campaign-test | 33 | 88 | 66 |
| arena | 5 | 0 | 0 |
| street | 769 | 460 | 395 |

### M5: SH clusters (indivisible oversize)

| Map | before | cap19 | cap26 |
|---|---|---|---|
| occlusion-test | 197 (0) | 394 (2) | 390 (2) |
| hallway | 3,386 (0) | 6,297 (3) | 6,207 (3) |
| campaign-test | 188 (0) | 750 (1) | 721 (2) |
| arena | 752 (0) | 1,082 (1) | 1,055 (2) |
| street | 1,915 (0) | 3,378 (26) | 3,352 (28) |

### id-51 lightmap mandatory set at L 16 m, max per camera cell (texels)

| Map | before | cap19 | cap26 |
|---|---|---|---|
| occlusion-test | 30.6 M (p95 30.4; 0 over 62.9) | 24.1 M (p95 24.0; 0 over 62.9) | 24.3 M (p95 24.1; 0 over 62.9) |
| hallway | 97.3 M (p95 57.9; 177 over 62.9) | 102.7 M (p95 65.0; 455 over 62.9) | 116.3 M (p95 76.8; 714 over 62.9) |
| campaign-test | 37.3 M (p95 34.4; 0 over 62.9) | 35.5 M (p95 27.6; 0 over 62.9) | 34.7 M (p95 32.8; 0 over 62.9) |
| arena | 1.2 M (p95 0.7; 0 over 62.9) | 1.2 M (p95 0.8; 0 over 62.9) | 1.2 M (p95 0.8; 0 over 62.9) |
| street | 2.8 M (p95 2.7; 0 over 62.9) | 2.4 M (p95 2.4; 0 over 62.9) | 2.4 M (p95 2.4; 0 over 62.9) |

### M6: portals tested, mean / p95 per pose

| Map | before | cap19 | cap26 |
|---|---|---|---|
| occlusion-test | 464.3 / 1,749 | 209.4 / 582 | 179.6 / 467 |
| hallway | 65.3 / 196 | 64.6 / 357 | 53.3 / 279 |
| campaign-test | 108.1 / 296 | 100.7 / 338 | 74.4 / 250 |
| arena | 58.8 / 202 | 46.2 / 216 | 33.5 / 160 |
| street | 2,042.6 / 5,000 | 1,352.4 / 3,787 | 1,271.0 / 3,560 |

### M6: drawn cells, mean / p95 per pose

| Map | before | cap19 | cap26 |
|---|---|---|---|
| occlusion-test | 66.6 / 242 | 31.3 / 79 | 27.8 / 65 |
| hallway | 11.9 / 33 | 13.7 / 60 | 11.5 / 49 |
| campaign-test | 15.9 / 41 | 15.7 / 46 | 12.9 / 39 |
| arena | 9.6 / 28 | 11.3 / 40 | 8.6 / 30 |
| street | 272.1 / 1,386 | 130.8 / 376 | 114.5 / 324 |

### M6: tris in frustum, mean / p95 per pose

| Map | before | cap19 | cap26 |
|---|---|---|---|
| occlusion-test | 385.9 / 1,516 | 401.7 / 1,514 | 398.3 / 1,472 |
| hallway | 80.3 / 230 | 134.5 / 500 | 133.3 / 502 |
| campaign-test | 101.1 / 283 | 177.8 / 552 | 175.1 / 574 |
| arena | 65.4 / 200 | 103.0 / 354 | 96.7 / 328 |
| street | 1,939.3 / 10,297 | 2,044.4 / 6,213 | 2,046.0 / 6,282 |

### M6: drawn lightmap texels, mean / p95 per pose

| Map | before | cap19 | cap26 |
|---|---|---|---|
| occlusion-test | 9.41 M / 24.89 M | 7.22 M / 16.53 M | 7.74 M / 16.83 M |
| hallway | 6.15 M / 24.04 M | 5.94 M / 19.30 M | 6.94 M / 22.86 M |
| campaign-test | 5.66 M / 14.03 M | 5.12 M / 11.94 M | 6.23 M / 13.39 M |
| arena | 0.12 M / 0.52 M | 0.09 M / 0.26 M | 0.10 M / 0.30 M |
| street | 0.64 M / 2.31 M | 0.35 M / 0.94 M | 0.39 M / 1.04 M |

### M6: portals tested, max per pose (a tripped pose stops at 5,000)

| Map | before | cap19 | cap26 |
|---|---|---|---|
| occlusion-test | 2,084 (0 trips) | 731 (0 trips) | 596 (0 trips) |
| hallway | 998 (0 trips) | 952 (0 trips) | 755 (0 trips) |
| campaign-test | 524 (0 trips) | 465 (0 trips) | 378 (0 trips) |
| arena | 686 (0 trips) | 545 (0 trips) | 371 (0 trips) |
| street | 5,000 (518 trips) | 5,000 (29 trips) | 5,000 (19 trips) |

### Guardrail: mean portals tested per pose against before

| Map | before | cap19 | cap26 |
|---|---|---|---|
| occlusion-test | +0% | -55% | -61% |
| hallway | +0% | -1% | -18% |
| campaign-test | +0% | -7% | -31% |
| arena | +0% | -21% | -43% |
| street | +0% | -34% | -38% |

### Guardrail: mean tris in frustum per pose against before

| Map | before | cap19 | cap26 |
|---|---|---|---|
| occlusion-test | +0% | +4% | +3% |
| hallway | +0% | +67% | +66% |
| campaign-test | +0% | +76% | +73% |
| arena | +0% | +57% | +48% |
| street | +0% | +5% | +6% |

### Guardrail: mean drawn lightmap texels per pose against before

| Map | before | cap19 | cap26 |
|---|---|---|---|
| occlusion-test | +0% | -23% | -18% |
| hallway | +0% | -3% | +13% |
| campaign-test | +0% | -9% | +10% |
| arena | +0% | -23% | -12% |
| street | +0% | -46% | -40% |

### SH mandatory clusters per camera cell at L 16 m (owner closure included), median / p95 / max

| Map | before | cap19 | cap26 |
|---|---|---|---|
| occlusion-test | 21 / 22 / 22 | 16 / 16 / 16 | 14 / 14 / 14 |
| hallway | 83 / 100 / 105 | 122 / 141 / 150 | 119 / 136 / 143 |
| campaign-test | 7 / 14 / 15 | 14 / 21 / 22 | 11 / 16 / 16 |
| arena | 12 / 18 / 21 | 17 / 26 / 31 | 14 / 22 / 24 |
| street | 326 / 358 / 362 | 168 / 174 / 175 | 161 / 167 / 168 |

### SH probe reach into another cluster's cells (scratch reader)

| Map | before | cap19 | cap26 |
|---|---|---|---|
| occlusion-test | 158 pairs, 48,770 refs; owner edges 52 | – | – |
| hallway | 2,363 pairs, 60,903 refs; owner edges 930 | – | – |
| campaign-test | 45 pairs, 27,053 refs; owner edges 19 | – | – |
| arena | 537 pairs, 4,849 refs; owner edges 125 | – | – |
| street | 7,917 pairs, 85,920 refs; owner edges 949 | – | – |

### M7: arena box

| before | cap19 | cap26 |
|---|---|---|
| 58 cells, 117 portals on 40 planes | 112 cells, 253 portals on 16 planes | 74 cells, 166 portals on 12 planes |

### M7: stairs box

| before | cap19 | cap26 |
|---|---|---|
| 32 cells, 58 portals on 25 planes; riser planes 23, tread planes 0 | 6 cells, 7 portals on 3 planes; riser planes 0, tread planes 1 (11.379) | 6 cells, 7 portals on 3 planes; riser planes 0, tread planes 1 (11.379) |

### Street step-cap trips: total / also tripped before / new

| Set | before | cap19 | cap26 |
|---|---|---|---|
| street level (2,960 poses) | 518 / 518 / 0 | 29 / 29 / 0 | 19 / 19 / 0 |
| rooftop (608 poses) | 212 / 212 / 0 | 34 / 32 / 2 | 22 / 21 / 1 |

### Rooftop set (stress-street, 608 poses)

| Row | before | cap19 | cap26 |
|---|---|---|---|
| step-cap trips | 212 | 34 | 22 |
| new drawn-miss poses | 0 | 0 | 0 |
| culled / faceless (reported) | 0 / 0 | 0 / 0 | 0 / 0 |
| portals tested mean / p95 | 2,789.4 / 5,000 | 2,021.7 / 5,000 | 1,897.6 / 4,552 |
| drawn cells mean / p95 | 426.7 / 1,409 | 197.2 / 502 | 170.4 / 410 |
| tris in frustum mean / p95 | 3,035.3 / 10,847 | 3,140.1 / 10,060 | 3,077.3 / 8,162 |
| drawn lightmap texels mean / p95 | 0.86 M / 2.22 M | 0.52 M / 1.21 M | 0.55 M / 1.23 M |
| paths | ("portal", 396), ("step_limit", 212) | ("portal", 574), ("step_limit", 34) | ("portal", 586), ("step_limit", 22) |

### Uncapped portal tests per pose, street level (2,960 poses)

| Row | before | cap19 | cap26 |
|---|---|---|---|
| mean | 2,553.7 | 1,359.4 | 1,273.9 |
| p50 / p95 / p99 | 1,409 / 8,891 / 12,352 | 1,021 / 3,787 / 4,998 | 1,017 / 3,560 / 4,507 |
| max (worst pose) | 18,513 (`g5.0.0@180`) | 6,862 (`g6.3.0@180`) | 6,068 (`g5.3.0@180`) |
| poses over 5,000 | 518 | 29 | 19 |
| 2× worst walk | 37,026 | 13,724 | 12,136 |
| total tests | 7,558,805 | 4,023,860 | 3,770,785 |

### Uncapped portal tests per pose, rooftop (608 poses)

| Row | before | cap19 | cap26 |
|---|---|---|---|
| mean | 4,354.5 | 2,063.3 | 1,913.8 |
| p50 / p95 / p99 | 2,671 / 13,007 / 16,459 | 1,871 / 5,105 / 6,247 | 1,731 / 4,552 / 5,523 |
| max (worst pose) | 17,598 (`roof:A7.s2.c@225/-15`) | 7,242 (`roof:B4.s2.c@180/-15`) | 6,234 (`roof:B4.s2.c@180/-15`) |
| poses over 5,000 | 212 | 34 | 22 |
| 2× worst walk | 35,196 | 14,484 | 12,468 |
| total tests | 2,647,511 | 1,254,497 | 1,163,607 |

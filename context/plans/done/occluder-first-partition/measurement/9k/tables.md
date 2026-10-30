
### M1: step-cap trips; poses with new drawn misses / reported culled / faceless

| Map | before | 9g-cap26 | cap26-noprism | cap26 | cap19 | cap26-f1 |
|---|---|---|---|---|---|---|
| occlusion-test | 0 trips, 0 drawn / 0 culled / 0 faceless | 0 trips, 0 drawn / 0 culled / 0 faceless | 0 trips, 0 drawn / 0 culled / 0 faceless | 0 trips, 0 drawn / 0 culled / 0 faceless | 0 trips, 0 drawn / 0 culled / 0 faceless | – |
| hallway | 0 trips, 0 drawn / 1 culled / 2 faceless | 0 trips, 0 drawn / 0 culled / 0 faceless | 0 trips, 0 drawn / 0 culled / 0 faceless | 0 trips, 0 drawn / 0 culled / 0 faceless | 0 trips, 0 drawn / 0 culled / 0 faceless | – |
| campaign-test | 0 trips, 0 drawn / 0 culled / 0 faceless | 0 trips, 0 drawn / 0 culled / 0 faceless | 0 trips, 0 drawn / 0 culled / 0 faceless | 0 trips, 0 drawn / 0 culled / 0 faceless | 0 trips, 0 drawn / 0 culled / 0 faceless | – |
| arena | 0 trips, 0 drawn / 13 culled / 0 faceless | 0 trips, 0 drawn / 0 culled / 0 faceless | 0 trips, 0 drawn / 0 culled / 0 faceless | 0 trips, 0 drawn / 0 culled / 0 faceless | 0 trips, 0 drawn / 0 culled / 0 faceless | – |
| street | 518 trips, 0 drawn / 0 culled / 0 faceless | 19 trips, 0 drawn / 9 culled / 0 faceless | 19 trips, 0 drawn / 9 culled / 0 faceless | 312 trips, 0 drawn / 9 culled / 0 faceless | 389 trips, 0 drawn / 10 culled / 0 faceless | 615 trips, 0 drawn / 9 culled / 0 faceless |

### M2: deferred texels, random / far-point / pose tour

| Map | before | 9g-cap26 | cap26-noprism | cap26 | cap19 | cap26-f1 |
|---|---|---|---|---|---|---|
| occlusion-test | 0 / 0 / 0 | 0 / 0 / 0 | – | 0 / 0 / 0 | 0 / 0 / 0 | – |
| hallway | 0 / 0 / 0 | 0 / 0 / 0 | – | 0 / 0 / 0 | 0 / 0 / 0 | – |
| campaign-test | 0 / 0 / 0 | 0 / 0 / 0 | – | 0 / 0 / 0 | 0 / 0 / 0 | – |
| arena | 0 / 0 / 0 | 0 / 0 / 0 | – | 0 / 0 / 0 | 0 / 0 / 0 | – |
| street | 0 / 0 / 0 | 0 / 0 / 0 | – | 0 / 0 / 0 | 0 / 0 / 0 | – |

### M2 beside: left-behind texels, random / far / tour

| Map | before | 9g-cap26 | cap26-noprism | cap26 | cap19 | cap26-f1 |
|---|---|---|---|---|---|---|
| occlusion-test | 0.0 M / 0.0 M / 0.0 M | 0.0 M / 0.0 M / 0.0 M | – | 0.0 M / 0.0 M / 3.8 M | 0.0 M / 0.0 M / 4.2 M | – |
| hallway | 28.7 M / 38.4 M / 23,233.0 M | 219.2 M / 827.9 M / 30,705.1 M | – | 31.3 M / 12.8 M / 15,996.3 M | 33.0 M / 9.8 M / 13,509.5 M | – |
| campaign-test | 0.0 M / 0.0 M / 157.4 M | 0.0 M / 0.0 M / 84.0 M | – | 0.0 M / 0.0 M / 92.6 M | 0.0 M / 0.0 M / 112.0 M | – |
| arena | 0.0 M / 0.0 M / 9.3 M | 0.0 M / 0.0 M / 15.8 M | – | 0.0 M / 0.0 M / 22.4 M | 0.0 M / 0.0 M / 31.6 M | – |
| street | 2,020.8 M / 2,686.4 M / 82.5 M | 108.0 M / 92.3 M / 1.8 M | – | 3,154.9 M / 3,740.1 M / 133.9 M | 3,764.2 M / 4,609.4 M / 165.1 M | – |

### M2 beside: trip frames, random / far / tour

| Map | before | 9g-cap26 | cap26-noprism | cap26 | cap19 | cap26-f1 |
|---|---|---|---|---|---|---|
| occlusion-test | 0 / 0 / 0 | 0 / 0 / 0 | – | 0 / 0 / 0 | 0 / 0 / 0 | – |
| hallway | 0 / 0 / 0 | 0 / 0 / 0 | – | 0 / 0 / 0 | 0 / 0 / 0 | – |
| campaign-test | 0 / 0 / 0 | 0 / 0 / 0 | – | 0 / 0 / 0 | 0 / 0 / 0 | – |
| arena | 0 / 0 / 0 | 0 / 0 / 0 | – | 0 / 0 / 0 | 0 / 0 / 0 | – |
| street | 19,554 / 25,858 / 1,036 | 1,619 / 1,972 / 38 | – | 12,588 / 14,214 / 624 | 14,161 / 16,133 / 778 | – |

### M2 pool: peak layers (active MiB with the spare layer, 14.0 MiB per layer), random / far / tour

| Map | before | 9g-cap26 | cap26-noprism | cap26 | cap19 | cap26-f1 |
|---|---|---|---|---|---|---|
| occlusion-test | 10 (154 MiB) / 10 (154 MiB) / 10 (154 MiB) | 9 (140 MiB) / 9 (140 MiB) / 9 (140 MiB) | – | 9 (140 MiB) / 9 (140 MiB) / 10 (154 MiB) | 9 (140 MiB) / 9 (140 MiB) / 9 (140 MiB) | – |
| hallway | 34 (490 MiB) / 38 (546 MiB) / 34 (490 MiB) | 37 (532 MiB) / 44 (630 MiB) / 30 (434 MiB) | – | 23 (336 MiB) / 32 (462 MiB) / 20 (294 MiB) | 22 (322 MiB) / 29 (420 MiB) / 19 (280 MiB) | – |
| campaign-test | 15 (224 MiB) / 16 (238 MiB) / 15 (224 MiB) | 14 (210 MiB) / 15 (224 MiB) / 14 (210 MiB) | – | 15 (224 MiB) / 16 (238 MiB) / 15 (224 MiB) | 15 (224 MiB) / 17 (252 MiB) / 15 (224 MiB) | – |
| arena | 2 (42 MiB) / 2 (42 MiB) / 2 (42 MiB) | 2 (42 MiB) / 2 (42 MiB) / 2 (42 MiB) | – | 3 (56 MiB) / 3 (56 MiB) / 3 (56 MiB) | 2 (42 MiB) / 2 (42 MiB) / 2 (42 MiB) | – |
| street | 2 (42 MiB) / 2 (42 MiB) / 2 (42 MiB) | 2 (42 MiB) / 2 (42 MiB) / 2 (42 MiB) | – | 1 (28 MiB) / 1 (28 MiB) / 1 (28 MiB) | 1 (28 MiB) / 1 (28 MiB) / 1 (28 MiB) | – |

### M2 pool: headroom cost at peak, layers (MiB), random / far / tour

| Map | before | 9g-cap26 | cap26-noprism | cap26 | cap19 | cap26-f1 |
|---|---|---|---|---|---|---|
| occlusion-test | 1 (14.0 MiB) / 1 (14.0 MiB) / 1 (14.0 MiB) | 0 (0.0 MiB) / 0 (0.0 MiB) / 0 (0.0 MiB) | – | 0 (0.0 MiB) / 0 (0.0 MiB) / 0 (0.0 MiB) | 0 (0.0 MiB) / 0 (0.0 MiB) / 0 (0.0 MiB) | – |
| hallway | 0 (0.0 MiB) / 0 (0.0 MiB) / 7 (98.0 MiB) | 0 (0.0 MiB) / 0 (0.0 MiB) / 2 (28.0 MiB) | – | 0 (0.0 MiB) / 0 (0.0 MiB) / 0 (0.0 MiB) | 0 (0.0 MiB) / 0 (0.0 MiB) / 0 (0.0 MiB) | – |
| campaign-test | 0 (0.0 MiB) / 0 (0.0 MiB) / 0 (0.0 MiB) | 0 (0.0 MiB) / 0 (0.0 MiB) / 0 (0.0 MiB) | – | 0 (0.0 MiB) / 0 (0.0 MiB) / 0 (0.0 MiB) | 0 (0.0 MiB) / 0 (0.0 MiB) / 0 (0.0 MiB) | – |
| arena | 0 (0.0 MiB) / 0 (0.0 MiB) / 0 (0.0 MiB) | 1 (14.0 MiB) / 0 (0.0 MiB) / 1 (14.0 MiB) | – | 2 (28.0 MiB) / 1 (14.0 MiB) / 1 (14.0 MiB) | 1 (14.0 MiB) / 1 (14.0 MiB) / 1 (14.0 MiB) | – |
| street | 1 (14.0 MiB) / 1 (14.0 MiB) / 1 (14.0 MiB) | 0 (0.0 MiB) / 0 (0.0 MiB) / 0 (0.0 MiB) | – | 0 (0.0 MiB) / 0 (0.0 MiB) / 0 (0.0 MiB) | 0 (0.0 MiB) / 0 (0.0 MiB) / 0 (0.0 MiB) | – |

### M2 pool: first → peak layers, growths, random / far / tour

| Map | before | 9g-cap26 | cap26-noprism | cap26 | cap19 | cap26-f1 |
|---|---|---|---|---|---|---|
| occlusion-test | 10→10, 0 / 10→10, 0 / 10→10, 0 | 9→9, 0 / 9→9, 0 / 9→9, 0 | – | 9→9, 0 / 9→9, 0 / 9→10, 1 | 9→9, 0 / 9→9, 0 / 9→9, 0 | – |
| hallway | 15→34, 12 / 15→38, 6 / 15→34, 3 | 15→37, 7 / 15→44, 10 / 15→30, 2 | – | 15→23, 8 / 15→32, 8 / 15→20, 2 | 15→22, 7 / 15→29, 9 / 15→19, 2 | – |
| campaign-test | 15→15, 0 / 15→16, 1 / 15→15, 0 | 14→14, 0 / 14→15, 1 / 14→14, 0 | – | 15→15, 0 / 15→16, 1 / 15→15, 0 | 15→15, 0 / 15→17, 2 / 15→15, 0 | – |
| arena | 2→2, 0 / 2→2, 0 / 2→2, 0 | 2→2, 0 / 2→2, 0 / 2→2, 0 | – | 3→3, 0 / 3→3, 0 / 3→3, 0 | 2→2, 0 / 2→2, 0 / 2→2, 0 | – |
| street | 2→2, 0 / 2→2, 0 / 2→2, 0 | 2→2, 0 / 2→2, 0 / 2→2, 0 | – | 1→1, 0 / 1→1, 0 / 1→1, 0 | 1→1, 0 / 1→1, 0 / 1→1, 0 | – |

### M3: seal lines per mover

| Map | before | 9g-cap26 | cap26-noprism | cap26 | cap19 | cap26-f1 |
|---|---|---|---|---|---|---|
| occlusion-test | none | none | none | none | none | – |
| hallway | m0 5, m1 4, m2 2, m3 2, m4 5, m5 2, m6 1, m7 1, m8 1 | m0 6, m1 4, m2 5, m3 4, m4 6, m5 4, m6 2, m7 2, m8 2 | m0 6, m1 4, m2 5, m3 4, m4 6, m5 4, m6 2, m7 2, m8 2 | m0 7, m1 8, m2 4, m3 7, m4 4, m5 7, m6 2, m7 1, m8 2 | m0 7, m1 8, m2 4, m3 7, m4 4, m5 7, m6 2, m7 1, m8 2 | – |
| campaign-test | m0 2 | m0 5, m1 5 | m0 5, m1 5 | m0 5, m1 5 | m0 5, m1 5 | – |
| arena | m0 1, m1 1, m2 4, m3 4, m4 1, m5 4 | m0 3, m1 3, m2 4, m3 4, m4 3, m5 5 | m0 3, m1 3, m2 4, m3 4, m4 3, m5 5 | m0 3, m1 3, m2 5, m3 5, m4 3, m5 5 | m0 3, m1 3, m2 5, m3 5, m4 3, m5 5 | – |
| street | none | none | none | none | none | none |

### M4: leak; exterior / interior empty leaves (full tree); open edges

| Map | before | 9g-cap26 | cap26-noprism | cap26 | cap19 | cap26-f1 |
|---|---|---|---|---|---|---|
| occlusion-test | none; ext/int leaves 97 / 287; open edges 0 | none; ext/int leaves 122 / 416; open edges 0 | none; ext/int leaves 122 / 416; open edges 0 | none; ext/int leaves 160 / 487; open edges 0 | none; ext/int leaves 160 / 489; open edges 0 | – |
| hallway | none; ext/int leaves 411 / 2082; open edges 283 | none; ext/int leaves 416 / 3259; open edges 563 | none; ext/int leaves 416 / 3259; open edges 563 | none; ext/int leaves 416 / 4834; open edges 649 | none; ext/int leaves 416 / 5054; open edges 649 | – |
| campaign-test | none; ext/int leaves 114 / 207; open edges 9 | none; ext/int leaves 241 / 343; open edges 43 | none; ext/int leaves 241 / 343; open edges 43 | none; ext/int leaves 243 / 392; open edges 43 | none; ext/int leaves 243 / 472; open edges 37 | – |
| arena | none; ext/int leaves 228 / 560; open edges 24 | none; ext/int leaves 206 / 821; open edges 24 | none; ext/int leaves 206 / 821; open edges 24 | none; ext/int leaves 204 / 1120; open edges 28 | none; ext/int leaves 204 / 1262; open edges 28 | – |
| street | none; ext/int leaves 177 / 1985; open edges 396 | none; ext/int leaves 307 / 2917; open edges 591 | none; ext/int leaves 307 / 2918; open edges 591 | none; ext/int leaves 307 / 3761; open edges 624 | none; ext/int leaves 307 / 4053; open edges 663 | none; ext/int leaves 307 / 4148; open edges 589 |

### M5: occluders (occlude alone; sections occluding)

| Map | before | 9g-cap26 | cap26-noprism | cap26 | cap19 | cap26-f1 |
|---|---|---|---|---|---|---|
| occlusion-test | – | 29 alone; 5 of 11 sections (17 brushes); len 20 | 29 alone; 5 of 11 sections (17 brushes); len 20 | 29 alone; 5 of 11 sections (17 brushes); len 20 | 29 alone; 5 of 11 sections (17 brushes); len 20 | – |
| hallway | – | 738 alone; 12 of 99 sections (51 brushes); len 20 | 738 alone; 12 of 99 sections (51 brushes); len 20 | 738 alone; 12 of 99 sections (51 brushes); len 20 | 738 alone; 12 of 99 sections (51 brushes); len 20 | – |
| campaign-test | – | 40 alone; 7 of 15 sections (21 brushes); len 20 | 40 alone; 7 of 15 sections (21 brushes); len 20 | 40 alone; 7 of 15 sections (21 brushes); len 20 | 40 alone; 7 of 15 sections (21 brushes); len 20 | – |
| arena | – | 237 alone; 2 of 11 sections (10 brushes); len 20 | 237 alone; 2 of 11 sections (10 brushes); len 20 | 237 alone; 2 of 11 sections (10 brushes); len 20 | 237 alone; 2 of 11 sections (10 brushes); len 20 | – |
| street | – | 72 alone; 13 of 37 sections (1070 brushes); len 20 | 72 alone; 13 of 37 sections (1070 brushes); len 20 | 72 alone; 13 of 37 sections (1070 brushes); len 20 | 72 alone; 13 of 37 sections (1070 brushes); len 20 | 72 alone; 13 of 37 sections (1070 brushes); len 20 |

### M5: occluder-tier leaves

| Map | before | 9g-cap26 | cap26-noprism | cap26 | cap19 | cap26-f1 |
|---|---|---|---|---|---|---|
| occlusion-test | – | 313 | 313 | 416 | 421 | – |
| hallway | – | 5,110 | 5,110 | 6,973 | 7,183 | – |
| campaign-test | – | 559 | 559 | 606 | 668 | – |
| arena | – | 1,131 | 1,131 | 1,429 | 1,574 | – |
| street | – | 1,896 | 1,897 | 3,345 | 3,618 | 3,735 |

### M5: occluder-tier leaves holding several interior pockets (9j on)

| Map | before | 9g-cap26 | cap26-noprism | cap26 | cap19 | cap26-f1 |
|---|---|---|---|---|---|---|
| occlusion-test | – | – | 0 | 0 | 0 | – |
| hallway | – | – | 18 | 18 | 18 | – |
| campaign-test | – | – | 0 | 0 | 0 | – |
| arena | – | – | 4 | 0 | 0 | – |
| street | – | – | 78 | 179 | 193 | 193 |

### M5: shadow prisms (occluding faces; ended by the length bound)

| Map | before | 9g-cap26 | cap26-noprism | cap26 | cap19 | cap26-f1 |
|---|---|---|---|---|---|---|
| occlusion-test | – | – | 0 (0 length-bounded) | 17 (6 length-bounded) | 17 (6 length-bounded) | – |
| hallway | – | – | 0 (0 length-bounded) | 1,000 (164 length-bounded) | 1,000 (164 length-bounded) | – |
| campaign-test | – | – | 0 (0 length-bounded) | 11 (2 length-bounded) | 11 (2 length-bounded) | – |
| arena | – | – | 0 (0 length-bounded) | 238 (68 length-bounded) | 238 (68 length-bounded) | – |
| street | – | – | 0 (0 length-bounded) | 88 (14 length-bounded) | 88 (14 length-bounded) | 88 (45 length-bounded) |

### M5: cells (interior / solid / exterior)

| Map | before | 9g-cap26 | cap26-noprism | cap26 | cap19 | cap26-f1 |
|---|---|---|---|---|---|---|
| occlusion-test | 555 (287/171/97) | 484 (107/255/122) | 484 (107/255/122) | 592 (174/258/160) | 597 (179/258/160) | – |
| hallway | 5,671 (2,082/3,178/411) | 7,439 (1,490/5,533/416) | 7,465 (1,516/5,533/416) | 10,021 (3,268/6,337/416) | 10,252 (3,482/6,354/416) | – |
| campaign-test | 487 (207/166/114) | 832 (129/462/241) | 832 (129/462/241) | 879 (165/471/243) | 964 (227/494/243) | – |
| arena | 1,479 (560/691/228) | 1,389 (391/792/206) | 1,393 (395/792/206) | 1,718 (683/831/204) | 1,864 (821/839/204) | – |
| street | 3,709 (1,985/1,547/177) | 4,034 (850/2,877/307) | 4,792 (1,607/2,878/307) | 6,510 (3,144/3,059/307) | 6,832 (3,432/3,093/307) | 7,007 (3,546/3,154/307) |

### M5: runtime walkable portals

| Map | before | 9g-cap26 | cap26-noprism | cap26 | cap19 | cap26-f1 |
|---|---|---|---|---|---|---|
| occlusion-test | 693 | 241 | 241 | 430 | 444 | – |
| hallway | 4,722 | 2,368 | 2,368 | 5,357 | 5,798 | – |
| campaign-test | 524 | 288 | 288 | 402 | 570 | – |
| arena | 1,333 | 608 | 608 | 1,128 | 1,401 | – |
| street | 6,288 | 3,755 | 3,756 | 7,667 | 8,484 | 8,998 |

### M5: PRL faces = lightmap charts / triangles (extra charts from atlas sub-chart cuts, already in the face count)

| Map | before | 9g-cap26 | cap26-noprism | cap26 | cap19 | cap26-f1 |
|---|---|---|---|---|---|---|
| occlusion-test | 1,018 / 2,036 (0 cut) | 1,057 / 2,114 (0 cut) | 1,057 / 2,114 (0 cut) | 1,113 / 2,226 (0 cut) | 1,118 / 2,236 (0 cut) | – |
| hallway | 8,457 / 16,910 (12 cut) | 11,370 / 22,594 (0 cut) | 11,370 / 22,594 (0 cut) | 13,804 / 27,403 (0 cut) | 14,205 / 28,205 (0 cut) | – |
| campaign-test | 808 / 1,612 (0 cut) | 1,022 / 2,038 (0 cut) | 1,022 / 2,038 (0 cut) | 1,081 / 2,156 (0 cut) | 1,201 / 2,397 (0 cut) | – |
| arena | 2,239 / 4,478 (0 cut) | 2,899 / 5,798 (0 cut) | 2,899 / 5,798 (0 cut) | 3,362 / 6,724 (0 cut) | 3,633 / 7,266 (0 cut) | – |
| street | 7,657 / 15,315 (0 cut) | 10,441 / 20,881 (0 cut) | 10,441 / 20,881 (0 cut) | 11,101 / 22,201 (0 cut) | 11,303 / 22,605 (0 cut) | 11,368 / 22,735 (0 cut) |

### M5: interior cell largest extent p50 / p95 / max (m)

| Map | before | 9g-cap26 | cap26-noprism | cap26 | cap19 | cap26-f1 |
|---|---|---|---|---|---|---|
| occlusion-test | 13.0 / 21.5 / 26.0 | 17.1 / 26.0 / 26.6 | 17.1 / 26.0 / 26.6 | 11.6 / 24.4 / 26.0 | 11.6 / 21.9 / 25.2 | – |
| hallway | 10.2 / 26.0 / 52.0 | 9.8 / 27.6 / 53.6 | 9.8 / 27.6 / 53.6 | 6.5 / 21.1 / 32.5 | 6.5 / 19.5 / 26.0 | – |
| campaign-test | 9.8 / 22.8 / 35.8 | 15.8 / 26.0 / 26.0 | 15.8 / 26.0 / 26.0 | 13.8 / 26.0 / 26.0 | 12.2 / 20.3 / 24.8 | – |
| arena | 16.3 / 29.3 / 94.3 | 26.0 / 26.0 / 58.5 | 26.0 / 26.0 / 58.5 | 14.6 / 26.0 / 29.3 | 13.0 / 20.3 / 26.0 | – |
| street | 5.8 / 24.4 / 88.0 | 16.0 / 27.7 / 62.0 | 2.3 / 26.0 / 32.0 | 5.0 / 26.0 / 32.0 | 6.0 / 19.5 / 25.8 | 6.0 / 26.0 / 32.0 |

### M5: interior cells with smallest extent 0.25-0.5 m

| Map | before | 9g-cap26 | cap26-noprism | cap26 | cap19 | cap26-f1 |
|---|---|---|---|---|---|---|
| occlusion-test | 22 | 42 | 42 | 65 | 67 | – |
| hallway | 49 | 2 | 2 | 2 | 2 | – |
| campaign-test | 33 | 66 | 66 | 75 | 102 | – |
| arena | 5 | 0 | 0 | 0 | 0 | – |
| street | 769 | 395 | 998 | 1,727 | 1,843 | 1,914 |

### M5: interior cells with smallest extent under 0.5 m (slivers)

| Map | before | 9g-cap26 | cap26-noprism | cap26 | cap19 | cap26-f1 |
|---|---|---|---|---|---|---|
| occlusion-test | 25 | 48 | 48 | 76 | 78 | – |
| hallway | 92 | 3 | 11 | 28 | 28 | – |
| campaign-test | 39 | 66 | 66 | 75 | 102 | – |
| arena | 8 | 0 | 0 | 0 | 0 | – |
| street | 934 | 422 | 1,172 | 1,930 | 2,065 | 2,120 |

### M5: SH clusters (indivisible oversize)

| Map | before | 9g-cap26 | cap26-noprism | cap26 | cap19 | cap26-f1 |
|---|---|---|---|---|---|---|
| occlusion-test | 197 (0) | 390 (2) | 390 (2) | 433 (3) | 434 (3) | – |
| hallway | 3,386 (0) | 6,207 (3) | 6,192 (3) | 7,062 (3) | 7,084 (3) | – |
| campaign-test | 188 (0) | 721 (2) | 721 (2) | 734 (2) | 762 (1) | – |
| arena | 752 (0) | 1,055 (2) | 1,058 (2) | 1,106 (2) | 1,119 (1) | – |
| street | 1,915 (0) | 3,352 (28) | 3,701 (14) | 3,912 (5) | 3,889 (3) | 3,946 (6) |

### id-51 lightmap mandatory set at L 16 m, max per camera cell (texels)

| Map | before | 9g-cap26 | cap26-noprism | cap26 | cap19 | cap26-f1 |
|---|---|---|---|---|---|---|
| occlusion-test | 30.6 M (p95 30.4; 0 over 62.9) | 24.3 M (p95 24.1; 0 over 62.9) | – | 24.9 M (p95 24.8; 0 over 62.9) | 25.0 M (p95 24.9; 0 over 62.9) | – |
| hallway | 97.3 M (p95 57.9; 177 over 62.9) | 116.3 M (p95 76.8; 714 over 62.9) | – | 84.2 M (p95 50.7; 97 over 62.9) | 80.2 M (p95 50.0; 95 over 62.9) | – |
| campaign-test | 37.3 M (p95 34.4; 0 over 62.9) | 34.7 M (p95 32.8; 0 over 62.9) | – | 35.9 M (p95 33.1; 0 over 62.9) | 35.3 M (p95 27.9; 0 over 62.9) | – |
| arena | 1.2 M (p95 0.7; 0 over 62.9) | 1.2 M (p95 0.8; 0 over 62.9) | – | 0.9 M (p95 0.7; 0 over 62.9) | 0.9 M (p95 0.7; 0 over 62.9) | – |
| street | 2.8 M (p95 2.7; 0 over 62.9) | 2.4 M (p95 2.4; 0 over 62.9) | – | 2.5 M (p95 2.4; 0 over 62.9) | 2.4 M (p95 2.4; 0 over 62.9) | – |

### M6: portals tested, mean / p95 per pose

| Map | before | 9g-cap26 | cap26-noprism | cap26 | cap19 | cap26-f1 |
|---|---|---|---|---|---|---|
| occlusion-test | 464.3 / 1,749 | 179.6 / 467 | 179.6 / 467 | 316.7 / 925 | 321.3 / 972 | – |
| hallway | 65.3 / 196 | 53.3 / 279 | 51.2 / 279 | 104.3 / 539 | 108.4 / 555 | – |
| campaign-test | 108.1 / 296 | 74.4 / 250 | 74.4 / 250 | 100.0 / 387 | 128.8 / 481 | – |
| arena | 58.8 / 202 | 33.5 / 160 | 32.3 / 151 | 47.7 / 178 | 59.8 / 224 | – |
| street | 2,042.6 / 5,000 | 1,271.0 / 3,560 | 1,256.1 / 3,511 | 1,982.6 / 5,000 | 2,074.1 / 5,000 | 2,247.4 / 5,000 |

### M6: drawn cells, mean / p95 per pose

| Map | before | 9g-cap26 | cap26-noprism | cap26 | cap19 | cap26-f1 |
|---|---|---|---|---|---|---|
| occlusion-test | 66.6 / 242 | 27.8 / 65 | 27.8 / 65 | 40.3 / 101 | 40.2 / 106 | – |
| hallway | 11.9 / 33 | 11.5 / 49 | 11.0 / 47 | 20.7 / 73 | 21.6 / 78 | – |
| campaign-test | 15.9 / 41 | 12.9 / 39 | 12.9 / 39 | 14.9 / 48 | 17.7 / 53 | – |
| arena | 9.6 / 28 | 8.6 / 30 | 8.5 / 28 | 12.0 / 38 | 14.4 / 45 | – |
| street | 272.1 / 1,386 | 114.5 / 324 | 147.2 / 401 | 362.3 / 2,070 | 406.8 / 2,246 | 479.6 / 2,231 |

### M6: tris in frustum, mean / p95 per pose

| Map | before | 9g-cap26 | cap26-noprism | cap26 | cap19 | cap26-f1 |
|---|---|---|---|---|---|---|
| occlusion-test | 385.9 / 1,516 | 398.3 / 1,472 | 398.3 / 1,472 | 414.6 / 1,546 | 415.7 / 1,552 | – |
| hallway | 80.3 / 230 | 133.3 / 502 | 125.6 / 432 | 137.1 / 496 | 138.9 / 496 | – |
| campaign-test | 101.1 / 283 | 175.1 / 574 | 175.1 / 574 | 180.1 / 584 | 185.3 / 562 | – |
| arena | 65.4 / 200 | 96.7 / 328 | 93.4 / 296 | 97.9 / 308 | 103.4 / 332 | – |
| street | 1,939.3 / 10,297 | 2,046.0 / 6,282 | 1,777.4 / 5,402 | 2,548.5 / 15,637 | 2,771.2 / 16,287 | 3,445.7 / 16,905 |

### M6: drawn lightmap texels, mean / p95 per pose

| Map | before | 9g-cap26 | cap26-noprism | cap26 | cap19 | cap26-f1 |
|---|---|---|---|---|---|---|
| occlusion-test | 9.41 M / 24.89 M | 7.74 M / 16.83 M | 7.74 M / 16.83 M | 7.08 M / 15.88 M | 7.06 M / 16.15 M | – |
| hallway | 6.15 M / 24.04 M | 6.94 M / 22.86 M | 6.72 M / 22.44 M | 5.36 M / 19.11 M | 4.95 M / 17.44 M | – |
| campaign-test | 5.66 M / 14.03 M | 6.23 M / 13.39 M | 6.23 M / 13.39 M | 5.84 M / 12.66 M | 4.87 M / 11.40 M | – |
| arena | 0.12 M / 0.52 M | 0.10 M / 0.30 M | 0.10 M / 0.28 M | 0.09 M / 0.27 M | 0.08 M / 0.25 M | – |
| street | 0.64 M / 2.31 M | 0.39 M / 1.04 M | 0.37 M / 0.98 M | 0.39 M / 1.84 M | 0.39 M / 1.84 M | 0.46 M / 1.92 M |

### M6: portals tested, max per pose (a tripped pose stops at 5,000)

| Map | before | 9g-cap26 | cap26-noprism | cap26 | cap19 | cap26-f1 |
|---|---|---|---|---|---|---|
| occlusion-test | 2,084 (0 trips) | 596 (0 trips) | 596 (0 trips) | 1,146 (0 trips) | 1,299 (0 trips) | – |
| hallway | 998 (0 trips) | 755 (0 trips) | 755 (0 trips) | 1,614 (0 trips) | 1,639 (0 trips) | – |
| campaign-test | 524 (0 trips) | 378 (0 trips) | 378 (0 trips) | 568 (0 trips) | 674 (0 trips) | – |
| arena | 686 (0 trips) | 371 (0 trips) | 350 (0 trips) | 407 (0 trips) | 639 (0 trips) | – |
| street | 5,000 (518 trips) | 5,000 (19 trips) | 5,000 (19 trips) | 5,000 (312 trips) | 5,000 (389 trips) | 5,000 (615 trips) |

### Guardrail: mean portals tested per pose against before

| Map | before | 9g-cap26 | cap26-noprism | cap26 | cap19 | cap26-f1 |
|---|---|---|---|---|---|---|
| occlusion-test | +0% | -61% | -61% | -32% | -31% | – |
| hallway | +0% | -18% | -22% | +60% | +66% | – |
| campaign-test | +0% | -31% | -31% | -7% | +19% | – |
| arena | +0% | -43% | -45% | -19% | +2% | – |
| street | +0% | -38% | -39% | -3% | +2% | +10% |

### Guardrail: mean tris in frustum per pose against before

| Map | before | 9g-cap26 | cap26-noprism | cap26 | cap19 | cap26-f1 |
|---|---|---|---|---|---|---|
| occlusion-test | +0% | +3% | +3% | +7% | +8% | – |
| hallway | +0% | +66% | +56% | +71% | +73% | – |
| campaign-test | +0% | +73% | +73% | +78% | +83% | – |
| arena | +0% | +48% | +43% | +50% | +58% | – |
| street | +0% | +6% | -8% | +31% | +43% | +78% |

### Guardrail: mean drawn lightmap texels per pose against before

| Map | before | 9g-cap26 | cap26-noprism | cap26 | cap19 | cap26-f1 |
|---|---|---|---|---|---|---|
| occlusion-test | +0% | -18% | -18% | -25% | -25% | – |
| hallway | +0% | +13% | +9% | -13% | -19% | – |
| campaign-test | +0% | +10% | +10% | +3% | -14% | – |
| arena | +0% | -12% | -15% | -25% | -30% | – |
| street | +0% | -40% | -43% | -39% | -39% | -28% |

### SH mandatory clusters per camera cell at L 16 m (owner closure included), median / p95 / max

| Map | before | 9g-cap26 | cap26-noprism | cap26 | cap19 | cap26-f1 |
|---|---|---|---|---|---|---|
| occlusion-test | 21 / 22 / 22 | 14 / 14 / 14 | – | 15 / 16 / 16 | 16 / 16 / 16 | – |
| hallway | 83 / 100 / 105 | 119 / 136 / 143 | – | 122 / 144 / 165 | 116 / 141 / 158 | – |
| campaign-test | 7 / 14 / 15 | 11 / 16 / 16 | – | 15 / 18 / 18 | 13 / 21 / 22 | – |
| arena | 12 / 18 / 21 | 14 / 22 / 24 | – | 16 / 24 / 29 | 18 / 26 / 31 | – |
| street | 326 / 358 / 362 | 161 / 167 / 168 | – | 467 / 534 / 545 | 404 / 471 / 488 | – |

### SH probe reach into another cluster's cells (scratch reader)

| Map | before | 9g-cap26 | cap26-noprism | cap26 | cap19 | cap26-f1 |
|---|---|---|---|---|---|---|
| occlusion-test | 158 pairs, 48,770 refs; owner edges 52 | – | – | – | – | – |
| hallway | 2,363 pairs, 60,903 refs; owner edges 930 | – | – | – | – | – |
| campaign-test | 45 pairs, 27,053 refs; owner edges 19 | – | – | – | – | – |
| arena | 537 pairs, 4,849 refs; owner edges 125 | – | – | – | – | – |
| street | 7,917 pairs, 85,920 refs; owner edges 949 | – | – | – | – | – |

### M7: arena box

| before | 9g-cap26 | cap26-noprism | cap26 | cap19 | cap26-f1 |
|---|---|---|---|---|---|
| 58 cells, 117 portals on 40 planes | 74 cells, 166 portals on 12 planes | 74 cells, 166 portals on 12 planes | 86 cells, 193 portals on 13 planes | 129 cells, 292 portals on 17 planes | – |

### M7: stairs box

| before | 9g-cap26 | cap26-noprism | cap26 | cap19 | cap26-f1 |
|---|---|---|---|---|---|
| 32 cells, 58 portals on 25 planes; riser planes 23, tread planes 0 | 6 cells, 7 portals on 3 planes; riser planes 0, tread planes 1 (11.379) | 6 cells, 7 portals on 3 planes; riser planes 0, tread planes 1 (11.379) | 6 cells, 7 portals on 3 planes; riser planes 0, tread planes 1 (11.379) | 6 cells, 7 portals on 3 planes; riser planes 0, tread planes 1 (11.379) | – |

### Street step-cap trips: total / also tripped before / new

| Set | before | 9g-cap26 | cap26-noprism | cap26 | cap19 | cap26-f1 |
|---|---|---|---|---|---|---|
| street level (2,960 poses) | 518 / 518 / 0 | 19 / 19 / 0 | 19 / 19 / 0 | 312 / 245 / 67 | 389 / 288 / 101 | 615 / 369 / 246 |
| rooftop (608 poses) | 212 / 212 / 0 | 22 / 21 / 1 | 27 / 27 / 0 | 168 / 153 / 15 | 189 / 164 / 25 | 231 / 192 / 39 |

### Rooftop set (stress-street, 608 poses)

| Row | before | 9g-cap26 | cap26-noprism | cap26 | cap19 | cap26-f1 |
|---|---|---|---|---|---|---|
| step-cap trips | 212 | 22 | 27 | 168 | 189 | 231 |
| new drawn-miss poses | 0 | 0 | 0 | 0 | 0 | 0 |
| culled / faceless (reported) | 0 / 0 | 0 / 0 | 0 / 0 | 0 / 0 | 0 / 0 | 0 / 0 |
| portals tested mean / p95 | 2,789.4 / 5,000 | 1,897.6 / 4,552 | 1,916.3 / 4,696 | 2,956.8 / 5,000 | 3,042.4 / 5,000 | 3,226.0 / 5,000 |
| drawn cells mean / p95 | 426.7 / 1,409 | 170.4 / 410 | 231.9 / 561 | 557.5 / 1,963 | 613.8 / 2,077 | 660.8 / 2,115 |
| tris in frustum mean / p95 | 3,035.3 / 10,847 | 3,077.3 / 8,162 | 2,703.9 / 7,355 | 3,857.7 / 15,129 | 4,072.1 / 15,329 | 4,605.8 / 16,163 |
| drawn lightmap texels mean / p95 | 0.86 M / 2.22 M | 0.55 M / 1.23 M | 0.53 M / 1.19 M | 0.58 M / 1.75 M | 0.59 M / 1.72 M | 0.64 M / 1.82 M |
| paths | ("portal", 396), ("step_limit", 212) | ("portal", 586), ("step_limit", 22) | ("portal", 581), ("step_limit", 27) | ("portal", 440), ("step_limit", 168) | ("portal", 419), ("step_limit", 189) | ("step_limit", 231), ("portal", 377) |

### Uncapped portal tests per pose, street level (2,960 poses)

| Row | before | 9g-cap26 | cap26-noprism | cap26 | cap19 | cap26-f1 |
|---|---|---|---|---|---|---|
| mean | 2,553.7 | 1,273.9 | 1,259.5 | 2,150.6 | 2,303.8 | 2,671.8 |
| p50 / p95 / p99 | 1,409 / 8,891 / 12,352 | 1,017 / 3,560 / 4,507 | 995 / 3,511 / 4,636 | 1,471 / 6,298 / 8,317 | 1,525 / 6,845 / 9,090 | 1,687 / 8,108 / 10,619 |
| max (worst pose) | 18,513 (`g5.0.0@180`) | 6,068 (`g5.3.0@180`) | 6,045 (`g16.14.0@90`) | 11,587 (`g5.3.0@180`) | 12,711 (`g5.3.0@180`) | 13,867 (`g6.3.0@180`) |
| poses over 5,000 | 518 | 19 | 19 | 312 | 389 | 615 |
| 2× worst walk | 37,026 | 12,136 | 12,090 | 23,174 | 25,422 | 27,734 |
| total tests | 7,558,805 | 3,770,785 | 3,728,158 | 6,365,853 | 6,819,272 | 7,908,443 |

### Uncapped portal tests per pose, rooftop (608 poses)

| Row | before | 9g-cap26 | cap26-noprism | cap26 | cap19 | cap26-f1 |
|---|---|---|---|---|---|---|
| mean | 4,354.5 | 1,913.8 | 1,939.7 | 3,418.5 | 3,650.7 | 4,161.1 |
| p50 / p95 / p99 | 2,671 / 13,007 / 16,459 | 1,731 / 4,552 / 5,523 | 1,725 / 4,696 / 5,871 | 3,064 / 8,013 / 10,399 | 3,123 / 8,445 / 11,474 | 3,633 / 10,120 / 12,453 |
| max (worst pose) | 17,598 (`roof:A7.s2.c@225/-15`) | 6,234 (`roof:B4.s2.c@180/-15`) | 6,466 (`roof:B4.s2.c@180/-15`) | 11,955 (`roof:B4.s2.c@180/-15`) | 13,092 (`roof:B4.s2.c@180/-15`) | 14,153 (`roof:B4.s2.c@180/-15`) |
| poses over 5,000 | 212 | 22 | 27 | 168 | 189 | 231 |
| 2× worst walk | 35,196 | 12,468 | 12,932 | 23,910 | 26,184 | 28,306 |
| total tests | 2,647,511 | 1,163,607 | 1,179,313 | 2,078,423 | 2,219,611 | 2,529,936 |

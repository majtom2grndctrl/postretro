
### M1: step-cap trips; poses with new drawn misses / reported culled / faceless

| Map | before | 9k-cap26-noprism | 9k-cap26 | 9l-cap26 | 9l-cap19 |
|---|---|---|---|---|---|
| occlusion-test | 0 trips, 0 drawn / 0 culled / 0 faceless | 0 trips, 0 drawn / 0 culled / 0 faceless | 0 trips, 0 drawn / 0 culled / 0 faceless | 0 trips, 0 drawn / 0 culled / 0 faceless | 0 trips, 0 drawn / 0 culled / 0 faceless |
| hallway | 0 trips, 0 drawn / 1 culled / 2 faceless | 0 trips, 0 drawn / 0 culled / 0 faceless | 0 trips, 0 drawn / 0 culled / 0 faceless | 0 trips, 0 drawn / 0 culled / 0 faceless | 0 trips, 0 drawn / 0 culled / 0 faceless |
| campaign-test | 0 trips, 0 drawn / 0 culled / 0 faceless | 0 trips, 0 drawn / 0 culled / 0 faceless | 0 trips, 0 drawn / 0 culled / 0 faceless | 0 trips, 0 drawn / 0 culled / 0 faceless | 0 trips, 0 drawn / 0 culled / 0 faceless |
| arena | 0 trips, 0 drawn / 13 culled / 0 faceless | 0 trips, 0 drawn / 0 culled / 0 faceless | 0 trips, 0 drawn / 0 culled / 0 faceless | 0 trips, 0 drawn / 0 culled / 0 faceless | 0 trips, 0 drawn / 0 culled / 0 faceless |
| street | 518 trips, 0 drawn / 0 culled / 0 faceless | 19 trips, 0 drawn / 9 culled / 0 faceless | 312 trips, 0 drawn / 9 culled / 0 faceless | 29 trips, 0 drawn / 9 culled / 0 faceless | 41 trips, 0 drawn / 11 culled / 0 faceless |

### M2: deferred texels, random / far-point / pose tour

| Map | before | 9k-cap26-noprism | 9k-cap26 | 9l-cap26 | 9l-cap19 |
|---|---|---|---|---|---|
| occlusion-test | 0 / 0 / 0 | – | 0 / 0 / 0 | 0 / 0 / 0 | 0 / 0 / 0 |
| hallway | 0 / 0 / 0 | – | 0 / 0 / 0 | 0 / 0 / 0 | 0 / 0 / 0 |
| campaign-test | 0 / 0 / 0 | – | 0 / 0 / 0 | 0 / 0 / 0 | 0 / 0 / 0 |
| arena | 0 / 0 / 0 | – | 0 / 0 / 0 | 0 / 0 / 0 | 0 / 0 / 0 |
| street | 0 / 0 / 0 | – | 0 / 0 / 0 | 0 / 0 / 0 | 0 / 0 / 0 |

### M2 beside: left-behind texels, random / far / tour

| Map | before | 9k-cap26-noprism | 9k-cap26 | 9l-cap26 | 9l-cap19 |
|---|---|---|---|---|---|
| occlusion-test | 0.0 M / 0.0 M / 0.0 M | – | 0.0 M / 0.0 M / 3.8 M | 0.0 M / 0.0 M / 0.2 M | 0.0 M / 0.0 M / 4.1 M |
| hallway | 28.7 M / 38.4 M / 23,233.0 M | – | 31.3 M / 12.8 M / 15,996.3 M | 4.8 M / 376.0 M / 23,522.9 M | 11.3 M / 98.6 M / 18,133.4 M |
| campaign-test | 0.0 M / 0.0 M / 157.4 M | – | 0.0 M / 0.0 M / 92.6 M | 0.0 M / 0.0 M / 78.6 M | 0.0 M / 0.0 M / 120.4 M |
| arena | 0.0 M / 0.0 M / 9.3 M | – | 0.0 M / 0.0 M / 22.4 M | 0.0 M / 0.0 M / 17.7 M | 0.0 M / 0.0 M / 27.2 M |
| street | 2,020.8 M / 2,686.4 M / 82.5 M | – | 3,154.9 M / 3,740.1 M / 133.9 M | 242.1 M / 322.7 M / 7.1 M | 520.6 M / 699.8 M / 9.6 M |

### M2 beside: trip frames, random / far / tour

| Map | before | 9k-cap26-noprism | 9k-cap26 | 9l-cap26 | 9l-cap19 |
|---|---|---|---|---|---|
| occlusion-test | 0 / 0 / 0 | – | 0 / 0 / 0 | 0 / 0 / 0 | 0 / 0 / 0 |
| hallway | 0 / 0 / 0 | – | 0 / 0 / 0 | 0 / 0 / 0 | 0 / 0 / 0 |
| campaign-test | 0 / 0 / 0 | – | 0 / 0 / 0 | 0 / 0 / 0 | 0 / 0 / 0 |
| arena | 0 / 0 / 0 | – | 0 / 0 / 0 | 0 / 0 / 0 | 0 / 0 / 0 |
| street | 19,554 / 25,858 / 1,036 | – | 12,588 / 14,214 / 624 | 2,525 / 3,315 / 58 | 3,424 / 4,248 / 82 |

### M2 pool: peak layers (active MiB with the spare layer, 14.0 MiB per layer), random / far / tour

| Map | before | 9k-cap26-noprism | 9k-cap26 | 9l-cap26 | 9l-cap19 |
|---|---|---|---|---|---|
| occlusion-test | 10 (154 MiB) / 10 (154 MiB) / 10 (154 MiB) | – | 9 (140 MiB) / 9 (140 MiB) / 10 (154 MiB) | 9 (140 MiB) / 9 (140 MiB) / 9 (140 MiB) | 10 (154 MiB) / 10 (154 MiB) / 10 (154 MiB) |
| hallway | 34 (490 MiB) / 38 (546 MiB) / 34 (490 MiB) | – | 23 (336 MiB) / 32 (462 MiB) / 20 (294 MiB) | 31 (448 MiB) / 38 (546 MiB) / 24 (350 MiB) | 28 (406 MiB) / 35 (504 MiB) / 28 (406 MiB) |
| campaign-test | 15 (224 MiB) / 16 (238 MiB) / 15 (224 MiB) | – | 15 (224 MiB) / 16 (238 MiB) / 15 (224 MiB) | 15 (224 MiB) / 15 (224 MiB) / 15 (224 MiB) | 15 (224 MiB) / 17 (252 MiB) / 15 (224 MiB) |
| arena | 2 (42 MiB) / 2 (42 MiB) / 2 (42 MiB) | – | 3 (56 MiB) / 3 (56 MiB) / 3 (56 MiB) | 2 (42 MiB) / 2 (42 MiB) / 2 (42 MiB) | 2 (42 MiB) / 2 (42 MiB) / 2 (42 MiB) |
| street | 2 (42 MiB) / 2 (42 MiB) / 2 (42 MiB) | – | 1 (28 MiB) / 1 (28 MiB) / 1 (28 MiB) | 2 (42 MiB) / 2 (42 MiB) / 2 (42 MiB) | 2 (42 MiB) / 2 (42 MiB) / 2 (42 MiB) |

### M2 pool: headroom cost at peak, layers (MiB), random / far / tour

| Map | before | 9k-cap26-noprism | 9k-cap26 | 9l-cap26 | 9l-cap19 |
|---|---|---|---|---|---|
| occlusion-test | 1 (14.0 MiB) / 1 (14.0 MiB) / 1 (14.0 MiB) | – | 0 (0.0 MiB) / 0 (0.0 MiB) / 0 (0.0 MiB) | 0 (0.0 MiB) / 0 (0.0 MiB) / 0 (0.0 MiB) | 0 (0.0 MiB) / 0 (0.0 MiB) / 0 (0.0 MiB) |
| hallway | 0 (0.0 MiB) / 0 (0.0 MiB) / 7 (98.0 MiB) | – | 0 (0.0 MiB) / 0 (0.0 MiB) / 0 (0.0 MiB) | 0 (0.0 MiB) / 0 (0.0 MiB) / 0 (0.0 MiB) | 0 (0.0 MiB) / 0 (0.0 MiB) / 2 (28.0 MiB) |
| campaign-test | 0 (0.0 MiB) / 0 (0.0 MiB) / 0 (0.0 MiB) | – | 0 (0.0 MiB) / 0 (0.0 MiB) / 0 (0.0 MiB) | 0 (0.0 MiB) / 0 (0.0 MiB) / 1 (14.0 MiB) | 0 (0.0 MiB) / 0 (0.0 MiB) / 0 (0.0 MiB) |
| arena | 0 (0.0 MiB) / 0 (0.0 MiB) / 0 (0.0 MiB) | – | 2 (28.0 MiB) / 1 (14.0 MiB) / 1 (14.0 MiB) | 1 (14.0 MiB) / 0 (0.0 MiB) / 0 (0.0 MiB) | 1 (14.0 MiB) / 1 (14.0 MiB) / 1 (14.0 MiB) |
| street | 1 (14.0 MiB) / 1 (14.0 MiB) / 1 (14.0 MiB) | – | 0 (0.0 MiB) / 0 (0.0 MiB) / 0 (0.0 MiB) | 1 (14.0 MiB) / 1 (14.0 MiB) / 1 (14.0 MiB) | 1 (14.0 MiB) / 1 (14.0 MiB) / 1 (14.0 MiB) |

### M2 pool: first → peak layers, growths, random / far / tour

| Map | before | 9k-cap26-noprism | 9k-cap26 | 9l-cap26 | 9l-cap19 |
|---|---|---|---|---|---|
| occlusion-test | 10→10, 0 / 10→10, 0 / 10→10, 0 | – | 9→9, 0 / 9→9, 0 / 9→10, 1 | 9→9, 0 / 9→9, 0 / 9→9, 0 | 10→10, 0 / 10→10, 0 / 10→10, 0 |
| hallway | 15→34, 12 / 15→38, 6 / 15→34, 3 | – | 15→23, 8 / 15→32, 8 / 15→20, 2 | 15→31, 5 / 15→38, 9 / 15→24, 2 | 15→28, 9 / 15→35, 4 / 15→28, 2 |
| campaign-test | 15→15, 0 / 15→16, 1 / 15→15, 0 | – | 15→15, 0 / 15→16, 1 / 15→15, 0 | 15→15, 0 / 15→15, 0 / 15→15, 0 | 15→15, 0 / 15→17, 2 / 15→15, 0 |
| arena | 2→2, 0 / 2→2, 0 / 2→2, 0 | – | 3→3, 0 / 3→3, 0 / 3→3, 0 | 2→2, 0 / 2→2, 0 / 2→2, 0 | 2→2, 0 / 2→2, 0 / 2→2, 0 |
| street | 2→2, 0 / 2→2, 0 / 2→2, 0 | – | 1→1, 0 / 1→1, 0 / 1→1, 0 | 2→2, 0 / 2→2, 0 / 2→2, 0 | 2→2, 0 / 2→2, 0 / 2→2, 0 |

### M3: seal lines per mover

| Map | before | 9k-cap26-noprism | 9k-cap26 | 9l-cap26 | 9l-cap19 |
|---|---|---|---|---|---|
| occlusion-test | none | none | none | none | none |
| hallway | m0 5, m1 4, m2 2, m3 2, m4 5, m5 2, m6 1, m7 1, m8 1 | m0 6, m1 4, m2 5, m3 4, m4 6, m5 4, m6 2, m7 2, m8 2 | m0 7, m1 8, m2 4, m3 7, m4 4, m5 7, m6 2, m7 1, m8 2 | m0 6, m1 6, m2 5, m3 4, m4 6, m5 4, m6 2, m7 1, m8 2 | m0 6, m1 6, m2 4, m3 4, m4 6, m5 4, m6 2, m7 1, m8 2 |
| campaign-test | m0 2 | m0 5, m1 5 | m0 5, m1 5 | m0 5, m1 5 | m0 5, m1 5 |
| arena | m0 1, m1 1, m2 4, m3 4, m4 1, m5 4 | m0 3, m1 3, m2 4, m3 4, m4 3, m5 5 | m0 3, m1 3, m2 5, m3 5, m4 3, m5 5 | m0 3, m1 3, m2 5, m3 5, m4 3, m5 5 | m0 3, m1 3, m2 5, m3 5, m4 3, m5 5 |
| street | none | none | none | none | none |

### M4: leak; exterior / interior empty leaves (full tree); open edges

| Map | before | 9k-cap26-noprism | 9k-cap26 | 9l-cap26 | 9l-cap19 |
|---|---|---|---|---|---|
| occlusion-test | none; ext/int leaves 97 / 287; open edges 0 | none; ext/int leaves 122 / 416; open edges 0 | none; ext/int leaves 160 / 487; open edges 0 | none; ext/int leaves 136 / 428; open edges 0 | none; ext/int leaves 136 / 436; open edges 0 |
| hallway | none; ext/int leaves 411 / 2082; open edges 283 | none; ext/int leaves 416 / 3259; open edges 563 | none; ext/int leaves 416 / 4834; open edges 649 | none; ext/int leaves 416 / 3602; open edges 557 | none; ext/int leaves 416 / 3944; open edges 564 |
| campaign-test | none; ext/int leaves 114 / 207; open edges 9 | none; ext/int leaves 241 / 343; open edges 43 | none; ext/int leaves 243 / 392; open edges 43 | none; ext/int leaves 241 / 354; open edges 43 | none; ext/int leaves 241 / 427; open edges 37 |
| arena | none; ext/int leaves 228 / 560; open edges 24 | none; ext/int leaves 206 / 821; open edges 24 | none; ext/int leaves 204 / 1120; open edges 28 | none; ext/int leaves 206 / 963; open edges 28 | none; ext/int leaves 207 / 1161; open edges 39 |
| street | none; ext/int leaves 177 / 1985; open edges 396 | none; ext/int leaves 307 / 2918; open edges 591 | none; ext/int leaves 307 / 3761; open edges 624 | none; ext/int leaves 307 / 2966; open edges 601 | none; ext/int leaves 307 / 3150; open edges 628 |

### M5: occluders (occlude alone; sections occluding)

| Map | before | 9k-cap26-noprism | 9k-cap26 | 9l-cap26 | 9l-cap19 |
|---|---|---|---|---|---|
| occlusion-test | – | 29 alone; 5 of 11 sections (17 brushes); len 20 | 29 alone; 5 of 11 sections (17 brushes); len 20 | 29 alone; 5 of 11 sections (17 brushes); len 20 | 29 alone; 5 of 11 sections (17 brushes); len 20 |
| hallway | – | 738 alone; 12 of 99 sections (51 brushes); len 20 | 738 alone; 12 of 99 sections (51 brushes); len 20 | 738 alone; 12 of 99 sections (51 brushes); len 20 | 738 alone; 12 of 99 sections (51 brushes); len 20 |
| campaign-test | – | 40 alone; 7 of 15 sections (21 brushes); len 20 | 40 alone; 7 of 15 sections (21 brushes); len 20 | 40 alone; 7 of 15 sections (21 brushes); len 20 | 40 alone; 7 of 15 sections (21 brushes); len 20 |
| arena | – | 237 alone; 2 of 11 sections (10 brushes); len 20 | 237 alone; 2 of 11 sections (10 brushes); len 20 | 237 alone; 2 of 11 sections (10 brushes); len 20 | 237 alone; 2 of 11 sections (10 brushes); len 20 |
| street | – | 72 alone; 13 of 37 sections (1070 brushes); len 20 | 72 alone; 13 of 37 sections (1070 brushes); len 20 | 72 alone; 13 of 37 sections (1070 brushes); len 20 | 72 alone; 13 of 37 sections (1070 brushes); len 20 |

### M5: occluder-tier leaves

| Map | before | 9k-cap26-noprism | 9k-cap26 | 9l-cap26 | 9l-cap19 |
|---|---|---|---|---|---|
| occlusion-test | – | 313 | 416 | 339 | 350 |
| hallway | – | 5,110 | 6,973 | 5,465 | 5,755 |
| campaign-test | – | 559 | 606 | 566 | 623 |
| arena | – | 1,131 | 1,429 | 1,276 | 1,464 |
| street | – | 1,897 | 3,345 | 1,942 | 2,107 |

### M5: occluder-tier leaves holding several interior pockets (9j on)

| Map | before | 9k-cap26-noprism | 9k-cap26 | 9l-cap26 | 9l-cap19 |
|---|---|---|---|---|---|
| occlusion-test | – | 0 | 0 | 0 | 0 |
| hallway | – | 18 | 18 | 18 | 18 |
| campaign-test | – | 0 | 0 | 0 | 0 |
| arena | – | 4 | 0 | 3 | 3 |
| street | – | 78 | 179 | 76 | 87 |

### M5: shadow prisms (9k: occluding faces; 9l: wall faces with a free end, end planes; ended by the length bound)

| Map | before | 9k-cap26-noprism | 9k-cap26 | 9l-cap26 | 9l-cap19 |
|---|---|---|---|---|---|
| occlusion-test | – | 0 (0 length-bounded) | 17 (6 length-bounded) | 8 walls, 14 end planes (3 length-bounded) | 8 walls, 14 end planes (3 length-bounded) |
| hallway | – | 0 (0 length-bounded) | 1,000 (164 length-bounded) | 263 walls, 285 end planes (124 length-bounded) | 263 walls, 285 end planes (124 length-bounded) |
| campaign-test | – | 0 (0 length-bounded) | 11 (2 length-bounded) | 4 walls, 6 end planes (0 length-bounded) | 4 walls, 6 end planes (0 length-bounded) |
| arena | – | 0 (0 length-bounded) | 238 (68 length-bounded) | 136 walls, 166 end planes (64 length-bounded) | 136 walls, 166 end planes (64 length-bounded) |
| street | – | 0 (0 length-bounded) | 88 (14 length-bounded) | 17 walls, 28 end planes (1 length-bounded) | 17 walls, 28 end planes (1 length-bounded) |

### M5: cells (interior / solid / exterior)

| Map | before | 9k-cap26-noprism | 9k-cap26 | 9l-cap26 | 9l-cap19 |
|---|---|---|---|---|---|
| occlusion-test | 555 (287/171/97) | 484 (107/255/122) | 592 (174/258/160) | 510 (119/255/136) | 521 (130/255/136) |
| hallway | 5,671 (2,082/3,178/411) | 7,465 (1,516/5,533/416) | 10,021 (3,268/6,337/416) | 7,891 (1,859/5,616/416) | 8,249 (2,151/5,682/416) |
| campaign-test | 487 (207/166/114) | 832 (129/462/241) | 879 (165/471/243) | 840 (136/463/241) | 919 (193/485/241) |
| arena | 1,479 (560/691/228) | 1,393 (395/792/206) | 1,718 (683/831/204) | 1,550 (537/807/206) | 1,746 (718/821/207) |
| street | 3,709 (1,985/1,547/177) | 4,792 (1,607/2,878/307) | 6,510 (3,144/3,059/307) | 4,864 (1,648/2,909/307) | 5,080 (1,831/2,942/307) |

### M5: runtime walkable portals

| Map | before | 9k-cap26-noprism | 9k-cap26 | 9l-cap26 | 9l-cap19 |
|---|---|---|---|---|---|
| occlusion-test | 693 | 241 | 430 | 270 | 298 |
| hallway | 4,722 | 2,368 | 5,357 | 2,926 | 3,499 |
| campaign-test | 524 | 288 | 402 | 306 | 460 |
| arena | 1,333 | 608 | 1,128 | 844 | 1,188 |
| street | 6,288 | 3,756 | 7,667 | 3,885 | 4,381 |

### M5: PRL faces = lightmap charts / triangles (extra charts from atlas sub-chart cuts, already in the face count)

| Map | before | 9k-cap26-noprism | 9k-cap26 | 9l-cap26 | 9l-cap19 |
|---|---|---|---|---|---|
| occlusion-test | 1,018 / 2,036 (0 cut) | 1,057 / 2,114 (0 cut) | 1,113 / 2,226 (0 cut) | 1,055 / 2,110 (0 cut) | 1,066 / 2,132 (0 cut) |
| hallway | 8,457 / 16,910 (12 cut) | 11,370 / 22,594 (0 cut) | 13,804 / 27,403 (0 cut) | 11,736 / 23,326 (0 cut) | 12,160 / 24,182 (0 cut) |
| campaign-test | 808 / 1,612 (0 cut) | 1,022 / 2,038 (0 cut) | 1,081 / 2,156 (0 cut) | 1,035 / 2,064 (0 cut) | 1,148 / 2,291 (0 cut) |
| arena | 2,239 / 4,478 (0 cut) | 2,899 / 5,798 (0 cut) | 3,362 / 6,724 (0 cut) | 3,068 / 6,136 (0 cut) | 3,456 / 6,911 (0 cut) |
| street | 7,657 / 15,315 (0 cut) | 10,441 / 20,881 (0 cut) | 11,101 / 22,201 (0 cut) | 10,534 / 21,067 (0 cut) | 10,711 / 21,421 (0 cut) |

### M5: interior cell largest extent p50 / p95 / max (m)

| Map | before | 9k-cap26-noprism | 9k-cap26 | 9l-cap26 | 9l-cap19 |
|---|---|---|---|---|---|
| occlusion-test | 13.0 / 21.5 / 26.0 | 17.1 / 26.0 / 26.6 | 11.6 / 24.4 / 26.0 | 13.0 / 26.0 / 26.0 | 13.7 / 21.9 / 25.2 |
| hallway | 10.2 / 26.0 / 52.0 | 9.8 / 27.6 / 53.6 | 6.5 / 21.1 / 32.5 | 12.2 / 26.0 / 35.8 | 13.0 / 20.3 / 39.0 |
| campaign-test | 9.8 / 22.8 / 35.8 | 15.8 / 26.0 / 26.0 | 13.8 / 26.0 / 26.0 | 15.0 / 26.0 / 26.0 | 13.0 / 20.3 / 24.8 |
| arena | 16.3 / 29.3 / 94.3 | 26.0 / 26.0 / 58.5 | 14.6 / 26.0 / 29.3 | 16.3 / 26.0 / 58.5 | 14.6 / 22.8 / 39.0 |
| street | 5.8 / 24.4 / 88.0 | 2.3 / 26.0 / 32.0 | 5.0 / 26.0 / 32.0 | 3.0 / 26.0 / 32.0 | 5.7 / 19.5 / 26.0 |

### M5: interior cells with smallest extent 0.25-0.5 m

| Map | before | 9k-cap26-noprism | 9k-cap26 | 9l-cap26 | 9l-cap19 |
|---|---|---|---|---|---|
| occlusion-test | 22 | 42 | 65 | 47 | 50 |
| hallway | 49 | 2 | 2 | 2 | 2 |
| campaign-test | 33 | 66 | 75 | 66 | 88 |
| arena | 5 | 0 | 0 | 0 | 0 |
| street | 769 | 998 | 1,727 | 1,016 | 1,076 |

### M5: interior cells with smallest extent under 0.5 m (slivers)

| Map | before | 9k-cap26-noprism | 9k-cap26 | 9l-cap26 | 9l-cap19 |
|---|---|---|---|---|---|
| occlusion-test | 25 | 48 | 76 | 55 | 58 |
| hallway | 92 | 11 | 28 | 11 | 11 |
| campaign-test | 39 | 66 | 75 | 66 | 88 |
| arena | 8 | 0 | 0 | 0 | 0 |
| street | 934 | 1,172 | 1,930 | 1,196 | 1,277 |

### M5: SH clusters (indivisible oversize)

| Map | before | 9k-cap26-noprism | 9k-cap26 | 9l-cap26 | 9l-cap19 |
|---|---|---|---|---|---|
| occlusion-test | 197 (0) | 390 (2) | 433 (3) | 404 (2) | 404 (2) |
| hallway | 3,386 (0) | 6,192 (3) | 7,062 (3) | 6,287 (3) | 6,368 (3) |
| campaign-test | 188 (0) | 721 (2) | 734 (2) | 723 (2) | 751 (1) |
| arena | 752 (0) | 1,058 (2) | 1,106 (2) | 1,074 (3) | 1,099 (2) |
| street | 1,915 (0) | 3,701 (14) | 3,912 (5) | 3,730 (14) | 3,755 (14) |

### id-51 lightmap mandatory set at L 16 m, max per camera cell (texels)

| Map | before | 9k-cap26-noprism | 9k-cap26 | 9l-cap26 | 9l-cap19 |
|---|---|---|---|---|---|
| occlusion-test | 30.6 M (p95 30.4; 0 over 62.9) | – | 24.9 M (p95 24.8; 0 over 62.9) | 25.6 M (p95 25.6; 0 over 62.9) | 25.3 M (p95 25.3; 0 over 62.9) |
| hallway | 97.3 M (p95 57.9; 177 over 62.9) | – | 84.2 M (p95 50.7; 97 over 62.9) | 104.2 M (p95 61.2; 371 over 62.9) | 99.3 M (p95 57.7; 198 over 62.9) |
| campaign-test | 37.3 M (p95 34.4; 0 over 62.9) | – | 35.9 M (p95 33.1; 0 over 62.9) | 34.4 M (p95 32.5; 0 over 62.9) | 35.2 M (p95 27.2; 0 over 62.9) |
| arena | 1.2 M (p95 0.7; 0 over 62.9) | – | 0.9 M (p95 0.7; 0 over 62.9) | 0.9 M (p95 0.7; 0 over 62.9) | 0.9 M (p95 0.7; 0 over 62.9) |
| street | 2.8 M (p95 2.7; 0 over 62.9) | – | 2.5 M (p95 2.4; 0 over 62.9) | 2.4 M (p95 2.4; 0 over 62.9) | 2.4 M (p95 2.3; 0 over 62.9) |

### M6: portals tested, mean / p95 per pose

| Map | before | 9k-cap26-noprism | 9k-cap26 | 9l-cap26 | 9l-cap19 |
|---|---|---|---|---|---|
| occlusion-test | 464.3 / 1,749 | 179.6 / 467 | 316.7 / 925 | 197.8 / 536 | 215.7 / 627 |
| hallway | 65.3 / 196 | 51.2 / 279 | 104.3 / 539 | 56.4 / 275 | 68.0 / 358 |
| campaign-test | 108.1 / 296 | 74.4 / 250 | 100.0 / 387 | 79.6 / 256 | 106.8 / 343 |
| arena | 58.8 / 202 | 32.3 / 151 | 47.7 / 178 | 38.9 / 160 | 53.1 / 220 |
| street | 2,042.6 / 5,000 | 1,256.1 / 3,511 | 1,982.6 / 5,000 | 1,286.8 / 3,610 | 1,365.8 / 3,936 |

### M6: drawn cells, mean / p95 per pose

| Map | before | 9k-cap26-noprism | 9k-cap26 | 9l-cap26 | 9l-cap19 |
|---|---|---|---|---|---|
| occlusion-test | 66.6 / 242 | 27.8 / 65 | 40.3 / 101 | 29.7 / 71 | 31.9 / 80 |
| hallway | 11.9 / 33 | 11.0 / 47 | 20.7 / 73 | 12.8 / 48 | 14.7 / 59 |
| campaign-test | 15.9 / 41 | 12.9 / 39 | 14.9 / 48 | 13.2 / 40 | 16.1 / 46 |
| arena | 9.6 / 28 | 8.5 / 28 | 12.0 / 38 | 10.3 / 32 | 13.2 / 42 |
| street | 272.1 / 1,386 | 147.2 / 401 | 362.3 / 2,070 | 152.5 / 405 | 170.2 / 449 |

### M6: tris in frustum, mean / p95 per pose

| Map | before | 9k-cap26-noprism | 9k-cap26 | 9l-cap26 | 9l-cap19 |
|---|---|---|---|---|---|
| occlusion-test | 385.9 / 1,516 | 398.3 / 1,472 | 414.6 / 1,546 | 396.6 / 1,476 | 399.0 / 1,498 |
| hallway | 80.3 / 230 | 125.6 / 432 | 137.1 / 496 | 124.0 / 426 | 127.6 / 448 |
| campaign-test | 101.1 / 283 | 175.1 / 574 | 180.1 / 584 | 175.2 / 578 | 177.9 / 552 |
| arena | 65.4 / 200 | 93.4 / 296 | 97.9 / 308 | 94.2 / 296 | 102.0 / 328 |
| street | 1,939.3 / 10,297 | 1,777.4 / 5,402 | 2,548.5 / 15,637 | 1,817.5 / 5,422 | 1,864.4 / 5,468 |

### M6: drawn lightmap texels, mean / p95 per pose

| Map | before | 9k-cap26-noprism | 9k-cap26 | 9l-cap26 | 9l-cap19 |
|---|---|---|---|---|---|
| occlusion-test | 9.41 M / 24.89 M | 7.74 M / 16.83 M | 7.08 M / 15.88 M | 7.85 M / 17.29 M | 7.49 M / 17.00 M |
| hallway | 6.15 M / 24.04 M | 6.72 M / 22.44 M | 5.36 M / 19.11 M | 6.09 M / 21.54 M | 5.39 M / 18.45 M |
| campaign-test | 5.66 M / 14.03 M | 6.23 M / 13.39 M | 5.84 M / 12.66 M | 6.17 M / 13.39 M | 5.06 M / 11.77 M |
| arena | 0.12 M / 0.52 M | 0.10 M / 0.28 M | 0.09 M / 0.27 M | 0.10 M / 0.28 M | 0.09 M / 0.25 M |
| street | 0.64 M / 2.31 M | 0.37 M / 0.98 M | 0.39 M / 1.84 M | 0.37 M / 0.97 M | 0.33 M / 0.87 M |

### M6: portals tested, max per pose (a tripped pose stops at 5,000)

| Map | before | 9k-cap26-noprism | 9k-cap26 | 9l-cap26 | 9l-cap19 |
|---|---|---|---|---|---|
| occlusion-test | 2,084 (0 trips) | 596 (0 trips) | 1,146 (0 trips) | 679 (0 trips) | 803 (0 trips) |
| hallway | 998 (0 trips) | 755 (0 trips) | 1,614 (0 trips) | 691 (0 trips) | 904 (0 trips) |
| campaign-test | 524 (0 trips) | 378 (0 trips) | 568 (0 trips) | 378 (0 trips) | 465 (0 trips) |
| arena | 686 (0 trips) | 350 (0 trips) | 407 (0 trips) | 407 (0 trips) | 639 (0 trips) |
| street | 5,000 (518 trips) | 5,000 (19 trips) | 5,000 (312 trips) | 5,000 (29 trips) | 5,000 (41 trips) |

### Guardrail: mean portals tested per pose against before

| Map | before | 9k-cap26-noprism | 9k-cap26 | 9l-cap26 | 9l-cap19 |
|---|---|---|---|---|---|
| occlusion-test | +0% | -61% | -32% | -57% | -54% |
| hallway | +0% | -22% | +60% | -14% | +4% |
| campaign-test | +0% | -31% | -7% | -26% | -1% |
| arena | +0% | -45% | -19% | -34% | -10% |
| street | +0% | -39% | -3% | -37% | -33% |

### Guardrail: mean tris in frustum per pose against before

| Map | before | 9k-cap26-noprism | 9k-cap26 | 9l-cap26 | 9l-cap19 |
|---|---|---|---|---|---|
| occlusion-test | +0% | +3% | +7% | +3% | +3% |
| hallway | +0% | +56% | +71% | +54% | +59% |
| campaign-test | +0% | +73% | +78% | +73% | +76% |
| arena | +0% | +43% | +50% | +44% | +56% |
| street | +0% | -8% | +31% | -6% | -4% |

### Guardrail: mean drawn lightmap texels per pose against before

| Map | before | 9k-cap26-noprism | 9k-cap26 | 9l-cap26 | 9l-cap19 |
|---|---|---|---|---|---|
| occlusion-test | +0% | -18% | -25% | -17% | -20% |
| hallway | +0% | +9% | -13% | -1% | -12% |
| campaign-test | +0% | +10% | +3% | +9% | -10% |
| arena | +0% | -15% | -25% | -20% | -28% |
| street | +0% | -43% | -39% | -43% | -48% |

### SH mandatory clusters per camera cell at L 16 m (owner closure included), median / p95 / max

| Map | before | 9k-cap26-noprism | 9k-cap26 | 9l-cap26 | 9l-cap19 |
|---|---|---|---|---|---|
| occlusion-test | 21 / 22 / 22 | – | 15 / 16 / 16 | 14 / 14 / 14 | 14 / 14 / 14 |
| hallway | 83 / 100 / 105 | – | 122 / 144 / 165 | 118 / 143 / 154 | 122 / 143 / 156 |
| campaign-test | 7 / 14 / 15 | – | 15 / 18 / 18 | 12 / 17 / 17 | 14 / 21 / 22 |
| arena | 12 / 18 / 21 | – | 16 / 24 / 29 | 14 / 19 / 23 | 15 / 25 / 29 |
| street | 326 / 358 / 362 | – | 467 / 534 / 545 | 462 / 512 / 514 | 448 / 505 / 506 |

### SH probe reach into another cluster's cells (scratch reader)

| Map | before | 9k-cap26-noprism | 9k-cap26 | 9l-cap26 | 9l-cap19 |
|---|---|---|---|---|---|
| occlusion-test | 158 pairs, 48,770 refs; owner edges 52 | – | – | – | – |
| hallway | 2,363 pairs, 60,903 refs; owner edges 930 | – | – | – | – |
| campaign-test | 45 pairs, 27,053 refs; owner edges 19 | – | – | – | – |
| arena | 537 pairs, 4,849 refs; owner edges 125 | – | – | – | – |
| street | 7,917 pairs, 85,920 refs; owner edges 949 | – | – | – | – |

### M7: arena box

| before | 9k-cap26-noprism | 9k-cap26 | 9l-cap26 | 9l-cap19 |
|---|---|---|---|---|
| 58 cells, 117 portals on 40 planes | 74 cells, 166 portals on 12 planes | 86 cells, 193 portals on 13 planes | 86 cells, 193 portals on 13 planes | 129 cells, 292 portals on 17 planes |

### M7: stairs box

| before | 9k-cap26-noprism | 9k-cap26 | 9l-cap26 | 9l-cap19 |
|---|---|---|---|---|
| 32 cells, 58 portals on 25 planes; riser planes 23, tread planes 0 | 6 cells, 7 portals on 3 planes; riser planes 0, tread planes 1 (11.379) | 6 cells, 7 portals on 3 planes; riser planes 0, tread planes 1 (11.379) | 6 cells, 7 portals on 3 planes; riser planes 0, tread planes 1 (11.379) | 6 cells, 7 portals on 3 planes; riser planes 0, tread planes 1 (11.379) |

### Street step-cap trips: total / also tripped before / new

| Set | before | 9k-cap26-noprism | 9k-cap26 | 9l-cap26 | 9l-cap19 |
|---|---|---|---|---|---|
| street level (2,960 poses) | 518 / 518 / 0 | 19 / 19 / 0 | 312 / 245 / 67 | 29 / 29 / 0 | 41 / 40 / 1 |
| rooftop (608 poses) | 212 / 212 / 0 | 27 / 27 / 0 | 168 / 153 / 15 | 26 / 26 / 0 | 37 / 37 / 0 |

### Rooftop set (stress-street, 608 poses)

| Row | before | 9k-cap26-noprism | 9k-cap26 | 9l-cap26 | 9l-cap19 |
|---|---|---|---|---|---|
| step-cap trips | 212 | 27 | 168 | 26 | 37 |
| new drawn-miss poses | 0 | 0 | 0 | 0 | 0 |
| culled / faceless (reported) | 0 / 0 | 0 / 0 | 0 / 0 | 0 / 0 | 0 / 0 |
| portals tested mean / p95 | 2,789.4 / 5,000 | 1,916.3 / 4,696 | 2,956.8 / 5,000 | 1,938.5 / 4,785 | 2,075.1 / 5,000 |
| drawn cells mean / p95 | 426.7 / 1,409 | 231.9 / 561 | 557.5 / 1,963 | 235.8 / 564 | 269.5 / 882 |
| tris in frustum mean / p95 | 3,035.3 / 10,847 | 2,703.9 / 7,355 | 3,857.7 / 15,129 | 2,721.4 / 7,413 | 2,865.6 / 10,464 |
| drawn lightmap texels mean / p95 | 0.86 M / 2.22 M | 0.53 M / 1.19 M | 0.58 M / 1.75 M | 0.52 M / 1.19 M | 0.49 M / 1.32 M |
| paths | ("portal", 396), ("step_limit", 212) | ("portal", 581), ("step_limit", 27) | ("portal", 440), ("step_limit", 168) | ("portal", 582), ("step_limit", 26) | ("portal", 571), ("step_limit", 37) |

### Uncapped portal tests per pose, street level (2,960 poses)

| Row | before | 9k-cap26-noprism | 9k-cap26 | 9l-cap26 | 9l-cap19 |
|---|---|---|---|---|---|
| mean | 2,553.7 | 1,259.5 | 2,150.6 | 1,292.5 | 1,375.7 |
| p50 / p95 / p99 | 1,409 / 8,891 / 12,352 | 995 / 3,511 / 4,636 | 1,471 / 6,298 / 8,317 | 1,017 / 3,610 / 4,994 | 1,041 / 3,936 / 5,259 |
| max (worst pose) | 18,513 (`g5.0.0@180`) | 6,045 (`g16.14.0@90`) | 11,587 (`g5.3.0@180`) | 6,467 (`g13.30.0@0`) | 6,775 (`g6.3.0@180`) |
| poses over 5,000 | 518 | 19 | 312 | 29 | 41 |
| 2× worst walk | 37,026 | 12,090 | 23,174 | 12,934 | 13,550 |
| total tests | 7,558,805 | 3,728,158 | 6,365,853 | 3,825,818 | 4,071,953 |

### Uncapped portal tests per pose, rooftop (608 poses)

| Row | before | 9k-cap26-noprism | 9k-cap26 | 9l-cap26 | 9l-cap19 |
|---|---|---|---|---|---|
| mean | 4,354.5 | 1,939.7 | 3,418.5 | 1,967.2 | 2,127.5 |
| p50 / p95 / p99 | 2,671 / 13,007 / 16,459 | 1,725 / 4,696 / 5,871 | 3,064 / 8,013 / 10,399 | 1,747 / 4,785 / 6,017 | 1,965 / 5,243 / 6,539 |
| max (worst pose) | 17,598 (`roof:A7.s2.c@225/-15`) | 6,466 (`roof:B4.s2.c@180/-15`) | 11,955 (`roof:B4.s2.c@180/-15`) | 6,608 (`roof:B4.s2.c@180/-15`) | 7,291 (`roof:B4.s2.c@180/-15`) |
| poses over 5,000 | 212 | 27 | 168 | 26 | 37 |
| 2× worst walk | 35,196 | 12,932 | 23,910 | 13,216 | 14,582 |
| total tests | 2,647,511 | 1,179,313 | 2,078,423 | 1,196,083 | 1,293,505 |

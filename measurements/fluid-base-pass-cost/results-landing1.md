# Fluid base-pass ablation (landing1)

Fluid = Fluid Surface Pass per complete frame; frame = union of every labelled interval per complete frame; clock = median busy core clock (min-max) per run.

| arm | runs valid | fluid ms | spread | forward ms | GPU frame ms | clocks |
|---|---|---|---|---|---|---|
| near-4k-stock | 2/2 | 2.164 | 0.007 | 23.929 | 27.930 | 1191 (503-1220); 1151 (1021-1205) |
| near-4k-today | 2/2 | 53.870 | 0.354 | 23.939 | 79.622 | 1222 (1097-1232); 1200 (1140-1228) |
| test-4k-stock | 2/2 | 2.128 | 0.002 | 19.356 | 25.915 | 1157 (874-1201); 1106 (282-1223) |
| test-4k-today | 2/2 | 21.716 | 0.008 | 20.080 | 50.732 | 1213 (1007-1229); 1182 (1043-1225) |
| near-auto-stock | 2/2 | 0.509 | 0.104 | 4.846 | 6.157 | 998 (308-1036); 81 (0-861) |
| near-auto-today | 2/2 | 9.900 | 0.010 | 4.341 | 14.959 | 1107 (917-1127); 1091 (936-1144) |
| test-auto-stock | 1/2 | 0.538 | n/a | 4.810 | 7.527 | 554 (385-770); 592 (423-977) |
| test-auto-today | 2/2 | 4.135 | 0.013 | 3.886 | 9.975 | 909 (727-1014); 980 (895-1085) |
| mid-auto-stock | 1/2 | 0.544 | n/a | 6.648 | 8.204 | 577 (464-733); 0 (0-960) |
| mid-auto-today | 1/2 | 4.180 | n/a | 4.322 | 9.201 | 948 (843-988); 1008 (934-1098) |

Invalid runs:
- landing1-test-auto-stock-r1: GPU frame time differs between complete (8.313 ms) and incomplete (9.2517 ms) frames: dropped rows lost time, not only labels
- landing1-mid-auto-today-r1: GPU frame time differs between complete (9.2608 ms) and incomplete (8.968 ms) frames: dropped rows lost time, not only labels
- landing1-mid-auto-stock-r2: GPU frame time differs between complete (6.3507 ms) and incomplete (7.8986 ms) frames: dropped rows lost time, not only labels

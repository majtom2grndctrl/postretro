# Follow-up — lightmap payload split on the frame thread

> **Status:** seed, not a spec. Recorded by `spatial-residency--lightmap-bounded-pool`, which leaves this path alone.

`payload_from_pair_bytes` splits each pair on the frame thread with `Vec::split_off`, copying the direction plane and shadowmask group B. `done/spatial-residency--lightmap-cell-blocks/findings.md` AC 23 measured a median of 0.5 ms, with spikes to 11–37 ms on the hallway from page faults on fresh allocations.

Question for a future session: can the issuer thread split, or read each plane into its own buffer? Absent mask planes shrink the copy, so measure after that brief lands.

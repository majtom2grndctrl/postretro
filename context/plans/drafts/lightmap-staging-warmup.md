# Follow-up — lightmap staging warm-up

> **Status:** seed, not a spec. Recorded by `spatial-residency--lightmap-bounded-pool`, which leaves this path alone.

The first lightmap drain pays for a cold staging pool: about 2.4 ms/MiB against 0.25 warm (`done/spatial-residency--lightmap-cell-blocks/findings.md`, anomaly 5). The spawn preload is exactly that drain.

Question for a future session: is it worth warming staging before the spawn preload? Absent mask planes shrink that drain, so measure after that brief lands.

# Follow-up — lightmap pool growth peak

> **Status:** seed, not a spec. Recorded by `spatial-residency--sh-bounded-pool`, which leaves this pool alone.

The lightmap pool grows the way the SH pool did: a replacement texture alongside the old one, retired after the copy (`PoolTextures`, `LightmapPoolModel`). On `stress-warren-hallway-inspection` it reaches 490 to 546 MiB across several growths, so old and new pools coexist in play. With the SH pool bounded, projected total peak is still near 2.2 GB against the 2 GB target (`context/lib/index.md` §1.1, `development_guide.md` §1.4).

Measured once the SH pool landed, hallway, Metal Resource Events `resource-size`: steady 2,170 MiB, peak 2,574 MiB at 27 s. That peak is this pool: its three textures grow 116/58/232 to 140/70/280 MiB with old and new alive together, about 404 MiB over steady and 526 MiB over the 2 GB target. Steady is flat from 60 s on. A full-route capture leaves about 19 GB of raw `.ktrace` in the user temp directory.

Question for a future session: can this pool be sized once from baked data, as the SH pool is? Id 51 and cell blocks hold the inputs. Measure first with `xctrace --instrument "Metal Resource Events"` after the SH work lands, because the peak's size depends on it.

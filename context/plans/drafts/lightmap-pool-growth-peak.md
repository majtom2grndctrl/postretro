# Follow-up — lightmap pool growth peak

> **Status:** seed, not a spec. Recorded by `spatial-residency--sh-bounded-pool`, which leaves this pool alone.

The lightmap pool grows the way the SH pool did: a replacement texture alongside the old one, retired after the copy (`PoolTextures`, `LightmapPoolModel`). On `stress-warren-hallway-inspection` it reaches 490 to 546 MiB across several growths, so old and new pools coexist in play. With the SH pool bounded, projected total peak is still near 2.2 GB against the 2 GB target (`context/lib/index.md` §1.1, `development_guide.md` §1.4).

Question for a future session: can this pool be sized once from baked data, as the SH pool is? Id 51 and cell blocks hold the inputs. Measure first with `xctrace --instrument "Metal Resource Events"` after the SH work lands, because the peak's size depends on it.

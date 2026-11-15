# Legacy SH bodies in ids 34/35/41

**Status: decided, opt-in flag (2026-11-15); supersedes the earlier keep
decision.** The owner chose `prl-build --whole-load-sh`, default off: it writes
the bodies of ids 27/34/35/41/45, and without it they are omitted whenever id 50
is emitted. The measurements below stand. The durable contract is in
`context/lib/build_pipeline.md` §PRL section IDs (Whole-load SH bodies).

## Question

A planning spike reported about 34 MiB of SH payload in ids 34/35/41 that the
default streaming modes never read. Options were: keep them, write them
smaller, or drop the bodies and feed `POSTRETRO_SH_STREAMING=off` from id 50.

## Who reads what (verified in source at `9580828`)

- **Streaming (`async`, `sync-proof`).** The SH manifest reads only metadata
  prefixes: id 34's header and per-probe records, id 35's 76-byte header, and
  id 41's header plus CSR tables (masks, levels, offsets, lights). The
  positional container refuses any whole-body read of ids 27/34/35/41/45 while
  SH streams. Payload comes from id 50 chunks.
- **`off`.** Reads ids 34/35/41 whole and installs the global atlases and the
  id-41 delta buffer directly.
- **Id-50 validation** checks source metadata, versions and geometry against
  id 34's metadata. It reads no legacy body.
- **Tests.** `off` is the reference the streamed path is checked against, for
  example the renderer test
  `c10_off_and_sync_proof_compose_the_same_channel_tiles_at_every_probe` and the
  mover frame captures run under both modes.

## Measurement

| Control | Value |
| --- | --- |
| Source revision | `9580828` |
| Source map | owner-supplied external showcase map, large open spaces, 158 point and 140 spot lights; not checked in |
| Source map SHA-256 | `8390307a34715047d6fd4c792213b0b4558aefdd3740a60797fc29451d61ced1` |
| Command | `prl-build showcase.map -o showcase.prl --no-tui -j 4 --lightmap-density 0.025` (dev profile, first build in a fresh checkout) |
| Bake time | 972 s total; SH Bake 233 s, Direct SH Bake 13 s, Packing 39 s |

| Id | Section | Section bytes | Metadata (read by every mode) | Body (read only by `off`) |
| --- | --- | ---: | ---: | ---: |
| 34 | OctahedralShVolume | 35,751,680 | 20,728,304 (2,590,980 probe records + header/tail) | 15,023,376 (BC6H atlas) |
| 35 | DirectShVolume | 15,023,452 | 76 | 15,023,376 (BC6H atlas) |
| 41 | DirectShDeltaVolumes | 6,264,722 | 553,898 (42,400 affinity cells, 668 entries) | 5,710,824 (f16 tiles) |
| | **Total** | 57,039,854 (54.40 MiB) | 21,282,278 (20.30 MiB) | **35,757,576 (34.10 MiB)** |

For scale: the PRL is 1,128,432,757 B (1,076 MiB). Id 22 is 490 MiB, id 42
204 MiB, id 50 235 MiB. The bodies are 3.2% of the file. The spike's 34 MiB
figure is the body total; the 20 MiB of metadata is not removable while
streaming reads it.

After: see the build's after-bake table (to be filled)

## Decision

- **Dropping in favour of id 50 has no cheap reconstruction.** The legacy
  atlas packs 6×6 tiles (border included) at a 6-texel pitch, so 4×4 BC6H
  blocks straddle tiles. Id 50 isolates each tile in its own 8×8 cell. Feeding
  `off` from id 50 means a CPU BC6H decode and re-encode of every dense tile at
  load, or uploading Rgba16Float atlases 8× larger than today's. Either way
  `off` would sample id 50's encoding, so it would stop being an independent
  reference for the streamed path. Rejected.
- **A second PRL shape, opt-in.** Chosen by the owner over keeping the bodies,
  which this record first concluded because the weight bought 3% of this file.
  Ids 27/34/35/41/45 each bump their section version and carry a body-presence
  flag. `--whole-load-sh` writes the bodies; the default omits them when id 50
  is emitted. A level without id 50 always carries them. `--release` does not
  imply the flag, because warm and `--release` builds must bake the same bytes.
  `off` refuses a level without bodies with one named error that names the
  rebake.
- **No player path reads the bodies.** `off` has no automatic fallback; it is a
  dev and test mode, so the default costs players nothing. Dev and test runs
  under `off` opt in with the flag.

## Revisit when

- `off` is retired or rebuilt on the streaming pool. The flag and the bodies
  then retire with it: the streaming manifest already reads only the sections'
  metadata.

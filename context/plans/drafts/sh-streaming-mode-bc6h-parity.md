# Follow-up — `off` and streamed SH loads decode different base tiles

> **Status:** deferred bug note, not a spec. Recorded so a later brief can scope it.
>
> **Origin:** surfaced by task 11b of `mover-light-flow--probe-channel` (2026-11-09) while proving C10.

## The problem

With no mover state and no id 57, a level loaded under `POSTRETRO_SH_STREAMING=off` composes different SH base tiles from the same level loaded streamed (`sync-proof`, `async`).

- The whole load decodes ids 34 and 35 from their compact atlases, which pack 6×6 tiles side by side, so each 4×4 BC6H block straddles tiles.
- Id 50 re-encodes every tile on its own in a block-aligned 8×8 cell.
- The same source tile therefore decodes to different texels in each mode.

Measured, at the composed tiles:

| Map | Indirect halves that differ | Worst | Direct halves that differ | Worst |
|---|---|---|---|---|
| mover-closet | 85,902 of 156,816 | 0.228 | 48,071 | 1.12 |
| mover-setpiece | 899,740 of 1,719,648 | 0.038 | 683,319 | 0.47 |

Frames differ by up to 9 levels on the closet's static indirect and up to 16 on the set-piece's baked direct.

Built with `--uncompressed-irradiance`, every tile and frame is bit-identical across modes. That proves the encoding is the whole cause.

Evidence: `context/plans/in-progress/mover-light-flow--probe-channel/research.md` §Task 11 and §Task 11b.

## Shape of a fix (unscoped)

One encoding serves both modes. For example, the whole-load atlas could decode from id 50's block-aligned per-tile encoding, or the compact atlases could align tiles to 4×4 blocks. Price the bytes against today's atlases.

## Acceptance sketch

On `mover-closet` and `mover-setpiece` with BC6H, every composed base tile is bit-identical between `off` and `sync-proof`.

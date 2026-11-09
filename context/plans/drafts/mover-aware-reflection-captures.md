# Follow-up — reflection probes ignore movers

> **Status:** deferred bug note, not a spec. Recorded so a later brief can scope it.
>
> **Origin:** surfaced at Stop 2 of `mover-light-flow--probe-channel` (2026-11-08), while capturing the dark closet with its door closed.

## The problem

Reflection probes (PRL id 54) are captured at compile time with every mover absent (`resource_management.md` §5.2). A probe near a mover therefore mirrors the scene as if the mover were not there. On `mover-closet`, the closet pool's probe sees the bright room through the open doorway. With the door closed, the pool still reflects a lit doorway.

- With the probe channel in place, station 2 reads 0.12 at the default reflection tier and 0.845 at the probe-only tier (`low`). The tiled static-door reference reads 0.009.
- With reflections off, the posed closet matches the reference: 0.009 at station 2 and 0.032 at station 3, against 0.009 and 0.033.
- The leak comes only from reflections. It is outside the probe channel, so `mover-light-flow--probe-channel` judges its closet rows with reflections off (owner, 2026-11-08).

Evidence: `context/plans/in-progress/mover-light-flow--probe-channel/research.md` §Captures, and `captures/t2-closet-sheet.png` (×20 rows) in the same folder.

## Shape of a fix (unscoped)

- A probe whose capture sees a mover needs that mover's pose. A door that travels needs a capture at each end of its travel, blended by the mover's t′, as the probe channel blends S₀ and S₁. That reuses the probe channel's blend scalar and ramp.
- Which probes need it can be decided at compile time from their capture rays, just as the probe channel derives its depth domain.
- Cost: id 54's tiles are 128² colour plus distance. At the cap, the arrays already cost 12 MiB. Two poses per affected probe would grow that, so price it against the reflection-probe cap.

## Acceptance sketch

On `mover-closet` with the door closed, at the default reflection tier, stations 2 and 3 read no brighter than the tiled static-door reference plus 0.02. With the door open, they read as today.

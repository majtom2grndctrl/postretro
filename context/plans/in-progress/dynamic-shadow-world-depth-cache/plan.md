# dynamic-shadow-world-depth-cache — plan of record

status: blocked
read at: 07a9c209

## Decision premise false

- **“Separate, capped spot and cube budgets … each well below its pool’s size”**
  cannot currently be reconciled with the coverage requirement for campaign-test.
  The map contains 11 `light_dynamic_spot` fixtures and 7 `light_dynamic` (point)
  fixtures, while `CUBE_COUNT` is 6. A six-layer cube cache is the only cap that
  can cover every occupied cube-pool slot; it is not below that pool’s size. The
  cited “~18–19” figure is the combined fixture/candidate count, not a measured,
  per-kind stable pool occupancy. The renderer has no recorded campaign-test
  occupancy probe. A diagnostic launch reached the desktop app but produced no
  occupancy record before it was stopped, so it cannot establish a smaller cube
  cap.

The owner must choose one of these Decision-level changes before planning can
continue:

1. Permit the dynamic cube cache budget to equal `CUBE_COUNT` when campaign-test
   coverage requires it; or
2. Replace the coverage/timing requirement with a measured per-kind occupancy
   ceiling, then set the cube cache cap to that measured lower value.

## Source verification

- The frozen candidate/matrix premise holds: candidate lists are installed in
  `renderer_full_init.rs` and `renderer_resources.rs`, and slot updates rebuild
  their matrices from those frozen candidates.
- Dynamic and promoted lights remain separated by the candidate selection and
  count-split forward paths; no dynamic candidate produces a
  `PromotedStaticLightRecord`.
- The cited dynamic depth-pass, forward-world, entity-receiver, promoted-cache,
  lifecycle, and bind-group seams all exist at the named paths.

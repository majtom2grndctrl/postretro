# Task 1 addendum — runtime-safe selective controller

Follow-up to the emitted-reconstruction and attribution diagnostic. This is
new corrective scope for the active plan. It is bake-time only. It changes no
PRL version, payload layout, or runtime compose shader.

## Evidence

The corrected 1.0 m Stress-Warren bake has 37 final-output failures after
sparse-L1 parity correction. Each passes when one present section is restored
to L0 in the stored all-unit state. That result identifies a small repair set,
but it is not a runtime safety proof: direct-light promotion weights can vary
independently, and the id-27/id-45 contributions can be script-mutable or
animated.

Default-on remains blocked until the runtime-safe measurements below are
recorded and their policy cost is accepted.

## Runtime-safe split

### Id 41 direct entries

Id 41 uses an entry-resolved triangle-residual envelope. For each candidate
cell, evaluate the emitted reconstruction residual of every direct entry
against its dense L0 reference. Bound the residual for independent promotion
weights in `[0,1]` by summing entry residual magnitudes. Do not rely on
cross-entry cancellation. The final clamp is included only when the same
envelope bounds it safely.

The gate is a **cancellation-free absolute-error budget normalized by dense
reference illumination and the current darkness floor**. It is not a universal
relative-error guarantee for every runtime light state.

### Id 27 and id 45

Existing id-27 script-mutable slots and id-45 animated/script-mutable slots
have no finite bake-known amplitude bound. The runtime-safe path must not
assume one. Treat their affected cells as L0 until a separate authored/runtime
contract supplies a finite bound.

Task 1 must measure and report, before selecting a promotion policy:

- affected cells and entries by section;
- cells that must remain L0 under this rule; and
- resulting payload bytes and retained-payload ratio.

An empty immutable subset is a valid result. It is evidence for a later bounded
animation contract, not permission to use an all-unit proxy.

## Controller

After independent classification, apply the id-41 envelope check to final
levels using emitted reconstruction: f16 storage round trip, L2 mean, and
sparse-L1 shader-zero fallback. For a failing participating cell, consider
restoring that id-41 cell to L0. Keep the restore only when it brings the
envelope within both settled limits. If it cannot, fail the bake loudly and
leave the cell L0. Do not invent a runtime fallback.

Run existing participating-cell seam smoothing after restores. Re-run the
envelope after smoothing until levels reach a fixed point. The process only
demotes to L0, so it terminates. Preserve I5; repair and smoothing never lift
a level.

This addendum does not define joint optimization across sections. Id-27/id-45
remain L0 wherever their mutable/animated status prevents a finite envelope.

## Verification

At 1.0 m on `stress-warren-showcase.map`, report:

- id-41 envelope failures before and after repair;
- selected id-41 restore count and incremental payload after smoothing;
- forced-L0 id-27/id-45 cells, entries, payload bytes, and retained ratios;
- post-smoothing participating I5; and
- the cancellation-free budget values and dense-reference floor used.

Re-run Campaign and Kinematic only after the safety result clears and the
forced-L0 cost is accepted. Retain their existing win, cap, timing, and visual
gates.

## Non-goals

- Treating the stored all-unit combined state as a runtime safety guarantee.
- Joint optimization across multiple restored sections.
- A bounded id-27/id-45 animation or script-mutation contract.
- Changing coarsening thresholds, base density, payload cap policy, shader
  reconstruction, PRL format, or runtime light behavior.

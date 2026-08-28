# Task 1 addendum — selective joint final-output controller

Follow-up to the emitted-reconstruction and attribution diagnostic. This is
new corrective scope for the active plan. It is bake-time only. It changes no
PRL version, payload layout, or runtime compose shader.

## Evidence

The corrected 1.0 m Stress-Warren bake has 37 final combined-output failures
after sparse-L1 parity correction. Each is remediable by restoring one present
section to L0 under the emitted reconstruction oracle. The selected restores
add an estimated 478,080 payload bytes before a later smoothing pass. This is
small beside the retained coarsened payload and turns a broad classifier-parity
failure into a bounded final-output control problem.

The attribution is cancellation-safe: it measures the combined residual after
one section is restored, rather than ranking independent section errors.

## Controller

Run the controller after independent per-section classification. It evaluates
the final emitted-style combined RGB reconstruction against dense L0 truth,
using the settled floored composed-error limits. It operates only on present,
grid-matched sections.

For every failing participating cell:

1. Consider each present section independently, restoring that cell in that
   section to L0 while keeping all other final levels unchanged.
2. Reconstruct the resulting combined output with the emitted representation.
   A sparse-L1 candidate uses the shader-equivalent zero fallback; L2 uses its
   emitted mean; stored values include the f16 round trip.
3. Keep only candidates that pass both final composed-error limits.
4. Choose the candidate with the least incremental emitted payload bytes.
   Break equal-byte ties by ascending `SectionId` (27, then 41, then 45).
5. Apply the chosen restore. If no one-section restore passes, fail the bake
   loudly and leave the cell all-L0. Do not invent a runtime fallback or a
   multi-section recovery policy in this scope.

Run the existing seam smoothing after restores, then re-run the same final
emitted-style combined-error check. If smoothing changes any level and creates
a failure, repeat controller and smoothing until levels reach a fixed point.
The process only demotes to L0, so it terminates. If a fixed-point failure has
no passing one-section restore, use the same fail-loud, all-L0 outcome.

## Constraints

- Preserve I5: every controller restore participates in the existing
  participating-brick seam smoothing. Do not lift a level during repair.
- Preserve the external format and runtime reconstruction path. This is an
  offline level-selection refinement, not a wire or renderer change.
- Dense L0 remains the truth and the final-error denominator. The oracle must
  not substitute emitted magnitude for dense-truth magnitude.
- Runtime direct-light weights, signs, and clamps remain outside this scope.
  The controller validates the defined baked combined-output state; it does not
  claim a guarantee for unmodeled runtime light-selection states.

## Verification

At 1.0 m on `stress-warren-showcase.map`, re-bake and report final combined
failures, selected restore count by section, incremental payload after final
smoothing, emitted post-smoothing I5, and the retained payload ratios. The
gate passes only with zero final composed-error failures and zero participating
I5 violations. Re-run the representative Campaign and Kinematic fixtures only
after this safety result clears; retain their existing win, cap, timing, and
visual gates.

## Non-goals

- Joint optimization across multiple restored sections.
- Runtime-dependent or cancellation-unsafe bounds.
- Changing coarsening thresholds, base density, payload cap policy, shader
  reconstruction, PRL format, or runtime light behavior.

# M15 Phase 0 Findings

## Post-Tick Forced-Rounding Divergence

Method:

- Ran the headless `sim::simulate_tick` seam for 600 ticks with two pawn roles and a recorded resolved `SimCommand` stream.
- Compared a baseline sim against a forced-rounding sim that rounds pawn transform positions and movement velocities after every tick.
- Forced rounding is test-only under `#[cfg(test)]`; the normal engine path does not compile in or call it.
- Rounding quantum: `1.0 / 1_048_576.0 m` (`0.000000954 m`).
- Limitation: this spike did not round every intermediate/sub-step value. It measures
  post-tick state sensitivity, not a precise cross-ISA float drift bound.

Measurement:

- Max per-axis position divergence: `x=8.267551422 m`, `y=0.802780628 m`, `z=14.206228256 m`.
- Max total position divergence: `16.436830521 m`.
- Final per-axis position divergence at tick 600: `x=8.267551422 m`, `y=0.005555868 m`, `z=14.206228256 m`.
- Final total position divergence at tick 600: `16.436830521 m`.
- First total divergence over `0.001 m`: tick `3`.
- First total divergence over `0.01 m`: tick `3`.
- First total divergence over `0.05 m`: tick `4`.

Recommendation:

Use `0.001 m` as the run-to-run equality epsilon for deterministic harness comparisons, but do not treat this spike as proof of a production drift tolerance. The post-tick forced-rounding run crosses a visible correction threshold by tick 4 and can branch into meter-scale divergence over a 600-tick uncorrected run.

For Phase 2 movement prediction + reconciliation, start with a `0.05 m` positional reconciliation threshold for applying correction and smoothing, with exact authoritative comparisons still using the tighter `0.001 m` epsilon. Treat this as a starting point for the Phase 2 harness, not a finalized tolerance. It keeps tiny float noise out of correction churn while catching early branch divergence before it becomes visibly large.

The repo does not currently have a CI matrix or second-architecture runner. Post-tick forced rounding is sufficient for this spike's stress signal; a real arm64/x86_64 comparison runner would be a future bonus.

## Predict/Reconcile Prototype

Prototype:

- Added a dev-tools/test-gated seed harness at `crates/postretro/src/sim/predict_reconcile.rs`.
- It is not wired into the normal engine path. `sim/mod.rs` only exposes it under `#[cfg(any(test, feature = "dev-tools"))]`.
- The harness runs two in-process sims over the same command stream:
  - client predicts immediately with `sim::simulate_tick`;
  - server processes the same commands authoritatively after injected one-way latency/jitter;
  - authoritative snapshots are delivered back after injected one-way latency/jitter.
- Reconciliation is local to the prototype: it snapshots/restores the pawn `Transform` and `PlayerMovementComponent`, then rewinds to the acked authoritative tick and replays stored local commands. The `simulate_tick` seam remains forward-only.

Empirical read:

- Replay setup used 6-8 ticks one-way latency, deterministic jitter patterns, and a 0.35 m initial client offset to force visible reconciliation work.
- Basic rewind/replay converged cleanly: after the final authoritative ack, client/server final position error was within the `0.001 m` harness epsilon.
- Dash replay behaved acceptably in the seed harness: a correction delivered after an already-predicted dash restored the authoritative ack, replayed the dash command window, and did not amplify the original offset or leave velocity drift.
- Raw correction is still visible at this scale. The largest measured correction in the replay is the injected `0.35 m` lateral offset, so Phase 2 should plan render-side smoothing/decay for corrections of this size instead of snapping the camera directly.

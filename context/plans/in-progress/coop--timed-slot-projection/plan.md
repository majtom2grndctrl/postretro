# coop--timed-slot-projection — plan of record

mode: compact
status: active
read at: 545c5c2f3

## Corrections
- Time-sync and snapshot/newest-gate consumers changed since d6f3471; re-opened current intake, estimator, reset and UI snapshot boundaries. `ClientTimeSync` now exposes an initialized-only estimate and RTT; use those directly.
- `StateApplyOutcome` still conflates an empty accepted batch and rejection. Add a local accepted-batch signal and per-slot refresh exclusions before collecting held-value samples. Wire and schema stay unchanged.
- No additional timed weapon slot was found. Charge progress is client-local activation presentation, outside the owner-private host timer stream; it is not projected.

## Delegated answers
- Hold cap: 16 server ticks (266.67 ms), exceeding twice the longest four-tick stamp gap plus 60 ms jitter and one tick (210 ms). Check at 45 ms delay / 60 ms jitter / 5% loss and 144 Hz.
- Slope tolerance: 8%, covering a 1 ms carry over a shortest 16 ms timer interval (6.25%). Compare two positive signed slopes relative to the smaller.
- Timeline: initialized server estimate minus RTT/2, no snapshot-arrival re-anchor. Playback slews at 0.75–1.25× using frame delta and never rewinds. A reset clears its anchor, including while time sync remains live.

## AC-to-proof

| AC | Acceptance | Proof | Status |
|---|---|---|---|
| 1 | Reproduces first. The harness sends at `SNAPSHOT_TICK_INTERVAL`, advances the host at 60 Hz (50 | conditioned timer harness, committed red reproduction | achievable as stated |
| 2 | Onset | conditioned timer harness + onset bounds | achievable as stated |
| 3 | Between samples | conditioned timer harness frame classification | achievable as stated |
| 4 | On samples | conditioned timer harness sample/feed bounds | achievable as stated |
| 5 | Breaking | timer harness slope-change/loss cases | achievable as stated |
| 6 | The slope is the host samples' slope | host/client tuning mismatch case | achievable as stated |
| 7 | Refuse | timer jump/delay/wrap cases | achievable as stated |
| 8 | Bounds | timer bounds + correlated-capacity cases | achievable as stated |
| 9 | Refuse | cancel/switch/exit-fade integration | achievable as stated |
| 10 | In step | reload endpoint and completion timing cases | achievable as stated |
| 11 | Sample intake, observed at the intake seam | state-apply intake seam tests | achievable as stated |
| 12 | Frame cadence | newest-gate multi-snapshot and low-FPS harness | achievable as stated |
| 13 | Outage | cap/outage deterministic tests | achievable as stated |
| 14 | Before time sync has a sample, the target is the newest raw sample (P30). Its first sample mid- | first-sync timeline tests | achievable as stated |
| 15 | Resets | reset/correlation/absence lifecycle tests | achievable as stated |
| 16 | Raw readers stay raw | raw weapon presentation and reconcile tests | achievable as stated |
| 17 | State crossings on an enumerated slot fire once per crossing in the applied host samples, never | state-crossing integration tests | achievable as stated |
| 18 | UI reads | UI snapshot / binding / predicate / max / tween tests | achievable as stated |
| 19 | Host and single-player are unchanged | role-isolation snapshot tests | achievable as stated |
| 20 | No client heat or cell state is written | raw-store/component isolation and write grep | achievable as stated |
| 21 | The wire is unchanged | version/fingerprint/wire diff gate | achievable as stated |
| 22 | Loopback recipe (`networking.md` §Manual loopback recipe) on `coop-presentation-test`, client c | owner loopback runbook | achievable as stated |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Extend loss harness with independent host/client clocks, production timers, newest gate and conditioned time sync; commit failing stepped-reload reproduction | integrating executor / bounded harness worker | — | done: `conditioned_reload_target_advances_between_samples` failed as intended (0 pass / 1 fail); all 248 eligible non-sample frames held. Setup assertions passed; 869-test target compiled. |
| 2 | Add accepted sample intake, four-slot agreed-slope projection and bounded slew timeline; resets and deterministic seam tests | integrating executor | 1 | pending |
| 3 | Integrate projection before every UI snapshot for connected clients; heat HUD and SDK documentation; UI/isolation proof | bounded UI worker / integrating executor | 2 contract | pending |
| 4 | Extend conditioned harness across ramps, jumps, loss, clocks, caps, resets and raw readers; map every acceptance result | integrating executor / bounded harness worker | 2, 3 | pending |
| 5 | Readiness checks, review-panel/fix loop, final preflight, manual runbook and result report | integrating executor | 4 | pending |

Hot-path constraints: four fixed histories; bounded pending samples; no allocations in frame projection; UI map substitution uses existing keys. Intake work is bounded by the existing validated batch and only four projected slots. Measure frame projection with the deterministic harness and document fixed memory bounds.

## Landing
Manual visual proof is outstanding and is not inferred from automated checks. Keep the brief in progress until the owner supplies that result and says “land the plane.”

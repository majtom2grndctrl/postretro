# Task 2 — Re-measurement gate: capture + written go/no-go

Post-Fix-A re-measurement on `content/dev/maps/movement-feel.prl`, two-process host+client
loopback session, `RUST_LOG=postretro::netdiag=debug`. Captured run: **vsync-on**
(client `frames ≈ 60/s`). Continuous client movement, mouse-look sweep, and jump taps driven.

## Task-1 ACs — confirmed from the host log

| AC | Target | Measured | Verdict |
|---|---|---|---|
| queue real/neutral | real ≥ ~55/60, neutral ≈ 0 | real **57–61/60**, neutral **0** (was real 8–15, neutral 30–45) | PASS |
| jump | `dropped_neutral=0`, `dropped_trim=0` | **0 / 0** across all attempts (was dropped_neutral ≈ attempts) | PASS |
| `cursor_lead[last]` | small **negative**, bounded, not growing | **−2 to −4** steady, `trims=0` | PASS |

Observation: `cursor_lead` rides at ~−3 rather than the predicted ~−1. The host's high render
rate (250–330 fps) drains several fixed ticks per once-per-frame `ingest`, so the playout
buffer sits a few ticks deeper than the disarm depth alone predicts. ~50 ms authoritative-input
latency, hidden by client prediction (owner confirmed smooth feel), tunable via
`INPUT_BUFFER_TARGET`. Not a defect; within the documented bounded-playout tolerance.

## Fix C — NOT triggered

Fix C requires the sending client to render *below* 60 fps (consecutive ticks reuse one yaw).
The client rendered at `frames ≈ 60/s` (vsync-on); a vsync-off run renders *above* 60. In
neither case is host-observed orientation limited by client render rate. Fix C drops from scope;
no `input.md` §3 divergence is taken.

## Fix B — TRIGGERED (owner go: proceed)

Owner observation: the client sees the host's rotation more smoothly than the host sees the
client's. This is the source-confirmed structural asymmetry — the client smooths every remote
through the delay-buffered `RemoteInterpolationBuffer` (position lerp + rotation slerp across a
playout window; client log shows `frames=61 presented=61` resampled across `target_span=60`),
while the host presents the client's pawn live with only a single-tick slerp and no buffer.

Data reads it as a presentation-quality gap rather than data starvation: host `distinct_yaw`
scales with turn intensity, peaking at **~45–52/60** during vigorous turns (≈1 when still). The
host receives fresh orientation near the send rate; the residual "less smooth" is interpolation
quality (live slerp vs. buffered playout) — exactly Fix B's domain.

**Decision (owner): GO on Fix B.** Proceed to Task 3. Fix C: no-go (not triggered).

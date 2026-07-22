# E10 — Jittering Wedge at a Mandatory Vertex Escapes Escalation

> **Draft stub.** Spun out of `E10--slow-agent-arrival-stuck` (it was that spec's first
> open question). This is the MIRROR IMAGE of that spec: a false **negative** in the
> prior-art E10 mandatory-waypoint gate, not the false positive that spec removes. Keep
> them separate — opposite-signed defects, and this one needs nav-geometry fixtures the
> arrival-stuck fix does not.

## Goal

Ensure a genuine wedge AT a mandatory clearance vertex still escalates to stuck recovery,
even when the wedge jitters occasionally above the easing floor. Today such a wedge can
stall permanently and silently, masked by the same easing gate that exists to let a
legitimate corner turn through.

## Background (the cause)

All in `crates/postretro/src/agent_steering.rs`.

`update_stuck_ticks` (:962) accumulates `stuck_ticks` only when a tick's goal-projected
progress falls under the active floor. Inside a mandatory vertex's arrival band that floor
is `MANDATORY_EASING_PROGRESS_EPSILON` (`STUCK_PROGRESS_EPSILON * 0.05` ≈ 0.00025 m/tick,
:86), selected by `easing_onto_mandatory_waypoint` (:944). Escalation needs
`STUCK_TICKS_THRESHOLD` (20, :106) CONSECUTIVE sub-floor ticks.

A wedge that jitters even occasionally above that tiny floor — numerical jitter from
`collide_and_slide`, or a shallow slide along a wall that yields a small positive
goal-projected step — resets `stuck_ticks` to 0 on those ticks and never arms recovery. A
silent permanent stall, masked by the easing gate.

## Fix direction (not a prescription)

Accumulate against NET progress over a bounded window instead of a single-tick floor, or
cap how many consecutive easing-suppressed ticks are allowed before falling back to the
absolute floor. Either way a genuine wedge must still escalate, and a legitimate corner
turn must still pass.

## Open question (the arbiter, unresolved)

Whether a real jittering wedge actually sustains occasional above-floor ticks (resetting
the counter) or settles sub-floor (escalating on its own) is **not established**. Build the
repro first: a mandatory vertex the capsule cannot plane-pass while still sliding along it.
No such test exists today. Resolve empirically there before deciding this needs a code
change — the repro test is the arbiter.

## Relationship to `E10--slow-agent-arrival-stuck`

Any fix here must compose with — not duplicate or bypass — the arrival-band relaxation that
sibling spec introduces. The mandatory-gate branch and the arrival-band relaxation must not
both suppress accumulation in a way that masks a mandatory-vertex wedge.

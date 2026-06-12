# M14 Task 5 — Findings

Empirical input recorded while authoring the dev content and modder docs for the
movement-dash runtime-values adopter. Two consumers: plan 4 (the vocabulary review
gate) and the M13 G1 draft (the `liveValue()` naming reservation).

## Authored dev-content expressions

Two of the six dash fields in `content/dev/scripts/player.ts` ship as expressions;
the other four stay literal.

- `momentumRetention: runtime.select(runtime.read("grounded"), 0.4, 0.7)`
  — entry-moment; exercises a boolean `read` feeding `select` directly.
- `steerControl: runtime.clamp(runtime.div(runtime.read("elapsedMs"), 150.0), 0.0, 1.0)`
  — per-tick; exercises arithmetic + `clamp` and a ramp over `elapsedMs`, kept
  inside the 200 ms `DASH_MAX_MS` bound (150 ms ramp window) so it stays
  observable.

## Combinator demand (plan-4 review gate)

**No combinator demand from the two authored fields.**

- `momentumRetention` branches on a *single* boolean input (`grounded`), so
  `select(cond, a, b)` consumes it directly — no `and`/`or`/`not` needed, nor any
  negation (the two branch values are simply ordered grounded-then-airborne).
- `steerControl` is purely numeric (`div` → `clamp`); it has no boolean subterm at
  all.

So neither authored expression forced an awkward workaround around the absent
boolean combinators (`and`/`or`/`not`), and neither wanted vector values or stateful
nodes. The fixed vocabulary was sufficient and read cleanly.

Caveat for plan 4: this is a two-field sample on a single descriptor. The first
plausible combinator pull is a *compound* gate — e.g. "retain momentum only when
grounded **and** moving fast" (`and(read("grounded"), gt(read("speed"), k))`).
`select` cannot fold two conditions without nesting `select(a, select(b, …), …)`,
which is expressible but reads worse than an `and`. No such expression was demanded
here, so this is a watch-item, not a demand — recorded so plan 4 can weigh it
against future adopters (shield/health policy) before widening the vocabulary.

## Naming reservation (M13 G1)

The M13 G1 roadmap term `liveValue()` should **not** keep that name. With this plan,
the SDK taxonomy is settled: `Runtime`/`RuntimeValue` means *computed by the engine,
never stored*, while `State`/`StateValue`/`defineStore` means *stored*. A G1 hook
that reads stored UI-local state is therefore state-named, not runtime-named, and
belongs under the `ui` namespace.

**Reservation:** `liveValue()` -> working name **`ui.localState()`** (a state-named
hook under the `ui` namespace). The roadmap/G1 wording is amended at promotion per
convention, not in this task — recorded here so the rename is not lost.

# Experimental Spikes

> **Read this when:** scoping, reviewing, or implementing a spec whose goal is to learn rather than to ship a hardened feature.
> **Key distinction:** a spike is build-to-learn. The deliverable is a decision, not a contract.
> **Related:** [Architecture Index](./index.md) · [Context Style Guide](./context_style_guide.md) §8 · [Roadmap](../plans/roadmap.md)

---

## What a spike is

A spike builds working code to answer a question: how does it run, is further optimization needed, or is a cheaper knob enough? The spec exists to produce that answer. Example: `context/plans/drafts/perf-animated-sh-light-culling/`.

A spike is not a license to cut architecture or correctness. Get the design right and the data path correct — a spike with silent corruption teaches nothing. Cut scope and hardening, not rigor. (Bounds the velocity shift; see roadmap.)

## Acceptance criteria

Split the criteria by what they prove:

| Kind | What it gates | Form |
|------|---------------|------|
| Honesty gate | The experiment ran correctly | Automated: clean boot, version-reject path, invariants hold, correct output shape |
| Measured finding | The numbers the experiment exists to learn | Measure-and-report: a log line or recorded value that feeds a decision — not a pass/fail threshold |

Don't contract the findings. A footprint or frame-time target is the hypothesis, not a gate; landing over it is a recorded result that feeds the recommendation, not a silent failure. Visual quality is a manual-visual check (per [style guide §8](./context_style_guide.md)) — never parity to the thing being replaced, which is the baseline the spike is abandoning.

## Tuning levers

Levers a spike introduces live as debug-tool sliders — the sandbox for experimenting with settings. They are candidates for a future global quality-settings spec (gated on the UI system landing); never wire them as user-facing settings inside a spike.

## Deliverable

The spike's final task is a findings note: the measured values against their targets, the manual-visual read, and a recommendation — does the implementation suffice as-is, is an optimization warranted, or do the debug knobs defer the question to a later spec?

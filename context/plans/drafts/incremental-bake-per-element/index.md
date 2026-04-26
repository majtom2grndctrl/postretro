---
name: Incremental Bake Per Element
description: Stub. Per-face lightmap and per-probe SH caching keyed by spatial input hash. Blocked on build-stage-cache landing first.
type: plan
---

# Incremental Bake Per Element

> **STATUS: stub.** This plan is intentionally not specified yet. Several design decisions depend on discoveries that will land during the sibling plan `build-stage-cache/`. Drafting in detail now would mean specifying against a mental model of the bakers that doesn't match the post-cache reality.

## Goal (sketch)

Push the build cache from "skip the whole bake" to "skip the parts of the bake whose inputs didn't change." Per-face lightmap data and per-probe SH coefficients are cached individually, keyed by the hash of their *spatial* inputs (geometry within the texel/probe's sample range, lights within influence radius). Editing one light in a corner re-bakes only the texels and probes that light reaches, not the whole atlas or the whole volume.

Acceptable to ship with "near-equivalent to a full bake, with occasional seams that a full bake before release fixes." Iteration speed is the goal, not bit-exact equivalence.

## Why this is a stub

This plan needs answers that `build-stage-cache/` will produce as a side effect:

- **Stage coupling boundaries.** What inputs the lightmap and SH bakers actually consume — surfaced concretely once their inputs are serialized for the stage cache.
- **Atlas packing stability.** Whether the lightmap atlas packer is order-stable across builds. If not, per-face cache reuse needs a packer rewrite as a prerequisite. The stage cache will surface this as a determinism issue.
- **Spatial input vocabulary.** What "lights within influence radius of this face" looks like in code — depends on how the light list is structured after Task 2 of `build-stage-cache/`.
- **Determinism baseline.** Per-element caching needs the same determinism invariants the stage cache establishes, applied at finer granularity. The fixes from Task 3 of `build-stage-cache/` set the floor.
- **Per-element cache substrate.** Whether the `StageCache` substrate from Task 1 of `build-stage-cache/` extends to fine-grained entries (millions of small blobs) or needs a different storage shape (single packed file, sqlite, sled, etc.).

## When to promote this plan

Promote (i.e., draft in full) once `build-stage-cache/` is implemented and the questions above have concrete answers grounded in code. At that point:

- Re-survey the bakers and write a real spec (scope, AC, tasks, sequencing) following the standard plan template.
- Decide whether per-face lightmap and per-probe SH ship as one plan or two — they share substrate but the algorithms differ enough that splitting may make sense.
- Decide on the "acceptable seams" envelope: what visual artifacts are tolerated, how a full bake is triggered before release (CLI flag? worldspawn property? convention?).

## Reminder mechanism

This file's existence in `context/plans/drafts/` is the reminder. The `/draft-plan` skill lists drafts at the top of every drafting session. When `build-stage-cache/` moves to `done/`, revisit this stub.

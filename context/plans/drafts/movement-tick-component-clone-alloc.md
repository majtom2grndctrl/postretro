# Follow-up — per-tick player movement component clone allocates when dash fields use expressions

> **Status:** deferred bug note. Out of M14 scope; recorded so the next movement-plumbing pass can address it.
>
> **Origin:** surfaced during the M14 movement-dash-runtime-values review. The eval path added by M14 is genuinely zero-alloc; this regression is in the surrounding tick plumbing.

## The problem

The per-tick game-logic path clones the player movement component out of the component registry, runs the tick, then writes it back. This clone-out/write-back pattern predates M14.

When `PlayerMovementComponent` held only plain scalar dash fields the clone was allocation-free. M14 made six dash fields expression-capable: an `Ir`-variant field carries an `IrNode` tree, and `DashPrograms` carries `BoundProgram`/`BoundNode` trees. A per-tick clone of the component now deep-clones those trees — heap-allocating on every game-logic tick whenever any dash field is authored as an expression.

The eval path itself (`dash_intent`) is zero-alloc; it borrows the component and allocates nothing. The regression is only in the registry clone-out that wraps it.

## Why the M14 zero-alloc AC did not catch it

The `AllocSnapshot` test arms its allocation counter around a direct `dash_intent` call. That call borrows the component and allocates nothing — so the counter stays zero and the test passes. The registry clone-out happens in the movement tick driver, outside the snapshot window, so the test cannot observe it.

## Why deferred

The clone-out/write-back is pre-existing tick plumbing, not part of M14's eval-path scope. Present cost is small: one player pawn, a handful of small allocations per tick. But "avoid per-frame allocations" is the stated hot-path default (development_guide §1.4), and `DashPrograms` grows as more fields and adopters become expression-capable — cost scales the wrong way.

## Candidate approaches

Three independent directions to consider; not a mandate, the implementor should weigh them against the full tick-plumbing picture.

- **In-place tick.** Remove the clone-out entirely — take the component out of the registry or pass a mutable borrow for the duration of the tick, then release it. Write-back only if the tick actually mutates state.
- **Shared bound programs.** Wrap `DashPrograms` in `Rc` (or equivalent). Cloning the component becomes a reference-count bump — shallow and allocation-free. Immutable programs are never written back anyway.
- **State/program split.** Separate the per-tick-mutable scalar state from the immutable bound programs. Only the cheap scalar part participates in the clone-out/write-back loop; programs are referenced separately.

## Acceptance sketch

A movement tick with expression-authored dash fields performs zero heap allocations across the full tick driver — not just the inner `dash_intent` call. Extend the `AllocSnapshot` window to cover the tick boundary to lock this in. Literal-only paths and all existing movement behavior must be unchanged.

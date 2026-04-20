---
name: draft-plan
description: >
  Drafts feature or epic specs for PostRetro. A session may produce zero, one,
  or several plans depending on scope. Use when starting new planning work.
  Does not promote to ready/ — that is a separate step after review.
---

# Draft Plan

Explore scope, write specs. Output lives in `context/plans/drafts/<feature-name>/index.md`.

A drafting session may produce 0, 1, or N plans. Scope often shifts during planning — let it. Don't lock a feature name before scope settles.

## Current plans

!`ls context/plans/drafts/ context/plans/ready/ context/plans/in-progress/ 2>/dev/null`

## Process

### 1. Understand the goal

Read the user's description. Ask focused questions when scope is unclear — don't over-interrogate.

Pin down:
- What outcome matters
- What constraints apply
- What subsystems are touched
- What "done" looks like — concrete, verifiable

### 2. Research

Read `context/lib/context_style_guide.md` first. All spec prose follows it.

Load relevant library files:

!`ls context/lib/`

Use subagents for exploration — codebase reading, pattern discovery, doc lookup. Target 80% confidence. Stop when you have enough to spec the work.

**Research notes stay out of the spec.** If findings are useful but don't drive decisions, put them in a sibling `research.md` in the plan folder. The spec captures decisions and behavior, not the investigation that produced them.

### 3. Write the spec

Create `context/plans/drafts/<feature-name>/index.md`.

```markdown
# <Feature Name>

## Goal
1–3 sentences. What this achieves. Why it matters.

## Scope

### In scope
- Bullet list.

### Out of scope
- Explicit non-goals. No "TBD" — decide or drop.

## Acceptance criteria
- [ ] Verifiable conditions for "done."

## Tasks
(Optional for small plans. Use when work splits cleanly.)

### Task 1: <name>
One paragraph. What to build.

### Task 2: <name>
...

## Sequencing
(Required when Tasks section exists. Feeds /orchestrate.)

**Phase 1 (sequential):** Task 1 — blocks everything.
**Phase 2 (concurrent):** Task 2, Task 3 — independent.
**Phase 3 (sequential):** Task 4 — consumes Task 2/3 output.

## Rough sketch
(Optional.) Implementation direction, key modules, algorithm hints. Named types and functions live here, not in AC.

## Open questions
Unresolved items, risks, alternatives considered.
```

**Length smell:** most plans land at 50–200 lines. Past 250 lines usually means the spec carries research notes (→ `research.md`) or scope should split into multiple plans.

### 4. Acceptance criteria

AC names observable behavior. Someone who didn't write the plan must be able to verify it without reading the implementation.

| Too loose | Right | Too strict |
|---|---|---|
| "Movement feels good" | "Player walks slopes ≤ 45°; cannot pass through walls; jump launches when grounded" | "`CharacterController::step()` calls `trace_box()` with hull (16, 16, 56)" |
| "Performance is acceptable" | "Frame time < 16ms on `assets/maps/stress.prl` at 1080p" | "BVH traversal ≤ 3.2ms measured via tracy" |
| "Leaks are detected" | "`prl-build` exits non-zero on leaked map; writes `.pts` TrenchBroom loads" | "`LeakReport { seed_leaf, void_leaf, portal_path }` returned from `visibility::flood_fill()`" |

Named types, functions, and line numbers belong in the sketch — not AC. AC survives a rewrite of the implementation; a spec keyed to function names does not.

### 5. Sequencing

Feeds `/orchestrate`. Terse is fine — models read short phase blocks reliably.

Rules:
- Concurrent by default.
- Sequential only when a later task consumes an earlier one's output, shares files, or breaks a contract if parallelized.
- Name the dependency in one clause ("Task 3 consumes the vertex format from Task 2"). No essays.
- Each phase completes fully before the next begins.

One phase per line. No per-task sub-bullets unless a dependency needs calling out.

### 6. Commit

Stage and commit the plan folder (`index.md` + optional `research.md`).

**Do not update `context/lib/` during drafting.** Durable capture happens at promotion — after review. Reviewer agents often reshape the spec; library updates should land once, against the final shape.

### 7. Report

- What was planned, or if the session produced no plan (scope already covered, etc.)
- Task count and phase summary
- Open questions left for the user
- Plan lives in `drafts/` — not ready for `/orchestrate` until promoted

## Promoting a plan to `ready/`

Not part of the drafting session. Happens after review — often after reviewer agents pass.

A draft is ready when:
- Scope in/out is decided — no "TBD" markers
- AC is verifiable by someone who didn't write the plan
- Open questions are resolved, or explicitly scoped as decisions-during-implementation
- User signs off (reviewer agents may run first)

At promotion:
1. Capture durable decisions in `context/lib/` — new architectural constraints, subsystem contracts, pipeline topology. Agents working the plan find full context in the library, not in the plan document.
2. `git mv context/plans/drafts/<name> context/plans/ready/<name>`
3. Commit the move and the `context/lib/` updates together.

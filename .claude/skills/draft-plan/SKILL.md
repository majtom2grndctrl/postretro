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

**Code-grounding is non-negotiable.** Every Rust/TS/Lua identifier the spec will name — function, struct, type, field, enum variant — must be confirmed against current source before the spec asserts anything about it. Don't write "X returns Y" or "X has fields A, B" from memory. Open the file, read the signature, then write. Memory drift is the largest single source of spec inaccuracy.

**Warrant every work-eliminating claim.** Code-grounding's sibling failure: asserting from memory that something *need not be built*. A spec's riskiest sentences are the ones whose function is to buy it out of work it would otherwise owe — "identical by construction," "follows automatically," "no separate test required," "derivable from existing state," "same as the single-player path." Each removes a task, a test, or a code path, and each is the one kind of sentence that produces no artifact to check it against. They are usually true, which is what makes the false ones expensive: they survive to implementation and surface as rewrites, not as bugs.

State the warrant inline — the specific reason, grounded in source, not a restatement of the claim. "Identical because both paths call `apply_command()` with the same input struct" is a warrant; "identical by construction" is the claim wearing a warrant's clothes. If the warrant cannot be written, the claim is a guess: spec the work instead, or record it as an open question. `/review-draft-spec` challenges every one of these, so an unwarranted claim costs a review round.

**Oversized-file watch.** Watch source-file size while grounding. Flag any file already past ~800 lines that the plan will extend — a soft smell, not a gate. A cohesive 900-line table is fine; a tangled 600-line module may not be. Carry the flag forward as a split-first task (§3).

**Research notes stay out of the spec.** If findings are useful but don't drive decisions, put them in a sibling `research.md` in the plan folder. The spec captures decisions and behavior, not the investigation that produced them.

**Lifecycle diagram before tasks.** When the plan changes state or timing across seams — latches, deferred effects, cross-frame hand-offs — diagram the full lifecycle in Mermaid before writing tasks: `sequenceDiagram` for cross-seam flows (frame boundaries as participants when timing matters), `stateDiagram-v2` for latch/FSM lifecycles. No arrow without a read call site — the diagram drives code-grounding. Derive the Invariants table (§3) and task boundaries from it. Diagram goes to `research.md`; keep it in the spec only when it is the clearest statement of a pinned decision.

**Enumerate observers, not just the flow.** A flow diagram traces one path and renders every vantage on it as a single line. When the same state is observed from more than one position — host and client, local and remote, live and replayed, authored and generated — the spec owes the cross-product of vantage × lifecycle stage, not the flow. Name the vantages explicitly, then say which ones differ and which are the same. A vantage asserted identical to another is a work-eliminating claim and needs its warrant.

**Enumerate orderings, not just the sequence.** A lifecycle diagram traces one ordering and renders every other as impossible. When the plan introduces mutable state, a timer, or an event, the spec owes the orderings that actually occur: two events the prose separates landing on one tick, B arriving before A, a timer crossing a reset or unload, N of an event where the handler expects one, a duration authored at zero, a consumer sampling slower than the producer mutates. Pin them as a table of scenario, ordering, and expected outcome. Unlike the diagram, this table is spec text, not `research.md`: task agents need it, and the test task cites its rows rather than restating them.

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

## Direction
(Three short subsections. See "Direction questions while drafting" below.)

**Problem.** One sentence. The cause, not a symptom of it.

**Prior commitments.** What the project already decided that this touches, cited.
Where this diverges, say so and argue it — unstated divergence is the defect,
divergence itself is often right.

**Alternatives rejected.** The strongest rival shape and why not it. Cheap now,
expensive to reconstruct later.

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

## Boundary inventory
(Required when the plan crosses Rust ↔ JS/Lua ↔ wire ↔ FGD KVP boundaries. Skip otherwise.)

Pin casing and encoding once for every cross-boundary name. Reference this inventory throughout the spec instead of re-deciding inline.

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| (example) `BillboardEmitter` | `ComponentValue::BillboardEmitter` | `"billboard_emitter"` | `"billboard_emitter"` | `"billboard_emitter"` | n/a |

## Wire format
(Required when the plan adds a binary or PRL section. Skip otherwise.)

For each new binary surface, pin: endianness, integer signedness, length-prefix integer width, entry-count placement, per-entry field order, empty-list encoding, sentinel/null representation per runtime. State explicitly which existing section the new layout mirrors.

## Invariants
(Required when a behavioral guarantee — exactly/at-most-once, ordering, state reachability, timing — is established or preserved across more than one task or seam. Skip otherwise.)

Pin each cross-task invariant once. `/orchestrate` hands this table to every task agent with the Goal and AC list — task paragraphs reference rows, never restate them.

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| (example) Kill reported exactly once, at removal | Task 1 (sweep latch), Task 2 (removal-pass sink) | `setHealth` clears latch + pending credit; direct `registry.despawn` bypasses the sink | AC 7, 8, 9 |

## Script syntax examples
Show examples of what scripts authored by modders utilizing this functionality would look like (only use section when applicable).

## Open questions
Unresolved items, risks, alternatives considered, if applicable (only use section when necessary).
```

**Size smell:** measure the task paragraph, not the document. Past ~150 lines a task is doing two things; past ~4 ACs its contract is not settled. Total length is descriptive, not a smell: the task-paragraph contract below makes every paragraph restate what its agent cannot otherwise see, so specs grow with tasks × restated surface. Move derivation to `research.md`; split scope when the Scope section covers two problems.

**Plumbing rule.** Every "edit X to do Y" instruction must say how X gets access to what it needs. New side-tables need owners. New struct fields need writer call-sites. Function signature changes need their callers enumerated. Don't punt access plumbing to the implementer — the implementer has less context than the spec author.

**Task-paragraph contract.** Each task paragraph is an execution contract: `/orchestrate` hands a task agent only its own paragraph, the plan's Goal, the AC list, and the Invariants table when present — never the Scope section. Don't point at Scope ("the list in Scope"); inline load-bearing enumerations in the task paragraph, or pin them in an AC. The AC list and Invariants table are the only shared channels across tasks.

**Split-before-extend rule.** When the plan adds functionality to a source file already past ~800 lines, split it first — a behavior-preserving task that breaks the file along seams you already see. Sequence the split right before the task that extends that file; don't drag an off-critical-path file forward. Splitting and extending in one task buries a refactor inside a feature diff — keep them separate.

### 4. Acceptance criteria

AC names observable behavior. Someone who didn't write the plan must be able to verify it without reading the implementation.

| Too loose | Right | Too strict |
|---|---|---|
| "Movement feels good" | "Player walks slopes ≤ 45°; cannot pass through walls; jump launches when grounded" | "`CharacterController::step()` calls `trace_box()` with hull (16, 16, 56)" |
| "Performance is acceptable" | "Frame time < 16ms on `assets/maps/stress.prl` at 1080p" | "BVH traversal ≤ 3.2ms measured via tracy" |
| "Leaks are detected" | "`prl-build` exits non-zero on leaked map; writes `.pts` TrenchBroom loads" | "`LeakReport { seed_leaf, void_leaf, portal_path }` returned from `visibility::flood_fill()`" |
| "Rider stays attached to the platform" | "Rider pose tracks the mover at rotation start, reversal, and stop; holds across a frame with zero fixed ticks and one with two" | "`RiderState::yaw_offset` written from `apply_mover_rotation()`" |

The last row is the general lesson, not a movement one: AC written against steady state is the default failure mode. Steady state is where the behavior is easiest to describe and least likely to break. Name the edges — start, stop, reverse, completion, detach, zero-or-many iterations of whatever ticks.

Named types, functions, and line numbers belong in the sketch — not AC. AC survives a rewrite of the implementation; a spec keyed to function names does not.

### 5. Sequencing

Feeds `/orchestrate`. Terse is fine — models read short phase blocks reliably.

Rules:
- Concurrent by default.
- Sequential only when a later task consumes an earlier one's output, shares files, or breaks a contract if parallelized.
- Name the dependency in one clause ("Task 3 consumes the vertex format from Task 2"). No essays.
- Each phase completes fully before the next begins.

One phase per line. No per-task sub-bullets unless a dependency needs calling out.

**Thin slice before fan-out.** When a plan spans producer → boundary → consumer — anything where one side writes and another reads across a seam — phase 1 is a narrow vertical slice through every layer, integrated and exercised end to end. The fan-out comes after.

Concurrent-by-default argues against this, and that is the point: under the plain rule the producer tasks are independent, so they all run first and integration lands last. That ordering keeps the spec's assumptions unfalsified until the widest possible moment. A slice exists to falsify them while rewrites are still cheap — it is a test of the spec, not a delivery increment, so make it the thinnest path that crosses every seam rather than the first useful feature. Name it as such in the phase line: "Phase 1 (sequential): Task 1 — thin slice, falsifies the boundary assumptions."

### 5b. Direction questions while drafting

Six questions govern whether a spec is a reasonable solution to the problem at
hand. Here they are a solo exercise — generative, shaping what you write.
`/validate-plan` asks the same six adversarially at step 8, through a reviewer
who did not draft the spec. Same questions, opposite direction.

Work them yourself, in this context — never dispatch an agent for them. The
exercise shapes what you write next, which only works if you did the thinking;
a delegated answer is a report, and it grounds no one.

The questions are defined normatively in `/validate-plan`. If the two lists
disagree, `/validate-plan` wins — update this one to match.

1. What problem is this actually solving — cause or symptom — and what
   observation produced it?
2. Is it being solved at the right level? Name the placement axis first;
   this repo has several beyond engine-vs-mod.
3. What does this foreclose?
4. What has this project already committed to that this touches?
5. Is this a one-way door, and what does undoing it cost?
6. What is the strongest alternative, and why not that?

Proportionality is not among them. Over-built and under-scoped are
`/validate-plan` verdicts, settled by comparison against Q6's alternative.

Asking a question is not the same as recording its answer. Only some produce
spec text:

| | Artifact |
|---|---|
| 1, 4, 6 | The `Direction` section. Cheap to write now, expensive to reconstruct. |
| 2 | Record the *fact* of the placement and the reason for it, where the design decision lives. This is input for the reviewer, not a clearing of the question — a drafter who placed something in the wrong layer does not know it. "This belongs in the engine floor because…" is useful; "the layering was assessed and is correct" is noise. |
| 3, 5 | Record specific foreclosures and one-way doors where known, with what undoing them would cost. Both are factual and survive self-assessment. Never write "nothing significant" — that is the default answer and it carries no information; "nothing material" after actually looking is a fine answer. |

Question 2 needs a reader who has not spent the session inside the solution —
a drafter who placed something in the wrong layer does not know it. Answer it
for yourself; do not trust your own answer.

### 6. Cross-check

Before committing, walk the spec:

- **Task → AC.** For every task line item, ask: "What AC verifies this behavior?" If nothing does, either the AC is missing or the task should drop.
- **AC → task.** For every AC, ask: "Which task produces the behavior this verifies?" If nothing does, either the task is missing or the AC is aspirational.
- **Invariant → task + AC.** (When the Invariants table exists.) For every row, ask: "Which tasks own the establishing and preserving edits, and which AC verifies the guarantee?" An invariant breakable without failing any AC is the gap the two walks above miss.

All directions must close. Gaps signal that something was assumed without being written down.

### 7. Commit

Stage and commit the plan folder (`index.md` + optional `research.md`).

**Do not update `context/lib/` during drafting.** Durable capture happens at promotion — after review. Reviewer agents often reshape the spec; library updates should land once, against the final shape.

### 8. Validate direction

Run `/validate-plan <name>` on each plan the session produced, before reporting.

It dispatches a fresh reviewer to judge direction — is this a reasonable solution
to the problem at hand — at an altitude this session cannot reach for its own
work. Immersion is what makes a locally-correct wrong shape feel obviously right,
and a drafting session is maximally immersed.

Surface its verdict; never act on a *Reshape*, *Not a spec*, or *Under-scoped*
finding unilaterally. Those are owner decisions.

### 9. Report

- What was planned, or if the session produced no plan (scope already covered, etc.)
- Task count and phase summary
- The `/validate-plan` verdict per plan
- Open questions left for the user
- Plan lives in `drafts/` — not ready for `/orchestrate` until promoted

## Promoting a plan to `ready/`

Not part of the drafting session. Happens after review — often after reviewer agents pass.

A draft is ready when:
- Scope in/out is decided — no "TBD" markers
- AC is verifiable by someone who didn't write the plan
- Open questions are resolved, or explicitly scoped as decisions-during-implementation
- User signs off (reviewer agents may run first)
- A reviewer agent (or panel) can only find issues by reading source code, not by reading the spec. Issues that surface only at code-anchor depth signal the spec has hit diminishing returns — promotion is appropriate.

At promotion:
1. Capture durable decisions in `context/lib/` — new architectural constraints, subsystem contracts, pipeline topology. Agents working the plan find full context in the library, not in the plan document.
2. `git mv context/plans/drafts/<name> context/plans/ready/<name>`
3. Commit the move and the `context/lib/` updates together.

---
name: review-draft-spec-priced-deferral
description: >
  Multi-agent review of a draft spec in `context/plans/drafts/` —
  priced-deferral variant of review-draft-spec, whose broad reviewer
  additionally sweeps every scope-eliminating claim and reports each one
  the spec does not foreclose in source. Spawns three parallel reviewers
  — a broad reviewer, a codebase-anchor reviewer that fact-checks every
  named identifier against source, and a temporal reviewer that attacks
  the spec's orderings. Auto-applies mechanical fixes via a Sonnet
  sub-agent unless --no-auto-apply is set. Recommends apply / re-review /
  promote. Use after a draft session, or when a human wants to validate
  before promoting to ready/.
---

# Review Draft Spec

Three reviewers in parallel. One broad, one anchored to source, one attacking orderings. Aggregate findings, auto-apply mechanical fixes, recommend whether to apply more, re-review, or promote.

Detail altitude. Direction — is this a reasonable solution to the problem at all — belongs to `/validate-plan` and runs first; findings here are keyed to the spec's current shape, and a reshape invalidates them.

## Process

### 1. Locate the spec

Argument is the plan folder name (e.g. `entity-model-foundation`) or a full path. If absent, list drafts and ask which one:

```
!`ls context/plans/drafts/`
```

Resolve to `context/plans/drafts/<name>/index.md`.

### 2. Read the spec once

Read the full spec yourself before delegating. Decisions about which reviewers to run depend on what the spec contains. Reviewer prompts inline the spec content — don't pass paths and assume agents will read them. Paths drift.

### 3. Run reviewers in parallel

One message, three `Agent` tool calls. No sequential rounds.

**One lens per agent.** A lens needs sustained attention; an agent handed two satisfices and does both shallowly. Never blend them — a lens that does not fit the spec is skipped whole, not folded into another. Independence is the point: the same defect reported by two lenses that could not see each other is the signal worth acting on, and it is how a confidently wrong finding gets caught. Merging costs that and saves only a duplicated spec read.

#### Broad reviewer (Opus)

Receives:
- Full spec content inline
- The relevant `context/lib/` slices for subsystems the spec touches (route via `context/lib/index.md`)
- Instructions to find:
  - Contradictions within the spec
  - Casing or boundary inconsistencies
  - AC ↔ task gaps in either direction
  - Scope-boundary violations
  - Plumbing handwaves — "edit X to do Y" without stating how X gets access
  - Missing wire-format or FFI pins
  - Unwarranted work-eliminating claims (see below) — report each as a finding
  - Unforeclosed scope-eliminating claims (see below) — report each as a finding
  - Anything else that forces an implementer to guess

Plus two explicit sweeps, each run as its own pass rather than folded into general reading.

**Work-eliminating claims:**

> Extract every claim whose function is to eliminate work — to assert that some
> code path, test, or task need not exist. Markers: "identical by construction,"
> "follows automatically," "no separate test required," "derivable from," "same
> as the X path," "trivially," "by symmetry." For each, ask whether the spec
> states a checkable reason, grounded in named source, or only restates the
> claim. Report every unwarranted one. These are usually true — say so where
> they are, and do not manufacture doubt. But they are the only sentences in a
> spec that produce no artifact to verify, so an unexamined one survives to
> implementation and surfaces as a rewrite rather than a bug.

**Scope-eliminating claims:**

> Extract every claim whose function is to eliminate scope — to assert that some
> case is not this spec's to handle. Markers: "this spec does not build it,"
> "deferred to the follow-on spec," "out of scope," "a later spec owns this,"
> "not reachable today," "a future spec fixes it." For each, ask whether the
> spec names source that forecloses the case — a validation rule that rejects
> the input, a type that cannot represent the state, a call site that cannot be
> reached — or only asserts the deferral. Code that *produces* the case does not
> count as code that forecloses it: a spec tracing the mechanism in detail has
> explained the defect, not removed it. Content observations do not count:
> "no shipped content does this," "no dev map reaches it," "authors are unlikely
> to" describe today's assets, not the permitted surface, and a spec that pairs
> one with an expectation that authored content will reach the case has stated a
> defect. Naming where a future fix would go — a "fix seam," a marker that would
> have to change shape — does not count either. Report every claim with no
> source-level foreclosure, and say plainly that the case is reachable: it is a
> defect the spec is choosing to ship, which is the owner's decision, not a
> footnote or an AC caveat. Most deferrals are legitimate — say so where they
> are, and do not manufacture doubt. But deferring costs the author nothing and
> always reads responsible, so an unexamined one survives to implementation and
> surfaces there as the case the spec said would not arise.

Output: list of `{ location, problem, fix }` triples. "No issues found" if clean. No padding, no praise.

#### Codebase-anchor reviewer (Opus)

Receives:
- Full spec content inline
- Instruction: "For every Rust/TS/Lua identifier the spec names — function, struct, type, field, enum variant, module path — open the file in source, confirm the spec's claim, report any divergence between the spec and current code reality. First step: extract the identifier list from the spec. Then resolve files via Glob/Grep. Then batch-read."
- Additional instruction: "Where the spec warrants a work-eliminating claim by citing source — two paths called identical because they share a function, state called derivable because a field already holds it — verify the warrant, not just that the identifier exists. A warrant that names real code and still does not support the claim is the highest-value finding in this pass."

Output: same `{ location, problem, fix }` triples. Each fix references the source location that contradicts the spec — cite by identifier and file, never by line number: fixes land in spec text (often AC or task paragraphs) that must survive future edits, and a line number is stale the moment the file changes.

#### Temporal reviewer (Opus)

Skip only for a spec that introduces no mutable state, no timer, and no event ordering — rare enough to justify inline.

Receives:
- Full spec content inline
- Instruction to ground orderings in source: where the spec asserts an order of operations, open the function and confirm the real order. A spec sentence describing a tick as "advance timers, then evaluate intents" is worthless if the shipped function does the reverse.
- This posture, stated as its own pass:

> Do not ask whether the spec discusses ordering. Ask whether you can
> construct an ordering that satisfies every sentence the spec writes and
> still violates what it clearly intends. Apply each probe below to every
> invariant the spec states and every piece of mutable state or timer it
> introduces. A finding without a constructible ordering — an actual
> sequence of events, not an abstract worry — is not a finding.

| Probe | Question |
|---|---|
| Same-tick collision | Two events the prose implies land on different ticks arrive on one. Which wins? Is the intermediate state observable? |
| Reversed arrival | B arrives before A, where the prose implies A precedes B. |
| Boundary crossing | A timer, queued intent, or in-flight message crosses a reset, unload, respawn, or authority handoff. Survives when it should not, or dies when it should not? |
| Batching | N of the same event in one tick. All processed, first only, last only? Does the invariant hold for every N including 0? |
| Zero-duration | A duration authored at 0 or shorter than one tick. Is the state entered? Observed? Does completion fire on the start tick? |
| Stage order | Where in Input → Game logic → Audio → Render → Present does each read and write land? Does an observer read pre- or post-mutation state, and is that stated or accidental? |
| Sampling cadence | A consumer sampling slower than the producer mutates — a snapshot interval, a per-frame publish over a per-tick change. Which samples are dropped or repeated? |

Output: `{ location, problem, fix }` triples, **plus a pin table** — `(scenario, ordering, expected outcome)` rows the spec ought to state and does not, each concrete enough to write a test from.

The pin table is the lens's primary artifact. The defect class it targets is "invariant stated, mechanics unpinned," and prose findings get applied and forgotten while a table becomes a spec section later rounds check against. Fold it in as its own section rather than dissolving its rows into existing paragraphs, and have the spec's test task reference the rows instead of restating them.

### 4. Aggregate

Collect both reports. Dedupe — when the same issue surfaces from both lenses, keep the codebase-anchor framing (more precise).

Triage by severity:

| Severity | Meaning |
|---|---|
| Blocker | Implementer cannot proceed without guessing |
| Complicates | Implementer can guess but might guess wrong |
| Nit | Style, voice, minor inconsistency |

Then split into two buckets:

| Bucket | Examples | Default action |
|---|---|---|
| Mechanical | Casing fix, missing AC bullet, wire-format pin, deletion of stale phrase | Auto-apply via Sonnet (unless `--no-auto-apply`) |
| Architectural | Reshape a contract, decide between two paths, change scope | Surface to caller; do not auto-apply |

Triage is a 30-second judgment, not a heuristic. Make the call inline. Don't delegate it to a sub-agent.

### 5. Apply mechanical fixes

If any mechanical findings exist and `--no-auto-apply` is not set:

Spawn one Sonnet agent with a numbered list of `{ location, problem, fix }` items. One Edit per item. Match the existing prose voice — terse, direct, no rewrites of surrounding paragraphs.

After the agent reports back, re-read the spec to confirm edits landed.

### 6. Decide next action

| Outcome | Recommendation |
|---|---|
| No findings, or only nits already auto-applied | Run `/review-implementability`, then promote to `ready/` |
| Mechanical fixes applied, no architectural findings | Re-run this skill once to verify fixes are clean |
| Architectural findings present | Surface to caller with locations and suggested directions. Do not auto-apply. Do not recommend promotion. |
| Findings only emerge from source-reading; spec text alone reveals nothing | Spec has hit diminishing returns. Run `/review-implementability`, then promote. |

Last row is the explicit stopping rule.

**Implementability gate.** This skill reviews what the spec *says*; `/review-implementability` reviews whether a task agent could *execute* it (task-paragraph self-sufficiency, AC achievability). Run it only once this review is structurally clean — its findings are keyed to task paragraphs, so structural rework invalidates them. Skip if it already ran clean on this revision.

### 7. Report

Concise. Include:
- What reviewers ran
- Total finding count by severity (Blocker / Complicates / Nit)
- What was auto-applied (count, not full list — caller can read the diff)
- What needs the caller's attention (architectural findings, full text)
- The recommendation

Cap at ~15 lines unless architectural findings demand more.

## Flags

- `--no-auto-apply` — surface mechanical findings to caller instead of editing. Default for human-in-the-loop use.

## Working rules

- Don't pad. Every sentence earns its place.
- No emojis anywhere — skill or prompts.
- Reviewer prompts inline the spec content. Paths drift.
- Tables for mappings, prose for behavior.
- Voice match draft-plan-priced-deferral.

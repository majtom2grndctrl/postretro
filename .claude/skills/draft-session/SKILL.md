---
name: draft-session
description: >
  Guides a planning conversation before choosing an execution artifact.
  Grounds the problem in source, resolves owner decisions, defines proof,
  and routes the work to a direct build, a problem brief, a full spec, or
  no work. Invoke first, before /draft-brief or /draft-plan, unless the
  route is already settled.
---

# Draft Session

Run the conversation that precedes an artifact. The session ends in a handoff block; the route's skill consumes it. `/draft-brief` and `/draft-plan` carry their format rules and assume this session already happened.

## Current plans

!`ls context/plans/drafts/ context/plans/ready/ context/plans/in-progress/ 2>/dev/null`

## Stance

**Build more right faster.** AI coding agents produce code quickly enough that incremental baby-steps waste more time than they save. When the destination is clear, decide the full shape and build it in strides. Small increments earn their cost only where the path is genuinely uncertain; the session's job is to resolve that uncertainty up front. When the work lays a foundation, its first consumer ships in the same unit of work — the consumer proves the foundation and keeps it from landing as a stub nothing exercises.

**Work as an orchestrator.** After a baseline read, dispatch bounded, read-only research agents — one question each, returning claim, symbol, and confidence. Keep your own context for decisions. Never delegate a product or architectural decision: a delegated answer is a report, and it grounds no one.

**Do not delegate task decomposition, line-level verification, or a map of the whole tree.** Each is the executor's job. Pre-doing it is how a conversation turns into a spec nobody asked for.

## The loop

### 1. Understand the observation

Who raised it — player, modder, developer, a review finding — and what outcome matters. Name the basis: an observed defect, a requested capability, an anticipated need, or an experiment. A defect gets a cause in one sentence; check that the outcome cures it, because an outcome that reads just as well with the cause deleted is aimed at a symptom. A capability gets the behavior it adds and who uses it; do not manufacture a defect to justify it.

### 2. Ground the premise

The user's diagnosis or framing is a hypothesis. Route through `context/lib/index.md`, open the source, and separate verified facts from hypotheses before anything else. A research question is well-formed when you can name the decision its answer would change.

| Research | What it answers |
|---|---|
| Basis | Whether the defect's cause holds in source, or whether the requested capability already exists in some form. Cheapest place to discover a different problem than the one requested. |
| Premise | The claim about the code each candidate decision rests on. One claim, one agent, answered with the symbol. |
| Commitment | What the repo already decided that touches this — `plans/done/` and `context/lib/`, grepped by concept (ownership, authority, mechanism-vs-policy), not by subsystem. |
| Precedent | A sibling feature with the same shape. Where "follows X exactly" becomes a cheap decision. |
| Doors | What the shape opens or closes later — adjacent drafts, the roadmap. |

Habits that keep hypotheses from becoming facts:

- **A coherent rationale is not evidence.** Open the file before building an argument on what it does, then check the step you took from what you read. Common slips: *this path* → *every path*; *runs there* → *knows what's in scope there*; *handles this type* → *accepts this value*.
- **Cite by symbol.** Line numbers get reconstructed from memory and go stale on the next edit.
- **Warrant work-creating claims as well as work-eliminating ones.** "This requires changing X" invents scope and a one-way door, and passes review because it looks conservative. It needs the same warrant as "no separate test needed."
- **Name the consumer before calling something a gap.** A missing value nothing reads is not a gap. No reader, no defect.
- **Verify falsity before flagging a premise as wrong.** Suspicion is not a finding. The real defect is sometimes adjacent, with a different fix.

Stop when the basis is confirmed and every candidate decision's premise is grounded.

### 3. Find the decisions

Identify what the work must settle: behavior, policy, defaults, ownership, layer placement, compatibility, one-way doors. Name the placement axis before judging it — engine-vs-mod, mechanism-vs-policy, host-vs-client, load-time-vs-runtime, descriptor-vs-code, floor-vs-authored. Non-goals are decisions too; carry a warrant only where a reader would assume the work was owed.

### 4. Ask only owner questions

Usually after step 2 — a grounded question is short and a hypothetical one wastes the owner's turn. Ask earlier only when the requested outcome is too ambiguous to aim source research. Ask about choices the repository cannot answer: product behavior, policy, a modder-facing surface, a one-way door. Recommend a direction with each question. Do not ask what source already settles.

### 5. Define proof

State the observable evidence, split into automated and manual — visual, playtest, audio, network, runtime diagnostics. Name edges, not steady state: start, stop, reverse, zero iterations, two on one tick, a timer across a reset. Pin both sides of every predicate — the case it must refuse and the case it must permit; a guard tested only where it refuses passes while over-tight. Run each row against the Problem: if it could pass with the defect still present, it measures something adjacent.

### 6. Choose the route

Route by the cost of a wrong decision after code exists, not by breadth. Work that crosses movement, scripting, content, and networking can still be a direct build when its decisions are settled and cheap to reverse.

| Route | Use when |
|---|---|
| Direct build | Decisions are clear and reversible after code exists; one integrating executor retains the conversation; durable capture can wait for landing |
| Problem brief | A decision must be reviewed before code exists — a modder-facing surface, a wire format, a one-way door — or the record must outlive the session. One integrating executor still owns decomposition, seams, and sequencing. Compact vs resumable is `/draft-brief`'s call |
| Full spec | Task contracts and sequencing must be owner-reviewed before implementation, or several executors work from isolated portions of the artifact |
| No work | Source disproves the premise, existing behavior already delivers the outcome, or an existing plan owns it |

A spike is not a route. Build-to-learn work takes the brief or spec route with the AC shape from `context/lib/experimental_spikes.md`.

Routing is provisional. `/validate-plan` can still return *Not a spec* or *Under-scoped* once an artifact exists; surface that verdict, never act on it unilaterally.

**Direct build loop.** `/build-brief` assumes a promoted brief and lifecycle directories; a direct build has neither, so its loop is stated here. The session is the integrating executor:

1. Recheck the handoff's load-bearing facts if source changed since `Read at`.
2. Keep one owner for shared contracts, integration, and commits.
3. Delegate bounded slices only with the full handoff and routed context; workers do not commit.
4. Run focused tests during implementation; confirm filters matched tests.
5. Run `/preflight`, then `/review-panel` → `/fix-review-findings` → focused retest until new concrete findings stop.
6. Record each proof row's result and any outstanding manual proof in the PR body, under the handoff block. No silent gaps.
7. Land durable decisions in `context/lib/` with the change. A direct build defers the record; it does not skip it.

## Handoff

End the session with this block. The route's skill reads it; for a direct build it is the record.

`Next action` is inferred from authorization already given, never assumed. A request to plan or explore ends at the handoff and waits. A request to build, or a "proceed" after the route is stated, continues into it. Planning authorization is not implementation authorization.

```markdown
Problem:
Outcome:
Read at: <short-sha>
Verified facts: (by symbol)
Not verified:
Decisions:
Non-goals:
Proof:
Open owner questions:
Route: direct build | brief | spec | no work
Why this route:
Next action: discuss | draft | build
```

## Closing note

Self-review finds typos and contradictions you remember making. It does not find defects you argued yourself into — the argument still seems sound. Independent lenses that cannot see each other catch those, which is why every route past *no work* still runs review.

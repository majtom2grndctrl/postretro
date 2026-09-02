---
name: draft-brief
description: >
  Drafts a problem brief for PostRetro — the lightweight spec form for a
  long-horizon executor that has the repo. Records the problem, the decisions,
  the acceptance criteria, and a non-binding path; leaves task decomposition
  and source verification to build time. Use instead of /draft-plan when
  trialing the brief process. Does not promote to ready/ — that happens after
  /validate-plan and owner sign-off.
argument-hint: "[feature-name]"
---

# Draft Brief

Explore scope, write a brief. Output lives in `context/plans/drafts/<feature-name>/index.md`, with the line under the title marking it as a brief so downstream skills can tell it from a `/draft-plan` spec.

A brief is written for a reader with the repo, not a reader with a paragraph. It records judgment and leaves verification to implementation time. Target 60–120 lines. Anything that would make it longer is derivation (→ `research.md`) or task decomposition (→ the executor's plan of record, written at build time).

## Current plans

!`ls context/plans/drafts/ context/plans/ready/ context/plans/in-progress/ 2>/dev/null`

## Three rules

1. **Nothing is restated for an agent that cannot see the rest.** The executor gets the whole brief, `research.md`, and the source tree. There is no task-paragraph contract.
2. **Decisions in, verification out.** Ground the premise of every *Decision* against source this session — a decision built on a false premise is the expensive kind. Everything else is cited by symbol and left for the executor to re-verify: the header records the commit the source was read at, and a stale *Path* claim is reported in the plan of record, not fixed in a review round. No line numbers.
3. **One review gate, at direction.** `/validate-plan` runs once. No identifier-checking review, no implementability review. The diff is reviewed instead, by `/review-panel`.

## Process

### 1. Understand the problem

Read the user's description. Ask focused questions when the problem is unclear — don't over-interrogate. Pin down what was observed and by whom, the cause, and what is true when the work is done.

### 2. Research

Read `context/lib/context_style_guide.md` first. All brief prose follows it.

Route through `context/lib/index.md` to the docs governing the subsystem. Grep `context/plans/done/` for the *concepts* the brief touches — ownership, authority, mechanism-vs-policy, layering — not just the subsystem name. Cross-epic commitments are the ones a subsystem-local drafter misses.

Use subagents for exploration. Target 80% confidence. Stop when you can write the Decisions section with every premise grounded.

Findings that inform but don't decide go to a sibling `research.md`. Lifecycle diagrams go there too; keep one in the brief only when it is the clearest statement of a decision.

### 3. Write the brief

```markdown
# <feature-name>

Brief · Epic <N> (omit if none) · reads: `context/lib/<doc>.md` §x · read at <short-sha>

## Problem
One paragraph. What was observed and by whom — player, modder, developer,
a review finding, or an anticipated need; say which. The cause in one
sentence, not a symptom of it. Then what is true when this is done, written
as behavior. This is what the executor orients by.

## Decisions
- One bullet per decision: what, and why. Cite the commitment it touches
  (`context/lib/…`, `plans/done/…`). Where it diverges from one, say so and
  argue it in a sentence. Note the undo cost only where it is not trivial.
- Non-goals are decisions too. Add the warrant only where a reader would
  otherwise assume this brief owes the work ("wall-normal forwarding:
  `movement--wall-run` owns it; slide reads only the floor normal").
- State the layer placement and the reason — engine vs mod, mechanism vs
  policy, host vs client, load-time vs runtime, descriptor vs code, floor vs
  authored; whichever axes are in play.

## Acceptance
Observable, edge-named, verifiable by someone who did not write the brief.
Orderings are rows here, not prose: two events on one tick, B before A, a
timer across a reset, N where one is expected, zero duration. A guarantee
that spans several rows gets a one-line heading over them. Named types and
functions do not appear here.

### Automated
- [ ] …
### Manual-visual
- [ ] …

## Path
Non-binding. Research distilled to what would change the executor's plan.
- Seams and precedents to build on, by symbol.
- The shape chosen and the strongest rival, one sentence each.
- The first slice: the thinnest path that falsifies the riskiest assumption.
- Files past ~800 lines this extends: split first, behavior-preserving, own commit.
- A code sketch only when it is the clearest statement of a decision.

## Open questions
- <question> — owner: <who> — **blocks build**
- <question> — **delegated**: the executor decides and reports it in the plan of record
```

**Decisions vs Path.** If the executor deviates from it, is that a defect or a note in the plan of record? Defect → Decisions. Note → Path. "Descriptor surface follows `dash`/`crouch` exactly" is a Decision; where the entry branch sits inside `normal_intent` is Path.

**Size smell** is on the Problem paragraph, not the document. Two causes in one paragraph is two briefs. Past ~120 lines, look for derivation that belongs in `research.md` or task decomposition that belongs to the executor.

**Wire formats and cross-boundary names.** When the brief adds a binary or PRL section, or crosses Rust ↔ JS/Luau ↔ wire ↔ FGD, append the `Wire format` and `Boundary inventory` sections from `/draft-plan` unchanged. There the document *is* the contract between sides built separately, and the brief is only its front half.

**Spikes.** A build-to-learn brief follows `context/lib/experimental_spikes.md`: honesty-gate ACs are pass/fail, measured findings are measure-and-report, and the last delivered item is a findings note.

### 4. Cross-check

- Every Acceptance row: which Decision or Problem sentence makes it necessary? None → it is aspirational; drop it or add the decision.
- Every Acceptance row: could it pass on a build that leaves the Problem's defect in place? Yes → it is measuring something adjacent; reword it, or label it a regression guard.
- Every Decision: which Acceptance row would fail if it were violated? None → it is either a Path hint wearing a decision's clothes, or an AC is missing.
- Every Decision premise about the code: read this session, cited by symbol.
- Every "not doing": would a reader assume this brief owed it? If so, it carries a warrant.
- Open questions: each is marked **blocks build** or **delegated**. No unmarked entries.

### 5. Commit

Stage and commit the plan folder. Amend as the brief iterates in-session; one commit per brief, not one per edit.

Do not update `context/lib/` during drafting. Durable capture happens at promotion.

### 6. Validate direction

Run `/validate-plan <name>`. It reads the brief the same way it reads a spec; the six questions apply unchanged.

Surface the verdict. Never act on *Reshape*, *Not a spec*, or *Under-scoped* unilaterally — those are owner decisions.

### 7. Report

- The problem, in one line
- Decision count, AC count, open-question count by kind
- The `/validate-plan` verdict
- Open questions marked **blocks build**, for the owner
- The brief lives in `drafts/` until promoted

## Working open questions

Between draft and promotion the owner and the drafter resolve **blocks build** questions. A resolution becomes one Decisions bullet, and the Open questions entry is removed. Re-run `/validate-plan` only when a resolution changes the Problem paragraph or swaps the chosen shape for a rival; a resolution that pins a value or a mechanism does not need it.

## Promoting to `ready/`

A brief is ready when:
- `/validate-plan` returned *Direction sound*, or the owner accepted a reshape and the brief reflects it
- No **blocks build** entries remain; every surviving entry is **delegated**
- The owner signs off

At promotion:
1. Capture durable decisions in `context/lib/` — new constraints, subsystem contracts, pipeline topology.
2. `git mv context/plans/drafts/<name> context/plans/ready/<name>`
3. Commit the move and the `context/lib/` updates together.

## What happens after

The executor is one long-horizon session, not a fan-out. Before any code it writes `plan.md` beside the brief:

- The task split and order, with the first slice named — and why, if it differs from the Path.
- Each cited identifier confirmed or corrected; any brief claim found false and what it is doing instead.
- Each **delegated** question's answer.
- An **AC-to-proof table**: every Acceptance row, the test or manual-visual step that will prove it, and a status. Manual-visual rows are proven by the owner in-engine, and the table says so.

| AC | Proof | Status |
|---|---|---|
| 3 | `slide_entry_banks_speed` | achievable as stated |
| 7 | grep gate, not a runnable test | not a test |
| 9 | proposed rewording, verbatim | needs restatement |

**ACs are owner-owned.** The executor proposes a restatement and stops; it never edits the Acceptance section. The owner accepts or pushes back before code exists. This is what keeps the executor from loosening the criteria it is about to be measured against.

The owner skims `plan.md`. That is the only check-in before the diff.

Build → focused tests per task → preflight once → `/review-panel` → `/fix-review-findings`.

**Landing report** is the AC-to-proof table with results: every AC, the test that proved it, pass or fail. An AC with no proof is a named gap in the report, never silence. At landing the brief moves to `done/` with `plan.md` beside it as the record of what was built.

**Opt-in pre-build check.** When the stakes warrant it — wire formats, cache keys, a one-way door — run `/review-brief-acceptance` before promotion for an independent read on whether each AC is achievable and proves the Problem. Not a default step.

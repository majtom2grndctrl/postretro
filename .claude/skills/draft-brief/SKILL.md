---
name: draft-brief
description: >
  Drafts a problem brief for PostRetro — the lightweight spec form for a
  repo-aware executor. Records the problem, decisions, acceptance criteria,
  and a non-binding path. Chooses compact or resumable execution before prose
  grows around the work. Use after /draft-session routes work to a brief.
  Promotion follows /validate-plan and owner sign-off.
argument-hint: "[feature-name]"
---

# Draft Brief

Explore scope, choose execution weight, write a brief. Output lives in `context/plans/drafts/<feature-name>/index.md`. The line under the title identifies the document and its execution mode.

A brief is written for a reader with the repo. One integrating executor owns the whole brief, `research.md`, and the source tree; it writes the task split and may delegate bounded slices with the same context. Write for that reader: nothing restated, nothing pre-chewed. The brief records judgment and leaves task decomposition to build time. Target 40–90 lines for compact work, 60–120 for resumable work. Longer material is derivation (`research.md`) or task decomposition (`plan.md`).

## Current plans

!`ls context/plans/drafts/ context/plans/ready/ context/plans/in-progress/ 2>/dev/null`

## Rules

1. **Nothing is restated for an agent that cannot see the rest.** Every executor gets the whole brief, relevant research and context, and the source tree. There is no task-paragraph contract.
2. **Decisions in, verification out.** Ground the premise of every *Decision* against source this session — a decision built on a false premise is the expensive kind. Everything else is cited by symbol and left for the executor to re-verify: the header records the commit the source was read at, and a stale *Path* claim is reported in the plan of record, not fixed in a review round. No line numbers.
3. **One review gate, at direction.** `/validate-plan` runs once. No identifier-checking review, no implementability review. The diff is reviewed instead, by `/review-panel`. `/review-draft-spec` never runs on a brief: its lenses emit task-paragraph fixes and pin-table prose the form has no home for, and applied they land in Decisions as binding clauses. Where the stakes warrant a detail read, `/review-brief` is the opt-in — it writes only Acceptance rows and `research.md`, and everything else is a finding for the owner.
4. **Execution weight follows coordination cost.** Cross-boundary contracts add detail to the brief. They do not alone require resumable execution. Choose resumable mode only when durable checkpoints or handoffs will earn their cost.

## Process

### 1. Frame and size

Start from the `/draft-session` handoff: problem, outcome, verified facts, decisions, proof, and the route. If none exists, run `/draft-session` first — routing is its job, not this skill's. Ask a focused question only where the handoff leaves the Problem paragraph unwritable.

Choose the execution mode:

| Mode | Use when |
|---|---|
| Compact | One coherent outcome can be built in one sustained session. This is the default. |
| Resumable | Work likely spans sessions, has ordered phases, needs durable handoffs, waits on later manual proof, or has an irreversible migration. |

Public APIs, wire changes, and cross-subsystem behavior still require complete Decisions, Acceptance, and boundary sections. They do not force resumable mode.

Revisit the mode after research. Change it before writing when new facts change the coordination cost.

### 2. Research

Read `context/lib/context_style_guide.md` first. All brief prose follows it.

Route through `context/lib/index.md` to the docs governing the subsystem. Grep `context/plans/done/` for the *concepts* the brief touches — ownership, authority, mechanism-vs-policy, layering — not just the subsystem name. Cross-epic commitments are the ones a subsystem-local drafter misses.

The handoff's verified facts are the floor, not the ceiling. Use subagents only for bounded, independent research questions — one claim, one agent, answered with the symbol. Stop when you can write Decisions with every premise grounded and the Problem's basis confirmed, not just reported.

Findings that inform but don't decide go to a sibling `research.md`. Lifecycle diagrams go there too; keep one in the brief only when it is the clearest statement of a decision.

### 3. Write the brief

```markdown
# <feature-name>

Brief · <compact|resumable> · Epic <N> (omit if none) · reads: `context/lib/<doc>.md` §x · read at <short-sha>

## Problem
One paragraph. Who raised it — player, modder, developer, a review finding
— and its basis: an observed defect, a requested capability, an anticipated
need, or an experiment. A defect gets its cause in one sentence, not a
symptom of it; a capability gets the behavior it adds and who uses it. Then
what is true when this is done, written as behavior.

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

### Scripting surface
Only when the brief adds or changes a modder-facing API. One example, in
the language a modder writes; normative for names, argument order, defaults,
return shape and calling pattern; silent on what sits behind them.

## Acceptance
Observable, edge-named, verifiable by someone who did not write the brief.
Orderings are rows here, not prose: two events on one tick, B before A, a
timer across a reset, N where one is expected, zero duration. A guarantee
that spans several rows gets a one-line heading over them. Named types and
functions do not appear here.

### Automated
- [ ] …
### Manual
- [ ] … (visual, playtest, audio, network, runtime diagnostics)

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

**Decisions vs Path.** For each sentence: if the executor deviates from it, is that a defect or a note in the plan of record? Defect → Decisions. Note → Path. "Descriptor surface follows `dash`/`crouch` exactly" is a Decision; where the entry branch sits inside `normal_intent` is Path.

**Scripting surface.** A modder-facing API — an SDK function, a descriptor field, a script event — is designed by the owner, and its shape is a Decision: once a mod depends on it, it is a one-way door. The brief carries it as a code example under `### Scripting surface` inside Decisions, written the way a modder would write it. The example is normative for the surface — names, argument order, defaults, return shape, the calling pattern — and says nothing about the engine behind it; SDK internals and Rust do not appear. It is also a fixture: one Acceptance row runs it, as a test or a `content/dev` script, so the example cannot drift from what ships. A TypeScript example implies its Luau mirror, and the Boundary inventory says whether both ship. Path may sketch an alternative shape for the owner to weigh; Path never carries the one that ships.

**Argue in `research.md`; conclude in the brief.** A Decisions bullet that runs past three sentences is still arguing. State a fact once — a fact in two places is a defect waiting for a fix to land in one of them — and never write a count in prose; the enumeration stays right when the count goes stale.

**Size smell** is on the Problem paragraph, not the document. Two problems in one paragraph is two briefs. Past the mode's target, move derivation to `research.md` and task decomposition to the executor. Required boundary and wire tables do not count toward the target.

**Wire formats and cross-boundary names.** When the brief adds a binary or PRL section, or crosses Rust ↔ JS/Luau ↔ wire ↔ FGD, append the `Wire format` and `Boundary inventory` sections from `/draft-plan` unchanged. There the document *is* the contract between sides built separately, and the brief is only its front half.

**Spikes.** A build-to-learn brief follows `context/lib/experimental_spikes.md`: honesty-gate ACs are pass/fail, measured findings are measure-and-report, and the last delivered item is a findings note.

### 4. Cross-check

- Every Acceptance row: which Decision or Problem sentence makes it necessary? None → it is aspirational; drop it or add the decision.
- Every Acceptance row: could it pass on a build that leaves the Problem unsolved — the defect present, the capability absent? Yes → it is measuring something adjacent; reword it, or label it a regression guard.
- Every Decision: which Acceptance row would fail if it were violated? None → it is either a Path hint wearing a decision's clothes, or an AC is missing.
- Every Decision premise about the code: read this session, cited by symbol.
- Every "not doing": would a reader assume this brief owed it? If so, it carries a warrant.
- The Scripting surface example, if present: an Acceptance row runs it, and every name in it either resolves against the SDK or is one this brief adds.
- Open questions: each is marked **blocks build** or **delegated**. No unmarked entries.

### 4b. Revising

- A resolved question becomes one Decisions bullet and leaves. No history: not "was open," not "we settled on." The executor never saw the question.
- A review finding lands as an Acceptance row, a `research.md` pin row, or a Decisions edit the owner makes in their own words. Reviewer prose never lands in Decisions — that parenthetical is how a brief turns back into a spec.
- Re-read Decisions after any Problem edit. A reframed basis orphans a decision that answered the old one, and the diff never touches the orphan.
- Read the diff, not the result. Amend-only histories destroy the per-round diff, so snapshot before each round and diff against the snapshot. An edit that drops a trailing line reads fine everywhere you think to look.

### 5. Commit

For resumable drafting, stage and commit the plan folder. Amend as the brief iterates in-session; one commit per brief, not one per edit.

For compact work continuing in the same session, keep the draft uncommitted until promotion. Commit sooner when the session may end or the owner wants a durable review point.

Do not update `context/lib/` during drafting. Durable capture happens at promotion.

### 6. Validate direction

Run `/validate-plan <name>`. It reads the brief the same way it reads a spec; the six questions apply unchanged.

Surface the verdict and read it as a fresh reader would; do not rebut it from inside the session that drafted the brief. Never act on *Reshape*, *Not a spec*, or *Under-scoped* unilaterally — those are owner decisions.

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

`/build-brief` reads the mode from the header. Both modes write `plan.md` with corrections, delegated answers, task split, and an AC-to-proof table.

- **Compact:** write the plan, commit it with the move to `in-progress/`, and continue. Promotion plus invocation is approval; there is no second plan stop.
- **Resumable:** commit the proposed plan and stop for the owner's skim before implementation.

**Decisions and Acceptance are owner-owned.** A material change to either requires a proposed restatement and an owner decision. A clarification that preserves both goes in `plan.md`; it does not stop the build. A false Decision premise always stops.

At landing the table gains a result column — every AC, its proof, pass or fail; a gap is named, never silent — and the brief moves to `done/` with `plan.md` beside it.

**Opt-in pre-build check.** When the stakes warrant it — a renderer state machine, a wire format, a cache key, a one-way door — run `/review-brief` before promotion. It fact-checks Decision premises, pins orderings as rows, asks whether each AC is achievable and proves the Problem, and asks what could be deleted; it edits only Acceptance and `research.md`, and every finding on Decisions is the owner's. Not a default step, and never a second round — a brief that needs one has a Decisions problem, which is `/validate-plan`'s.

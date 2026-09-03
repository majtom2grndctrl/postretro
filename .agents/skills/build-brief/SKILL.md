---
name: build-brief
description: >
  Executes one promoted PostRetro problem brief end to end: verifies it against
  source, creates a resumable plan of record, waits for approval, implements
  sequentially, reviews, and lands it. Use for a brief in context/plans/ready/
  or to resume its in-progress build; do not use for task-fan-out plans.
argument-hint: "[brief-name]"
---

# Build Brief

Build one promoted brief in a single long-horizon session. This is the
single-executor counterpart to `orchestrate`: the executor writes the task
split and implementation, and `plan.md` is the durable checkpoint.

Stop only after proposing `plan.md`, or when a Decision premise is false.
Record other source drift as a correction and continue.

## Locate the brief

Inspect `context/plans/ready/` and `context/plans/in-progress/`.

| State | Action |
|---|---|
| Brief is in `ready/` | Start at **Take the brief**. |
| `plan.md` says `status: proposed` | Report the plan and wait for the user. |
| `plan.md` says `status: approved` with unfinished tasks | Resume at the first unfinished task. |
| `plan.md` says `status: approved` with all tasks done | Resume at **Preflight and review**. |
| `plan.md` says `status: blocked` | Report the block and wait for the user. |

## Take the brief

Start from a clean, current `main`. Move the requested folder from `ready/` to
`in-progress/`, commit that state transition, then create the feature branch
using the repository's current branch convention. Read, in order:

1. `context/lib/index.md` and the subsystem documents it routes to.
2. `context/lib/development_guide.md` and `context/lib/testing_guide.md`.
3. The brief's `index.md`, then its `research.md` when present.

The Problem paragraph is the standard for later choices.

## Verify against source

Re-read every source symbol named by the brief's Decisions and Path.

- A stale **Path** claim is expected implementation drift. Add a concise
  correction to `plan.md` describing the current seam and the adjusted path.
- A false **Decision** premise is user-owned. Set `status: blocked` in
  `plan.md`, name the false premise and evidence, commit, and stop. Do not
  implement around it or edit Decisions/Acceptance criteria.

If a proposed task would extend a source file already around 800 lines, plan a
behavior-preserving split first unless the source inspection shows no coherent
seam.

## Write the plan of record

Create `plan.md` beside the brief. It must make resumption possible without
the previous conversation.

```markdown
# <brief name> — plan of record

status: proposed
read at: <short sha>

## Corrections
- <brief claim> → <current source fact>; planning around it by <approach>

## Delegated answers
- <brief open question> — <answer and one-sentence rationale>

## AC-to-proof

| AC | Proof | Status |
|---|---|---|
| <acceptance criterion> | `<focused_test>` | achievable as stated |
| <acceptance criterion> | user, in-engine | manual-visual |

## Tasks

| # | Task | Status |
|---|---|---|
| 1 | <thinnest slice that tests the riskiest premise> | |
```

Include every Acceptance criterion. Give each a focused automated proof,
manual user proof, or `needs restatement` plus exact proposed wording. The
first task should be the smallest slice that tests the highest-risk premise;
explain any departure from the Path.

Commit `plan.md`, report corrections, proof gaps, and task order, then wait for
the user's approval before editing implementation code.

## Approval and build

On approval, set `status: approved` and commit. If the user revises an
Acceptance criterion, update its proof row to the accepted wording.

Build tasks in order. For every task:

1. Read dependent code and relevant context before editing.
2. Implement and run focused tests for the touched crate/module; verify the
   filter actually ran tests.
3. Commit the task with its task number in the message.
4. Mark the task `done · <short sha> · <test command>` in `plan.md` and commit
   the checkpoint.

Keep implementation in this executor. Do not fan out task paragraphs to
agents; use `orchestrate` when a reviewed plan explicitly calls for concurrent
task execution. A Decision that becomes unbuildable is a blocked stop; a Path
change is a `Corrections` entry.

## Preflight and review

After all tasks are complete, use the project's `preflight`, `review-panel`,
and `fix-review-findings` skills. Fix mechanical findings. If a fix would alter
an owner-owned Decision or Acceptance criterion, record the evidence and stop
for the user's direction.

## Land

Add a landing report to `plan.md` with every Acceptance criterion, its proof,
and pass/fail result. Name any remaining manual proof as a gap. Add trial notes:

```markdown
## Trial notes
- sessions used: N
- tasks that needed rework after their first commit: N
- review-panel findings: N (N acted on)
- Decision premises found false: N
- Path claims corrected: N
```

Update applicable `context/lib/` contracts, move the brief folder to
`context/plans/done/`, and commit the move with its documentation update.
Report the branch, landing table, trial notes, and outstanding manual checks.

## Invariants

- `plan.md` is the committed source of resumption state.
- Decisions and Acceptance criteria belong to the user; propose wording but do
  not silently rewrite them.
- This workflow is sequential by design. Use `orchestrate` for task fan-out.

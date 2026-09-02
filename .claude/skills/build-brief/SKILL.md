---
name: build-brief
description: >
  Executes a promoted problem brief as one long-horizon session: verifies the
  brief against source, writes the plan of record, stops for the owner's skim,
  then builds task by task with plan.md as the checkpoint, reviews the diff, and
  lands with an AC-to-proof report. Use on a brief in ready/ or to resume one in
  in-progress/. Counterpart to /orchestrate for the brief process — no task
  fan-out, no task-paragraph contract.
argument-hint: "[brief-name]"
---

# Build Brief

One session builds the whole brief. It reads the brief, `research.md`, and the source tree; it writes its own task split; it keeps its state in `plan.md` so a fresh session can pick up where this one stopped. Nothing is dispatched a paragraph at a time.

Two stops, and only two: after `plan.md` is written, and on a false Decision premise. Everything else is a note in `plan.md` and keep going.

## Where to enter

!`ls context/plans/ready/ context/plans/in-progress/ 2>/dev/null`

| State | Do |
|---|---|
| Brief is in `ready/` | Start at step 1 |
| `plan.md` says `status: proposed` | Stop. Report the plan; wait for the owner |
| `plan.md` says `status: approved`, tasks remain | Resume at the first task without a `done` row |
| `plan.md` says `status: approved`, all tasks `done` | Resume at step 6 |
| `plan.md` says `status: blocked` | Report the block; wait for the owner |

## Process

### 1. Take the brief

```
git mv context/plans/ready/<name> context/plans/in-progress/<name>
git checkout -b <name>
```

Commit the move. Read, in order: `context/lib/index.md` and the docs it routes to for this subsystem, `context/lib/development_guide.md`, the brief, `research.md` if present. The brief's Problem paragraph is what every later choice is measured against; read it twice.

### 2. Verify against source

Re-read every symbol the brief cites, in Decisions and in Path. Two outcomes, and the difference matters:

- **A Path claim is stale** — the seam moved, the precedent was refactored, the sketch does not fit. Note it in `plan.md` under *Corrections* and plan around it. Path is non-binding; being wrong there is expected.
- **A Decision premise is false** — the claim the decision rests on does not hold. Set `status: blocked` in `plan.md`, name the premise and what you found, commit, and stop. The brief was built on that premise; whether the decision survives is the owner's call. Do not route around it.

Decisions and Acceptance are owner-owned. The executor edits neither. It proposes, verbatim, and stops.

### 3. Write the plan of record

`plan.md` beside the brief. This is the checkpoint the whole build runs from — a session that dies mid-build is recovered from this file, so keep it current.

```markdown
# <name> — plan of record

status: proposed
read at: <short-sha>

## Corrections
- <cited symbol> → <what is there now>, planning around it by <how>

## Delegated answers
- <open question from the brief> — <answer, one sentence of why>

## AC-to-proof

| AC | Proof | Status |
|---|---|---|
| 1 | `test_name` | achievable as stated |
| 4 | owner, in-engine | manual-visual |
| 6 | proposed rewording: "<verbatim>" | needs restatement |

## Tasks
| # | Task | Status |
|---|---|---|
| 1 | <first slice, and why it is first if that differs from the Path> | |
| 2 | … | |
```

The first slice is the thinnest change that falsifies the brief's riskiest assumption. Take the Path's suggestion unless verification gave a reason not to; say the reason if so.

Every Acceptance row appears in the table. A row with no proof is `needs restatement` with a proposed wording, never omitted. Manual-visual rows are the owner's to prove and the table says so.

Commit `plan.md`. **End the turn.** Report the plan (Corrections, any `needs restatement` rows, the task order) and do not write code until the owner says go.

### 4. Approval

The owner skims and says go, or edits the brief and says go. On go, set `status: approved` in `plan.md` and commit. If the owner accepted a restatement, they edit the brief's Acceptance; update the table's row to `achievable as stated` and cite the new wording. This is the only pre-code check-in.

### 5. Build

Task by task, in the plan's order. For each:

- Implement. Read dependent code before editing it; follow the subsystem docs from step 1.
- Focused tests for the touched crate or module — `cargo test -p <crate> <filter>`, one target (`--lib` or `--bin <name>`). Check the count; a filter matching nothing prints `0 passed` and exits `ok`.
- One commit per task, with the plan's task number in the message. Not amended — the review diff and the resumability both need the history.
- Mark the row `done · <short-sha> · <test>` in `plan.md` and commit that too. A task is not done until its row says so.

A Decision that turns out to be unbuildable as written is a step-2 stop, not a deviation: `status: blocked`, name it, end the turn. A Path deviation is a *Corrections* line. If unsure which, it is a Decision.

Subagents are for reading — tracing a call path, checking a precedent. The executor writes the code, so the plan and the diff come from one context.

Files past ~800 lines that a task extends are split first, behavior-preserving, in their own commit, before the task's commit.

### 6. Preflight and review

Run `/preflight` once, on the finished branch. Fix mechanical failures; a failure that needs a design choice is a `blocked` stop.

Then `/review-panel`, then `/fix-review-findings`. Findings that would change a Decision or an AC go to the owner, not into the diff.

### 7. Land

Fill the **landing report** at the bottom of `plan.md`: the AC-to-proof table with a result column — every AC, the test or owner step that proved it, pass or fail. An AC with no proof is a named gap; never silence.

Then a **trial notes** block, five lines, so the process can be measured against `/orchestrate`:

```markdown
## Trial notes
- sessions used: N
- tasks that needed rework after their first commit: N
- review-panel findings: N (N acted on)
- Decision premises found false: N
- Path claims corrected: N
```

Update `context/lib/` for any behavior the build changed — subsystem docs, constraints, contracts. `git mv` the folder to `done/` with `plan.md` beside it. Commit the move and the doc updates together. Report: the branch, the landing table, the trial notes, and any manual-visual rows still waiting on the owner.

## Working rules

- **`plan.md` is truth.** Any state that matters to resumption lives there, committed. A fresh session should never need this session's transcript.
- **Two stops.** Plan written; Decision premise false. A session that wants a third stop is usually looking at a Path deviation and should write the *Corrections* line instead.
- **Owner-owned text.** Decisions and Acceptance are edited by the owner. The executor's job is to propose exactly and stop.
- **No fan-out.** This skill does not dispatch tasks to agents with a paragraph each. That is `/orchestrate`, for `/draft-plan` specs.

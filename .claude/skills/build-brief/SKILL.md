---
name: build-brief
description: >
  Executes a promoted PostRetro problem brief end to end. Compact briefs use
  one continuous build with light checkpoints. Resumable briefs add an owner
  plan review and durable task checkpoints. Both verify decisions, map
  acceptance to proof, run review and fix loops, and land the result.
argument-hint: "[brief-name]"
---

# Build Brief

Build one promoted brief. Read its `compact` or `resumable` mode from the header. Both modes preserve the same Decisions and Acceptance contract. Mode changes coordination weight.

The integrating executor owns `plan.md`, shared contracts, commits, and final verification. It may delegate bounded implementation slices. Every worker reads the whole brief and relevant context. Never dispatch a task paragraph alone.

## Locate the brief

Inspect `context/plans/ready/` and `context/plans/in-progress/`.

| State | Action |
|---|---|
| Brief in `ready/` | Start at **Take the brief**. |
| Compact `plan.md` says `active` | Resume the first unfinished task. |
| Resumable `plan.md` says `proposed` | Report the plan and wait for owner approval. |
| Resumable `plan.md` says `approved` | Resume the first unfinished task. |
| All tasks done | Resume at **Preflight and review**. |
| `plan.md` says `blocked` | Report the block and wait for the owner. |

When the owner resolves a block, apply only the authorized wording or decision. Return to the step that raised it and re-run that check. Set the mode's normal status only after the block clears.

## Take the brief

Start from clean, current `main`. Create the feature branch and move the brief from `ready/` to `in-progress/`. Do not commit the move yet.

Read, in order:

1. `context/lib/index.md` and routed subsystem docs.
2. `context/lib/development_guide.md` and `context/lib/testing_guide.md`.
3. Brief `index.md`, then `research.md` when present.

Problem defines success. Decisions and Acceptance define the contract.

## Verify source

Resumable mode re-reads every source symbol cited by Decisions and Path.

Compact mode inspects source changes since the brief's `read at` commit. If no source changed, reuse the grounded Decision reads. Otherwise re-open affected cited symbols and their boundary consumers. Conversation continuity is not proof of unchanged source.

- **Stale Path:** record current source and adjusted approach under *Corrections*. Continue.
- **False Decision premise:** create a minimal `plan.md` with `status: blocked` and the evidence. Commit it with the move to `in-progress/`, then stop. The owner decides whether the Decision survives.

Decisions and Acceptance belong to the owner. Stop for a material change to either. Record a clarification under *Corrections* and continue when both keep the same meaning.

## Write the plan of record

Create `plan.md` beside the brief. It must support resumption without the prior conversation.

```markdown
# <brief name> — plan of record

mode: compact | resumable
status: active | proposed | approved | blocked
read at: <short sha>

## Corrections
- <brief claim> → <current source fact>; planning around it by <approach>

## Delegated answers
- <brief question> — <answer and one-sentence reason>

## AC-to-proof

| AC | Proof | Status |
|---|---|---|
| 1 | `focused_test` | achievable as stated |
| 4 | owner, in-engine | manual |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | <thin slice testing highest-risk premise> | integrating executor | — | |
```

Include every Acceptance row. Assign automated proof, manual proof, or `needs restatement` with exact proposed wording. First task tests the riskiest assumption through the thinnest useful slice.

If any row needs restatement, set `status: blocked`, commit the plan with the move, and stop. This applies to both modes.

For compact mode, set `status: active`. Commit the move and plan together, then continue. Promotion and `/build-brief` invocation are approval.

For resumable mode, set `status: proposed`. Commit the move and plan together. Report corrections, ownership, and task order. Stop for the owner's skim. On approval, set `status: approved` and commit before implementation so a new session can recover the approval.

## Build

Execute tasks in dependency order. Keep shared contracts and integration with the integrating executor.

Delegate only when a slice has clear ownership and can be reviewed independently. Give each worker:

- Full brief and relevant `research.md`.
- Routed context docs and full Acceptance list.
- Named files or subsystem ownership.
- Upstream contracts and downstream consumers.
- Instruction not to edit `plan.md` or commit.

Run independent workers concurrently when their files and contracts do not overlap. The integrating executor resolves shared seams and runs Cargo commands after concurrent edits finish.

For each task:

1. Read dependent code before editing.
2. Implement and integrate.
3. Run focused tests; confirm filters matched tests.
4. Update the task row with proof and status.

Compact mode commits coherent milestones. One commit may cover the feature. Include the matching `plan.md` update in the same commit.

Resumable mode commits each completed task with its `plan.md` update. Do not create a second status-only commit.

A false Decision premise remains a blocked stop. A Path change remains a Correction. A material Decision or Acceptance change requires owner direction. A clarification that preserves their meaning does not.

## Preflight and review

Run `/preflight` after integration. Then run `/review-panel` and `/fix-review-findings` as a review → fix → focused retest loop. Repeat only while new concrete findings appear.

Mechanical fixes proceed. Findings that change a Decision or Acceptance row go to the owner. After fixes, run the full relevant gate once.

## Land

Add a result column to the AC-to-proof table. Record pass, fail, or outstanding manual proof for every row. No silent gaps.

Update durable `context/lib/` contracts. Move the brief to `context/plans/done/`. Commit the move, plan, and context updates with the final coherent change.

Add trial notes only when the owner is evaluating the process:

```markdown
## Trial notes
- mode: compact | resumable
- sessions used: N
- delegated implementation slices: N
- review-panel findings: N (N acted on)
- Decision premises found false: N
- Path claims corrected: N
```

Report the branch, landing table, review loop, and outstanding manual checks.

## Invariants

- `plan.md` holds state needed to resume.
- Decisions and Acceptance remain owner-owned.
- Compact changes checkpoints, not contract rigor.
- Delegation shares full context; no task-paragraph contracts.
- Integrating executor owns seams, tests, and commits.

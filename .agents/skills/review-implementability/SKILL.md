---
name: review-implementability
description: >
  Single-agent review of a spec through the executor's lens: would a task
  agent given only its own task paragraph plus the AC list build the right
  thing, and is each AC realistically achievable and a sound metric? Run
  after structural review is clean (`review-draft-spec` recommends promote),
  or a la carte on a ready spec before `orchestrate`.
argument-hint: "[plan-name]"
---

# Review Implementability

One reviewer, one lens: execution. This is not a general spec review. Run it
only after the spec is structurally sound: no contradictions, no AC to task
gaps, scope settled. Implementability findings are keyed to specific task
paragraphs; structural rework invalidates them, so sequencing matters.

## Premise

`orchestrate` gives each task agent only:

- Its own task paragraph
- The plan's acceptance criteria
- The `context/lib/` router
- Source access

The task agent does not receive Scope, other tasks' text, or the full plan
document. This review simulates that contract.

## Process

### 1. Locate And Read

Argument is a plan folder name or full path. If it is a folder name, look in
`context/plans/ready/` first, then `context/plans/drafts/`. Resolve to
`index.md`. If absent, list ready and draft plans and ask which one.

Read the full spec yourself before delegating.

### 2. Spawn One Reviewer

Spawn one high-reasoning read-only reviewer. Inline the full spec content in
the prompt; paths drift. Also pass:

- Locked owner decisions, marked do-not-relitigate
- Relevant `context/lib/` docs routed through `context/lib/index.md`
- Key source files the spec touches

The reviewer answers two questions exhaustively.

**Q1 — Per Task:** would a fresh agent with only this paragraph, the AC list,
lib docs, and source access build the right thing?

- Does the paragraph name every file or seam to touch, or are there unstated
  call sites to discover?
- Are earlier phases' outputs identified well enough to find in the tree? The
  task agent cannot read earlier task text.
- Is any load-bearing detail stated only in Scope, where a task-only reader
  misses it?
- Could a literal reading satisfy the task text while violating the spec's
  intent?

**Q2 — Per AC:** is each AC realistically achievable against the actual
codebase, and is it a sound metric?

- Verify achievability against source: round-trip claims vs. serialization
  behavior, counters and seams the assertions need, `#[cfg(test)]` gating,
  whether warnings or logs are observable by the harness, fixture literals vs.
  type choices.
- Flag ACs that are untestable as stated, over-specified enough to fail a
  correct implementation, or under-specified enough to pass a wrong one.
- Negative-existence claims such as "no X is added" are review or grep gates,
  not runnable tests. Mark them that way.

Output format:

- Per task: one-line verdict, either `Sets up success` or `Needs tightening`,
  plus findings as `{ location, problem, fix, severity: Blocker | Complicates | Nit }`.
- Per AC: verdict, either `Achievable+sound` or `Problem`, plus findings.
- Per spec: summary line, either `ready to orchestrate` or what to tighten first.

### 3. Triage And Apply

The reviewer reports; the orchestrating session owns fixes. Apply determinate
fixes directly: completions of the stated design, plumbing enumeration, fixture
or type corrections. Surface genuine path choices and anything touching a
locked decision to the human.

If the user explicitly requested commit or promotion, commit applied fixes.
Otherwise leave edits unstaged and report them.

### 4. Report

Report:

- Spec verdict
- Blocker count and disposition: fixed vs. surfaced
- Any architectural or owner-decision items requiring the caller
- Recommendation: orchestrate as-is, tighten first, or escalate

## Working Rules

- One reviewer, both questions. Do not split into per-task agents; cross-task
  phasing gaps are half the findings.
- Inline the spec; paths drift.
- The reviewer never edits; this session does.
- Do not run on a structurally unsettled spec. Run `review-draft-spec` first.
- No padding. No praise. No emojis.

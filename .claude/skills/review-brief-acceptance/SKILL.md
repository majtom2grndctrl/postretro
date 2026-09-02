---
name: review-brief-acceptance
description: >
  Opt-in single-agent review of a problem brief's Acceptance section: is each
  AC realistically achievable against current source, and would it fail on a
  build that leaves the Problem uncured? Run before promoting a brief whose
  stakes warrant an independent read — wire formats, cache keys, one-way
  doors. Not a default step; the executor's AC-to-proof table covers the
  ordinary case.
argument-hint: "[brief-name]"
---

# Review Brief Acceptance

One reviewer, one section. Never task decomposition, never identifiers outside the Acceptance rows, never direction — `/validate-plan` owns that and runs first.

## Premise

The executor writes an AC-to-proof table before code and proposes restatements to the owner. That is the ordinary accountability path. This skill exists for the brief where a wrong AC costs more than a review: the criteria gate a persistent format, a cache epoch, or something that cannot be re-cut after landing.

## Process

### 1. Locate and read

Argument is a plan folder name; look in `drafts/` first, then `ready/`. Read the brief yourself before delegating. The line under the title must mark it as a brief; on a `/draft-plan` spec, stop and point at `/review-implementability` instead.

### 2. Spawn one reviewer (read-only, fresh context)

Inline the brief verbatim — paths drift. Pass the `context/lib/` docs the header names and the source files the Path cites. Instruct: report only, no edits, no cargo commands.

Two questions, per AC row:

**Achievable.** Against the actual codebase, can a test assert this as written? Check the seams the assertion needs: counters behind `#[cfg(test)]`, whether a warn is observable by the harness, serde round-trip behavior, fixture literals vs. type choices. Negative-existence claims ("no X is added") are grep gates, not tests — mark them so.

**Proves the Problem.** Could this row pass on a build that leaves the Problem paragraph's defect in place? If yes, name what it is measuring instead. A regression guard is fine when it says it is one; a headline AC that only guards is the finding.

Also, once for the whole section: is there a Decision with no row that would fail if it were violated? Name it.

**Output:** per AC, `Achievable + proves` or a finding `{ row, problem, proposed rewording, severity: Blocker | Complicates | Nit }`. One line for uncovered Decisions. One summary line: promote, or which rows to fix first.

### 3. Triage

The reviewer reports; this session owns edits. ACs are owner-owned: apply a rewording only when it is a completion of what the row already meant. Anything that changes what is being measured goes to the owner as a choice, with the proposed text.

### 4. Report

Row count, findings by severity, what was applied and what was surfaced, and the recommendation.

## Working rules

- Acceptance rows only. A Path claim that turns out false is the executor's to correct in `plan.md`; raising it here dilutes the lens.
- Inline the brief; paths drift.
- The reviewer never edits; this session does, and only completions.
- No padding, no praise, no emojis. Voice match `/draft-brief`.

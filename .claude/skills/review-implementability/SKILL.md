---
name: review-implementability
description: >
  Single-agent review of a spec through the executor's lens: would a task
  agent given only its own task paragraph plus the AC list build the right
  thing, and is each AC realistically achievable and a sound metric? Run
  after structural review is clean (review-draft-spec recommends promote),
  or a la carte on a ready/ spec before /orchestrate.
argument-hint: "[plan-name]"
---

# Review Implementability

One Opus agent, one lens: execution. Not a general spec review — run it only after the spec is structurally sound (no contradictions, no AC ↔ task gaps, scope settled). Implementability findings are keyed to specific task paragraphs; structural rework invalidates them, so sequencing matters.

## Premise

`/orchestrate` gives each task agent ONLY: its own task paragraph, the plan's Goal, the plan's AC list, the `context/lib/` router, and source access. No Scope section, no other tasks' text, no full plan document. This review simulates that contract.

The contract is defined normatively in `/orchestrate`. If the two disagree, `/orchestrate` wins — update this skill to match.

## Process

### 1. Locate and read

Argument is a plan folder name; look in `ready/` first, then `drafts/`. Read the full spec yourself before delegating.

### 2. Spawn one reviewer (Opus, read-only)

Inline the full spec content in the prompt — paths drift. Also pass: the locked owner decisions (marked do-not-relitigate), the relevant `context/lib/` docs, and the key source files the spec touches. Instruct: report findings only, make no edits.

The agent answers two questions, exhaustively:

**Q1 — per task:** would a fresh agent with only this paragraph + the plan Goal + the AC list + lib docs + source access build the RIGHT thing?
- Does the paragraph name every file/seam to touch, or are there unstated call sites to discover?
- Are earlier phases' outputs identified well enough to find in the tree (the agent can't read their task text)?
- Is any load-bearing detail stated only in Scope, where a task-only reader misses it?
- Could a literal reading satisfy the task text while violating the spec's intent?

**Q2 — per AC:** realistically achievable against the actual codebase, and a sound metric?
- Verify achievability against source: round-trip claims vs. serde behavior, counters/seams the assertions need (`#[cfg(test)]` gating), whether warns/logs are observable by the harness, fixture literals vs. type choices.
- Flag ACs that are untestable as stated, over-specified (would fail a correct implementation), or under-specified (would pass a wrong one).
- Negative-existence claims ("no X is added") are review/grep gates, not runnable tests — mark them so.

**Output format:** per task, a one-line verdict (Sets up success / Needs tightening) plus findings as `{ location, problem, fix, severity: Blocker | Complicates | Nit }`. Per AC, a verdict (Achievable+sound / Problem) plus findings. Per-spec summary line: ready to orchestrate, or what to tighten first.

### 3. Triage and apply

The reviewer reports; the orchestrating session owns fixes. Apply determinate fixes directly — completions of the stated design, plumbing enumeration, fixture/type corrections. Surface genuine path choices and anything touching a locked decision to the human. Commit applied fixes.

### 4. Report

Verdicts per spec, blocker count and disposition (fixed vs. surfaced), and the recommendation: orchestrate as-is, tighten first, or escalate.

## Working rules

- One agent, both questions. Don't split into per-task agents — cross-task phasing gaps are half the findings.
- Inline the spec; paths drift.
- The reviewer never edits; this session does.
- Don't run on a structurally unsettled spec — fix structure first (`/review-draft-spec`).
- No padding, no praise, no emojis.

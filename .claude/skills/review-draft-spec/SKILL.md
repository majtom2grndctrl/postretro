---
name: review-draft-spec
description: >
  Multi-agent review of a draft spec in `context/plans/drafts/`. Spawns
  two parallel reviewers — a broad reviewer and a codebase-anchor
  reviewer that fact-checks every named identifier against source.
  Auto-applies mechanical fixes via a Sonnet sub-agent unless
  --no-auto-apply is set. Recommends apply / re-review / promote.
  Use after a draft session, or when a human wants to validate before
  promoting to ready/.
---

# Review Draft Spec

Two reviewers in parallel. One broad, one anchored to source. Aggregate findings, auto-apply mechanical fixes, recommend whether to apply more, re-review, or promote.

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

One message, two `Agent` tool calls. No sequential rounds.

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
  - Anything else that forces an implementer to guess

Output: list of `{ location, problem, fix }` triples. "No issues found" if clean. No padding, no praise.

#### Codebase-anchor reviewer (Opus)

Receives:
- Full spec content inline
- Instruction: "For every Rust/TS/Lua identifier the spec names — function, struct, type, field, enum variant, module path — open the file in source, confirm the spec's claim, report any divergence between the spec and current code reality. First step: extract the identifier list from the spec. Then resolve files via Glob/Grep. Then batch-read."

Output: same `{ location, problem, fix }` triples. Each fix references the source location that contradicts the spec.

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
| No findings, or only nits already auto-applied | Promote to `ready/` |
| Mechanical fixes applied, no architectural findings | Re-run this skill once to verify fixes are clean |
| Architectural findings present | Surface to caller with locations and suggested directions. Do not auto-apply. Do not recommend promotion. |
| Findings only emerge from source-reading; spec text alone reveals nothing | Spec has hit diminishing returns. Promote. |

Last row is the explicit stopping rule.

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
- Voice match draft-plan.

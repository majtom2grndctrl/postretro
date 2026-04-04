---
name: plan
description: >
  Plans a feature or epic by researching the problem space, writing a spec with
  task breakdown and sequencing, and updating context files with durable decisions.
  Use when starting new work that needs a spec before implementation.
disable-model-invocation: true
argument-hint: "[feature-name]"
---

# Plan

Plan a feature or epic for PostRetro. Output: a plan folder in `context/plans/drafts/` that moves to `context/plans/ready/` after review.

## Current project state

!`ls context/plans/drafts/ context/plans/ready/ context/plans/in-progress/ 2>/dev/null`

## Process

### 1. Understand the goal

Read the user's description. If scope is unclear, ask focused questions — don't over-interrogate. Understand:
- What outcome matters
- What constraints apply
- What subsystems are involved
- **What "done" looks like** — define concrete, verifiable completion criteria with the user. Not "it works" but "BSP faces render with correct lightmap UVs, fallback to white when RGB lump missing."
- **Quality gates** — measurable bar for the work. Examples: tests pass, no clippy warnings, specific behaviors verified manually, performance within a threshold. These become acceptance criteria on individual tasks.

### 2. Research

Read `context/lib/context_style_guide.md` first — all plan content and context file updates follow this style. Then load relevant context library files:

!`ls context/lib/`

Use subagents for research when useful — codebase exploration, reading docs, understanding existing patterns. The goal is 80% confidence, not exhaustive coverage. Stop researching when you have enough to spec the work.

### 3. Write the plan

Create a folder: `context/plans/drafts/<feature-name>/`

Create `plan.md` with this structure:

```markdown
# <Feature Name>

## Goal
What this achieves and why it matters. 1-3 sentences.

## Scope
What's in and what's out. Explicit non-goals.

## Shared Context
Architectural decisions, constraints, and contracts that apply across
all tasks. Any agent working a task in this plan should read this section.

## Tasks

### Task 1: <name>
**Description:** What to build.
**Acceptance criteria:**
- [ ] Specific, verifiable conditions for "done"
**Depends on:** (none | Task N)

### Task 2: <name>
...

## Sequencing

Explicit instructions for how tasks should be executed:

**Phase 1 (sequential):**
- Task 1 — must complete before anything else

**Phase 2 (concurrent):**
- Task 2, Task 3 — independent, safe to run in parallel

**Phase 3 (sequential, after Phase 2):**
- Task 4 — depends on Task 2 and Task 3 output

## Notes
Open questions, risks, alternatives considered.
```

**Sequencing rules:**
- Tasks are concurrent by default unless they share files, data contracts, or have input/output dependencies.
- Be specific about WHY tasks are sequential — name the dependency ("Task 3 consumes the vertex format defined in Task 2").
- When in doubt, sequential is safer than concurrent.
- Each phase completes fully before the next phase begins.

### 4. Update context files

Before the plan can move to `ready/`, capture durable decisions in `context/lib/`:
- New architectural decisions or constraints
- New subsystem contracts or boundaries
- Updates to existing context files that reflect planning decisions

Agents working the plan should find full context in the codebase, not derive it from the plan.

### 5. Commit

Stage and commit plan folder and context file updates together.

### 6. Done

Report to the user:
- What was planned and why
- Task count and sequencing summary
- Context files updated
- Plan is in `drafts/` — `git mv` to `ready/` when satisfied

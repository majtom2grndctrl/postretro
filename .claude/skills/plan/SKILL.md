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

You are planning a feature or epic for PostRetro. Your output is a plan folder in `context/plans/drafts/` that an agent can pick up and execute from `context/plans/ready/`.

## Current project state

!`ls context/plans/drafts/ context/plans/ready/ context/plans/in-progress/ 2>/dev/null`

## Process

### 1. Understand the goal

Read the user's description of what they want to build. If the scope is unclear, ask focused questions — but don't over-interrogate. Understand:
- What outcome matters
- What constraints apply
- What subsystems are involved

### 2. Research

Before writing the spec, research what you need to know. Load relevant context library files:

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

This ensures agents working the plan have full context in the codebase — they shouldn't need to derive architectural knowledge from the plan alone.

### 5. Commit

Stage and commit the plan folder and any context file updates together. The plan is now in `drafts/` for review.

### 6. Done

Tell the user:
- What you planned and why
- How many tasks, what the sequencing looks like
- What context files were updated
- The plan is in `drafts/` — review it, and `git mv` to `ready/` when satisfied

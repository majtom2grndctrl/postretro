---
name: orchestrate
description: >
  Orchestrates execution of a plan by spawning agents to work on tasks according
  to the plan's sequencing instructions. Reads a plan from context/plans/ready/,
  moves it to in-progress, and coordinates task execution across phases.
  Use when a reviewed plan is ready for implementation.
disable-model-invocation: true
argument-hint: "[plan-name]"
---

# Orchestrate

You are the orchestrator for a plan in `context/plans/ready/`. Your job is to coordinate — not produce. Read the plan, dispatch agents for each task, and track progress.

## Available plans

!`ls context/plans/ready/ 2>/dev/null || echo "(none)"`

## Process

### 1. Load the plan

Read these context library files first:
- `context/lib/index.md` — agent router, architectural principles
- `context/lib/development_guide.md` — conventions, constraints, coding standards
- `context/lib/testing_guide.md` — what to test, test patterns

Then read `context/plans/ready/$ARGUMENTS/plan.md`. If the plan doesn't exist, list available plans and ask the user which one to run.

Understand:
- The shared context section (every agent needs this)
- Each task's description and acceptance criteria
- The sequencing: which phases, which tasks are concurrent vs sequential, and why

### 2. Move to in-progress

```bash
git mv context/plans/ready/<plan-name> context/plans/in-progress/<plan-name>
```

Commit this move so the plan's state is tracked.

### 3. Execute phases in order

For each phase in the sequencing section:

**Sequential tasks:** Run one agent at a time. Wait for completion before starting the next.

**Concurrent tasks:** Spawn all agents in the phase simultaneously using multiple Agent tool calls in a single message. Use `isolation: "worktree"` for concurrent agents to avoid file conflicts.

**For each agent, provide:**
1. The plan's **Shared Context** section
2. The agent's **specific task** — description, acceptance criteria
3. Instruction to read relevant `context/lib/` files for architectural guidance
4. Instruction to follow `context/lib/development_guide.md` conventions
5. Instruction to run `cargo check` and `cargo test` before considering the task complete

**Do NOT provide:**
- Other tasks' details (the agent doesn't need them)
- The full plan document (wastes context)
- Freedom to expand scope beyond acceptance criteria

### 4. Integrate results

After each phase:
- Review what agents produced
- Verify acceptance criteria are met
- If a task completed partially or blocked, surface to the user with context
- If using worktrees, merge completed work back to the main branch

Between phases, check that prerequisites for the next phase are satisfied.

### 5. Complete

When all phases are done:
- Run preflight checks: `cargo fmt --check && cargo clippy -- -D warnings && cargo test`
- Move the plan to done: `git mv context/plans/in-progress/<plan-name> context/plans/done/<plan-name>`
- Commit
- Report to the user: what was completed, any partial results, any follow-ups filed

### Error handling

- **Agent fails a task:** Surface the error and acceptance criteria to the user. Ask whether to retry, skip, or abort.
- **Merge conflict from concurrent agents:** Resolve if straightforward; escalate to user if the conflict involves architectural decisions.
- **Preflight fails:** Fix if the issue is mechanical (formatting, simple clippy lint). Escalate if the fix requires design decisions.

### Principles

- **You coordinate, you don't produce.** Every tool call spent building is context not spent orchestrating.
- **Guard context.** Don't load information you don't need. Each agent gets the minimum viable context for their task.
- **3 of 4 completing is enough.** Graceful degradation over perfection. Partial progress with clear status beats blocking on one stuck task.
- **Surface, don't guess.** When something unexpected happens, tell the user. Don't make architectural decisions on their behalf.

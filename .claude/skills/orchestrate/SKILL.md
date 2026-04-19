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

Orchestrate a plan from `context/plans/ready/`. Coordinate — don't produce. Dispatch agents, track progress.

## Available plans

!`ls context/plans/ready/ 2>/dev/null || echo "(none)"`

## Process

### 1. Load the plan

Read these context library files first:
- `context/lib/index.md` — agent router, architectural principles
- `context/lib/development_guide.md` — conventions, constraints, coding standards
- `context/lib/testing_guide.md` — what to test, test patterns

Then read `context/plans/ready/$ARGUMENTS/index.md`. If missing, list available plans and ask which to run.

Understand:
- Shared context section (every agent needs this)
- Each task's description and acceptance criteria
- Sequencing: phases, concurrency, and dependencies

### 2. Move to in-progress

```bash
git mv context/plans/ready/<plan-name> context/plans/in-progress/<plan-name>
```

Commit the move.

### 3. Execute phases in order

For each phase in the sequencing section:

**Sequential:** One Opus agent at a time. Wait for completion before starting the next.

**Concurrent:** Spawn all phase Opus agents simultaneously via multiple Agent tool calls in one message. Use `isolation: "worktree"` to avoid file conflicts.

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
- Run a `/review-panel` on code edited in this session
- Report review panel findings to user to discuss which feedback to act on

### 6. Landing the plane

When the user says "land the plane":
- Move the plan to done: `git mv context/plans/in-progress/<plan-name> context/plans/done/<plan-name>`
- Clean up worktrees from the session
- Commit & push

### Error handling

- **Agent fails a task:** Surface the error and acceptance criteria to the user. Ask whether to retry, skip, or abort.
- **Merge conflict from concurrent agents:** Resolve if straightforward; escalate to user if the conflict involves architectural decisions.
- **Preflight fails:** Fix if the issue is mechanical (formatting, simple clippy lint). Escalate if the fix requires design decisions.

### Principles

- **You coordinate, you don't produce.** Every tool call spent building is context not spent orchestrating.
- **Guard context.** Each agent gets minimum viable context for their task.
- **3 of 4 completing is enough.** Partial progress with clear status beats blocking on one stuck task.
- **Surface, don't guess.** Tell the user when something unexpected happens. Don't make architectural decisions on their behalf.

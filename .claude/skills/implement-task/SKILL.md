---
name: implement-task
description: >
  Implements a single task from a plan or ad-hoc description. Reads the task,
  loads relevant context via the agent router, examines dependent code, and
  builds the feature. Use when working a specific task from a plan, or when
  given a focused implementation request.
context: fork
allowed-tools: Read, Glob, Grep, Bash, Edit, Write
argument-hint: "[plan-name/task-name | task description]"
---

# Implement Task

Implement a single task. Read the spec, load context, understand dependencies, build the feature. Do not commit — the caller handles integration.

## Process

### 1. Load the task

**If given a plan/task path:**
Read the plan from `context/plans/in-progress/<plan-name>/plan.md`. Extract:
- Shared Context section (constraints that apply across all tasks)
- Your specific task — description, acceptance criteria, dependencies

**If given an ad-hoc description:**
Use the description as the task spec. Ask clarifying questions only if acceptance criteria are ambiguous.

!`ls context/plans/in-progress/ 2>/dev/null`

### 2. Load context

Read the development guide carefully for engineering guidelines:
- `context/lib/development_guide.md`

Read the testing guide carefully for guidance on how to write tests:
- `context/lib/testing_guide.md`

Read `context/lib/index.md` — use agent router to identify which context files are relevant to this task. Load only what you need.

### 3. Examine dependencies

Before writing code, understand the systems your task touches:
- Read existing code in modules you'll modify or depend on
- Trace data contracts at subsystem boundaries — what do consumers expect?
- Check for related tests that document existing behavior
- If your task depends on another task's output, verify that output exists

### 4. Implement

Build the feature according to the task spec and context file conventions.

- Deliver the acceptance criteria. No more, no less.
- Follow development guide conventions (§1–§5).
- Write tests for cross-subsystem interactions, boundary parsing, and domain logic per testing guide.
- Handle error states and degradation paths within scope.
- Do not add scope the task didn't ask for.

### 5. Verify

Before reporting done:
- All acceptance criteria met
- `cargo check` passes
- New code follows conventions from the development guide
- Tests exist where the testing guide says they should

### 6. Report

Report what was built, which files were changed, and whether all acceptance criteria are met. If any criteria are partially met or blocked, explain why.

Do not commit, push, or run preflight. The caller handles integration.

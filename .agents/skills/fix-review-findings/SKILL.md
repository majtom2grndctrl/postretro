---
name: fix-review-findings
description: >
  Acts on review panel findings by dispatching concurrent GPT 5.5 workers with
  medium reasoning effort for small-blast-radius items (one per file), then a
  GPT 5.5 worker with high reasoning effort for remaining issues with knock-on
  effects. All agents read relevant context files.
  Use after /review-panel produces findings.
allowed-tools: Read, Glob, Grep, Bash, Agent
argument-hint: ""
---

# Fix Findings

Triage review panel findings and dispatch agents to fix them. Coordinate — don't produce.

## Agent brief (provide to every agent)

- The specific findings to address (`file:line`, problem, fix)
- Read `context/lib/index.md` and any files the router points to for the relevant area
- Read `context/lib/context_style_guide.md` before updating any comments or docs
- Read `context/lib/development_guide.md` before writing code
- **Edit only — do not run `cargo check`, `cargo test`, or any cargo command.** Make the change and report what you touched. These workers run concurrently against one shared `target/`; cargo takes an exclusive build lock, so parallel invocations serialize and churn each other's incremental cache — running cargo here would forfeit the concurrency this step exists for. A single test-runner step compiles and tests once, on a warm cache, after all edits land (step 5).

## Process

### 1. Triage

Classify each finding from the review panel output:

**Small blast radius** — GPT 5.5 medium reasoning effort, concurrent:
- Confined to a single file
- No interface or contract changes
- No knock-on effects in other packages
- Examples: missing error handling, nit, stale comment, dead code

**Everything else** — GPT 5.5 high reasoning effort, sequential:
- Crosses file or package boundaries
- Interface, contract, or exported type changes
- Knock-on effects likely
- Requires architectural judgment

Group small findings by file. Each file gets one agent.

### 2. Medium-reasoning workers (parallel)

Spawn one `worker` agent per file in a single message. Use `model: "gpt-5.5"` with `reasoning_effort: "medium"`. Provide the agent brief above.

### 3. Wait and assess

Review outputs. Note unresolved findings.

### 4. High-reasoning workers (sequential)

Spawn 1–2 `worker` agents, one at a time. Use `model: "gpt-5.5"` with `reasoning_effort: "high"`. Provide the agent brief, plus an enumeration of likely knock-on targets. These workers are also edit-only.

### 5. Compile-and-test gate

After all edits have landed, spawn **one** test-runner `worker` (`model: "gpt-5.5"`, `reasoning_effort: "low"`) to compile and test on the now-warm shared cache. It runs:
- `cargo check` for the touched crates
- focused tests for the touched crate/module/behavior — `cargo test -p <crate> <name_filter>` (`--lib` to skip integration tests). WARN: never run a bare `cargo test -p postretro-level-compiler` — its `tests/` integration suite triggers cold `prl-build` SH/lightmap bakes (~1h).

It returns a structured summary: which crates fail to compile and which tests fail, mapped to the file/finding responsible where possible. A dedicated runner keeps the coordinator's context clean of verbose build/test output. For one or two trivial findings, the coordinator may run this gate inline instead of spawning a runner.

### 6. Fix failures

For each compile error or test failure, dispatch a fix `worker` — one per file for independent failures (`reasoning_effort: "medium"`, concurrent, edit-only), sequential `reasoning_effort: "high"` for anything cross-cutting. Re-run step 5 until the gate is clean or the remaining failures need a user decision. Don't have fix workers run cargo themselves — the same shared-lock contention applies.

### 7. Report

What was fixed, what was skipped and why, and the gate result (which checks/tests passed).

After the gate is clean, the coordinator runs the full preflight once before commit or push: `cargo fmt --check`, `cargo clippy -- -D warnings`, and `cargo test`. This full gate is authoritative; the focused gate in step 5 is for fast feedback and lower cache churn.

---
name: fix-review-findings
description: >
  Acts on review panel findings by dispatching concurrent Sonnet agents for
  small-blast-radius items (one per file), then an Opus agent for remaining
  issues with knock-on effects. All agents read relevant context files.
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
- **Edit only — no `cargo check`, `cargo test`, or any cargo command.** Make the change, report what you touched. Concurrent agents share one `target/`; cargo's exclusive build lock serializes them and churns the incremental cache. Step 5 compiles and tests once, on a warm cache, after all edits land.

## Process

### 1. Triage

Classify each finding from the review panel output:

**Small blast radius** — Sonnet, concurrent:
- Confined to a single file
- No interface or contract changes
- No knock-on effects in other packages
- Examples: missing error handling, nit, stale comment, dead code

**Everything else** — Opus, sequential:
- Crosses file or package boundaries
- Interface, contract, or exported type changes
- Knock-on effects likely
- Requires architectural judgment

Group small findings by file. Each file gets one agent.

### 2. Sonnet agents (parallel)

Spawn one agent per file in a single message. Provide the agent brief above.

### 3. Wait and assess

Review outputs. Note unresolved findings.

### 4. Opus agents (sequential)

Spawn 1–2 agents, one at a time. Provide the agent brief, plus an enumeration of likely knock-on targets. Edit-only, same as above.

### 5. Compile-and-test gate

Once all edits land, spawn **one** test-runner agent. On the warm cache it runs:
- `cargo check` for touched crates
- **focused** tests for the touched crate/module — `cargo test -p <crate> <name_filter>` (`--lib` skips integration tests). WARN: never run a bare `cargo test -p postretro-level-compiler` — its integration suite triggers cold `prl-build` bakes (~1h).

It reports which crates fail to compile and which tests fail, mapped to the responsible file. A dedicated runner keeps the coordinator's context clean of build output. For one or two trivial findings, run the gate inline instead.

### 6. Fix failures

Dispatch a fix agent per failure — one per file for independent failures (concurrent, edit-only), sequential for cross-cutting ones. Re-run step 5 until clean, or until a failure needs a user decision. Fix agents don't run cargo either — same lock contention.

### 7. Report

What was fixed, what was skipped and why, and the gate result.

Once clean, the coordinator runs the full preflight before commit or push: `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`. This full gate is authoritative; step 5 is for fast feedback and lower cache churn.

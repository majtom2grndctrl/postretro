---
name: fix-review-findings
description: >
  Acts on review panel findings by dispatching concurrent GPT-5.6 Terra workers
  with medium reasoning effort for small-blast-radius items (one per file), then
  a GPT-5.6 Sol worker with high reasoning effort for remaining issues with
  knock-on effects. All agents read relevant context files.
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
- **Edit only — no `cargo check`, `cargo test`, or any cargo command.** Make the change, report what you touched. Concurrent workers share one `target/`; cargo's exclusive build lock serializes them and churns the incremental cache. Step 5 compiles and tests once, on a warm cache, after all edits land.

## Process

### 1. Triage

Classify each finding from the review panel output:

**Small blast radius** — GPT-5.6 Terra at medium reasoning effort, concurrent:
- Confined to a single file
- No interface or contract changes
- No knock-on effects in other packages
- Examples: missing error handling, nit, stale comment, dead code

**Everything else** — GPT-5.6 Sol at high reasoning effort, sequential:
- Crosses file or package boundaries
- Interface, contract, or exported type changes
- Knock-on effects likely
- Requires architectural judgment

Group small findings by file. Each file gets one agent.

### 2. Medium-reasoning workers (parallel)

Spawn one `worker` agent per file in a single message. Use `model: "gpt-5.6-terra"` with `reasoning_effort: "medium"`. Provide the agent brief above.

### 3. Wait and assess

Review outputs. Note unresolved findings.

### 4. High-reasoning workers (sequential)

Spawn 1–2 `worker` agents, one at a time. Use `model: "gpt-5.6-sol"` with `reasoning_effort: "high"`. Provide the agent brief, plus an enumeration of likely knock-on targets. Edit-only, same as above. If Sol is unavailable, use `gpt-5.6-terra` at high; reserve `max` for unusually consequential or subtle repairs.

### 5. Compile-and-test gate

Once all edits land, spawn **one** test-runner `worker` (`model: "gpt-5.6-terra"`, `reasoning_effort: "low"`). On the warm cache it runs:
- `cargo check` for touched crates
- focused tests for the touched crate/module — `cargo test -p <crate> <name_filter>` (`--lib` skips integration tests). WARN: never run a bare `cargo test -p postretro-level-compiler` — its integration suite triggers cold `prl-build` bakes (~1h).

It reports which crates fail to compile and which tests fail, mapped to the responsible file. A dedicated runner keeps the coordinator's context clean of build output. For one or two trivial findings, run the gate inline instead.

### 6. Fix failures

Dispatch a fix `worker` per failure — one per file for independent failures (`model: "gpt-5.6-terra"`, `reasoning_effort: "medium"`, concurrent, edit-only), sequential `model: "gpt-5.6-sol"` with `reasoning_effort: "high"` for cross-cutting ones. Re-run step 5 until clean, or until a failure needs a user decision. Fix workers don't run cargo either — same lock contention.

### 7. Report

What was fixed, what was skipped and why, and the gate result.

Once clean, the coordinator runs the full preflight before commit or push: `cargo fmt --check`, `cargo clippy --target-dir target/preflight-clippy -- -D warnings`, `cargo test`. This full gate is authoritative; step 5 is for fast feedback and lower cache churn.

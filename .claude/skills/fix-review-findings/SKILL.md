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
- Run `cargo build` and `cargo test` before considering the task done

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

Spawn 1–2 agents, one at a time. Provide the agent brief, plus an enumeration of likely knock-on targets.

### 5. Report

What was fixed, what was skipped and why, and whether `cargo build` and `cargo test` pass.

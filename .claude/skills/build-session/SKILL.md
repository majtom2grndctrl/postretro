---
name: build-session
description: >
  Runs a feature conversation, then orchestrates agents to build it in the
  same session. Surfaces the owner's blind spots on details that change the
  details they care about, writes a design contract every agent reads before
  dispatch, runs implementation waves, and folds durable decisions into
  context/lib as each wave lands. Use when the user wants to talk a feature
  through and have it built now, without a spec.
disable-model-invocation: true
argument-hint: "[feature]"
---

# Build Session

Two moves: settle the design with the owner, then orchestrate the build.

Process is yours to choose. Wave shape, agent count, slice boundaries, how much you verify between waves — judgment calls, and you have better information than this file does. The rules below are the ones that cost real work when broken.

This skill assumes current-generation agents. Most of what it forbids is orchestration written for weaker ones: step lists, starved context, work sliced small enough to be checked. `/orchestrate` still carries those habits — it was written for an older roster. Don't read it into this session.

**You coordinate. Agents produce.** Every tool call spent building is context not spent holding the whole file map.

## 1. Conversation

The owner cares about a few details. Not all of them. Your job is to get up to speed from the repo, not from their turns.

- Route through `context/lib/index.md`, read the two or three docs that matter, then read source. Grounding is yours.
- **Find the blind spots.** Name the unraised details that change the details they *did* raise. Carry each one as a recommendation with its consequence — "X means Y; I'd do Z" — not as an open question. Two or three, not a questionnaire.
- Ask only what the repo cannot answer: product behavior, policy, a modder-facing surface, a one-way door. Recommend a direction with every question.
- Everything else: decide from project context, state the call in one line, keep moving.

A detail the owner never asked about and that touches nothing they asked about is yours to settle silently.

## 2. Design contract

Before any dispatch, write `context/plans/in-progress/<slug>-contract.md`. Every agent reads this file first, before its own brief.

This is the highest-leverage thing in the skill. Agents that share a written contract build compatible pieces with zero communication; agents inferring the design from their own prompt do not.

Contents:

- Goal — one orienting paragraph.
- Decisions from the conversation, each with the consequence that makes it load-bearing.
- Invariants no agent may break: names, units, orderings, budgets, conventions.
- File ownership per slice.
- Acceptance per slice.
- Open questions, marked as open.

Pin every convention with two plausible readings, even the ones obvious to you: height vs. depth, 0- vs. 1-based, units, byte order, which side of a seam owns a clamp. Two agents guessing opposite ways is a silent integration bug.

Written once, before dispatch. Amended in the file when a wave changes a decision — and amended *before* the next dispatch, not after.

## 3. Waves

### Briefing

**Brief for intent, not procedure.** Give the whole slice up front: outcome, constraints that must hold, acceptance, and the contract. Then stop. Ordered step lists, prescribed search strategies, and told-you-how decomposition lower the quality of a current agent — the capability they were compensating for is no longer missing.

**Give the whole contract.** Withholding it to save context is a false economy at current window sizes, and an agent that infers a decision guesses differently than the one next to it. Other agents' briefs stay out — irrelevant, not expensive.

**Slice whole, not small.** One agent per coherent vertical slice with its own acceptance. Not one per file, not one per step. Handoff overhead between micro-tasks now costs more than the tasks do.

**Never ask an agent to double-check its work.** That buys over-verification, not coverage. Name the specific assertion that would catch the failure you actually fear — an offset assert, a rejected stale input — or ask for nothing.

### Sizing

Set `model:` on every Agent call. Omitting it inherits the session model: top tier spent on plumbing, or a contract slice handed to a scout.

Split on blast radius, never on size. Does the slice **establish** a contract that other code consumes — a format, a binding layout, a cache key, a cross-crate seam? Or does it **execute inside** one already settled?

| `model:` | Use for |
|---|---|
| `opus` | Establishing contracts, seams, layouts. Ambiguity resolved by reading code. |
| `sonnet` | Execution inside a settled contract, however large. Tests for specified behavior. |
| `haiku` | Read-only scouting, call-site sweeps, fact-finding. |
| `fable` | A reasoning slice that genuinely exceeds `opus`. Rare; costs accordingly. |

The aliases are durable; what backs them is not. When one stops resolving, fix this table — don't route around it.

### The rules that hold regardless

**Only you spawn agents that write files.** Workers may spawn read-only agents freely; a worker fanning out to find call sites is good. Workers never spawn writers. You cannot see your grandchildren — only an aggregate report from the parent. So you cannot partition file ownership among agents you did not create, and you cannot stop one misbehaving agent without killing its parent. Depth 2 for writes, unlimited for reads.

**Two scouts on load-bearing facts. You resolve conflicts from source.** Any fact the design turns on — a budget, a limit, a capability, an asserted invariant — gets two independent reads. When they disagree, open the file yourself. Do not trust the more recent report, or the more confident one. A stale comment sitting directly above the assert it describes reads exactly like the truth.

**Compile-forced spillover is allowed, and must be reported.** Adding an enum variant breaks exhaustive matches elsewhere. An agent makes the minimal change that keeps the workspace compiling, even outside its brief, and flags it in its report. A strict lane rule that leaves the tree uncompilable is worse. Use those reports to narrow the next wave's briefs.

**Every agent reads `context/lib/context_style_guide.md`.** It governs code comments and any prose the agent writes.

### Repo physics

Not model-generational — these are the machine's limits.

- Concurrent agents in isolated worktrees cap at **3** (`development_guide.md`, "Concurrent agents in isolated worktrees"). Beyond that, parallel engine builds exhaust CPU and disk, and correct work fails on linker errors. Need more width? Batch the next group after the first merges.
- A fresh worktree's first build is the engine's most expensive compile. Concurrent agents skip `cargo check` and tests entirely; verification lands once, after merge, against the integration branch's warm target. Sequential agents work on the branch directly and test as they go.
- Focused tests only: `cargo test -p <crate> <filter>`, narrowed to one target (`--lib`, or `--bin prl-build` for `postretro-level-compiler`). Require the test count in the report — a filter matching nothing prints `0 passed` and exits `ok`. Never `-- --ignored` in a routine pass; those are the ~5–7 min cold bakes.

## 4. Context maintenance, per wave

When a wave lands: merge, verify, then fold what it made durable into `context/lib/` — before the next dispatch.

Deferring every context edit to the end has two costs. The next wave reads a stale library. And by the end you are writing from memory instead of from the code.

Record only what survives refactoring. A sentence that breaks when a file is renamed belongs in a code comment, not in `context/`. Update the `index.md` router when a wave adds a concept someone would search for. Amend the contract with anything the wave changed.

## 5. Landing

`/review-panel` → `/fix-review-findings` → `/preflight` once, as the single full-suite gate. Report findings to the owner before acting on the ambiguous ones.

Record each acceptance row's result and any outstanding manual proof in the PR body. Then move the contract to `context/plans/done/`, or delete it once `context/lib/` fully absorbs it.

## Never

- Never write a step-by-step procedure into an agent brief.
- Never withhold the contract to save an agent's context.
- Never split a coherent slice to make the pieces smaller.
- Never ask an agent to double-check its work.
- Never omit `model:` on an Agent call.
- Never let a worker spawn a writing agent.
- Never design against a single scout's read of a load-bearing fact.
- Never resolve a scout conflict by picking a report. Open the source.
- Never dispatch before the contract file exists.
- Never let an agent infer a decision that belongs in the contract.
- Never dispatch a wave against a contract an earlier wave invalidated.
- Never interrogate the owner on details the repo already settles.
- Never present a blind spot as an open question when you have a recommendation.
- Never make a product or architectural decision on the owner's behalf. Surface it.
- Never delegate the integration — contracts, merges, and commits stay yours.
- Never batch all context updates to the end of the session.

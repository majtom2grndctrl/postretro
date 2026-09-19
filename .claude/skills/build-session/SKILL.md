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

**You coordinate. Agents produce.** Every tool call spent building is context not spent holding the whole file map.

## 1. Conversation

The owner cares about a few details. Not all of them. Your job is to get up to speed from the repo, not from their turns.

- Route through `context/lib/index.md`, read the two or three docs that matter, then read source. Grounding is yours.
- **Find the blind spots.** Name the unraised details that change the details they *did* raise. Carry each one as a recommendation with its consequence — "X means Y; I'd do Z" — not as an open question. Two or three, not a questionnaire.
- Ask only what the repo cannot answer: product behavior, policy, a modder-facing surface, a one-way door. Recommend a direction with every question.
- Everything else: decide from project context, state the call in one line, keep moving.

A detail the owner never raised, touching nothing they did raise, is yours to settle silently.

## 2. Design contract

Before any dispatch, write `context/plans/in-progress/<slug>-contract.md`. Every agent reads this file first, before its own brief.

Agents that share a written contract build compatible pieces with zero communication; agents inferring the design from their own prompt do not.

Contents:

- Goal — one orienting paragraph.
- Decisions from the conversation, each with the consequence that makes it load-bearing.
- Invariants no agent may break: names, units, orderings, budgets, conventions.
- File ownership per slice.
- Acceptance per slice.
- Open questions, marked as open.

Pin every convention with two plausible readings, even the ones obvious to you: height vs. depth, 0- vs. 1-based, units, byte order, which side of a seam owns a clamp. Two agents guessing opposite ways is a silent integration bug.

A prompt drifts as you retype it across waves. A file does not.

Written once, before dispatch. Amended in the file when a wave changes a decision — and amended *before* the next dispatch, not after.

## 3. Waves

Verification is the expensive part. An agent iterating until a named set of checks passes can run several hundred thousand tokens. That price is worth paying for a wire format, a GPU path, or a cache key. For lower-stakes work shorten the check list — never swap it for "make sure it's right," which costs nearly as much and proves nothing.

### Briefing

**Brief for intent, not procedure.** Give the whole slice up front: outcome, constraints that must hold, acceptance, and the contract. Then stop. Ordered step lists, prescribed search strategies, and told-you-how decomposition lower output quality. They compensate for a weakness the agent does not have.

**State constraints alongside instructions, even when the instruction already satisfies them.** This redundancy is what lets an agent catch you. "Put byte accounting in `render-cpu`" paired with "no upward crate edges, `layering_invariants_hold` enforces it" gets refused and corrected. The same instruction alone gets followed, and the breakage surfaces later as a mysterious regression. An unstated constraint turns the agent into an amplifier of your errors instead of a check on them.

**Describe a defect by its symptom and how you found it** — never by file and line. A location inherited from another report is a hypothesis you are laundering into a fact. An agent told "the defect is at `smoke.rs:263`" looks there. An agent told "a surface-map `.prm` renders as a placeholder, and here is why I believe that" reproduces first, and finds the real fault next door.

**Mark hard constraints apart from preferences.** A hard constraint breaks the build or the design when violated. Name it as one, justify it, and expect the agent to turn it into a test assertion that binds every future change. A preference stated in the same register gets enforced just as hard, and costs the flexibility you wanted.

**Give the whole contract.** Withholding it to save context is a false economy, and an agent that infers a decision guesses differently than the one next to it. Other agents' briefs stay out — irrelevant, not expensive.

**Slice whole, not small.** One agent per coherent vertical slice with its own acceptance. Not one per file, not one per step. Handoff overhead between micro-tasks costs more than the tasks do.

**Do not ask for verification — `opus` already does it.** It verifies its own work unprompted. Telling it to verify, re-check, or confirm buys over-verification with no gain in coverage, so delete that scaffolding rather than rewording it. This inverts the usual self-check advice and is a per-model carve-out: `fable` is the opposite and wants an explicit checking harness run on a cadence.

What still belongs in a brief is the **gate**, not the instruction to be careful: the named checks the work must pass, as commands with expected output. "Iterate until these greps return empty" is a task. "Double-check your work" is a tax.

### Reports

What you require back is as load-bearing as what you dispatch.

- **What the agent could not verify, and where it would look first.** Require this section. A wave's most valuable output is often the edge it could not reach — a GPU-only behavior, a timing window, a path with no fixture. That list is your first stop when the thing finally runs.
- **Environment findings, forwarded.** A lint that already fails on unmodified HEAD, a missing system package, a target dir that fills the disk. Carry each into the next wave's brief. Otherwise every agent rediscovers the same pothole at full price.
- **Artifacts, not only tests.** Tests prove the code does what it says; an artifact proves the thing works. Ask for both, separately. When the artifact is an image, require the agent to look at it — a generated depth map that merely traces albedo passes every distribution check.

### Sizing

Set `model:` on every Agent call. Omitting it inherits the session model: top tier spent on plumbing, or a contract slice handed to a scout.

Split on blast radius, never on size. Does the slice **establish** a contract that other code consumes — a format, a binding layout, a cache key, a cross-crate seam? Or does it **execute inside** one already settled?

| `model:` | Use for |
|---|---|
| `opus` | Establishing contracts, seams, layouts. Ambiguity resolved by reading code. |
| `sonnet` | Execution inside a settled contract, however large. Tests for specified behavior. |
| `haiku` | Read-only scouting, call-site sweeps, fact-finding. |
| `fable` | A reasoning slice that genuinely exceeds `opus`. Rare; costs accordingly. |

The aliases are durable; what backs them is not. When one stops resolving, fix this table — don't route around it. Verification posture rides on this choice: see **Briefing** for what each tier wants.

### Standing rules

**Only you spawn agents that write files.** Workers may spawn read-only agents freely; a worker fanning out to find call sites is good. Workers never spawn writers. You cannot see your grandchildren — only an aggregate report from the parent. So you cannot partition file ownership among agents you did not create, and you cannot stop one misbehaving agent without killing its parent. Depth 2 for writes, unlimited for reads.

**In-flight verification stays in your loop.** Never spawn an agent to check a wave's work while the wave is running. A verifier subagent re-establishes context from nothing, re-explores, and returns a judgment formed without your history — which is how a reviewer produces confident findings on a file it never opened. The landing review in §5 is the one sanctioned exception, and it carries that cost; nothing mid-wave does. Keep spawn counts low throughout — `opus` over-delegates left to itself.

**Commit to the delegation.** Once a worker reports, do not redo its work or re-derive its findings. Reading source to settle a load-bearing fact is not redoing the work — that is a two-line check, not a re-implementation.

**Two scouts on load-bearing facts. You resolve conflicts from source.** Any fact the design turns on — a budget, a limit, a capability, an asserted invariant — gets two independent reads. When they disagree, open the file yourself. Do not trust the more recent report, or the more confident one. A stale comment sitting directly above the assert it describes reads exactly like the truth.

**Compile-forced spillover is allowed, and must be reported.** Adding an enum variant breaks exhaustive matches elsewhere. An agent makes the minimal change that keeps the workspace compiling, even outside its brief, and flags it in its report. A strict lane rule that leaves the tree uncompilable is worse. Use those reports to narrow the next wave's briefs.

**Every agent reads `context/lib/context_style_guide.md` and `development_guide.md` §2.** Style governs code comments and any prose the agent writes; §2 governs file size and splitting. Slicing whole makes god files the likely failure — a worker authoring new modules never edits an already-large file, so nothing trips the usual threshold.

### Repo physics

Not model-generational — these are the machine's limits.

- Parallelism is bounded by the build graph, not the file list. Two agents editing disjoint files in one workspace still share a target dir: they contend on its lock and can compile against each other's half-written edits. Isolated worktrees with separate target dirs are the fix; a separate workspace (`tools/`) or a directory with no build at all (`content/`) parallelizes freely.
- Concurrent agents in isolated worktrees cap at **3** (`development_guide.md`, "Concurrent agents in isolated worktrees"). Beyond that, parallel engine builds exhaust CPU and disk, and correct work fails on linker errors. Need more width? Batch the next group after the first merges.
- A fresh worktree's first build is the engine's most expensive compile. Concurrent agents skip `cargo check` and tests entirely; verification lands once, after merge, against the integration branch's warm target. Sequential agents work on the branch directly and test as they go.
- Focused tests only: `cargo test -p <crate> <filter>`, narrowed to one target (`--lib`, or `--bin prl-build` for `postretro-level-compiler`). Require the test count in the report — a filter matching nothing prints `0 passed` and exits `ok`. Never `-- --ignored` in a routine pass; those are the ~5–7 min cold bakes.

## 4. Context maintenance, per wave

When a wave lands: merge, verify, then fold what it made durable into `context/lib/` — before the next dispatch.

Deferring every context edit to the end has two costs: the next wave reads a stale library, and you finish writing from memory instead of from code.

Record only what survives refactoring. A sentence that breaks when a file is renamed belongs in a code comment, not in `context/`. Update the `index.md` router when a wave adds a concept someone would search for. Amend the contract with anything the wave changed.

## 5. Landing

`/review-panel` → `/fix-review-findings` → `/preflight` once, as the single full-suite gate. Report findings to the owner before acting on the ambiguous ones.

A review panel is subagents judging code they did not write, so it fails in a known way: a confident finding about a file the reviewer never opened. Require every finding to quote the line it concerns and name the file and symbol. Drop any finding whose quote you cannot locate in the tree — that is verifying a citation, not re-deriving the work. Fix what survives; do not re-argue it.

Record each acceptance row's result and any outstanding manual proof in the PR body. Then move the contract to `context/plans/done/`, or delete it once `context/lib/` fully absorbs it.

## Never

- Never write a step-by-step procedure into an agent brief.
- Never give an instruction without the constraint that would catch it if it is wrong.
- Never withhold the contract to save an agent's context.
- Never split a coherent slice to make the pieces smaller.
- Never ask an agent to verify, re-check, or confirm its own work.
- Never spawn an agent to review another agent's output.
- Never redo a worker's work after it reports.
- Never hand an agent a file and line for a defect you have not reproduced.
- Never state a preference in the register of a hard constraint.
- Never accept a report with no "could not verify" section.
- Never let a wave's environment findings die in its report.
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

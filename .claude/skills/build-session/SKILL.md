---
name: build-session
description: >
  Runs a feature conversation, then builds it in the same session — working
  directly, and dispatching parallel agents for tracks that are genuinely
  independent. Surfaces the owner's blind spots on details that change the
  details they care about, writes a design contract every agent reads before
  dispatch, and folds durable decisions into context/lib as each track lands.
  Use when the user wants to talk a feature through and have it built now,
  without a spec.
disable-model-invocation: true
argument-hint: "[feature]"
---

# Build Session

Two moves: settle the design with the owner, then build it.

Process is yours to choose. Track boundaries, how wide to go, when to stop — judgment calls, and you have better information than this file does. The rules below are the ones that cost real work when broken.

## 1. Conversation

The owner cares about a few details. Not all of them. Get up to speed from the repo, not from their turns.

- Route through `context/lib/index.md`, read the two or three docs that matter, then read source. Grounding is yours.
- **Find the blind spots.** Name the unraised details that change the details they *did* raise. Carry each as a recommendation with its consequence — "X means Y; I'd do Z" — not as an open question. Two or three, not a questionnaire.
- Ask only what the repo cannot answer: product behavior, policy, a modder-facing surface, a one-way door. Recommend a direction with every question.
- Everything else: decide from project context, state the call in one line, keep moving.

A detail the owner never raised, touching nothing they did raise, is yours to settle silently.

End the conversation with the whole specification settled. Long-horizon work goes best when the complete task spec exists in one place before execution starts, rather than accumulating across interactive turns.

## 2. Design contract

Before any dispatch, write `context/plans/in-progress/<slug>-contract.md`. Every agent reads this file first, before its own brief.

Agents that share a written contract build compatible pieces with zero communication; agents inferring the design from their own prompt do not. A prompt also drifts as you retype it across tracks. A file does not.

Contents:

- Goal — one orienting paragraph.
- Decisions from the conversation, each with the consequence that makes it load-bearing.
- Invariants no agent may break: names, units, orderings, budgets, conventions.
- File ownership per track.
- Acceptance per track, as commands with expected output wherever it can be one.
- Open questions, marked as open.

Pin every convention with two plausible readings, even the ones obvious to you: height vs. depth, 0- vs. 1-based, units, byte order, which side of a seam owns a clamp. Two agents guessing opposite ways is a silent integration bug.

Match the contract's length to the decisions it carries. Padding it with restated background, redundant summaries, or boilerplate sections costs every agent that reads it.

Amend the file when a track changes a decision — before the next dispatch, not after.

## 3. Build

**Do the work yourself by default.** A subagent re-establishes context from nothing, re-explores, reports back, and then you read the report. Anything you could finish in a handful of tool calls is cheaper and more reliable done directly — a few file reads, a handful of edits, a focused search, a check.

**Dispatch along the workspace's seams.** This engine is 23 crates with an enforced layering direction, so one feature routinely spans several of them — a bake stage in `level-compiler`, a section in `level-format`, its `level-loader` read side, a CPU reference in `render-cpu`, the shader in `renderer`. Those are real tracks: different crates, different expertise, a pinned contract between them, and the compiler enforcing the boundary. That is what makes the parallelism real and the overhead repaid.

Size alone is not the signal. A large change living inside one crate is one track, however many lines it runs to. Two crates on opposite sides of a contract you have already pinned are two tracks, even when each is small. Give every track a whole vertical slice with its own acceptance — never one agent per file or per step.

**Keep the count low, and send them together.** One agent beats several on the same job. Launch independent tracks in a single message so they run concurrently. Never split one modest job across parallel agents.

**Brief precisely the first time.** Launching, waiting, and re-briefing costs a full context rebuild each round. Give the whole slice up front — outcome, constraints, acceptance, the contract — then let it run.

**Commit to the dispatch.** When a track reports, do not redo its work or re-derive its findings.

**Writes stay at depth 2.** A track may read as widely as it likes, including by fanning out read-only. It never spawns an agent that writes — you cannot partition file ownership among agents you did not create, and a child you cannot see outlives the parent that made it.

### What goes in a brief

**Intent, not procedure.** Ordered step lists, prescribed search strategies, and told-you-how decomposition lower output quality. They compensate for a weakness the agent does not have.

**Constraints alongside instructions, even when the instruction already satisfies them.** This redundancy is what lets an agent catch you. "Put byte accounting in `render-cpu`" paired with "no upward crate edges, `layering_invariants_hold` enforces it" gets refused and corrected. The same instruction alone gets followed, and the breakage surfaces later as a mysterious regression. An unstated constraint turns the agent into an amplifier of your errors instead of a check on them.

**A defect by its symptom and how you found it** — never by file and line. A location inherited from another report is a hypothesis you are laundering into a fact. An agent told "the defect is at `smoke.rs:263`" looks there. An agent told "a surface-map `.prm` renders as a placeholder, and here is why I believe that" reproduces first, and finds the real fault next door.

**Hard constraints marked apart from preferences.** A hard constraint breaks the build or the design when violated. Name it as one, justify it, and expect the agent to turn it into a test assertion that binds every future change. A preference stated in the same register gets enforced just as hard, and costs the flexibility you wanted.

**The acceptance gate as commands with expected output.** This is what makes a track self-correcting: given a gate it can run, an agent loops until the gate is green without being told to be careful. "Iterate until these greps return empty, and `git diff --stat context/lib/` stays empty" is a task with a built-in stopping condition. Name the check that would catch the failure you actually fear — an offset assert, a rejected stale input, a directory that must not change.

**No instruction to verify.** `opus` verifies its own work unprompted; telling it to verify, re-check, or confirm buys extra work and no coverage. Delete that scaffolding rather than rewording it. This inverts the usual self-check advice and rides on the tier — `fable` is the opposite and wants an explicit checking harness on a cadence.

**Scope discipline, when a track has room to wander.** Deliver what was asked at the scope intended; make routine judgment calls; say so in a sentence and keep going if the ask looks mistaken, rather than quietly narrowing or widening it; report completion only when it is actually done.

### What to require back

- **What the agent could not verify, and where it would look first.** Asking what it *couldn't* reach is not asking it to verify. A track's most valuable output is often the edge it could not test — a GPU-only behavior, a timing window, a path with no fixture. That list is your first stop when the thing runs.
- **A report you can read once.** Your context is spent reading reports, not writing briefs, and that is the budget that decides how long you can hold the whole picture. Ask for what changed, what the gate returned, what could not be verified, and what surprised them — not a replay of how the work went. Take it as given and move on; re-deriving a track's findings costs your context twice and buys nothing.
- **Environment findings, forwarded.** A lint that already fails on unmodified HEAD, a missing system package, a target dir that fills the disk. Carry each into the next brief. Otherwise every agent rediscovers the same pothole at full price.
- **Artifacts, not only tests.** Tests prove the code does what it says; an artifact proves the thing works. When the artifact is an image, require the agent to look at it — a generated depth map that merely traces albedo passes every distribution check.
- **Compile-forced spillover, reported.** Adding an enum variant breaks exhaustive matches elsewhere. The minimal change that keeps the workspace compiling is allowed outside a brief, and must be flagged. A strict lane that leaves the tree uncompilable is worse.

### Sizing

Set `model:` on every Agent call. Omitting it inherits the session model: top tier spent on plumbing, or a contract track handed to a scout.

Split on blast radius, never on size. Does the track **establish** a contract that other code consumes — a format, a binding layout, a cache key, a cross-crate seam? Or does it **execute inside** one already settled?

| `model:` | Use for |
|---|---|
| `opus` | Establishing contracts, seams, layouts. Ambiguity resolved by reading code. |
| `sonnet` | Execution inside a settled contract, however large. Tests for specified behavior. |
| `haiku` | Read-only sweeps too wide to run yourself. |
| `fable` | A reasoning track that genuinely exceeds `opus`. Rare; costs accordingly. |

The aliases are durable; what backs them is not. When one stops resolving, fix this table — don't route around it.

### Load-bearing facts

Read them from source yourself. A budget, a limit, an asserted invariant — anything the design turns on is two tool calls, and you are the one who has to hold it. Dispatch a scout only when *finding* the fact needs a wide sweep, and open the file yourself before designing against what it returns. A stale comment sitting directly above the assert it describes reads exactly like the truth.

### Repo physics

The machine's limits, not the roster's.

- Parallelism is bounded by the build graph, not the file list. Two agents editing disjoint files in one workspace still share a target dir: they contend on its lock and can compile against each other's half-written edits. Isolated worktrees with separate target dirs are the fix; a separate workspace (`tools/`) or a directory with no build (`content/`) parallelizes freely.
- Concurrent agents in isolated worktrees cap at **3** (`development_guide.md`, "Concurrent agents in isolated worktrees"). Beyond that, parallel engine builds exhaust CPU and disk, and correct work fails on linker errors. Batch the next group after the first merges.
- A fresh worktree's first build is the engine's most expensive compile, and a worktree track pays it before it can compile or test at all. Decide per track which side to pay. Give it a gate needing no build — greps, byte comparisons, file assertions — and verification lands once after merge, on the warm target. Let it pay the cold build and it catches its own compile and test failures instead of handing them back to you, which is the cheaper trade whenever the track is large or the seam is one you would rather not debug at integration time. Working on the branch directly avoids the question: you inherit the warm target and test as you go.
- Focused tests only: `cargo test -p <crate> <filter>`, narrowed to one target (`--lib`, or `--bin prl-build` for `postretro-level-compiler`). Require the test count — a filter matching nothing prints `0 passed` and exits `ok`. Never `-- --ignored` in a routine pass; those are the ~5–7 min cold bakes.

**Every agent reads `context/lib/context_style_guide.md` and `development_guide.md` §2.** Style governs code comments and any prose; §2 governs file size and splitting. Whole-slice tracks make god files the likely failure — an agent authoring new modules never edits an already-large file, so nothing trips the usual threshold.

## 4. Context maintenance

When a track lands: merge, then fold what it made durable into `context/lib/` before the next dispatch. Defer it all to the end and the next agent reads a stale library, while you write from memory instead of from code.

Record only what survives refactoring. A sentence that breaks when a file is renamed belongs in a code comment. Update the `index.md` router when the work adds a concept someone would search for. Amend the contract with anything that changed.

## 5. Landing

**One review track, not a panel.** `opus` finds real bugs at high recall in a single pass, so several reviewers on one diff is one modest job split across parallel agents. A panel also returns claims you then have to re-verify, which spends the context you spent the whole session protecting.

Dispatch one agent to review the diff and fix what it finds. Keep three stages distinct in its brief.

**Find.** Several lenses in one pass — correctness, the contract's invariants, layering and crate direction, test coverage, resource bounds, and prose against `context_style_guide.md`. Coverage is the job at this stage:

> Report every issue you find, including ones you are uncertain about or consider low-severity. Do not filter for importance here — a later stage does that. For each finding, give a confidence and a severity, and quote the line with its file and symbol.

**Filter.** Rank the findings and drop what does not hold: a quote that cannot be located in the tree, a finding the contract already answers, a preference dressed as a defect.

**Fix.** Apply what survives. Stop at anything that would change a decision in the contract — those come back to you, unfixed, with the reasoning.

Require back what it found, what it changed, and what it left for a decision. Read the first and last; take the middle as given and let `/preflight` judge it.

A track that lands mid-session can take the same brief on `sonnet` as a cheap early pass. Accuracy holds at lower cost, so a quick pass per track and a thorough one at landing is worth more than a single review at the end.

Then `/preflight` once, as the single full-suite gate.

Record each acceptance row's result and any outstanding manual proof in the PR body. Move the contract to `context/plans/done/`, or delete it once `context/lib/` absorbs it.

## Reporting to the owner

The owner reads your text between tool calls and sees neither your thinking nor the raw results. Lead with the outcome — what happened, what you found — then the detail. Say what you are about to do before a long dispatch, and speak up mid-track when you hit something load-bearing or change direction. Write in complete sentences, spelled out, without shorthand or labels they would have to cross-reference.

## Never

- Never dispatch an agent for work you could finish in a handful of tool calls.
- Never split one modest job across parallel agents.
- Never dispatch a track with no gate it can run.
- Never let a track spawn an agent that writes.
- Never launch, wait, then re-brief — the whole slice goes in the first brief.
- Never redo a track's work after it reports.
- Never ask an agent to verify, re-check, or confirm its own work.
- Never write a step-by-step procedure into a brief.
- Never give an instruction without the constraint that would catch it if it is wrong.
- Never hand an agent a file and line for a defect you have not reproduced.
- Never state a preference in the register of a hard constraint.
- Never tell a reviewer to report only what matters, or to skip the nits.
- Never design against a fact you have not read in source.
- Never dispatch before the contract file exists.
- Never let an agent infer a decision that belongs in the contract.
- Never dispatch against a contract an earlier track invalidated.
- Never interrogate the owner on details the repo already settles.
- Never present a blind spot as an open question when you have a recommendation.
- Never make a product or architectural decision on the owner's behalf. Surface it.
- Never delegate the integration — contracts, merges, and commits stay yours.
- Never batch all context updates to the end of the session.

---
name: review-panel
description: >
  Runs a multi-agent review panel of specialized review lenses, sized to the
  diff by a triage step. Aggregates findings with deduplication and severity
  merging. Runs in a forked context so the active agent's context window stays
  clean and reviewers have no bias from prior work. Use mid-session after
  implementing a feature, or before opening a pull request.
allowed-tools: Read, Glob, Grep, Bash, Agent
argument-hint: "[file-path | plan-name] [reviewers:N] [effort:high|medium]"
---

# Review Panel

Review panel coordinator, isolated from the implementing agent's context. Reviewers evaluate code on its own merits — no access to prior reasoning or conversation history.

Triage the diff, spawn the right review lenses, collect findings, present a unified review. Do not review code yourself.

## Panel model

The panel is **lens-based**, not subject-based. A lens is a way of reviewing, not a region of code. Seam bugs hide between files; only a reviewer committed to one stance finds them.

- **Correctness tracer** _(depth)_ — picks one data flow and mentally executes it end to end across files. Catches ordering bugs, lifecycle gaps, producer/consumer mismatches.
- **Contract verifier** _(depth)_ — takes one public surface (SDK types, descriptors, wire format, reaction set) and checks that type, validation, error message, doc, and runtime behavior all agree.
- **Adversarial tester** _(depth)_ — constructs edge cases that break invariants: double-fire, re-entrancy, zero/missing input, an event mid-transition.
- **Hygiene + drift** _(breadth)_ — scans the surface against a checklist: hot-path waste, dead code, clippy-level issues, naming, and comment drift.

**Grouping rule.** Depth lenses need sustained attention on one thing; an agent juggling two satisfices and does both shallowly. So: **one agent per depth lens; one agent for the whole breadth cluster.** Never pair a depth lens with anything else.

## Scope detection

Determine review target from first argument (same rules as `/code-review`):

- **Plan name:** all files touched by the plan's tasks
- **File path:** that file and closely related files
- **No argument:** uncommitted changes

!`git diff --stat HEAD 2>/dev/null`
!`git diff --stat --cached 2>/dev/null`
!`ls context/plans/in-progress/ context/plans/done/ 2>/dev/null`

## Process

### 1. Parse arguments

Extract from `$ARGUMENTS`:
- The review target (plan name, file path, or empty for uncommitted changes)
- `reviewers:N` — force exactly N depth agents, bypassing the dispatch table
- `effort:high|medium` — reasoning effort for the depth agents (default: high)

### 2. Triage the diff

Scan the diff (~30s) before spawning. Produce two lists — they are what you brief the depth agents with:

- **Flows worth tracing** — 1–3 data paths that cross files or carry ordering/lifecycle/state logic. Name each flow's entry point and the files it touches.
- **Contract surfaces touched** — any authored/public surface the diff changes (SDK types, descriptors, wire format, reaction set).

A tracer with no named flow reviews like a generic reviewer and misses the seams. If triage yields no flows and no surfaces, the diff is mechanical.

### 3. Dispatch the panel

Size the panel from triage:

| Diff shape | Panel |
|------------|-------|
| Mechanical / refactor / comment-only | 1 — hygiene+drift alone |
| Localized logic, no seams | 2 — one depth (tracer or verifier, whichever fits) + hygiene+drift |
| Crosses subsystem seams; ordering/lifecycle/state logic | + 1 correctness tracer per flow (cap 3) |
| Touches a contract surface | + 1 contract verifier |
| Subtle invariants / many edge conditions | + 1 adversarial tester |

**Floor.** Always run the hygiene+drift agent. For any diff carrying logic, run at least one depth agent alongside it — one agent reviewing everything loses the cross-check that makes this a panel. Typical panel is 2–3; rarely past 5, and only when the diff genuinely spans seams.

`reviewers:N` forces the depth-agent count to N and skips the table — triage still picks the lens mix (N=3 might be two tracers and one verifier). `effort:medium` runs depth agents at medium reasoning effort. The hygiene+drift agent always runs at medium reasoning effort, unaffected by overrides.

### 4. Spawn all agents in parallel

Launch every agent in a single message. No `isolation: "worktree"` needed — reviewers read code and report findings, they don't write files.

- **Each depth agent:** a `worker` agent with `model: "gpt-5.5"` and the specified `reasoning_effort` (default: high). Pass the shared preamble (below), then its lens prompt, then the specific flow or surface from triage. The lens governs — depth agents do not run the general code-review checklist; that is the breadth pass's job.
- **Hygiene + drift agent:** a `worker` agent with `model: "gpt-5.5"` and `reasoning_effort: "medium"`. Pass full content of `.Codex/skills/code-review/SKILL.md` (the breadth checklist), then the hygiene+drift prompt (below).

Every agent reports findings **bucketed by lens** so coverage per lens stays visible at aggregation.

### 5. Aggregate results

**Deduplicate.** If lenses flag the same issue (same file, same concern), keep the most specific description and note how many caught it. Agreement is strong signal.

**Merge severity.** On disagreement, take the higher severity. One 🔴 outweighs another's 🟡.

**Keep comment drift separate** — present it as its own section, don't fold it into code findings.

### 6. Present unified review

```
## Review Panel Summary

**Panel:** [lenses that ran and their effort, e.g. "2 correctness tracers + 1 contract verifier (high effort) + hygiene/drift (medium effort)"]
**Target:** [what was reviewed]
**Verdict:** approve / request changes / needs discussion

## Code Review Findings

### 🔴 Must fix
[Deduplicated findings. Tag each with its lens and reviewer agreement, e.g. "(tracer, 2x)"]

### 🟡 Should fix
[...]

### 🟢 Nits
[...]

## Comment Drift Findings

### 🔴 Stale or misleading comments
[Comments that would lead an agent astray]

### 🟡 Comments that need updating
[Comments weakened by the changes but not yet wrong]

### 🟢 Suggested improvements
[Opportunities to add context that would help future agents]

## What's done well
[Merged from all lenses — deduplicated]
```

Omit empty severity categories. If the panel unanimously approves with no findings, say so clearly.

---

## Lens prompts

Each depth agent gets the shared preamble below, then its lens block. The hygiene+drift agent skips the preamble — it runs the full `code-review` skill, then the hygiene+drift block. Pass blocks verbatim.

### Shared preamble (depth agents)

```
Before reviewing, read: context/lib/index.md (architecture, subsystem boundaries), context/lib/development_guide.md (conventions, constraints), context/lib/testing_guide.md (test expectations). If the target is a plan, read the plan too.

You run one lens, not a general review. Stay in it — the broad completeness/correctness checklist is another agent's pass; don't duplicate it. Report with the project severity legend: 🔴 must fix · 🟡 should fix · 🟢 nit.
```

### Correctness tracer

```
You are a **Correctness Tracer**. Review by execution, not by scanning.

Your brief names one data flow — an entry point and the files it crosses. Trace it end to end. Mentally execute it: at each step, what is the state, what does this step hand the next, does the consumer expect that shape and that order? Review code outside the flow only where it feeds or reads the flow.

Hunt for: ordering and lifecycle bugs (X runs before Y but depends on it), state-machine gaps (a transition nothing handles), producer/consumer mismatches (a caller passes what the callee never reads), and degradation that aborts where the contract says degrade.

A finding names the step, what you expected when executing it, and what the code does instead.
```

### Contract verifier

```
You are a **Contract Verifier**. Your brief names one public surface the diff changes — SDK types, descriptors, wire format, the reaction set, an authored field.

Check that every layer of that surface agrees: the generated or declared type, the runtime validation, the error message, the doc comment, and the actual runtime behavior. The contract is broken when any two disagree — the doc promises behavior the code doesn't ship, validation accepts what the runtime rejects, an error names the wrong field.

Where the surface is cross-runtime, verify both runtimes: the QuickJS and Luau twins must match — same validation, same degradation, neither aborting where the other degrades.

A finding names the two layers that disagree and quotes both.
```

### Adversarial tester

```
You are an **Adversarial Tester**. Try to break the change.

Construct edge cases the happy path ignores: double-fire, re-entrancy, zero/empty/missing input, an event mid-transition (a switch during a fade, a write during a rebuild), boundary values, out-of-order calls. For each, trace what the code actually does — panic, corrupt state, silently drop, or hold the invariant?

Tie every case to code; do not list hypotheticals you can't ground. A finding is a concrete input sequence, the invariant it violates, and where in the code it goes wrong.
```

### Hygiene + drift

```
You are the **Hygiene + Drift** reviewer — the breadth pass. Beyond the code-review checklist above (hot-path waste, dead code, clippy-level issues, naming), audit comment integrity. Comments are living documentation; a stale comment is worse than none — it sets agents up for failure.

Read first:
- context/lib/development_guide.md §5 (Code Comments)
- context/lib/context_style_guide.md (durable vs ephemeral content)

**Verify before you report.** For every drift finding, read the current on-disk text and decide: is it wrong *now*, or was it wrong on an earlier commit and already fixed? Report only what is wrong now. Flagging a suspicious phrase without reading the live line is this lens's defining failure — quote the current text in every finding.

Check, in changed and adjacent code:
- file headers pointing at the wrong context file
- comments describing behavior the code no longer implements
- non-obvious new code missing a "why"
- orphan TODOs, comments that merely restate code, spec pointers to nonexistent docs
- adjacent comments naming a contract this diff changed (importers/importees, moved subsystem boundaries, stale "consumed by Y" cross-refs)

Severity: 🔴 would mislead an agent now · 🟡 weakened but not yet wrong · 🟢 context worth adding. Each finding: file, quoted live text, what's wrong, the fix.
```

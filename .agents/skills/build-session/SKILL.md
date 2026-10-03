---
name: build-session
description: >
  Settles a feature design with the owner and builds it in the same session,
  directly or across independent agent tracks. Use when the user wants to
  discuss a feature and have it implemented now without a spec.
disable-model-invocation: true
argument-hint: "[feature]"
---

# Build Session

Settle the design with the owner, then build it. Use judgment for the process,
track boundaries, and stopping point. The rules below protect decisions and
integration work that are expensive to redo.

## Disk space

After each numbered workflow step and completed task, check free space on the
workspace filesystem. If less than 15 GB remains, clear Cargo incremental build
caches in the active `target/` and workflow-owned target directories. Recheck.
If still below 15 GB, clear Cargo's downloaded crate archive cache. Recheck
before continuing. Delete caches only; preserve target directories, checkouts,
worktrees, and unrelated files.

## 1. Conversation

Get grounded in the repository before relying on the conversation:

- Read `context/lib/index.md`, then the two or three routed docs that matter.
- Read relevant source and trace the actual contract at the boundaries.
- Surface two or three unraised details that change the details the owner did
  raise. Recommend a direction and explain its consequence; do not turn these
  into a questionnaire.
- Ask only about product behavior, policy, modder-facing surfaces, or
  irreversible choices the repository cannot settle. Include a recommendation
  with each question. Decide routine implementation details from project
  context and keep moving.

Finish the conversation with the whole feature specification settled before
implementation starts. The complete design should be readable in one place.

## 2. Design contract

Create a feature branch from `main` before writing feature files. Write
`context/plans/in-progress/<slug>-contract.md` and commit it before dispatching
any agents. Every track reads this committed contract before its brief.

Include only decisions that constrain compatible implementation:

- Goal and decisions from the conversation, with the consequence that makes
  each decision load-bearing.
- Invariants: names, units, ordering, budgets, formats, and conventions no
  track may reinterpret.
- File ownership and acceptance criteria per track. Use commands and expected
  output where practical.
- Open questions, explicitly marked open.

Pin conventions that admit plausible competing interpretations, including
units, index bases, byte order, and ownership of a boundary clamp. Keep the
contract proportional to the decisions it carries. Show its decisions and
invariants to the owner before the first dispatch. Amend it when a track
changes a decision, before dispatching dependent work.

## 3. Build

Do the work directly by default. A subagent must re-establish context, explore,
and report back; use one only when an independently reviewable slice repays
that cost. Split on blast radius and contracts, not line count. A large change
inside one crate is still one track. Two small changes on opposite sides of an
already-settled contract may be separate tracks.

Keep the number of agents low. Use sequential tracks when one result informs
the next, design is uncertain, or a shared warm target makes iteration cheaper.
Use concurrent agents only for independent tracks with a pinned contract;
launch them together and give each an isolated worktree. In this workspace,
cap concurrent isolated worktrees at three. Account for each cold build and
separate target directory when choosing concurrency.

### Match GPT models to the work

Set both `model` and `reasoning_effort` on every spawned agent. The coordinator
keeps ownership of design, integration, and commits.

| Work | Model and effort |
|---|---|
| Establishing an uncertain cross-module contract, layout, or seam | `gpt-6.1-sol`, high or xhigh; use `gpt-6-astra` at high or xhigh when the reasoning is unusually difficult or the lifecycle invariant is subtle |
| Executing a settled implementation track, focused tests, or a thorough landing review | `gpt-6.1-sol`, medium; raise effort only for real uncertainty or broad contracts |
| Prescribed mechanical edits or a read-only sweep that is too wide to do directly | `gpt-6-luna`, low or medium |

Use the smallest model and effort that can safely own the acceptance criteria.
Do not spend a top-tier model on specified plumbing, or split a modest task to
use agents. A read-only scout is warranted only when finding a fact needs a
wide sweep; read the source yourself before designing against the scout's
report.

### Brief each track

Give the full slice the first time. A brief contains intent and constraints,
not a prescribed search or implementation procedure. Include:

- Outcome, acceptance criteria, and ownership boundaries.
- Relevant context docs and the contract; each agent reads the relevant
  `context/lib/` docs, `context/lib/context_style_guide.md`, and
  `context/lib/development_guide.md` §2.
- Upstream contracts, downstream consumers, and hard constraints alongside
  instructions that might conflict with them.
- A gate that catches the failure feared. Use a command and expected result
  where practical. Concurrent worktree tracks skip Cargo checks and tests;
  verify them after integration on the warm target. Sequential tracks run
  `cargo check` and focused tests for touched behavior, confirming each filter
  matched tests. Do not use bare
  `cargo test -p postretro-level-compiler` (it triggers cold `prl-build` bakes).

Describe defects by observed symptom and reproduction, not an inherited file
and line guess. Keep hard constraints separate from preferences. Avoid
instructions to “verify” without naming the evidence that would catch a
specific failure.

For persistent or mirrored layouts, identify stable fields, offsets, bindings,
versions, and cache epochs; all consumers; required malformed or stale-input
cases; and whether optional data degrades or fails loading. Require assertions
where the layout is hand-mirrored.

Ask tracks to report concisely: changes made, acceptance results, what could
not be verified and where they would look first, environment failures, and
surprises. Require artifact inspection when an artifact proves behavior tests
cannot, such as looking at a generated image. Carry relevant environmental
findings into later briefs. Permit the minimum compile-forced spillover needed
to keep the workspace compiling, but require it to be reported.

### Load-bearing facts

Read budgets, limits, asserted invariants, and other facts the design turns on
from source yourself. Dispatch a scout only when finding a fact needs a wide
sweep, then open the source before designing against the report. Comments can
be stale even when they sit beside the assertion they describe.

## 4. Context maintenance

When a track lands, integrate it and fold durable knowledge into `context/lib/`
before dispatching the next dependent track. Record only facts that survive
refactoring. Put file-specific details in code comments. Update `index.md` when
the work adds a concept people will search for, and amend the contract when a
decision changes.

## 5. Landing

After integration, dispatch one landing reviewer. Use `gpt-6.1-sol` at medium
for a settled diff; use high or xhigh if review requires tracing an ambiguous
cross-module contract. Ask for three distinct stages:

1. **Find:** inspect correctness, contract invariants, crate layering,
   coverage, resource bounds, and prose against `context_style_guide.md`.
   Report every candidate, including uncertain or low-severity findings, with
   confidence, severity, symbol, and a located quote.
2. **Filter:** discard unlocatable claims, claims already answered by the
   contract, and preferences presented as defects.
3. **Fix:** apply findings that survive. Stop before a fix that changes a
   contract decision; return that decision to the owner.

Ask the reviewer to report findings, changes, and decisions left to the owner.
Treat its investigation as the review; do not repeat it yourself. A quick
medium-effort review may run after a track lands; do the thorough landing review
once after integration.

Run `/preflight` once as the final full-suite gate. Record each acceptance
result and outstanding manual proof in the PR body. Move the contract to
`context/plans/done/`, or remove it once `context/lib/` absorbs its durable
content.

## Reporting

Lead updates with what happened or what is about to happen. Explain a
load-bearing discovery or change in direction while work is ongoing. The final
report covers changes, gates and results, what could not be verified, and
surprises. Do not replay the entire process.

## Never

- Never dispatch an agent for work you can finish in a handful of tool calls.
- Never split one modest job across parallel agents.
- Never dispatch without a committed contract and a concrete acceptance gate.
- Never omit model or effort on an agent call.
- Never allow tracks to spawn writing agents; writes stay at depth two.
- Never re-brief after waiting when the full slice could have been sent once.
- Never redo a track's work or re-derive its findings after it reports.
- Never design against an unverified fact or a scout's uninspected source claim.
- Never turn preferences into hard constraints or make product decisions for
  the owner.
- Never put the whole implementation plan in an agent brief.
- Never let a track infer a decision that belongs in the contract.
- Never batch all durable context updates until the end.

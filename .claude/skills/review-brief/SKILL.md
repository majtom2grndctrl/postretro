---
name: review-brief
description: >
  Opt-in detail review of a problem brief in `context/plans/drafts/`. Three
  lenses — premise, rows, subtract — each confined to the sections it may
  change: reviewers may add Acceptance rows and research pin rows, and nothing
  else is edited; every finding on Decisions, Problem or Path goes to the owner.
  Never /review-draft-spec on a brief. Run after /validate-plan, before
  promotion, when the stakes warrant an independent read — a renderer state
  machine, a wire format, a cache key, a one-way door. Not a default step.
argument-hint: "[brief-name]"
---

# Review Brief

The detail review for the brief process. Same three concerns as `/review-draft-spec` — is the source claim true, is the ordering pinned, is the shape right — under one rule that skill does not have: **a reviewer's text lands only in Acceptance and `research.md`.** Decisions are owner-owned. A finding about a Decision is a question for the owner, never a clause applied to it.

## Premise

`/review-draft-spec` is built for a spec's anatomy: task paragraphs to fact-check, pin rows to assign to tasks, prose fixes landed as `FIND`/`REPLACE`. Run on a brief, its lenses still emit that shape, and applied it does one thing reliably: it appends reviewer prose to Decisions. Every clause a finding adds to a Decision is binding on the executor, so one round turns judgment the owner recorded into mechanism the reviewer prescribed — "must be witnessed", "covered by the reload family", a parenthetical answering a finding nobody else will ever read. The brief grows back toward a spec, one applied fix at a time.

The lenses are not the problem. Where their output is allowed to go is. This skill keeps the lenses and moves the boundary.

## What may change

| Section | Reviewer may | Applier may |
|---|---|---|
| Problem | Report a cause it cannot confirm | Nothing |
| Decisions | Report a false premise, a Path sentence wearing a Decision's clothes, a non-goal missing its warrant | Nothing |
| Acceptance | Propose a new row; propose a completion of an existing row | Add rows; apply a completion that does not change what the row measures |
| Path | Nothing — a stale Path claim is the executor's, reported in `plan.md` | Nothing |
| Open questions | Report an unmarked entry | Nothing |
| `research.md` | Propose pin-table rows | Add rows |

A "completion" finishes what a row already meant — names the edge it implied, adds the permit case beside its refuse case. Anything that changes the measurement is the owner's, surfaced with the proposed text.

## Process

### 1. Locate and read

Argument is a plan folder name; look in `drafts/` first, then `ready/` for an a-la-carte read on a brief already promoted. The line under the title must mark it as a brief; on a `/draft-plan` spec, stop and point at `/review-draft-spec`.

Read the brief and `research.md` yourself. Snapshot both to the scratch directory before any reviewer runs: briefs are amended into one commit, so the snapshot is the only per-round diff.

### 2. Dispatch three lenses

One message, three `Agent` calls, read-only, fresh context. Or one agent carrying all three in the order below — if so, the subtract lens answers **before** the agent opens any source file; once it has adjudicated identifiers, the shape question gets a closing paragraph instead of attention.

Every prompt carries: the confirmed absolute paths of the brief and `research.md`, the `context/lib/` docs the header names, the source files Path cites, `context/lib/context_style_guide.md` to read first, the table above, the finding contract, and where to write its findings file. Report only — no edits, no cargo commands.

**Premise lens** — Decisions only. For each bullet, find the claim about the code it rests on and open the symbol. Three verdicts: *holds*, *false*, *unverifiable*. A *false* premise is the highest-value finding this skill produces and it goes straight to the owner; the reviewer does not propose a replacement decision. A `### Scripting surface` block gets the same treatment: every name in the example either resolves against the SDK or is one the brief adds, and a TypeScript example names its Luau mirror or the Boundary inventory says why not. Nothing outside Decisions is fact-checked here — Path claims are the executor's.

**Rows lens** — Acceptance and the pin table. Two passes:

- *Ordering.* Attack the Decisions temporally: two events on one tick, B before A, a timer across a reset, N where one is expected, zero iterations, a slot re-tenanted the frame its layer is freed. Every ordering that is implied and not written becomes a row — `(id, scenario, ordering, expected outcome)` in `research.md`, and an Acceptance row that cites it. A row is the whole output; the ordering is never a sentence added to a Decision.
- *Achievable and proving.* Per Acceptance row: can a test assert this against current source (counters behind `#[cfg(test)]`, an observable warn, a fixture literal that exists)? Could it pass on a build that leaves the Problem's defect in place? A negative-existence row is a grep gate; say so. Once for the section: is there a Decision with no row that would fail if it were violated, and is there a row no Decision makes necessary?

**Subtract lens** — shape, answered before source. Four questions, each with a named answer:

- One mechanism this brief carries that could be deleted, and what deleting it costs. "None" is allowed and must say what was considered.
- Each Decision sentence: if the executor disagreed with it and were still right, it is Path. Name the sentences.
- Each non-goal: would a reader assume the brief owed it? If so, does it carry a warrant?
- Restated facts and bare counts in prose; the length against the ~120-line target, with the excess located by section.

**Finding contract.** Every finding, every field:

| Field | Requirement |
|---|---|
| LENS | premise / rows / subtract |
| LOCATION | Section and bullet or row id |
| CONSUMER | Who experiences the defect, named. No consumer, no finding |
| EVIDENCE | `path:line` for every source claim, from a file opened this session |
| TARGET | `acceptance-row` / `research-row` / `owner` |
| TEXT | For a row target: the literal row, in the brief's voice, no line numbers. For `owner`: the question, and the choices if there are two |
| SEVERITY | Blocker / Complicates / Nit |

Reviewers write their findings file and report counts plus one line per finding. The orchestrator never retypes a row.

### 3. Apply rows, surface the rest

Read all three files. Merge rows that pin the same ordering — lenses cannot see each other, so ids collide; final numbering is yours. Then:

- Append `research-row` findings to the pin table, and `acceptance-row` findings to Acceptance, each row citing its pin.
- Apply a completion to an existing row only when it does not change the measurement. When in doubt it does.
- Everything targeted `owner` goes in the report, grouped by section, with the proposed text. Nothing is applied to Problem, Decisions, Path or Open questions, whatever the severity.

Read the diff against the snapshot, not the result. Then re-read Decisions against the rows you added: a row that is only satisfiable by a mechanism no Decision names is a missing Decision, and that is an owner item too.

Amend the brief's commit.

### 4. Report

Under fifteen lines:

- Rows added, completions applied, by lens
- Owner items, grouped by section, each with its proposed text — a false premise first
- Recommendation: *promote*; *owner decides* (list); or *reshape*, when the subtract lens found a rival the brief never held — that one goes back through `/validate-plan`, not through another round here

One pass. A second runs only after the owner has edited Decisions, and then only the premise lens over the bullets that changed.

## Working rules

- The table under **What may change** is the skill. A finding with no legal target is an owner item, not an exception.
- Reviewers never edit. The orchestrator edits only rows.
- No rounds. Convergence by accretion is the failure this skill exists to prevent; if the brief needs a third pass, its Decisions are wrong, and that is `/validate-plan`.
- No padding, no praise, no emojis. Match the voice of `/draft-brief`.

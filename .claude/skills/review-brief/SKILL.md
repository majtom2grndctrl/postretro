---
name: review-brief
description: >
  Detail review of a problem brief in `context/plans/drafts/`. Three lenses
  — premise, rows, subtract — each confined to the sections one edit table
  allows. Runs as a review → fix → self-review → re-review loop after
  /validate-plan returns Direction sound, before promotion. A false Decision
  premise, a change to what a Decision or Acceptance row means, and any
  finding on the modder-facing blocks go to the owner.
argument-hint: "[brief-name]"
---

# Review Brief

The detail review for the brief process. Three concerns — is the source claim true, is the ordering pinned, is the shape right — under one rule: **a reviewer's text lands only in Acceptance and `research.md`.** Decisions are owner-owned. A finding about a Decision is a question for the owner, never a clause applied to it.

## Premise

Every clause a finding adds to a Decision is binding on the executor. Applied freely, review turns judgment the owner recorded into mechanism the reviewer prescribed — "must be witnessed", "covered by the reload family", a parenthetical answering a finding nobody else will ever read. The brief grows into a task-by-task plan, one applied fix at a time.

The lenses find real defects. The boundary on where their output may land keeps the brief a brief.

## What may change

One table governs every edit. *Review pass* is what a single review's step 3 applies. *Loop fix* is what the loop's Fix and Self-review steps add.

| Section | Reviewer reports | Review pass applies | Loop fix applies |
|---|---|---|---|
| Problem | A cause it cannot confirm | Nothing | Wording and coherence edits that keep the basis. A changed basis is the owner's |
| Decisions | A false premise, a Path sentence wearing a Decision's clothes, a non-goal missing its warrant | Nothing | Wording tightened with its meaning unchanged. A false premise is the owner's |
| Scripting surface, Boundary inventory, Wire format | Any defect | Nothing | Nothing. Every finding is the owner's: a name, casing, argument order, default, or byte layout is a one-way door for mods |
| Acceptance | A new row; a completion of an existing row | Add rows; apply a completion that does not change what the row measures | Same |
| Path | A stale or contradicted claim | Nothing | Correct the claim, cited by symbol |
| Open questions | An unmarked entry | Nothing | Mark an unmarked entry; clarify wording. Never resolve, delete, or re-mark a **blocks build** or **delegated** entry |
| `research.md` | Pin-table rows | Add rows | Add rows |

A "completion" finishes what a row already meant — names the edge it implied, adds the permit case beside its refuse case. Anything that changes the measurement is the owner's, surfaced with the proposed text. A finding with no legal target is an owner item, not an exception.

## Process

### 1. Locate and read

Argument is a plan folder name; look in `drafts/` first, then `ready/` for an a-la-carte read on a brief already promoted. The line under the title must mark it as a brief; otherwise stop and report.

Read the brief and `research.md` yourself. Snapshot both to the scratch directory before any reviewer runs: briefs are amended into one commit, so the snapshot is the only per-round diff.

### 2. Dispatch three lenses

One message, three `Agent` calls, read-only, fresh context. Or one agent carrying all three in the order below — if so, the subtract lens answers **before** the agent opens any source file; once it has adjudicated identifiers, the shape question gets a closing paragraph instead of attention.

Every prompt carries: the confirmed absolute paths of the brief and `research.md`, the `context/lib/` docs the header names, the source files Path cites, `context/lib/context_style_guide.md` to read first, the table above, the finding contract, and where to write its findings file. Report only — no edits, no cargo commands.

**Premise lens** — Decisions only. For each bullet, find the claim about the code it rests on and open the symbol. Three verdicts: *holds*, *false*, *unverifiable*. A *false* premise is the highest-value finding this skill produces and it goes straight to the owner; the reviewer does not propose a replacement decision. A `### Scripting surface` block gets the same treatment: every name in the example either resolves against the SDK or is one the brief adds, and a TypeScript example names its Luau mirror or the Boundary inventory says why not. Nothing outside Decisions is fact-checked here — Path claims are the executor's.

**Rows lens** — Acceptance and the pin table. Two passes:

- *Ordering.* Construct a sequence that satisfies every Decision as written and still violates the Problem's intent. No constructible sequence, no finding. Where a Decision asserts an order of operations, open the function and confirm the real order. Probe each stated invariant and each piece of mutable state or timer:
  - *Same tick.* Two events the prose puts on different ticks arrive on one.
  - *Reversed.* B arrives before A.
  - *Boundary.* A timer, queued intent, or in-flight message crosses a reset, unload, respawn, or authority handoff.
  - *Batching.* N of the same event, including 0 and 1.
  - *Zero duration.* A duration authored at 0 or under one tick.
  - *Stage order.* Input → Game logic → Audio → Render → Present: does a reader see pre- or post-mutation state?
  - *Sampling.* A consumer slower than its producer; a slot re-tenanted the frame its layer is freed.

  Every ordering implied and not written becomes a row — `(id, scenario, ordering, expected outcome)` in `research.md`, and an Acceptance row that cites it. A row is the whole output; the ordering is never a sentence added to a Decision. Skip for a brief with no mutable state, timer, or event ordering.
- *Achievable and proving.* Per Acceptance row: can a test assert this against current source (counters behind `#[cfg(test)]`, an observable warn, a fixture literal that exists)? Could it pass on a build that leaves the Problem's defect in place? A negative-existence row is a grep gate; say so. Once for the section: is there a Decision with no row that would fail if it were violated, and is there a row no Decision makes necessary?

**Subtract lens** — shape, answered before source. Four questions, each with a named answer:

- One mechanism this brief carries that could be deleted, and what deleting it costs. "None" is allowed and must say what was considered.
- Each Decision sentence: if the executor disagreed with it and were still right, it is Path. Name the sentences.
- Each non-goal: would a reader assume the brief owed it? If so, does it carry a warrant?
- Restated facts and bare counts in prose; length against the selected mode's target, excluding required boundary and wire tables. Locate any excess by section.

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
- Everything targeted `owner` goes in the report, grouped by section, with the proposed text. A review pass applies only what the table's *Review pass* column allows, whatever the severity.

Read the diff against the snapshot, not the result. Then re-read Decisions against the rows you added: a row that is only satisfiable by a mechanism no Decision names is a missing Decision, and that is an owner item too.

Amend the brief's commit.

### 4. Report

Under fifteen lines:

- Rows added, completions applied, by lens
- Owner items, grouped by section, each with its proposed text — a false premise first
- Recommendation: *promote*; *owner decides* (list); or *reshape*, when the subtract lens found a rival the brief never held — that one goes back through `/validate-plan`, not through another review

## Review loop

Runs once `/validate-plan` returns *Direction sound*. Steps 1–4 are one review. Repeat:

1. **Review.** Steps 1–4. From the second review on, reviewers also get the prior findings and the snapshot diff: confirm each fix landed, look for new defects, do not re-raise resolved ones.
2. **Fix.** Apply each finding that helps an executor build the right thing or helps PostRetro long term. A finding is a candidate, not a mandate: check it against source, and skip one whose premise fails, saying so in the report.
3. **Self-review.** Re-read the brief as the executor would. Fix contradictions, orphaned Decisions, a stale Problem, rows no Decision makes necessary. Repeat until the brief is correct and coherent.
4. **Re-review.** Fresh lenses on the fixed brief.

Stop on one review with no Blockers, or two consecutive reviews with one Blocker or fewer. Stop at four reviews and hand the owner the open Blockers.

Stop for the owner at once when:

- A fix would change what a Decision means or what an Acceptance row measures.
- A Decision's premise is false.
- A finding lands on the Scripting surface, Boundary inventory, or Wire format.
- A Blocker needs an owner decision.
- A round's fixes manufacture the next round's Blockers. The Decisions are wrong; that is `/validate-plan`.

Loop edits follow the table's *Loop fix* column. Reviewer prose never lands in a Decision as a new clause.

Report once, when the loop ends: reviews run, Blockers per review, fixes applied, findings skipped and why, owner items.

## Working rules

- The table under **What may change** is the skill.
- Reviewers never edit. The orchestrator edits only where the table allows.
- Every round ends in the stop rule or the owner. Convergence by accretion is the failure to prevent; a brief still producing Blockers after four reviews has wrong Decisions, and that is `/validate-plan`.
- No padding, no praise, no emojis. Match the voice of `/draft-brief`.

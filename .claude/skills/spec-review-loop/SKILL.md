---
name: spec-review-loop
description: >
  Runs the full review loop over a draft spec: /review-draft-spec, open-question
  resolution, /review-implementability, conditional re-reviews, then a
  context/lib audit for content the spec makes stale. Applies findings that hold
  up; surfaces blockers. Run in a session grounded by /spec-session, after a
  draft exists.
disable-model-invocation: true
argument-hint: "[plan-name]"
---

# Spec Review Loop

Drive one draft spec from "written" to "reviewed clean, library current." Blockers stop the loop and go to the owner.

Precondition: `/spec-session` ran this session. If it did not, run it first — the loop applies findings, and applying them without the habits reintroduces what the reviewers caught.

Argument is the plan folder name. Absent, list `context/plans/drafts/` and ask which.

## Loop

Three `/review-draft-spec` rounds per invocation, maximum. The budget is the thing that keeps this from cycling forever — a spec still producing Blockers after three rounds has a problem in its requirements or goals, and more rounds will not find it.

### 1. `/review-draft-spec` — round 1

Apply findings that hold up. A finding is not a mandate: check it against source before editing, and skip one whose premise is wrong — say so in the report rather than dropping it silently.

Record the round's Blocker count. Later steps branch on it.

### 2. Resolve open questions

Every open question in the spec gets answered now, before more review rounds run on text that is still undecided.

Answer from the spec's own ground: what problem does this spec exist to solve, and which design decisions already made constrain the answer? An answer that does not trace to one of those two is a guess — take it to the owner at step 6.

Each resolved question becomes decided prose in the spec body, not an annotation. Delete the question. A question whose answer is genuinely the owner's — cost, priority, a one-way door with no technical winner — stays in Open Questions with the owner named.

### 3. `/review-draft-spec` — round 2, conditional

Run it if round 1 returned more than two Blockers. Apply findings the same way.

A round with that many Blockers means the fixes were substantial, and fixes are the least-reviewed text in the file.

### 4. `/review-implementability`

Apply fixes where they make sense. Determinate ones — plumbing enumeration, fixture and type corrections, completions of the stated design — land directly. Genuine path choices and anything touching a locked owner decision go to the owner at step 6.

### 5. `/review-draft-spec` — round 3, conditional

Run it if the most recent `/review-draft-spec` round returned more than one Blocker. Apply fixes. This is the last round this invocation runs.

### 6. Clean, or pause for the owner

**No blockers open** — audit the library:

- Re-read `context/lib/context_style_guide.md` for durable-content guidance. Read it now, not from memory — the audit is where its rules bind.
- Find what this spec makes stale. Sweep `context/lib/` for contracts, invariants, ownership boundaries, and pipeline topology the spec changes, retires, or supersedes.
- Update in place, following the style guide. Durable decisions enter the library once decided, before they ship — state the contract, mark what is not built yet.
- New doc in `context/lib/` means a matching Agent Router entry in `index.md`. An unrouted doc is invisible.
- The library never links to plans. Copy the surviving contract in; point readers at another library doc or at current code.

**Blockers open** — pause. Do not promote, do not touch `context/lib/`, do not run a fourth round.

Pause immediately, before the budget is spent, for any blocker this session cannot settle on its own: one that needs an owner decision, contradicts a locked decision, or challenges the spec's direction. Burning rounds on those wastes context and changes nothing.

Hand the owner what they need to act:

- Each open blocker: location, what it breaks, and the direction it needs.
- Which blockers survived multiple rounds, and whether a round's fixes manufactured the next round's blockers. That pattern is the signal that requirements or goals — not prose — are the problem.
- The narrowest change that would clear the cluster: a goal to drop, a requirement to loosen, a spec to split.

Say plainly that this is a pause, not a verdict. The owner adjusts requirements, goals, or scope, and then either:

- **Resumes** — invoke `/spec-review-loop` again on the revised spec. The round budget resets and the loop restarts at step 1; findings from earlier rounds are keyed to the old shape and do not carry over.
- **Aborts** — the spec goes back further than this loop reaches: `/validate-plan` for direction, `/draft-plan` for a reshape, or the shelf. The loop's job ends there.

Amend the session's commit as the loop iterates rather than stacking one commit per round.

## Report

- Rounds run, and why the conditional ones did or did not fire
- Blocker count per round, and disposition: fixed, or open with location
- Findings skipped, and the premise that failed
- Open questions resolved, and any left to the owner
- `context/lib/` files changed, and what went stale
- On a pause: the open blockers, the pattern across rounds, and the narrowest change that would clear them

Cap at ~15 lines unless a pause needs more.

## Working rules

- One spec per invocation. Two drafts are two runs.
- Reviewers report; this session edits.
- Never a fourth `/review-draft-spec` round in one invocation. A spec that needs one needs the owner.
- A pause is a stop, not a caveat on a promotion recommendation.
- No padding, no praise, no emojis.

---
name: validate-plan
description: >
  Adversarial direction review of a spec: is this a reasonable solution to
  the problem at hand? One fresh reviewer judges framing, layer placement,
  foreclosure, unstated divergence from prior commitments, reversibility,
  and the strongest alternative. Can conclude the work needs no spec, or a
  bigger one. Run before detail review (`review-draft-spec`), after a draft
  session, or a la carte on any spec whose direction has gone stale.
argument-hint: "[plan-name]"
context: fork
agent: general-purpose
---

# Validate Plan

One reviewer, one altitude: direction. Never checks identifiers, AC wording, or task completeness — those are `/review-draft-spec` and `/review-implementability`, and they run after this.

## Premise

A spec can be locally correct and globally wrong: every identifier grounded, every AC sound, and still the wrong kind of solution. Detail review passes it, because nothing is wrong with the details.

That failure is attentional, not a capability gap. A reviewer asked to check identifiers *and* judge direction does the identifier work — concrete and gradeable — then gestures at direction in a closing paragraph. Separate passes are the point.

Because attention cannot be split by instruction, this skill does not ask a reviewer to resist anchoring. It controls read order instead.

## Process

### 1. Commitment search

The real defect is usually a **cross-cutting thesis** violated by a subsystem-local drafter — precisely the commitment they never read. Direct-predecessor lineage does not reach it. Worked example: a spec that hardcoded policy into the engine floor violated a thesis living in a different epic's substrate plan, while its own direct predecessor said nothing about policy ownership.

So obvious lineage is a **floor, not a scope**:

1. Route through `context/lib/index.md` to the docs governing the subsystem. Mandatory per CLAUDE.md; the reviewer reads what it routes to.
2. Grep `context/plans/done/` for the **concepts** the spec touches — ownership, authority, mechanism-vs-policy, determinism, layering — not the subsystem name. Cross-epic hits are the point.
3. Note the direct predecessor and the source files the spec names.

Hand all of it to the reviewer as a labeled list of paths, explicitly as a starting floor it must extend. The reviewer owns the search; a coordinator's guess must not become the ceiling.

### 2. Spawn one reviewer (read-only, fresh context)

Dispatch prompt contains exactly:

- The spec, **inlined verbatim**. No summary, no statement of intent, no "what I was going for" — the coordinator's framing anchors the reviewer as effectively as a bad spec.
- The step 1 paths, labeled, marked as a floor to extend.
- The six questions and the verdict set below.
- Locked owner decisions, with the rider in step 3.
- Instructions: report only, no edits, no cargo commands.

**Read order is load-bearing.** Instruct the reviewer to answer Q1, Q4 and Q6 from the spec's Goal, Scope and Tasks **before** reading its `Direction` section. Then read `Direction` and report the diff. Agreement is corroboration. An alternative the reviewer reached independently that is *absent* from "Alternatives rejected" is itself a finding — that section is the one a drafter can satisfy by rebutting a rival it never held, and it is the one with no external referent to check against.

On a brief (the line under the title says so): answer from Problem and Acceptance before reading Decisions and Path. Decisions is the direction; Path's "strongest rival" is the alternatives section.

The reviewer answers six questions and must reach a verdict on each; "some concerns" is not an answer.

1. **What problem is this actually solving, and what observation produced it?** One sentence for the problem — cause or symptom of something upstream? Then the evidence: a review finding, a bug, a modder request, or an anticipation. Anticipated problems are legitimate, but naming one as anticipated is how the strongest form of *Not a spec* — no one has hit this yet — becomes reachable.
2. **Is it being solved at the right level?** Name the placement axis before judging it. This repo has several: engine-vs-mod, mechanism-vs-policy, host-vs-client authority, load-time-vs-runtime, descriptor-data-vs-engine-code, floor-vs-authored. Pick the ones in play; do not default to the first.
3. **What does this foreclose?** What becomes harder or impossible afterward. "Nothing material" is a legitimate and common answer — say it plainly rather than manufacturing a foreclosure. What is banned is the empty hedge "nothing significant" standing in for not having looked.
4. **What has this project already committed to that this touches?** Extend the step 1 floor. See *Precedence* below.
5. **Is this a one-way door, and what does undoing it cost?** Distinct from Q3: foreclosure is what becomes impossible *afterward*, reversibility is what it costs to back out of *this*. It changes the calculus more than Q3 does — a reversible wrong direction is often worth shipping to learn from, while an irreversible right-looking one earns a reshape.
6. **What is the strongest alternative, and why not that?** Propose a rival shape; do not just list concerns. Committing to an alternative forces a real judgment and gives the owner something to compare.

**Verdict, one of:**

- *Direction sound* — proceed to `/review-draft-spec`.
- *Reshape* — the named alternative is better, or the placement is wrong.
- *Not a spec* — the work is smaller than a plan. Say what it is instead.
- *Under-scoped* — the real problem is bigger than what is scoped. Say what is missing.

The last two are first-class outcomes. A reviewer that can only grade direction good-or-bad never reaches them, and both are common.

Proportionality is not a seventh question — it is this verdict set. Over-built resolves to *Not a spec*, under-sized to *Under-scoped*, and the comparison that settles either is Q6's alternative. Asking it separately just gets it answered twice.

### 3. Locked decisions

The reviewer may not relitigate a locked owner decision. It **must** state when one is load-bearing for its verdict, and what would follow if it were reopened.

Without that rider, "Direction sound" silently means "sound conditional on a lock I was forbidden to examine" — and in a direction review the locked decisions often *are* the direction.

### 4. Report

Verdict, the reasoning, any load-bearing locks, and — for *Reshape*, *Not a spec*, or *Under-scoped* — the concrete alternative. Reshaping is the owner's decision: surface it, never auto-apply.

## Precedence is a hint, not a rule

Question 4 is not a conformance check. Read as "does this match what we did before," it privileges whatever happened first, mistakes included — this repo ships precedents documented as defects, and specs are right to decline them.

The defect is not divergence. It is *unwitting* divergence. A spec that says "we are doing X differently because Y" is doing its job, and the argument can be had on the merits. A spec that contradicts a thesis while citing its source approvingly has not noticed it is choosing.

## Working rules

- One reviewer, all six questions. The characteristic finding is a cross-question one; splitting returns local passes.
- Fresh context both halves. This skill runs forked so the coordinating session is unimmersed too — immersion is what makes a locally-correct wrong shape feel obviously right.
- Direction only. Ungrounded identifiers and AC problems belong to later passes; raising them here dilutes the lens.
- The reviewer never edits. Reshaping is an owner decision.
- The six questions are defined here normatively. `draft-plan` restates them as a solo drafting exercise; on disagreement this list wins.
- Don't pad. No praise, no emojis. Voice match `draft-plan`.

---
name: validate-plan
description: >
  Adversarial direction review of a spec: is this a reasonable solution to
  the problem at hand? One fresh reviewer judges framing, layer placement,
  foreclosure, divergence from established practice, sizing, and the
  strongest alternative. Can conclude the work needs no spec, or a bigger
  one. Run before detail review (`review-draft-spec`), after a draft
  session, or a la carte on any spec whose direction has gone stale.
argument-hint: "[plan-name]"
---

# Validate Plan

One reviewer, one altitude: direction. Not a detail review — it never checks identifiers, AC wording, or task completeness. Those are `/review-draft-spec` and `/review-implementability`, and they run after this one.

## Premise

A spec can be locally correct and globally wrong: every identifier grounded, every AC sound, and still the wrong kind of solution. Detail review passes it, because nothing is wrong with the details.

That failure is attentional, not a capability gap. A reviewer asked to check identifiers *and* judge direction does the identifier work — it is concrete and gradeable — then gestures at direction in a closing paragraph. Keeping the altitudes in separate passes is the whole point of this skill.

Direction review is also cheapest first. Redirecting a spec costs nothing when there are no details to throw away.

## Process

### 1. Locate and read

Argument is a plan folder name; look in `drafts/` first, then `ready/`, then `in-progress/`. Read the full spec and any sibling `research.md` yourself before delegating.

Identify the spec's lineage: the predecessor plan it succeeds or extends (`context/plans/done/`), the `context/lib/` docs governing the subsystem, and the source files it names. The reviewer needs these to judge anything.

### 2. Spawn one reviewer (Opus, read-only, fresh context)

Inline the full spec content — paths drift. Also pass the lineage from step 1, and the locked owner decisions marked do-not-relitigate. Instruct: report only, no edits, no cargo commands.

The reviewer answers six questions. It must reach a verdict on each; "some concerns" is not an answer.

1. **What problem is this actually solving?** One sentence. Is that the cause or a symptom of something upstream?
2. **Is it being solved at the right level?** Layer, ownership boundary, engine-vs-mod, mechanism-vs-policy. A right change in the wrong layer is the characteristic failure this pass exists to catch.
3. **What does this foreclose?** What becomes harder or impossible afterward. Name specific foreclosures or state plainly that there are none — never "nothing significant".
4. **What has this project already committed to that this touches?** The reviewer finds the commitments; it is not handed them. See *Precedence* below.
5. **Is it proportionate?** Both directions. Over-built for the problem, or scoped smaller than the real problem.
6. **What is the strongest alternative, and why not that?** Propose a rival shape, do not just list concerns. Committing to an alternative forces a real judgment and gives the owner something to compare against.

**Verdict, one of:**

- *Direction sound* — proceed to `/review-draft-spec`.
- *Reshape* — the named alternative is better, or the layer placement is wrong.
- *Not a spec* — the work is smaller than a plan. Say what it is instead.
- *Under-scoped* — the real problem is bigger than what is scoped. Say what is missing.

The last two are first-class outcomes, not escape hatches. A skill that can only grade direction good-or-bad never reaches them, and both are common.

### 3. Report

Verdict, the reasoning for it, and — when it is *Reshape*, *Not a spec*, or *Under-scoped* — the concrete alternative. Reshaping a spec is the owner's decision: surface it, never auto-apply.

## Precedence is a hint, not a rule

Question 4 is not a conformance check. Read as "does this match what we did before," it privileges whatever happened first, mistakes included — this repo ships precedents documented as defects, and specs are right to decline them.

The defect is not divergence. It is *unwitting* divergence. A spec that says "we are doing X differently because Y" is doing its job, and the argument can then be had on the merits. A spec that contradicts its own predecessor's thesis while citing that predecessor approvingly has not noticed it is choosing.

So the question is: does this diverge, and if so, is the divergence stated and argued?

## Reading the drafter's rationale

A well-drafted spec already answers questions 1, 4 and 6 — problem statement, prior commitments, alternatives rejected. Treat every one of those as **a claim to test, not context to accept**.

The rejected-alternatives section is the trap. It is the part a reviewer most naturally reads and nods along to. Ask whether the stated rejection reasoning holds, and whether the real alternative went unlisted.

## Working rules

- One reviewer, all six questions. Splitting them loses the cross-question findings, which are most of the value.
- Fresh context. Never run this inside the session that drafted the spec — immersion is what makes a locally-correct wrong shape feel obviously right.
- Direction only. Ungrounded identifiers and AC problems belong to the later passes; noting them here dilutes the lens.
- Read the predecessor plan, not just the spec. Most direction failures are visible only against lineage.
- The reviewer never edits. Reshaping is an owner decision.
- Don't pad. No praise, no emojis.
- Voice match draft-plan.

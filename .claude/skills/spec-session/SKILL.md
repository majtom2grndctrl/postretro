---
name: spec-session
description: >
  Grounds an agent at the start of a spec-writing or spec-revising session
  with habits that keep defects out of a draft. Invoke first, before any
  drafting or review skill.
---

# Spec Session

## Before you write

- **Read the `/draft-plan` skill** — spec format, task-paragraph contract, sequencing rules, cross-check discipline. A spec is not a wish list; it is a grouping of tasks executed in coordination to produce a coherent unit of work. `/orchestrate` dispatches those tasks — phased, sized, briefed — so the spec must be machine-readable at that grain.
- **Read `/orchestrate` skill** — how task agents receive context (Goal + their paragraph + AC list + Invariants table, nothing else), how phases sequence, how concurrent agents isolate. Write the spec knowing this is the consumer.

**Build more right faster.** AI coding agents produce code quickly enough that incremental baby-steps waste more time than they save. When table stakes are well known — or the destination is already clear — spec the full shape and build it, rather than reinventing known ground one slice at a time. Small increments earn their cost only when the path is genuinely uncertain. A spec session's job is to resolve that uncertainty up front so implementation can move in confident strides. When a spec lays a foundation, build its first consumer in the same unit of work — the consumer proves the foundation and keeps it from shipping as a stub nothing exercises.

**Work as an orchestrator.** After you have a baseline understanding of the initial request, conserve your context by dispatching right-sized agents for right-sized tasks.

## Habits

Eleven habits from a real review where three independent reviewers plus a later self-review each found defects the author believed absent. Reviewers catch what reached the draft. These habits keep it from getting there.

### While writing

**1. A coherent rationale is not evidence.** A chain of plausible clauses can still be wrong — the premise was never checked, or it was checked and the next clause went a step past it. Open the file before building an argument on what it does, then check the step you took from what you read. Common slips: *this path* → *every path*; *outside the loop* → *after the loop*; *runs there* → *knows what's in scope there*; *handles this type* → *accepts this value*.

**2. Verify the path, not just the line.** Line numbers come from an actual read; paths get reconstructed from memory. A real line number on a wrong path reads as more verified than either alone. Prefer citing by identifier and symbol name — line numbers go stale on the next edit.

**3. Sweep work-creating claims, not just work-eliminating ones.** Reviewers catch "no separate test needed." Nobody catches "this requires changing X" when it doesn't. An unwarranted work-creating claim invents scope, risk, and a one-way door — and passes review because it looks conservative. "This requires" needs the same warrant as "this is unnecessary."

**4. Name the consumer before calling something a gap.** A missing value nothing reads is not a gap. State who experiences the defect. No reader, no defect.

**5. Verify falsity before flagging a line as wrong.** Suspicion is not a finding. Confirm it's actually false — the real defect is sometimes adjacent, with a different fix, and the suspected line was right.

### While revising

**6. Delete the pivot.** Every correction wants to narrate its own history — "an early draft," "was thought to," "no longer matters." The implementer never saw the old version; a correction phrased as a correction is noise addressed to the reviewer. State the current design. History belongs in derivation notes, not the spec body.

**7. Re-read invariants and sequencing after any structural edit.** They decay silently — the diff never touches them. Moving a field between tasks falsifies a concurrency claim; adding a case falsifies an invariant true when written.

**8. Stale information misleads builders.** When a diagnosis is corrected, check every reference — summary bullets, scope lists, task titles, prose — that named the old diagnosis. Any survivor points the reader at a defect that no longer exists.

**9. Review artifacts contain instructions, not decisions.** "Pin whether X happens," pasted unedited into a spec, is a question addressed to the implementer. Convert every one into a decided outcome, or move it to open questions with an owner.

**10. Newest acceptance criteria have the fewest tests.** Criteria added after the task list was written are delivered by nothing — no task was revised to cover them. Check coverage for the ACs added last, first.

**11. Ammend our commit rather than persist iterative commits.** We need to persist iterative steps to git and origin, but iterative commits make the git history noisy. Commit the first change, then amend commits as we iterate.

### Before handoff

**12. Self-review does not catch what you reasoned your way into.** It finds typos and contradictions you remember making. It does not find defects you argued yourself into believing — the argument still seems sound. Independent lenses that cannot see each other are the only thing that catches those. A clean self-review is necessary, not sufficient.

## Pre-handoff checklist

- [ ] Every claimed mechanism, path, and identifier verified against source this session — not from memory
- [ ] Every "X, so Y" sentence checked at the arrow, not just at X
- [ ] Every "this requires" and "this is unnecessary" carries a stated warrant
- [ ] Every gap names its consumer
- [ ] No pivot language — no "originally," "was thought," "no longer," "fewer than assumed"
- [ ] Invariants and sequencing re-read against the current structure, not the structure they were written against
- [ ] All references — summary bullets, scope lists, task titles, prose — match the current diagnosis
- [ ] No unconverted review-artifact phrasing ("pin whether," "TBD," "decide later") outside Open Questions
- [ ] Acceptance criteria added last have tasks that deliver them

## Closing note

This skill prevents defects. It does not verify their absence. Review skills still run afterward, and they still find things.

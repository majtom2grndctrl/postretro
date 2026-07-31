---
name: spec-session
description: >
  Grounds an agent at the start of a spec-writing or spec-revising session
  with the habits that keep defects out of a draft. Not a review skill —
  `/validate-plan`, `/review-draft-spec`, and `/review-implementability`
  review the artifact after the fact; this one runs before or during
  writing. Invoke before starting a spec, or from a drafting skill that
  points at it.
---

# Spec Session

Eleven habits, gathered from a real review where three independent
reviewers plus a later self-review each found defects the author believed
absent. Reviewers catch what got into the draft. This skill is about not
putting it there.

## While writing

**1. A coherent rationale is not evidence.** A chain of clauses that each
follow from the last can still be wrong, because the premise was never
checked — reasoning from a plausible convention instead of the actual
mechanism. Open the file before building an argument on what it does.

**2. Verify the path, not just the line.** Line numbers survive from an
actual read; paths get reconstructed from memory. A real line number
attached to a wrong path reads as more verified than either would alone.
Prefer citing by identifier and symbol name — a line number goes
stale on the next edit regardless.

**3. Sweep every work-creating claim, not just the work-eliminating ones.**
Reviewers are trained to catch "no separate test needed" and "follows
automatically." Nobody catches "this requires changing X" when it doesn't.
An unwarranted work-creating claim invents scope, risk, and a one-way
door, and it passes review because it looks conservative. "This requires"
needs the same warrant as "this is unnecessary."

**4. Name the consumer before calling something a gap.** A missing value
that nothing reads is not a gap. State who experiences the defect. No
reader, no defect.

**5. Verify the falsity before flagging a line as wrong.** Suspicion of a
shipped comment or doc line is not a finding. Confirm it's actually false
— the real defect is sometimes adjacent, with a different fix, and the
line you suspected was right.

## While revising

**6. Delete the pivot.** Every correction wants to narrate its own history
— "an early draft," "was thought to," "no longer matters," "fewer than
first assumed." The implementer never saw the old version; a correction
phrased as a correction is noise addressed to the reviewer who just
corrected you. State the current design. History belongs in derivation
notes, never the spec body.

**7. Re-read invariants and sequencing rationale after any structural
edit.** They decay silently — the diff never touches them. Moving a field
between tasks falsifies a concurrency claim; adding a case falsifies an
invariant that was true when written.

**8. Stale labels are worse than stale prose.** When a diagnosis is
corrected, check the summary bullets, scope lists, and task titles that
named the old diagnosis. They still point the reader at a defect that no
longer exists.

**9. Review artifacts contain instructions, not decisions.** A finding
phrased "pin whether X happens," pasted unedited into a spec, is a
question addressed to the implementer. Convert every one into a decided
outcome, or move it to open questions with an owner.

**10. The newest acceptance criteria have the fewest tests.** Criteria
added after the task list was written are delivered by nothing — no task
was revised to cover them. Check coverage for the ACs added last, first.

## Before handoff

**11. Self-review does not catch what you reasoned your way into.**
Reviewing your own draft finds typos and contradictions you remember
making. It does not find the defects you argued yourself into believing,
because the argument still seems sound. Independent lenses that cannot
see each other are the only thing that catches those. A clean
self-review is necessary, not sufficient.

## Pre-handoff checklist

- [ ] Every claimed mechanism, path, and identifier verified against source this session — not from memory
- [ ] Every "this requires" and "this is unnecessary" carries a stated warrant
- [ ] Every gap names its consumer
- [ ] No pivot language — no "originally," "was thought," "no longer," "fewer than assumed"
- [ ] Invariants and sequencing rationale re-read against the current structure, not the structure they were written against
- [ ] Summary bullets, scope lists, and task titles match the current diagnosis
- [ ] No unconverted review-artifact phrasing ("pin whether," "TBD," "decide later") outside Open Questions
- [ ] Acceptance criteria added last have tasks that deliver them

## Closing note

This skill prevents defects. It does not verify their absence. Run
`/validate-plan`, `/review-draft-spec`, or `/review-implementability` as
appropriate — they still run, and they still find things.

---
name: brief-session
description: >
  Grounds an agent at the start of a brief-drafting or brief-revising session
  with the habits that keep a problem brief short, decided, and honest about
  what it does not know. Invoke first, before /draft-brief or any revision of
  a brief. Counterpart to /spec-session for the brief process.
---

# Brief Session

## Before you write

- **Read the `/draft-brief` skill** — the five sections, the Decisions-vs-Path test, the two kinds of open question, and what the executor does with the brief afterward.
- **Know the consumer.** One long-horizon executor with the whole brief, `research.md`, and the repo. It re-verifies source before building and writes its own task split. Write for that reader: nothing restated, nothing pre-chewed.

**Work as an orchestrator.** After a baseline read of the request, dispatch right-sized agents for research. Keep your own context for the Decisions section — that is the judgment only this session can make.

## Habits

### While writing

**1. A brief is the record of decisions, not the argument for them.** Argue in `research.md`; conclude in the brief. A Decisions bullet that runs past three sentences is still arguing.

**2. Ground the premise, not the whole tree.** A Decision rests on a claim about the code — "the substrate discards the floor normal," "`dash` and `crouch` are present-then-all-required." Open the file for those. Everything downstream in Path is a sketch; cite by symbol and move on. Line numbers are the tell that you are pre-verifying what the executor will re-verify anyway.

**3. Write the cause, then check that the outcome cures it.** The Problem paragraph fails when the outcome would be just as satisfying with the cause sentence deleted. If so, the cause was decorative and the brief is aimed at a symptom.

**4. A non-goal earns its warrant only when a reader would assume it was owed.** "No new input action" needs none. "Wall-normal forwarding belongs to wall-run" does — a reader of the Problem would expect slide to forward the normals it touches.

**5. Every AC names an edge or it names steady state.** Start, stop, reverse, zero iterations, two on one tick, the timer across a reset. Steady state is where behavior is easiest to describe and least likely to break. Orderings are AC rows, never prose.

**6. If the executor could disagree with it and still be right, it is Path.** The test from `/draft-brief`, applied per sentence. Decisions that turn out to be preferences bloat the binding section and make the executor's plan of record read as a list of violations.

### While revising

**7. A resolved question becomes one bullet and leaves.** No history: not "was open," not "we settled on." The executor never saw the question.

**8. Re-read Decisions after any Problem edit.** A reframed cause can orphan a decision that was answering the old one. The diff never touches the orphan.

**9. Mark every open question.** **Blocks build** or **delegated**. An unmarked question is a decision left in the executor's lap with no reporting obligation attached, which is how it becomes silent.

**10. Amend, don't stack.** One commit per brief per session. Push the amended commit so the work persists; keep the history clean.

### Before handoff

**11. Self-review finds typos, not wrong shapes.** `/validate-plan` is the only external lens this process runs before code. Run it, read the verdict as a fresh reader would, and do not rebut it from inside the session that drafted the brief.

## Pre-handoff checklist

- [ ] Problem paragraph names who observed it, the cause, and the outcome — one paragraph
- [ ] Every Decision premise about the code was read this session and is cited by symbol
- [ ] Every Decision has an AC that would fail if it were violated
- [ ] Every non-goal a reader would assume was owed carries its warrant
- [ ] No line numbers; no restated surface; no task paragraphs
- [ ] Path is marked non-binding and names a first slice
- [ ] Every open question is marked **blocks build** or **delegated**
- [ ] Header carries the commit the source was read at
- [ ] Under ~120 lines, or the excess is named as derivation and moved

## Closing note

This skill keeps a brief short and decided. It does not verify the brief is right. `/validate-plan` still runs, and the executor's plan of record is where the brief meets the code.

---
name: review-draft-spec
description: >
  Multi-agent review of a draft spec in `context/plans/drafts/`. Spawns
  three parallel reviewers: one broad, one anchored to source that
  fact-checks every named identifier, and one temporal that attacks the
  spec's orderings. Applies mechanical fixes from the reviewers' own
  FIND/REPLACE blocks, all-or-nothing, with collision detection, unless
  --no-auto-apply is set. Recommends apply / re-review / promote.
  Use after a draft session, or when a human wants to validate before
  promoting to ready/.
---

# Review Draft Spec

Three reviewers in parallel. One broad, one anchored to source, one attacking orderings. Aggregate findings, auto-apply mechanical fixes, recommend whether to apply more, re-review, or promote.

Detail altitude. Direction — is this a reasonable solution to the problem at all — belongs to `/validate-plan` and runs first; findings here are keyed to the spec's current shape, and a reshape invalidates them.

## Process

### 1. Locate the spec

Argument is the plan folder name (e.g. `entity-model-foundation`) or a full path. If absent, list drafts and ask which one:

```
!`ls context/plans/drafts/`
```

Resolve to `context/plans/drafts/<name>/index.md`.

### 2. Read the spec once

Read the full spec yourself before delegating. Decisions about which reviewers to run depend on what the spec contains, and you are the one who resolves collisions later — that needs the whole file in your head, not a summary.

Pass reviewers the absolute path, having confirmed this session that it exists, and say in the prompt that you confirmed it. Do not inline the spec: a byte-exact `FIND` has to be copied off disk, so every reviewer opens the file regardless, and three inlined copies of a 750-line spec buy nothing.

Then snapshot it. Copy both spec files to a scratch directory before any
reviewer runs. Sessions that amend a single commit have no per-round diff to
read afterwards, and reading the change is what catches regressions that
reading the result does not.

### 3. Run reviewers in parallel

One message, three `Agent` tool calls. No sequential rounds.

Every reviewer prompt carries three things beyond its lens: the change brief,
the finding contract, and where to write.

**The change brief.** Name what changed since the last round and tell the lens
to attack that hardest. Three rounds running on the dist-packaging spec, every
new Blocker came from the previous round's fixes — a revision is the least
reviewed text in the file, and the lens that found the defect it replaced
cannot see it. Say so explicitly; it changes where attention goes.

**The finding contract.** Every finding, every field:

| Field | Requirement |
|---|---|
| SEVERITY | Blocker / Complicates / Nit |
| BUCKET | Mechanical / Architectural, self-classified |
| LOCATION | Spec file and section or row id |
| CONSUMER | Who experiences the defect, named concretely. No consumer, no finding (habit 4) |
| EVIDENCE | `path:line` for every source claim, from a file opened this session. A source claim without one is void |
| FALSITY | For any finding alleging the spec is wrong: how it was confirmed false rather than suspicious (habit 5) |
| ORDERING | Temporal lens only: the constructible sequence. No ordering, no finding |
| PROBLEM | One paragraph |
| CONFIDENCE | high / medium / low |
| ALSO_TOUCHES | Every other location in the spec that states, counts, references, or depends on what this fix changes — each either fixed in the same finding or named for the orchestrator. "None, verified by grepping `<term>`" is a valid answer and must be stated, never omitted |
| FIX | A `FIND`/`REPLACE` pair, defined below |

The `FIX` field takes exactly this shape, delimiters included:

```
FIX:
<<<FIND
(byte-exact text from the spec file, multi-line where the text is multi-line)
FIND>>>
<<<REPLACE
(the literal prose that replaces it)
REPLACE>>>
```

Show reviewers that template verbatim. Describing the pair in prose is not
enough: three independent lenses given the same description rendered it three
ways — plain field lines, a two-column table, `FIND:` over a fenced block — and
a tool that reads one of them silently drops the other two lenses' work.

`FIX` is literal final spec prose, never advice — not "consider adding", not
"clarify that". The exact sentences that will land, in the spec's voice. `FIND`
is copied byte-for-byte out of the spec file, multi-line where the text is
multi-line, and must be unique; the reviewer verifies uniqueness before writing
the block. `REPLACE` carries no line numbers — that prose ships and must
survive the next source edit, so identifiers and paths only.

**Where to write.** Each reviewer writes its own findings file to the scratch
directory and reports back only counts and one line per finding. The
orchestrator never retypes a `FIND` block. Three of round 3's Blockers on the
dist-packaging spec came from the orchestrator compressing a correct
prescription and dropping a qualifier; three round-4 fixes were skipped because
the orchestrator flattened multi-line `FIND` strings into one line. Both
failure modes disappear when the reviewer writes the block and the applier
reads the file.

`ALSO_TOUCHES` is the field that stops a round from manufacturing the next
round's work. A spec is globally coupled and every fix is a local edit; the
largest defect class by a wide margin is a fact stated in two places where the
fix landed in one. Round 5's reviewers did this informally where they happened
to notice — "apply with T8", "apply B1 first or reject both" — and every time
they did, it held. Requiring it converts that luck into procedure.

**A rewrite emits a table, not prose.** When a fix changes the *shape* of a
rule rather than tweaking it — a guard, a predicate, a dispatch — the `FIX`
carries the full decision table: every input class against its verdict. Prose
describes the cases the author thought of; a table has visibly empty cells. One
payload-root guard was rewritten three times in prose and came out wrong in a
new direction each time, the last being a blanket permit nobody caught because
the sentence granting it read as a scope limit.

Never read a subagent's `output_file`. It is the full JSONL transcript.

**One lens per agent.** A lens needs sustained attention; an agent handed two satisfices and does both shallowly. Never blend them — a lens that does not fit the spec is skipped whole, not folded into another. Independence is the point: the same defect reported by two lenses that could not see each other is the signal worth acting on, and it is how a confidently wrong finding gets caught. Merging saves only a duplicated spec read.

**Every dispatched agent reads the style guide first.** Reviewers emit `REPLACE` prose that lands verbatim in the spec — they are text-editing agents, whatever the section calls them. An agent that has not read `context/lib/context_style_guide.md` writes in its own voice, and the spec drifts a little with every applied fix. Every reviewer prompt carries this instruction before the lens itself.

**Every dispatched agent checks its own work before reporting.** The orchestrator verifies too, but verification that only happens at the end scales badly and arrives late. Name the specific checks in the brief: for a reviewer, that every `FIND` string is byte-exact and unique against the spec file; for an editing agent, that `spec_integrity.py` prints `integrity: clean`, that no reference to anything it deleted survives, and that it has re-read its own diff hunk by hunk rather than the finished file.

#### Broad reviewer (Opus)

Receives:
- Full spec content inline
- The relevant `context/lib/` slices for subsystems the spec touches (route via `context/lib/index.md`)
- Instructions to find:
  - Contradictions within the spec
  - Casing or boundary inconsistencies
  - AC ↔ task gaps in either direction
  - Scope-boundary violations
  - Plumbing handwaves — "edit X to do Y" without stating how X gets access
  - Missing wire-format or FFI pins
  - Unwarranted work-eliminating claims (see below) — report each as a finding
  - Anything else that forces an implementer to guess

Plus an explicit sweep, run as its own pass rather than folded into general reading:

> Extract every claim whose function is to eliminate work — to assert that some
> code path, test, or task need not exist. Markers: "identical by construction,"
> "follows automatically," "no separate test required," "derivable from," "same
> as the X path," "trivially," "by symmetry." For each, ask whether the spec
> states a checkable reason, grounded in named source, or only restates the
> claim. Report every unwarranted one. These are usually true — say so where
> they are, and do not manufacture doubt. But they are the only sentences in a
> spec that produce no artifact to verify, so an unexamined one survives to
> implementation and surfaces as a rewrite rather than a bug.

Output: findings in the contract above, written to its own file. "No issues found" if clean. No padding, no praise.

#### Codebase-anchor reviewer (Opus)

Receives:
- Full spec content inline
- Instruction: "For every Rust/TS/Lua identifier the spec names — function, struct, type, field, enum variant, module path — open the file in source, confirm the spec's claim, report any divergence between the spec and current code reality. First step: extract the identifier list from the spec. Then resolve files via Glob/Grep. Then batch-read."
- Additional instruction: "Where the spec warrants a work-eliminating claim by citing source — two paths called identical because they share a function, state called derivable because a field already holds it — verify the warrant, not just that the identifier exists. A warrant that names real code and still does not support the claim is the highest-value finding in this pass."

Output: findings in the contract above. `EVIDENCE` cites `path:line` — that field is read by you and thrown away. `REPLACE` cites by identifier and file and never by line number: that prose lands in the spec and must survive future edits.

#### Temporal reviewer (Opus)

Skip only for a spec that introduces no mutable state, no timer, and no event ordering.

Receives:
- Full spec content inline
- Instruction to ground orderings in source: where the spec asserts an order of operations, open the function and confirm the real order. A spec sentence describing a tick as "advance timers, then evaluate intents" is worthless if the shipped function does the reverse.
- This posture, stated as its own pass:

> Do not ask whether the spec discusses ordering. Ask whether you can
> construct an ordering that satisfies every sentence the spec writes and
> still violates what it clearly intends. Apply each probe below to every
> invariant the spec states and every piece of mutable state or timer it
> introduces. A finding without a constructible ordering — an actual
> sequence of events, not an abstract worry — is not a finding.

| Probe | Question |
|---|---|
| Same-tick collision | Two events the prose implies land on different ticks arrive on one. Which wins? Is the intermediate state observable? |
| Reversed arrival | B arrives before A, where the prose implies A precedes B. |
| Boundary crossing | A timer, queued intent, or in-flight message crosses a reset, unload, respawn, or authority handoff. Survives when it should not, or dies when it should not? |
| Batching | N of the same event in one tick. All processed, first only, last only? Does the invariant hold for every N including 0? |
| Zero-duration | A duration authored at 0 or shorter than one tick. Is the state entered? Observed? Does completion fire on the start tick? |
| Stage order | Where in Input → Game logic → Audio → Render → Present does each read and write land? Does an observer read pre- or post-mutation state, and is that stated or accidental? |
| Sampling cadence | A consumer sampling slower than the producer mutates — a snapshot interval, a per-frame publish over a per-tick change. Which samples are dropped or repeated? |

Output: findings in the contract above, **plus a pin table** — `(id, scenario, ordering, expected outcome, kind)` rows the spec ought to state and does not, each concrete enough to write a test from, each naming the task that would execute it and whether that task's paragraph already covers it. Ids are provisional: lenses cannot see each other, so two may claim the same number and final numbering is yours.

The pin table is the lens's primary artifact. The defect class it targets is "invariant stated, mechanics unpinned." Prose findings get applied and forgotten; a table becomes a spec section later rounds check against. Fold it in as its own section rather than dissolving its rows into existing paragraphs, and have the spec's test task reference the rows instead of restating them.

### 4. Aggregate

Collect both reports. Dedupe — when the same issue surfaces from both lenses, keep the codebase-anchor framing (more precise).

Triage by severity:

| Severity | Meaning |
|---|---|
| Blocker | Implementer cannot proceed without guessing |
| Complicates | Implementer can guess but might guess wrong |
| Nit | Style, voice, minor inconsistency |

Then split into two buckets:

| Bucket | Examples | Default action |
|---|---|---|
| Mechanical | Casing fix, missing AC bullet, wire-format pin, deletion of stale phrase | Apply the reviewer's own `FIND`/`REPLACE` (unless `--no-auto-apply`) |
| Architectural | Reshape a contract, decide between two paths, change scope | Surface to caller; do not auto-apply |

Triage is a 30-second judgment, not a heuristic. Make the call inline. Don't delegate it to a sub-agent.

### 4b. Resolve collisions before the applier sees anything

Independent lenses cannot see each other. That is what makes them work, and it
is what leaves interaction defects to you. Two lenses editing the same sentence
in opposite directions, two claiming the same new row id, one patching a
mechanism another deleted — applied blind, these produce a self-contradictory
spec.

Run the applier in dry-run over every findings file at once. It computes each
`FIND`'s span and refuses the whole batch if any two overlap:

```
python3 .claude/skills/review-draft-spec/tools/apply_findings.py \
  --spec-dir context/plans/drafts/<name> --findings <each findings file>
```

Round 4 had four collisions, found by hand. Round 5 had eleven across eight
regions, found by the tool. For each, write a merged `FIND`/`REPLACE` yourself
taking the union of what the colliding lenses established — do not pick a
winner unless one genuinely supersedes the other, and record which findings the
merge supersedes.

Two collision classes the tool cannot see, which are yours to catch:

- **Same effect, different regions.** Round 5's anchor lens retargeted an
  existing pin row to an arm the guard rewrite had made unreachable, while the
  broad lens added a new row covering the same arm. No textual overlap, so no
  collision — but the applied result tested one arm twice and another never.
- **Superseded by a third finding.** A fix that patches a sentence another
  finding deletes wholesale.

### 5. Apply mechanical fixes

Write the verification before the edits, not after. An edit script that raises
partway through a loop and writes the file after it silently loses the edits it
already printed `ok` for.

Apply with the same tool, adding `--only <ids>` and `--apply`. It is
all-or-nothing: `FIND` must match exactly once (zero and two are both hard
failures, never a near-match fixup), every edit is post-verified in memory, and
the file is written only if all of them resolve.

Then **read the diff, not the result.** This is not optional and the tooling
does not replace it. In round 5 two orchestrator-merged blocks each dropped a
trailing line, severing a sentence mid-paragraph; post-verify passed both,
because `FIND` was absent and `REPLACE` present exactly as instructed. The diff
showed it immediately. The applier reports every run of words a block drops as
a `DROPS` line — advisory, since intentional deletions are real — but the diff
read is the control.

### 6. Check structural integrity

```
python3 .claude/skills/review-draft-spec/tools/spec_integrity.py \
  context/plans/drafts/<name>/index.md
```

Duplicate or out-of-order pin ids, rows named by no task, prose referencing a
row that no longer exists, group bullets disagreeing with the `Kind` column,
non-contiguous ACs, line-number citations that will go stale, pivot language,
unconverted review phrasing, severed sentences. It found two real defects on
the dist-packaging spec before any reviewer reported, one of them the core of a
Blocker — cheap, and it does not get tired.

Run it again after applying, and again before every commit.

Then sweep the invariants and sequencing yourself (habit 7). They decay
silently: the diff never touches them, and a structural edit falsifies claims
that were true when written. Every round of the dist-packaging review that
changed a mechanism also left an invariant row describing the old one.

### 7. Delta pass — review the round's own fixes

The fixes are the least-reviewed text in the document. Applied and committed,
they go unexamined until the next round — which is exactly why each round
manufactures the next round's work. Close the loop here instead of across
rounds.

One agent, one question. It receives the diff against the pre-round snapshot
and the full current spec, and answers: **for each changed hunk, what else in
this document is now false?** Point it at the classes that recur:

- A fact this hunk changed that is still stated in its old form elsewhere.
- A count, enumeration, or list the hunk added to or removed from.
- A pin row, AC, invariant, or task paragraph that referenced something the
  hunk moved, retagged, or deleted.
- A rule the hunk rewrote whose new shape admits or refuses a case the old
  shape did not.
- A clause the hunk added to an AC that no row reaches; or an existing row
  whose arm the hunk made unreachable, so it now passes while testing nothing.

Same finding contract. Apply, re-run the integrity check, read the diff, then
commit.

Do not skip this because the round looked clean. A round of three mechanical
fixes is precisely where an unswept referent survives.

### 8. Decide next action

| Outcome | Recommendation |
|---|---|
| No findings, or only nits already auto-applied | Run `/review-implementability`, then promote to `ready/` |
| Mechanical fixes applied, no architectural findings | Re-run this skill once to verify fixes are clean |
| Architectural findings present | Surface to caller with locations and suggested directions. Do not auto-apply. Do not recommend promotion. |
| Findings only emerge from source-reading; spec text alone reveals nothing | Spec has hit diminishing returns. Run `/review-implementability`, then promote. |

Last row is the explicit stopping rule.

**Implementability gate.** This skill reviews what the spec *says*; `/review-implementability` reviews whether a task agent could *execute* it (task-paragraph self-sufficiency, AC achievability). Run it only once this review is structurally clean — its findings are keyed to task paragraphs, so structural rework invalidates them. Skip if it already ran clean on this revision.

### 9. Report

Concise. Include:
- What reviewers ran
- Total finding count by severity (Blocker / Complicates / Nit)
- What was auto-applied (count, not full list — caller can read the diff)
- What needs the caller's attention (architectural findings, full text)
- The recommendation

Cap at ~15 lines unless architectural findings demand more.

Keep a ledger across rounds — one row per finding, and per round the counts by
severity, what was applied, what was skipped and why, and how many of this
round's Blockers were introduced by the previous round's fixes. That last
column is the one that tells you whether the spec is converging. Report
deferred findings to the caller rather than dropping them silently; a finding
you chose not to act on is a decision, and decisions are the thing a ledger
exists to hold.

## Flags

- `--no-auto-apply` — surface mechanical findings to caller instead of editing. Default for human-in-the-loop use.

## Working rules

- Don't pad. Every sentence earns its place.
- No emojis anywhere — skill or prompts.
- Reviewer prompts carry a confirmed absolute path; reviewers read the file themselves.
- Tables for mappings, prose for behavior.
- Never write a count in prose. "The three call sites" desynchronises the next time one is added; the enumeration that follows it is the checkable thing, so keep the list and drop the numeral.
- Voice match draft-plan.

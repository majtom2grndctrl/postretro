---
name: review-panel
description: >
  Runs a multi-agent review panel: parallel code reviewers plus a dedicated
  comment drift checker. Aggregates findings with deduplication and severity
  merging. Runs in a forked context so the active agent's context window stays
  clean and reviewers have no bias from prior work. Use mid-session after
  implementing a feature, or before opening a pull request.
disable-model-invocation: true
context: fork
allowed-tools: Read, Glob, Grep, Bash, Agent
argument-hint: "[file-path | plan-name] [reviewers:N] [model:opus|sonnet]"
---

# Review Panel

Review panel coordinator, isolated from the implementing agent's context. Reviewers evaluate code on its own merits — no access to prior reasoning or conversation history.

Spawn parallel review agents, collect findings, present a unified review. Do not review code yourself.

## Defaults

- **Code reviewers:** 2 agents, Opus model
- **Comment drift checker:** 1 agent, Sonnet model
- **Total:** 3 agents in parallel

Override with arguments:
- `reviewers:3` — run 3 code review agents instead of 2
- `model:sonnet` — use Sonnet for code reviewers instead of Opus

Comment drift checker: always 1 Sonnet agent, unaffected by overrides.

## Scope detection

Determine review target from first argument (same rules as `/code-review`):

- **Plan name:** all files touched by the plan's tasks
- **File path:** that file and closely related files
- **No argument:** uncommitted changes

!`git diff --stat HEAD 2>/dev/null`
!`git diff --stat --cached 2>/dev/null`
!`ls context/plans/in-progress/ context/plans/done/ 2>/dev/null`

## Process

### 1. Parse arguments

Extract from `$ARGUMENTS`:
- The review target (plan name, file path, or empty for uncommitted changes)
- `reviewers:N` — number of code review agents (default: 2)
- `model:opus|sonnet` — model for code review agents (default: opus)

### 2. Spawn all agents in parallel

Launch all agents simultaneously in a single message. All run with `isolation: "worktree"` (read-only, concurrent).

**Code review agents (N instances):**
Full content of `.claude/skills/code-review/SKILL.md` as instructions, plus review target. Specified model (default: opus).

**Comment drift agent (1 instance, always Sonnet):**
Comment drift instructions below, plus review target.

### 3. Aggregate results

Once all agents complete:

**Deduplicate:** If multiple reviewers flag the same issue (same file, same concern), keep the most specific description and note how many reviewers caught it. Agreement across reviewers is strong signal.

**Merge severity:** If reviewers disagree on severity for the same issue, use the higher severity. One reviewer seeing a 🔴 outweighs another seeing 🟡.

**Combine comment drift findings** as a separate section — don't mix them into the code review findings.

### 4. Present unified review

```
## Review Panel Summary

**Panel:** N code reviewers (model) + 1 comment drift checker (sonnet)
**Target:** [what was reviewed]
**Verdict:** approve / request changes / needs discussion

## Code Review Findings

### 🔴 Must fix
[Deduplicated findings, noting reviewer agreement where applicable]

### 🟡 Should fix
[...]

### 🟢 Nits
[...]

## Comment Drift Findings

### 🔴 Stale or misleading comments
[Comments that would lead an agent astray]

### 🟡 Comments that need updating
[Comments weakened by the changes but not yet wrong]

### 🟢 Suggested improvements
[Opportunities to add context that would help future agents]

## What's done well
[Merged from all reviewers — deduplicated]
```

Omit empty severity categories. If the panel unanimously approves with no findings, say so clearly.

---

## Comment Drift Checker Instructions

Pass these instructions to the comment drift agent verbatim.

```
You are a **Comment Integrity Reviewer** for PostRetro. Comments are living documentation — agents read them to make decisions. A stale or misleading comment is worse than no comment; it actively sets agents up for failure.

Read these files first:
- `context/lib/development_guide.md` §5 (Code Comments)
- `context/lib/context_style_guide.md` (persistent vs ephemeral content)

Review changed files and adjacent code (importers, importees, shared subsystem boundaries).

Check for:

### In changed code:
- **File headers that reference wrong context files** — does the header point to a context file that still governs this code?
- **Comments describing behavior that the code no longer implements** — the code changed but the comment didn't
- **New code missing "why" comments** — non-obvious decisions, ordering dependencies, architectural constraints that a future agent couldn't derive from the code alone
- **Orphan TODOs** — `// TODO` without a follow-up reference or actionable context
- **Comments restating code** — if the code is clear, the comment wastes context budget. If the code is unclear, improve the code.
- **Spec pointers to nonexistent docs** — `See: context/lib/foo.md` where foo.md doesn't exist

### In adjacent code:
- **Comments that reference contracts changed by this diff** — e.g., "Renderer expects vertex format X" when the diff just changed that format
- **File headers in adjacent modules whose responsibilities shifted** — did a subsystem boundary move?
- **Cross-references between modules that are now stale** — "This is consumed by module Y" when module Y no longer does that

### Output format:

Use the same severity format (🔴/🟡/🟢) as the code review:
- 🔴 **Stale or misleading** — would actively mislead an agent reading this code
- 🟡 **Needs updating** — weakened by the changes but not yet wrong
- 🟢 **Suggested improvement** — opportunity to add context that would help future agents

For each finding: name the file, quote the comment, explain what's wrong, and suggest the fix.
```

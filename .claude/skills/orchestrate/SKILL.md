---
name: orchestrate
description: >
  Orchestrates execution of a plan by spawning agents to work on tasks according
  to the plan's sequencing instructions. Reads a plan from context/plans/ready/,
  moves it to in-progress, and coordinates task execution across phases.
  Use when a reviewed plan is ready for implementation.
disable-model-invocation: true
argument-hint: "[plan-name]"
---

# Orchestrate

Orchestrate a plan from `context/plans/ready/`. Coordinate — don't produce. Dispatch agents, track progress.

## Available plans

!`ls context/plans/ready/ 2>/dev/null || echo "(none)"`

## Process

### 1. Load the plan

Read these context library files first:
- `context/lib/index.md` — agent router, architectural principles
- `context/lib/development_guide.md` — conventions, constraints, coding standards
- `context/lib/testing_guide.md` — what to test, test patterns

Then read `context/plans/ready/$ARGUMENTS/index.md`. If missing, list available plans and ask which to run.

Understand:
- Goal section (every agent needs this)
- Each task's description and acceptance criteria
- Sequencing: phases, concurrency, and dependencies

### 2. Prepare main and start feature branch

Start on `main`. Switch if needed. Sync with remote. Move the requested plan. Commit and push that move on `main` before creating the feature branch.

```bash
git switch main
git pull --ff-only
git mv context/plans/ready/<plan-name> context/plans/in-progress/<plan-name>
git commit -m "Move <plan-name> to in-progress"
git push origin main
git switch -c feature/<plan-name>
```

Use `feature/<plan-name>` as the integration branch for all implementation work.

### 3. Execute phases in order

For each phase in the sequencing section:

**Sizing.** Set `model:` on every Agent call. Omitting it inherits the session model — Opus spent on plumbing, or contract work handed to Sonnet.

**Opus** when the task touches:
- GPU contracts, shader layouts, bind groups, renderer scheduling
- Persistent formats, cache keys, PRL sections, migration behavior
- Offset-sensitive layouts: wire headers, byte builders, std140 mirrors, shader structs, cache payloads
- Cross-crate data flow or shared runtime contracts
- Producer/consumer seams across loader, compiler, renderer, shader, or diagnostics
- Ambiguous acceptance criteria; design choices resolved by reading code
- Manual visual behavior automated tests can't prove

**Sonnet** for:
- Localized implementation with a clear module home
- Focused tests for already specified behavior
- Loader/exposure plumbing for an already defined section
- Mechanical propagation across call sites
- Small review fixes, low blast radius

**Split on contract risk, not task size.** Settled contract → Sonnet, whatever the size. The contract itself is the work → Opus. Current Sonnet lands localized features and tests near Opus quality; Opus on a bounded task costs more and widens scope past the acceptance criteria. Haiku is not an implementation agent here.

**One local contract is the Sonnet boundary.** Another crate, runtime stage, shader, cache, or diagnostic path consuming the output means Opus — or split the task so the seam is its own Opus task.

**Effort is session-wide.** Not an Agent parameter; model is the only per-agent lever. A task needing more depth than the session affords gets split, not dialed up.

**Briefing Opus on layout and contract tasks.** Name what stays fixed: offsets, bindings, versions, cache epochs, mirror structs. Require offset/layout assertions where layouts are hand-mirrored or stale input must be rejected. Don't ask it to double-check its work — that buys over-verification, not coverage.

**Sequential:** One agent at a time. Wait for completion before starting the next.

**Concurrent:** Spawn all phase agents simultaneously via multiple Agent tool calls in one message.

> **Cargo under concurrency.** Run concurrent agents in isolated worktrees — separate `target/` dirs, cap 3 (see `development_guide.md`). Separate target dirs have no shared build lock, so each agent runs `cargo check` and focused tests freely. Agents sharing one `target/` must not: they serialize on cargo's build lock and churn the incremental cache. Defer their compile/test to one post-phase pass, as `/fix-review-findings` does.

Create agent worktrees from `feature/<plan-name>`. Integrate completed work back there.

**For each agent, provide:**
1. The plan's **Goal** section (one orienting paragraph)
2. The plan's **Invariants** table, when present — cross-task contract; break no row
3. The agent's **specific task** — description, acceptance criteria
4. Instruction to read relevant `context/lib/` files for architectural guidance
5. Instruction to follow `context/lib/development_guide.md` conventions
6. Instruction to run `cargo check` before considering the task complete (isolated worktrees only — see note above)
7. Instruction to run **focused** tests for the touched crate/module/behavior — not a full workspace or full-crate `cargo test` (concurrent agents: isolated worktrees only). Full-suite runs are the coordinator's final gate, not a per-task step. Prefer `cargo test -p <crate> <name_filter>`, narrowed to one target to skip the `tests/` suite — `--lib` for a library crate, `--bin <name>` for a binary one (`--bin prl-build` for `postretro-level-compiler`, whose compiler internals are not in its lib). Require agents to report the test count: a target/filter pair matching nothing prints `0 passed` and exits `ok`. WARN agents: `postretro-level-compiler`'s cold SH/lightmap bakes are `#[ignore]`-gated, so a bare `cargo test -p postretro-level-compiler` is cheap — but never add `-- --ignored` (~5–7 min), and never compile `stress-warren*`/`campaign-test` in a routine test.

**Do NOT provide:**
- Other tasks' details (the agent doesn't need them)
- The full plan document (wastes context)
- Freedom to expand scope beyond acceptance criteria

### 4. Integrate results

After each phase:
- Review what agents produced
- Verify acceptance criteria are met
- If a task completed partially or blocked, surface to the user with context
- If using worktrees, merge completed work back to `feature/<plan-name>`

Between phases, check that prerequisites for the next phase are satisfied.

### 5. Complete

When all phases are done:
- Run the full preflight **once after integration** (this is the coordinator's single full-suite gate): `cargo fmt --check && cargo clippy --target-dir target/preflight-clippy -- -D warnings && cargo test`. (Clippy uses its own target dir so it doesn't invalidate the warm dev/test cache — see the `preflight` skill.) If the session touched the bake/compile pipeline or its fixtures, ALSO run the on-demand cold-bake coverage as part of this one-time gate: `CARGO_PROFILE_TEST_SPLIT_DEBUGINFO=off cargo test -p postretro-level-compiler -- --ignored` (these are `#[ignore]`-gated because they cost ~5–7 min; the gate is where that cost belongs, not per-task).
- Run a `/review-panel` on code edited in this session
- Report review panel findings to user to discuss which feedback to act on

### 6. Landing the plane

When the user says "land the plane":
- Move the plan to done: `git mv context/plans/in-progress/<plan-name> context/plans/done/<plan-name>`
- If the plan is an item on `roadmap.md`, mark it as done
- Remove session worktrees, their dedicated target dirs, and session-owned temporary files
- Reclaim integration target space: run `cargo clean -p <crate>` for crates with significant session churn. Never run bare `cargo clean`.
- Commit & push `feature/<plan-name>`

### Error handling

- **Agent fails a task:** Surface the error and acceptance criteria to the user. Ask whether to retry, skip, or abort.
- **Merge conflict from concurrent agents:** Resolve if straightforward; escalate to user if the conflict involves architectural decisions.
- **Preflight fails:** Fix if the issue is mechanical (formatting, simple clippy lint). Escalate if the fix requires design decisions.

### Principles

- **You coordinate, you don't produce.** Every tool call spent building is context not spent orchestrating.
- **Guard context.** Each agent gets minimum viable context for their task.
- **3 of 4 completing is enough.** Partial progress with clear status beats blocking on one stuck task.
- **Surface, don't guess.** Tell the user when something unexpected happens. Don't make architectural decisions on their behalf.

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

**Sizing: pick a model per task.** Pass `model: "opus"` or `model: "sonnet"` explicitly on every Agent call. Omitting it inherits the session model, which spends Opus on plumbing and starves contract work when the session is on Sonnet. Model is the only per-agent capability lever — reasoning effort is a session-wide setting, not an Agent parameter, so a task that needs more depth than the session affords must be split, not dialed up.

Use **Opus** when the task touches any of:
- GPU contracts, shader layouts, bind groups, or renderer scheduling
- Persistent formats, cache keys, PRL sections, or migration behavior
- Offset-sensitive layouts: wire headers, byte builders, std140 mirrors, shader structs, cache payloads
- Cross-crate data flow or shared runtime contracts
- Producer/consumer seams split across loader, compiler, renderer, shader, or diagnostics code
- Ambiguous acceptance criteria, or design choices that must be resolved by reading code
- Manual visual behavior that automated tests cannot fully prove

Use **Sonnet** for:
- Localized implementation with a clear module home
- Focused tests for an already specified behavior
- Loader/exposure plumbing for an already defined section
- Mechanical propagation across call sites
- Small review fixes with low blast radius

**Sonnet is a first-class implementation agent, not a fix-up agent.** It lands localized features and tests at close to Opus quality, so the split is about *contract risk*, not task size: send Sonnet anything whose contract is already settled, and reserve Opus for tasks where the contract itself is the work. Opus is not the safe default — on a bounded task it costs more and is likelier to widen scope past the acceptance criteria.

**One local contract is the Sonnet boundary.** If another crate, runtime stage, shader, cache, or diagnostic path consumes the output, escalate to Opus or split the task so the seam becomes its own Opus task. Don't use Haiku for implementation in this workspace.

**Opus briefing for layout and contract tasks.** Name what stays fixed: unchanged offsets, bindings, versions, cache epochs, and mirror structs. Require offset/layout assertions when layouts are hand-mirrored or stale input must be rejected. Opus verifies its own work unprompted — don't add "double-check your work" instructions, they buy over-verification, not coverage.

**Sequential:** One agent at a time. Wait for completion before starting the next.

**Concurrent:** Spawn all phase agents simultaneously via multiple Agent tool calls in one message, choosing the model per task from the guide above.

> **Cargo under concurrency.** Run concurrent agents in isolated worktrees — separate `target/` dirs, cap 3 (see `development_guide.md`). Separate target dirs have no shared build lock, so each agent runs `cargo check` and focused tests freely. Agents sharing one `target/` must not: they serialize on cargo's build lock and churn the incremental cache. Defer their compile/test to one post-phase pass, as `/fix-review-findings` does.

Create agent worktrees from `feature/<plan-name>`. Integrate completed work back there.

**For each agent, provide:**
1. The plan's **Goal** section (one orienting paragraph)
2. The plan's **Invariants** table, when present — cross-task contract; break no row
3. The agent's **specific task** — description, acceptance criteria
4. Instruction to read relevant `context/lib/` files for architectural guidance
5. Instruction to follow `context/lib/development_guide.md` conventions
6. Instruction to run `cargo check` before considering the task complete (isolated worktrees only — see note above)
7. Instruction to run **focused** tests for the touched crate/module/behavior — not a full workspace or full-crate `cargo test` (concurrent agents: isolated worktrees only). Full-suite runs are the coordinator's final gate, not a per-task step. Prefer `cargo test -p <crate> <name_filter>` (`--lib` skips integration tests). WARN agents: the `postretro-level-compiler` `tests/` integration suite shells out to `prl-build` for cold SH/lightmap bakes (~1h) — never run a bare `cargo test -p postretro-level-compiler`, and never compile `stress-warren*`/`campaign-test` in a routine test.

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
- Run the full preflight **once after integration** (this is the coordinator's single full-suite gate): `cargo fmt --check && cargo clippy --target-dir target/preflight-clippy -- -D warnings && cargo test`. (Clippy uses its own target dir so it doesn't invalidate the warm dev/test cache — see the `preflight` skill.) If the session touched the bake/compile pipeline or its fixtures, ALSO run the on-demand cold-bake coverage as part of this one-time gate: `CARGO_PROFILE_TEST_SPLIT_DEBUGINFO=off cargo test -p postretro-level-compiler -- --ignored` (these are `#[ignore]`-gated because they cost ~1h; the gate is where that cost belongs, not per-task).
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

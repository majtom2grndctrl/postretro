# Completion notes

## Task 6 — extension-openness verification (2026-07-29)

An isolated `e16-openness-check` worktree based on the Task 5 integration head
temporarily added `WieldableState::PlaceholderTimed`. The deliberate compiler
failures were exactly the four state-decision sites:

1. `crates/entities/src/components/wieldable_state.rs:22` — `allows_fire`
2. `crates/entities/src/components/wieldable_state.rs:30` — `allows_reload`
3. `crates/entities/src/components/wieldable_state.rs:38` — `is_reload_activity`
4. `crates/postretro/src/sim/weapon_stage.rs:164` — `transition_wieldable_state`

The first `cargo check -p postretro-entities -p postretro` stops at the three
predicate errors because `postretro` depends on `postretro-entities`. Temporary
placeholder predicate arms in that same throwaway worktree allowed a second
`cargo check -p postretro` to expose the one downstream transition error. No
other production match site, timer field, or `ReloadOutcome::event_name` change
was required. The placeholder arm and worktree are discarded; neither is part of
the implementation branch.

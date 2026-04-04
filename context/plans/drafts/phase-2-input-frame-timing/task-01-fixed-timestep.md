# Task 01: Fixed-Timestep Loop

> **Dependencies:** none
> **Related:** `context/lib/rendering_pipeline.md` §1 (frame structure) · `context/lib/development_guide.md` §4.3 (frame ordering)

---

## Goal

Replace the current per-frame update with an accumulator-based fixed-timestep loop. Game logic ticks at a constant rate regardless of render framerate. Renderer interpolates between the two most recent game states for smooth visuals.

---

## Implementation Guidance

### Accumulator pattern

1. Each frame, add elapsed wall-clock time to the accumulator.
2. While the accumulator holds at least one tick's worth of time, run a game logic tick and subtract the tick duration.
3. After all ticks, compute the interpolation factor: `alpha = accumulator / tick_duration`.
4. Renderer uses alpha to interpolate between the previous and current game state.

### Tick rate

60 Hz (16.667ms per tick). Matches common display rates, keeps physics stable. Per-tick cost is negligible for Phase 2 (camera only, no entities).

### Edge cases

| Case | Behavior |
|------|----------|
| First frame | Only one game state exists. Duplicate it so interpolation produces the initial state with no blending artifact. |
| Long stall (alt-tab, disk I/O) | Clamp accumulator to 250ms max before ticking. Prevents dozens of catch-up ticks that freeze the frame. |
| Zero-time frame | Skip ticking, render with previous alpha. Can occur on some platforms. |

### State storage

Maintain two copies of interpolable state (camera position and orientation). After each tick, swap current into previous, write new current. Renderer reads both and lerps by alpha.

The interpolable state struct should be generic enough to extend in later phases (entity positions, etc.), but for now it only holds camera position and orientation.

### Integration point

The fixed-timestep loop wraps the game logic update call. The call signature should accept an action snapshot (built in Task 02) and produce updated game state. For this task, stub the action snapshot as an empty struct or use the existing raw input -- Task 06 wires the real action snapshot into the loop.

---

## Key Decisions

| Topic | Decision |
|-------|----------|
| Tick rate | 60 Hz. Re-evaluate at Phase 7 if collision needs higher fidelity. |
| Accumulator clamp | 250ms max. Matches `rendering_pipeline.md` §1. |
| Interpolation scope | Camera position and orientation only. Extend struct in later phases. |

---

## Acceptance Criteria

1. Game logic runs at a fixed 60 Hz tick rate regardless of render framerate.
2. Renderer interpolates between the last two game states using the alpha factor. No visible stuttering at render rates that aren't multiples of 60 Hz.
3. First frame renders correctly with no blending artifact.
4. Long stalls (simulate with a sleep or breakpoint) do not cause camera teleportation -- accumulator clamp limits catch-up ticks.
5. Zero-time frames do not crash or produce NaN values.

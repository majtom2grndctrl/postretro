# Task 05: Default Bindings

> **Dependencies:** Task 02 (input core types and binding structure exist)
> **Related:** `context/lib/input.md` §2 (action mapping)

---

## Goal

Define the complete default binding set for keyboard/mouse and gamepad. Bindings are hardcoded constants -- runtime rebinding is out of scope for Phase 2. All known actions get bindings even if nothing consumes them yet.

---

## Implementation Guidance

### Keyboard/mouse bindings

| Action | Type | Bindings |
|--------|------|----------|
| Move forward/back | Axis | W (+1), S (-1) |
| Move left/right | Axis | A (-1), D (+1) |
| Look yaw | Axis | Mouse X delta |
| Look pitch | Axis | Mouse Y delta |
| Jump | Button | Space |
| Use / interact | Button | E |
| Shoot | Button | Left mouse button |
| Alt-fire | Button | Right mouse button |
| Reload | Button | R |

### Gamepad bindings

| Action | Type | Bindings |
|--------|------|----------|
| Move forward/back | Axis | Left stick Y |
| Move left/right | Axis | Left stick X |
| Look yaw | Axis | Right stick X |
| Look pitch | Axis | Right stick Y |
| Jump | Button | A / Cross |
| Use / interact | Button | X / Square |
| Shoot | Button | Right trigger (threshold 0.5) |
| Alt-fire | Button | Left trigger (threshold 0.5) |
| Reload | Button | Y / Triangle |

### Structure

Bindings should be defined as a constant array or static data structure that the input subsystem loads at initialization. The binding type from Task 02 specifies the format. Each entry maps one physical input to one action ID with an optional scale factor (for axis direction: W = +1, S = -1).

Actions beyond what Phase 2 uses (Shoot, Alt-fire, Reload, Use) are defined now so the binding table is complete. They produce valid action state but nothing consumes them until later phases.

---

## Key Decisions

| Topic | Decision |
|-------|----------|
| Storage | Hardcoded constants. Config file is follow-up work. |
| Completeness | All known actions get bindings now. |
| Axis direction | Scale factor on binding (W = +1, S = -1) rather than separate positive/negative action IDs. |

---

## Acceptance Criteria

1. All actions in the Action ID enum have at least one keyboard/mouse binding and one gamepad binding.
2. Axis bindings include correct direction (positive/negative scale).
3. Bindings are defined as data (constants), not scattered through code.
4. Input subsystem loads bindings at initialization and uses them for resolution.

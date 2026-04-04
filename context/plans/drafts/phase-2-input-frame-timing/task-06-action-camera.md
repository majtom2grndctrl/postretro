# Task 06: Action-Driven Camera

> **Dependencies:** Task 01 (fixed-timestep loop), Task 02 (input core types), Task 03 (mouse handling), Task 04 (gamepad), Task 05 (default bindings)
> **Related:** `context/lib/rendering_pipeline.md` §1 (frame structure)

---

## Goal

Replace the Phase 1 raw winit camera with one that reads from the action snapshot. Same free-fly camera behavior, different input source. Camera updates run inside the fixed-timestep loop.

---

## Implementation Guidance

### Action consumption

Camera reads these actions from the snapshot each tick:

| Action | Use |
|--------|-----|
| MoveForward | Forward/back movement along camera facing |
| MoveRight | Strafe left/right |
| LookYaw | Horizontal rotation |
| LookPitch | Vertical rotation |

### Movement

Direction is relative to camera yaw: forward is the camera facing projected onto the XZ plane. Speed is 320 units/sec (from Phase 1 defaults, `rendering_pipeline.md` §9).

Still a free-fly camera -- no gravity, no collision. Vertical movement comes from look direction, same as Phase 1. The difference from Phase 1 is the input source, not the movement model.

### Look

Yaw and pitch applied from axis values. Pitch clamped to +/-89 degrees. Roll always zero.

### Mouse vs. gamepad look

Mouse and gamepad look feel different because they represent different physical actions:

| Source | Model | What the axis value means |
|--------|-------|--------------------------|
| Mouse | Displacement | Axis value = rotation amount (radians). Apply directly. |
| Gamepad | Velocity | Axis value = stick deflection = rotation speed. Multiply by gamepad look sensitivity and tick delta to get rotation amount. |

The action snapshot provides raw axis values. The camera must distinguish the source to apply the correct model. Options: tag axis values with their source in the snapshot, or use separate actions for mouse look vs. gamepad look. Use judgment on which is cleaner.

### Gamepad look defaults

| Parameter | Default | Notes |
|-----------|---------|-------|
| Gamepad look sensitivity | 2.5 rad/sec at full deflection | Comfortable turn speed. Adjust during testing. |
| Gamepad invert Y | false | Matches mouse default. |

### Interpolation

Camera position and orientation are the interpolable state from Task 01. Each tick writes new values to the current state. Renderer lerps position (linear) and orientation (slerp for quaternions, or lerp yaw/pitch angles) using the alpha factor.

---

## Key Decisions

| Topic | Decision |
|-------|----------|
| Movement speed | 320 units/sec. Carried from Phase 1. |
| Pitch clamp | +/-89 degrees. Prevents gimbal lock at poles. |
| Gamepad look sensitivity | 2.5 rad/sec. Needs testing. |
| Look model | Displacement for mouse, velocity for gamepad. |

---

## Acceptance Criteria

1. Camera reads movement and look from the action snapshot, not raw winit events.
2. Keyboard WASD moves the camera at 320 units/sec relative to facing direction.
3. Mouse look rotates camera with sensitivity applied. Invert-Y negates pitch.
4. Gamepad sticks drive movement and look. Look is velocity-based (deflection = rotation speed).
5. Camera updates run inside the fixed-timestep loop. Position and orientation interpolate smoothly between ticks.
6. Pitch is clamped to +/-89 degrees. No gimbal lock or flip at extremes.

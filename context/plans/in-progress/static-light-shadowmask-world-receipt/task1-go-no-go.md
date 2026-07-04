# Task 1 Go/No-Go Note

## Status

Decision: **go**. Prototype is implemented behind a diagnostics mode and passed visual verification on `combat-demo.map`.

Observed result: shadow-map PCF receipt is visible when `Shadowmask prototype` is enabled, and no shadow appears for a light whose intensity was intentionally too low to enter the selected/promoted set. User verification reported no blocker for continuing.

## Prototype

- Shader hook: `SdfShadowMode::ShadowmaskPrototype` / diagnostics label `Shadowmask prototype`.
- Normal rendering is unchanged unless that diagnostics mode is selected.
- The spike consumes promoted static lights from the existing `lights[light_count..total_light_count]` tail.
- It matches each promoted `GpuLight` back to `spec_lights` by position, range, and type.
- It derives `w` from the promoted tail color divided by the unweighted `spec_lights` color, because the parent path premultiplies promoted `GpuLight` color by `w`.

Matched reconstruction used by the prototype:

```text
L = normalize(light_position - world_position)
atten = max(1 - distance / range, 0)
cone = smoothstep(cos_outer, cos_inner, dot(-L, cone_axis))
direct_mesh = light_color_intensity * atten * cone * max(dot(mesh_normal, L), 0)
scale = min(max(dot(bump_normal, L), 0) / max(max(dot(mesh_normal, L), 0), 0.01), 4)
per_light_direct = direct_mesh * scale
baked_vis = saturate(luminance(lightmap_irradiance) / luminance(direct_mesh))
subtraction = per_light_direct * max(0, baked_vis - shadow_map_vis) * w
```

Fixture limits:

- `baked_vis` reconstruction assumes the tested receiver is dominated by one selected static light. That matches the Task 1 gate intent but is not a production mask substitute.
- `spec_lights` currently stores range but not falloff-model discriminant. The prototype uses the existing static-spec-light linear attenuation path.

Ramp-bias parameter under test:

- Promoted spot slots use a widened receiver-side PCF kernel in the prototype: 5x5 taps, 2.0 shadow-map texel spacing.
- Point/cube slots keep the existing cube PCF.

## Manual Gate Checklist

Map: `content/dev/maps/combat-demo.map`

Entity present:

- A moving enemy under a promoted static light darkens world geometry with an entity-shaped region.
- No visible ringing or directional artifact appears from the bumped-Lambert reconstruction.
- Where the runtime entity shadow crosses a baked shadow, the result does not double-darken.

Entity absent:

- With `Shadowmask prototype` active and no enemy under the promoted light, baked soft penumbrae show no perceptible net change.
- If the penumbra hardens or darkens, Task 1 is not green; tune the ramp-bias or record the failure mode in the plan.

## Screenshots

No screenshots produced by this agent.

## Verification Commands

- `cargo test -p postretro-render-cpu sdf_shadow_mode_round_trips_through_uniform --lib`
- `cargo test -p postretro-renderer forward_shader --lib`
- `cargo check`
- `cargo check -p postretro-renderer --features dev-tools`

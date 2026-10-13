# fluid-surface-motion — research

Read at e8f607b94. Findings that inform the brief without deciding it.

## Why the water looks smooth
- `fluid_warp_uv` adds `0.125 * sin(uv.yx * 8 + t)` per fragment. The offset varies continuously across screen pixels, so the nearest-texel lookup in `sample_post_retro` lands on a texel boundary that curves through the face.
- `sample_post_retro` is nearest-texel sampling with an anti-aliased band about one screen pixel wide at each seam (`floor(uv*dims + 0.5)` plus a clamped linear blend). Its seam width and mip come from the unwarped derivatives.
- Pinning: `fluid_post_retro_sampling_matches_the_mover_shader` compares the fluid and kinematic bodies textually. `kinematic_brush_shader_matches_forward_post_retro_sampling` compares only a call fingerprint against forward.

## Texel density
- Face UVs are in texels: `brush_side_texel_uv`, where 1 map unit is 0.0254 m. At face scale 1.0 that is about 39 texels/m.
- UVs are normalized by the texture's own size at install (`pack_fluid_surfaces` for fluids).
- No project-, mod- or map-level texel density exists. The only worldspawn density key is `_lightmap_density`.
- `fog_pixel_scale` is the precedent for a map-owned render parameter, and it needed its own PRL section field. That cost is one reason the brief keeps the grid on the texture.

## Currents
- `resolve_environment_volume` emits exactly one of gravity, push or fluid per entity. The FGD documents a current as a fluid volume with a coincident push volume.
- `EnvironmentVolumeRecord.push` is m/s² in engine Y-up, and is `None` on every fluid record.
- `resolve_environment` resolves push at a body's position and fluid along its capsule. Push wins by priority, then the smaller volume, then map order.
- Push volumes are part of `level_content_digest`, are never script-toggled, and are not replicated per volume. Only the player's drift layer is predicted.

## Dev content
- Fluids declared in `content/dev/start-script.ts` and `scripts/fluids-swim.ts` (with its Luau mirror): `water` and `sludge`. No lava or slime fluid exists yet.
- Maps naming them: `maps/stress-env-volumes.map`, `maps/e24-environment-test.map`. The current-vs-still manual capture needs a pool with a coincident push volume in one of these maps, or a new fixture.

## Review facts (detail review, round 1)
- **Viscosity-only changes skip the rebuild today.** `refresh_environment_fluids` replaces the table but returns before any volume rebuild when only a viscosity changed (startup/environment_install.rs). It is the single path for host reloads, client tuning installs and demotions.
- **Terminal velocity is player tuning, not level data.** It is the movement descriptor's `fall.terminal_velocity` (physics `movement/push.rs`).
- **Movement settles at push × dt above the tick rate.** When viscosity × dt ≥ 1, decay clamps drift to zero before the tick's push accrues. That is why drift shares movement's steady-state function rather than restating push ÷ viscosity.
- **Points on a face count as inside.** A point on a brush face is inside it (collision `leaf.rs`), which is why drift samples just inside the fluid.
- **Player and particle push share one acceleration index.** See collision `resolve.rs` and `acceleration.rs`. Force therefore has to be carried apart from the nominal push.
- **The digest misses sibling sections.** Its unhashed-field guard destructures `EnvironmentVolumeRecord` (`runtime_movers.rs`), so a force stored in a sibling section escapes it. Only the digest row catches a missed hash.
- **Section id 53.** A registry test asserts it is unassigned today (`section_registry.rs`).
- **Neither precedent multiplier accepts 0.** `light_range` warns and clamps values ≤ 0. `_lightmap_scale` fails the build on them.

## Pinned orderings

| Id | Scenario | Ordering | Expected outcome |
|---|---|---|---|
| R1 | A fluid's viscosity changes mid-level with names unchanged: a host manifest reload, or a client's new tuning payload | Lands after the level and its drift are installed; the table refresh returns before any volume rebuild | Drift re-resolves on that change. The next frame draws the new steady-state speed, on host and client |
| R2 | A client receives the host's tuning (a) before its level install, (b) after it, (c) again after a demotion | (b): level and renderer install while the table is empty, then the tuning refresh runs. (c): demotion empties the table, then new tuning arrives | (a) and (b) draw identical drift. While the host table is absent no fluid surface draws. After (c) the surface returns with the new table's drift |
| R3 | A push brush meets a fluid face from outside: a wind's bottom on the waterline, or a stream's push abutting a pool's glass-side face | The face lies on the push brush's boundary plane, and containment counts that as inside | The face samples just inside the fluid, so it does not drift with a push no swimmer feels |
| R4 | Two push brushes each cover part of one fluid face and overlap in its middle, at different priority | The face is cut by every push brush over it, not only the winner's | Each part lies under one constant set of pushes and drifts with its winner. The overlap takes the higher priority |
| R5 | A push brush's edge coincides with or grazes a fluid face's edge or corner, or overlaps it by under 1 mm² | The push cut runs after solid and sibling subtraction | A touch leaves the face whole and still. No part under 1 mm² is kept. Parts tile the face: their areas sum to the unsplit face's within 1 mm² |
| R6 | A split leaves a part as thin as the minimum kept piece | Drift projects to UV per part at install | Every part of one face under one push drifts identically. No drift comes from a sliver's ill-conditioned UV triangle |
| R7 | A ring centred on one cell spans neighbouring cells with different phases | The ring steps on its centre cell's boundary while each covered cell's shimmer steps on its own | A cell's displayed offset still changes at most once per its own step period, no faster than 3 Hz |
| R8 | Cells at negative texel coordinates | Cell index from the floor of the texel UV, then hashed | Each negative cell gets its own phase. Cells −1 and 0 differ. No block of negative cells steps on one frame |
| R9 | One level runs 24 h, so script time reaches 86 400 s as f32 | The drift offset is time × speed, unbounded | The scroll stays whole-texel and within one texel per second of true speed. The warped UV stays within one texture period of the unwarped UV |
| R10 | The drift offset wraps by one texture period | The wrap lands on a frame while shimmer and rings key on the fixed grid | Cells sit on the fixed face grid, not the scrolled one, so the wrap changes no cell's state |
| R11 | Force 0 and 0.5 on an updraft, a wind and a current | Movement reads push at lift-off, drift accrual, the terminal clamp and the fluid-jump check | Every movement read sees push × force. A force-0 updraft does not lift a grounded player. Particles still read the nominal push |
| R12 | A map built before this change, with a push over part of a fluid face, loads in the new engine | Its surface faces carry no split | It loads. The face drifts whole, or stays still, by its sample point's push until rebuilt |
| R13 | Fluid viscosity at or above the tick rate (viscosity × dt ≥ 1) | Decay clamps drift before the tick's push accrues | Drift uses movement's own steady-state function, so the surface matches what a swimmer settles at |
| R14 | Drift re-resolves mid-level on a frame that also draws pools | The re-resolve lands before that frame's draw collection | Every visible pool draws at its opacity. A frame with unchanged opacities uploads no parameter rows |
| R15 | A map with push volumes and no fluid, with and without an authored force | The split runs only for fluid faces | No surface faces. Environment-volume bytes unchanged. A force section appears only when a force other than 1 is authored |

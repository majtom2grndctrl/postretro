# Research — SDF spotlight cone shadows

Background that informed the spec. Decisions live in `index.md`; this is the
investigation, kept out of the spec per the style guide.

## Code grounding (current state)

- The SDF occlusion trace is already spot-correct: `trace_light_visibility`
  (sdf_shadow.wgsl) marches toward the light **position**; occlusion is geometry,
  independent of any cone. The gap is purely cone *shaping*.
- `SpecLight` (lighting/spec_buffer.rs, `SPEC_LIGHT_SIZE = 32`) carries only
  `position_and_range` and `color_and_sdf_flag`. `pack_spec_lights` reads `MapLight`
  but drops `cone_direction` / `cone_angle_inner` / `cone_angle_outer` / `light_type`
  (all present on `MapLight`, prl.rs:154). So static spots shade omni in:
  - the SDF diffuse loop (forward.wgsl ~731),
  - the static specular loop (forward.wgsl ~780) — this is the latent
    `static_light_map`-spot specular-spill bug,
  - the selection influence metric (`sdf_select_influence`, sdf_light_select.wgsl).
- `cone_attenuation` exists twice (forward.wgsl:329, billboard.wgsl:241), both taking
  raw angles and calling `cos()` inline — fine for the ≤12 dynamic loop, wasteful in the
  all-chunk-lights static specular loop. Motivates precomputed cosines + a cosine-taking
  shared helper.
- Storage-buffer record ⇒ widening needs only the WGSL struct decls (forward.wgsl,
  sdf_shadow.wgsl), the packer, and `SPEC_LIGHT_SIZE` — no BGL stride change.
- Selection parity seam: shared `sdf_light_select.wgsl` is concatenated into both the
  forward and the SDF pass; the host comparator `render/sdf_light_select_test.rs` is the
  durable record of selection order.

## External findings (web research, cited)

- **Penumbra softness is light *source radius*, not cone angle — keep them decoupled.**
  Quilez ("Soft Shadows in Raymarched SDFs", iquilezles.org/articles/rmshadows) ties the
  softness `k` to the inverse of source size. UE5 distance-field soft shadows use Source
  Radius for penumbra on both point and spot lights; the cone smoothstep is a separate
  concern. ⇒ Reserve a per-light source-radius slot, but drive penumbra from global
  `penumbra_k` in v1; never fold cone angle into softness.
- **Apply cone falloff and shadow visibility as independent multiplicative terms**, cone
  at full res in the forward pass (a cheap dot), not baked into the half-res slice — else
  the soft cone edge degrades through the bilinear upsample. Matches our existing
  occlusion-only slice. (UE5 spot lights; LearnOpenGL light-casters smoothstep.)
- **Cone-aware selection should weight by the continuous smoothstep value, not a binary
  gate.** A hard cone boundary flips neighboring fragments' top-K sets → spatial/temporal
  flicker. (Synthesized from continuity reasoning; no single production write-up found —
  flagged as the one uncited inference. Sphere-vs-cone math: Wronski, "Cull That Cone".)
- **Cone geometry gives free early-out / shorter rays:** reject before tracing when
  `dot(toFrag, axis) < cos(outer)` or out of range, and clamp march length to the cone's
  range. (Wronski; graphicrants sparse-shadow tracing.)
- **Don't over-engineer:** tracing toward position is correct; sphere-trace (not
  cone-trace) avoids leak; the Aaltonen estimator + surface bias we already use is the
  blessed improvement; DF shadows have few self-intersection problems, so resist
  shadow-map-style bias machinery; tight analytical spot-frustum culling is not worth it
  (Granite abandoned it).

## Hardware framing

Perf floor: NVIDIA GTX 16-series (Turing) — healthy headroom, so the gated SDF-spot path
is a confident bet and the 64 B record / precomputed-cosine choices are perf-neutral here.
AMD Radeon Pro 5500M-class (2020 16" MBP, Metal) is must-run, tuned via the live debug
panel — hence the cone-early-reject toggle is scoped as a gate-tuning instrument, not a
shipped control. Cone shaping adds no new device feature or texture format, so "must run
on AMD" is satisfied by construction (verify, don't engineer).

# M13 Goal SE — Post-UI Screen-Space Effects + Compositor Seam

> Wave plan 1 of 2 (sibling: **G2**, `drafts/M13--sdk-typesafety-a11y/`). Both
> ship in one /orchestrate; mutually independent except the typedef/barrel seam
> noted under Sequencing. Downstream: **BIS** consumes the effects (damage
> vignette); **M9 tonemap / M10 post** later extend the resolve SE establishes.
> Grounding: `research.md`, `ui-layer.md` §13, `lib/ui.md` §3. Prereqs: A–E
> shipped (`done/M13--*`).

## Goal

Establish the post-scene compositor seam and ride it with the first three
screen-space effects. Today every pass renders straight to the swapchain — no
shared target where flash/vignette/shake (or M9 tonemap, or M10 post) can
compose. SE introduces a `scene_color` offscreen target every scene/UI pass
renders into, plus a **resolve pass** that samples it into the swapchain while
applying flash, vignette, and screen shake from engine-owned slots. Goal E
shipped `screen.flash` (slot + reaction + decay) with no GPU consumer; SE is
that consumer and adds two sibling effects. The resolve is the durable seam M9
tonemap and M10 post extend — not a side-channel SE has to unwind later.

## Scope

### In scope

- **`scene_color` offscreen target + resolve pass.** A renderer-owned color
  target (surface format, surface size/sample-count) that the forward, skinned-
  mesh, billboard, fog-composite, wireframe, debug-line, and UI passes render
  into instead of the swapchain view. A new resolve pass samples `scene_color`
  and writes the swapchain — the sole swapchain writer after this change.
  **Visual-parity gate:** with no active effect, the resolve is an identity blit
  and output is byte-identical to today's direct-to-swapchain pipeline.
- **Three effects in the resolve.** Flash (color over-blend), vignette (edge
  tint/darken), screen shake (full-frame sample offset — correct now that the
  resolve reads `scene_color`, not the target it writes). Effect math is ALU on
  top of the identity blit; all-at-rest collapses to identity.
- **Two net-new engine-owned slots** beside `screen.flash`: `screen.vignette`
  (RGBA `Array` — rgb tint, a = strength) and `screen.shake` (`Array [dx, dy]`,
  current frame offset in logical-reference px). Engine-writable, mod-readonly.
- **Two decay drivers** mirroring `FlashDecay`: a vignette envelope and a shake
  driver (decaying oscillation → current offset). dt-accumulated game time;
  pause with game logic; clear on level unload.
- **Two reactions** — `vignette { color?, strength, durationMs }` and
  `screenShake { amplitude, durationMs, frequency? }` — as `SystemReactionCommand`
  variants + handlers (the `flashScreen` pattern), SDK constructors in
  `sdk/lib/ui/reactions.{ts,luau}`, typedef emission, docs. When `frequency` is
  omitted the shake driver defaults to **18 Hz**; when vignette `color` is omitted
  it defaults to **black** (pure edge-darken, strength-only).

### Out of scope

- **HDR / tonemap.** `scene_color` is the surface (LDR) format so output stays
  identical; an HDR upgrade + tonemap-in-the-resolve is M9's additive change.
- **M10 post / other resolve effects** — they extend this resolve when they
  land; SE ships only flash/vignette/shake.
- **Sustained / continuous effects** bound to a live value (low-health vignette
  that tracks `player.health`). v1 effects are one-shot reaction + decay, like
  flash; a continuous-bind model is deferred.
- **World-only shake** (HUD held stable). v1 shakes the whole composited frame
  (the literal post-UI screen-space reading).
- **Splitting `render/mod.rs`** — the retarget edits existing color-attachment
  sites across the large frame function; a split of that function is out of scope.

## Acceptance criteria

- [ ] All scene/UI passes render into `scene_color`; the resolve pass is the sole
  swapchain writer. With `screen.flash`/`screen.vignette`/`screen.shake` all at
  rest, output is byte-identical to the pre-SE pipeline — the foundation parity gate.
  (Depends on ALL re-point sites including Skinned Mesh `:5206` and Billboard
  `:5244`, `scene_color` at the sRGB surface format, and NEAREST resolve sampler.)
- [ ] A `flashScreen` reaction produces a full-screen color flash over scene +
  HUD decaying to transparent; `vignette` darkens/tints the edges to a peak then
  decays, center unaffected; `screenShake` offsets the whole composited image
  with a decaying oscillation that returns to exact center — shake samples
  `scene_color` and writes a different target (no read/write hazard). [Runnable:
  assert no read/write hazard. Manual verification: decay shape, return to exact
  center.]
- [ ] All three pause when game logic pauses (dt-accumulated time, never wall
  clock) and compose simultaneously in the single resolve.
- [ ] The pure pack fn `(UiReadSnapshot slot_values) -> EffectUniform` maps
  snapshot slot values into the resolve uniform (a test asserts the
  snapshot→uniform mapping). "Never the live slot table" is structural — the
  renderer holds no SlotTable/ScriptCtx handle — so the test asserts the mapping,
  not the absence of a table read.
- [ ] `screen.vignette`/`screen.shake` are engine-owned slots registered like
  `screen.flash`; a mod write no-ops (assertable, precedent `store.rs:1215`) and
  emits `log::warn!` (observable via log-capture harness — not required as a
  test assertion).
- [ ] `vignette`/`screenShake` exist in both TS + Luau SDK with emitted typedefs,
  dispatch through the same `SystemReactionCommand` path as `flashScreen`, and a
  descriptor without them keeps its pre-SE wire form byte-identical.
- [ ] The effect composes on top of the fog already composited into `scene_color`
  (samples the post-fog, post-UI image).
- [ ] Demo in the dev map: a low-health `onStateCrossing` fires flash + vignette +
  `screenShake` (all bound to the same crossing in
  `content/dev/scripts/arena-lights.ts`) — manual verification.
- [ ] `docs/scripting-reference.md` covers the two new reactions.

## Tasks

### Task 1: `scene_color` target + resolve pass (foundation)
Allocate a renderer-owned `scene_color` color texture (surface format/size/sample
count; resize with the surface). Re-point the color attachments of the scene and
UI passes (`render/mod.rs`: forward clear, Skinned Mesh Pass `:5206`, Billboard
Sprite Pass `:5244`, fog composite `:5317`, wireframe `:5352`, debug lines
`:5395`, UI `ui.encode :5497`) from the swapchain `view` to `scene_color`. In
practice: re-point every `view: &view` color attachment between the Textured Pass
(`:5114`) and the timing resolve (`:5511`) — the `view: &view` literals at
5117/5206/5244/5320/5355 plus the `debug_lines.render(…&view…)` (`:5395`) and
`ui.encode(…&view…)` (`:5497`) helper calls. `scene_color` MUST be allocated at
the surface format (sRGB — the surface picks `f.is_srgb()`, `render/mod.rs:1740`),
single-sample, and the resolve sampler MUST be NEAREST / pixel-aligned; this is
load-bearing for the byte-identical parity gate (per-pass sRGB-encode + 8-bit
quantize into `scene_color` must round-trip losslessly; fog_composite's dither
lands on that same 8-bit grid). SE's resolve is gameplay-path only; the splash
path (`render_splash_frame` / `record_splash_ui`) keeps writing the swapchain
directly. Add a `ScreenEffectsPass` (`render/screen_effects.rs`, new) +
`src/shaders/screen_effects.wgsl` (fullscreen-triangle, the `fog_composite`
precedent) that samples `scene_color` and writes `view`; encoded after
`ui.encode`, before timing resolve (`:5511`). Identity blit when no effect input is bound. Pre-Task-4: resolve uniform
zeroed/unbound → identity. Post-Task-4: at-rest slot values (transparent flash,
zero strength, zero shake) ALU-collapse to identity and MUST produce bit-identical
output to the unbound path so the parity gate holds across both. Parity gate:
byte-identical output. `Depends on` nothing.

### Task 2: effect slots + decay drivers
Register `screen.vignette` + `screen.shake` engine-owned slots beside
`screen.flash`; both register as mod-readonly / engine-writable exactly like
`screen.flash` (mod writes warn + no-op via the existing readonly slot path).
`screen.vignette` is `Array` RGBA (default `[0,0,0,0]`); `screen.shake` is
`Array [dx,dy]` (default `[0,0]`).
Add a vignette-envelope and a shake driver beside `flash_decay.rs`
(`scripting/systems/`), each with `start`/`tick`/`reset` entry points, started
by a drained command (Task 3), ticking at the game-logic stage beside
`FlashDecay.tick` on dt-accumulated time, writing its slot, clearing on level
unload. Factor shared envelope/lifecycle helpers if it reads cleanly. `Depends on`
nothing — concurrent with Task 1.

### Task 3: reactions + commands + SDK
Add `SystemReactionCommand::Vignette`/`::ScreenShake` beside `FlashScreen`;
register `vignette`/`screenShake` handlers (the `flashScreen:211` pattern)
pushing those commands; drain the drained `Vignette`/`ScreenShake` commands
to `driver.start()` — mirroring the `FlashScreen` → `FlashDecay.start` precedent
(App constructs the driver; `main.rs` ticks it; the drained command starts it).
SDK constructors in `sdk/lib/ui/reactions.{ts,luau}`, barrel export, typedef
emission, docs. `screenShake` args carry `frequency: Option<f32>` with
`#[serde(default)]`; the omitted-frequency 18 Hz default is applied by the shake
driver (Task 2), not the arg deserializer. `Depends on` Task 2. **Wave seam (G2):** coordinate
`scripting/typedef.rs` reaction-block + `sdk/lib/index.ts` barrel with G2;
regenerate once both land.

### Task 4: effects in the resolve + snapshot + demo
Pack the three effect slots from `UiReadSnapshot` into the resolve's uniform and
apply them in `screen_effects.wgsl` (shake = sample offset, vignette = edge
tint, flash = over-blend). The Rust pack step converts the shake dx/dy (logical-
reference px on a 1280×720 reference) to UV offsets before writing the uniform;
the shader applies a pure UV add (no dims needed in WGSL). Confirm the two new
slots flow through `build_ui_slot_snapshot`. The snapshot→uniform pack MUST be a pure fn `(UiReadSnapshot slot_values) ->
EffectUniform` so AC#4's assertion can run without a GPU/device; "never the live
slot table" is structural (the renderer holds no SlotTable/ScriptCtx handle), so
the test asserts the snapshot→uniform mapping (a divergence test), not the absence
of a table read. Add an assertion test proving the resolve reads effect slot values
from `UiReadSnapshot` via the pure pack fn.
Pre-Task-4 the resolve uniform is zeroed/unbound → identity; post-Task-4 the
at-rest case is resting slot values (transparent flash, zero strength, zero shake)
whose ALU collapses to identity — resting values MUST produce bit-identical output
to the unbound path so the parity gate holds across both. Demo: extend the
existing low-health crossing demo (Goal E's flash-demo site) to also fire
`vignette`, and bind `screenShake` to the same `onStateCrossing('player.health',
…)` in `content/dev/scripts/arena-lights.ts` (the `lowHealthFlash` reaction site)
— there is no entity-hit event seam a script can bind a reaction to; the trigger
surface is `onStateCrossing` + named events. Manual verification. `Depends on`
Tasks 1–3.

## Sequencing

**Phase 1 (concurrent):** Task 1 (renderer foundation), Task 2 (slots + decay) — disjoint files.
**Phase 2 (sequential):** Task 3 — consumes Task 2's driver entry points.
**Phase 3 (sequential):** Task 4 — consumes Tasks 1–3.
**Wave seam (G2):** Task 3 shares `scripting/typedef.rs` (reaction block) and
`sdk/lib/index.ts` (barrel) with G2's widget edits — different sections.
Coordinate; regenerate typedefs once both land. SE touches no
`render/ui/descriptor.rs` or tree-bridge code (G2-only).

## Rough sketch

- Target + resolve: `render/screen_effects.rs` (new, Renderer-owned) +
  `src/shaders/screen_effects.wgsl`; mirrors `FogPass.composite_pipeline` /
  `fog_composite.wgsl` (`draw(0..3, 0..1)`, no vertex buffer). Uniform `{ flash:
  vec4, vignette: vec4, shake: vec2, _pad }` packed from the snapshot.
- `scene_color`: surface format/size/sample-count; lives beside the depth target
  in the renderer, recreated on resize.
- Slots + decay: `scripting/systems/{vignette_decay,shake_decay}.rs` beside
  `flash_decay.rs`; reactions in `scripting/reactions/system_commands.rs`.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau |
|---|---|---|---|---|
| vignette reaction | `SystemReactionCommand::Vignette` | `"vignette"` | `vignette(args)` | `vignette(args)` |
| vignette args | `{ color, strength, duration_ms }` | `{ "color", "strength", "durationMs" }` | `{ color?, strength, durationMs }` | same |
| screenShake reaction | `SystemReactionCommand::ScreenShake` | `"screenShake"` | `screenShake(args)` | `screenShake(args)` |
| screenShake args | `{ amplitude, duration_ms, frequency: Option<f32> }` (`#[serde(default)]`) | `{ "amplitude", "durationMs", "frequency"? }` | `{ amplitude, durationMs, frequency? }` | same |
| vignette slot | `"screen.vignette"` (`Array` RGBA) | n/a | read-only bind name | same |
| shake slot | `"screen.shake"` (`Array [dx,dy]`) | n/a | read-only bind name | same |

## Decisions & deferrals

- **MSAA / format.** The surface is single-sample today (all pipelines use
  `MultisampleState::default()` count 1; depth `sample_count: 1`), so `scene_color`
  is single-sample at the surface (LDR) format and the resolve is a true identity
  blit — byte-identical parity is achievable. M9's HDR upgrade (swap format + add
  tonemap into the resolve) stays additive.
- **Idle cost (accepted).** Routing through `scene_color` adds one fullscreen
  resolve every frame even at rest — the standard cost of a compositor. The effect
  math is ALU on top; no extra pass when effects are active.
- **Cross-milestone coordination.** SE owns the resolve; M9 tonemap and M10 post
  must *extend* it (one resolve), not stack parallel resolves. Flag for M9/M10.
- **Sustained vignette deferred** (track a value while a condition holds) to a
  future continuous-effect model; v1 is one-shot decay.

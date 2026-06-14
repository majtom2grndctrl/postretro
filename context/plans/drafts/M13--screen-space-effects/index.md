# M13 Goal SE — Post-UI Screen-Space Effects

> Wave plan 1 of 2 (sibling: **G2**, `drafts/M13--sdk-typesafety-a11y/`). Both
> ship in one /orchestrate; mutually independent except the typedef/barrel seam
> noted under Sequencing. Downstream convergence: **BIS** (built-in screens)
> consumes SE's effects for the damage vignette. Grounding: `research.md`,
> `ui-layer.md` §13, `lib/ui.md` §3. Prereqs: A–E shipped (`done/M13--*`).

## Goal

Add the missing GPU consumer for engine-owned screen-effect slots: a single
post-UI pass that applies **flash**, **vignette**, and **screen shake** to the
composited frame, driven by slots a reaction sets and the engine decays. Goal E
shipped `screen.flash` (slot + `flashScreen` reaction + decay) but no pass draws
it; SE renders it and adds two sibling effects. Establishes the post-UI
screen-space resolve — the seam M9 tonemap / M10 post later share.

## Scope

### In scope

- A renderer-owned **screen-effects pass** drawn after the UI pass, before
  present, applying flash + vignette + shake to the final color target in one
  fullscreen draw. Skipped entirely when all three effect slots are at rest (no
  idle cost, no idle visual change).
- Two net-new engine-owned slots beside `screen.flash`: `screen.vignette`
  (RGBA `Array` — rgb tint, a = strength) and `screen.shake` (`Array` `[dx, dy]`
  — current frame offset in logical-reference px). Engine-writable, mod-readonly.
- Two net-new decay drivers mirroring `FlashDecay`: a vignette envelope
  (rise→hold→decay over `durationMs`) and a shake driver (decaying oscillation
  → current offset). dt-accumulated game time; pause with game logic; clear on
  level unload.
- Two net-new reactions — `vignette { color?, strength, durationMs }` and
  `screenShake { amplitude, durationMs, frequency? }` — as `SystemReactionCommand`
  variants + handlers (the `flashScreen` pattern), plus SDK constructors in
  `sdk/lib/ui/reactions.{ts,luau}` + typedef emission + docs.
- SE becomes the durable consumer of `screen.flash`; the effect composes on top
  of the M9 fog already present in the frame.

### Out of scope

- **Sustained / continuous effects** bound to a live value (e.g. a vignette
  that tracks `player.health` while low). v1 effects are one-shot reaction +
  decay, exactly like flash. A continuous-bind effect model is deferred.
- **World-only shake** (HUD held stable). v1 shakes the whole composited frame
  (scene + UI together) — the literal reading of a post-UI screen-space effect.
- New effect kinds beyond flash / vignette / shake (no chromatic aberration,
  scanlines, blur — those belong with the deferred post-processing milestone).
- HDR / tonemap / an offscreen scene-compositor target shared across passes —
  SE resolves into the existing swapchain target (see Open questions).
- Removing E's demo flash panel — incidental; SE's pass is the durable consumer
  and the demo may keep or drop its stand-in panel without affecting SE.

## Acceptance criteria

- [ ] With `screen.flash` / `screen.vignette` / `screen.shake` all at rest, the
  screen-effects pass is not encoded and the rendered frame is unchanged from
  the pre-SE pipeline (no regression, no idle cost).
- [ ] A `flashScreen` reaction produces a full-screen color flash over **both**
  scene and HUD that decays to transparent over `durationMs`; a `vignette`
  reaction darkens/tints the screen edges to a peak then decays, center
  unaffected; a `screenShake` reaction offsets the whole composited image with a
  decaying oscillation that returns to exact center (zero offset) at end.
- [ ] All three effects pause when game logic pauses (dt-accumulated time, never
  wall clock) and compose simultaneously in the single pass.
- [ ] The pass reads the three effect slots from the once-per-frame UI snapshot
  only — never the live slot table; a test asserts the consumed values come from
  `UiReadSnapshot`.
- [ ] `screen.vignette` and `screen.shake` are engine-owned slots registered the
  same way as `screen.flash`; a mod write to either warns and no-ops (the
  engine-owned-slot rule).
- [ ] `vignette` and `screenShake` exist in both TS and Luau SDK with emitted
  typedefs, dispatch through the same `SystemReactionCommand` path as
  `flashScreen`, and a descriptor without them keeps its pre-SE wire form
  byte-identical (round-trip test).
- [ ] The effect composes on top of the M9 fog already composited into the frame
  (the pass samples the post-fog, post-UI image).
- [ ] Demo in the dev map: an `onStateCrossing` at low health fires flash +
  vignette; an entity hit event fires `screenShake` — manual verification.
- [ ] `docs/scripting-reference.md` covers the two new reactions.

## Tasks

### Task 1: effect slots + decay drivers
Register `screen.vignette` (RGBA `Array`) and `screen.shake` (`Array [dx,dy]`)
as engine-owned slots beside `screen.flash`. Add two decay systems beside
`flash_decay.rs` (`scripting/systems/`): a vignette envelope and a shake driver
(owns frequency + amplitude → current decaying offset). Each is started by a
drained command (Task 2), ticks at the game-logic stage on dt-accumulated time,
writes its slot, and clears on level unload. Factor shared envelope/lifecycle
helpers if it reads cleanly — three near-identical decays is a known smell.
`Depends on` nothing.

### Task 2: reactions + commands + SDK
Add `SystemReactionCommand::Vignette { color, strength, duration_ms }` and
`::ScreenShake { amplitude, duration_ms, frequency }` beside `FlashScreen`;
register `vignette` / `screenShake` handlers (the `flashScreen:211` pattern)
pushing those commands; drain them to start Task 1's drivers. SDK constructors
in `sdk/lib/ui/reactions.{ts,luau}`, barrel export, typedef emission, docs.
`Depends on` Task 1. **Wave seam (G2):** coordinate `scripting/typedef.rs`
SDK-block and `sdk/lib/index.ts` barrel edits with G2 (different sections);
regenerate typedefs after both land.

### Task 3: screen-effects pass + shader
`ScreenEffectsPass` owned by `Renderer`, `screen_effects.wgsl` in `src/shaders/`
(fullscreen-triangle, the `fog_composite` precedent). Reads the three effect
slots from the UI snapshot into a uniform; one fullscreen draw applies shake
(sample offset), vignette (edge tint/darken), flash (color over-blend); encoded
after `ui.encode` (`render/mod.rs:5505`), before timing resolve (`:5511`).
Skips encoding when all three slots are at rest. Resample mechanism per Open
questions. `Depends on` nothing structurally (reads slots by name, absent =
at-rest) — concurrent with Task 1.

### Task 4: snapshot wiring + demo
Confirm the two new slots flow through `build_ui_slot_snapshot` into
`UiReadSnapshot` (registered slots propagate automatically — verify, don't
assume). Demo in the dev map: a low-health `onStateCrossing` firing flash +
vignette and an entity hit event firing `screenShake`. Manual verification.
`Depends on` Tasks 1–3.

## Sequencing

**Phase 1 (concurrent):** Task 1 (slots + decay, scripting/systems), Task 3
(pass + shader, renderer) — disjoint files.
**Phase 2 (sequential):** Task 2 — consumes Task 1's driver entry points.
**Phase 3 (sequential):** Task 4 — consumes Tasks 1–3.
**Wave seam (G2):** Task 2 shares `scripting/typedef.rs` (reaction block) and
`sdk/lib/index.ts` (barrel) with G2's widget-type edits — different sections.
Coordinate and regenerate typedefs once both land. SE touches no
`render/ui/descriptor.rs` or tree-bridge code (G2-only), so there is no
descriptor conflict.

## Rough sketch

- Pass + shader: `render/screen_effects.rs` (new, Renderer-owned) +
  `src/shaders/screen_effects.wgsl`; mirrors `FogPass.composite_pipeline` /
  `fog_composite.wgsl`. Uniform: `{ flash: vec4, vignette: vec4, shake: vec2,
  _pad }` packed from the snapshot.
- Slots + decay: `scripting/systems/{vignette_decay,shake_decay}.rs` beside
  `flash_decay.rs`; `screen.vignette` / `screen.shake` registered like
  `FLASH_SLOT`.
- Reactions: `Vignette` / `ScreenShake` in
  `scripting/reactions/system_commands.rs`; constructors in
  `sdk/lib/ui/reactions.{ts,luau}`.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau |
|---|---|---|---|---|
| vignette reaction | `SystemReactionCommand::Vignette` | `"vignette"` | `vignette(args)` | `vignette(args)` |
| vignette args | `{ color, strength, duration_ms }` | `{ "color", "strength", "durationMs" }` | `{ color?, strength, durationMs }` | same |
| screenShake reaction | `SystemReactionCommand::ScreenShake` | `"screenShake"` | `screenShake(args)` | `screenShake(args)` |
| screenShake args | `{ amplitude, duration_ms, frequency }` | `{ "amplitude", "durationMs", "frequency"? }` | `{ amplitude, durationMs, frequency? }` | same |
| vignette slot | `"screen.vignette"` (`Array` RGBA) | n/a | read-only bind name | same |
| shake slot | `"screen.shake"` (`Array [dx,dy]`) | n/a | read-only bind name | same |

## Open questions

- **Resample mechanism (the cross-milestone merge).** Shake must sample the
  composited frame at an offset — a pass can't read and write the same
  attachment. Two paths: (a) **copy** `view`→a temp texture, then the SE pass
  samples the temp and writes `view` (localized to SE; needs `COPY_SRC` on the
  surface or the temp); (b) an **offscreen frame target** every prior pass
  renders into, resolved to the swapchain by the SE pass (the durable compositor
  seam M9 tonemap / M10 post would share, but re-targets every pass — invasive).
  Recommend (a) for this lean first slice and fold into (b) when a second
  consumer (tonemap) actually needs the shared resolve — the project's
  "defer until duplication is real" rule. Flag for implementability review and
  M9/M10 coordination.
- **Flash blend vs. overlay.** Flash and vignette need no resample (overlay
  blend); only shake does. The pass may fast-path to an overlay-only draw when
  `screen.shake` is at rest (skip the copy). Optimization, not required for v1.
- **Sustained vignette** (track a value while a condition holds) is deferred to
  a future continuous-effect model; v1 is one-shot decay only.

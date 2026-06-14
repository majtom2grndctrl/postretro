# SE — Research Notes

Grounding for the SE draft. Identifiers confirmed against source 2026-06-14.
Ephemeral — line numbers drift; the spec references roles, this file the anchors.

## Frame order (single swapchain target)

Every pass renders directly to the surface `view` — **no offscreen HDR /
tonemap target exists today.** Order in `render/mod.rs::render_frame_indirect`:

1. depth prepass
2. forward / textured (clears color)
3. skinned-mesh forward
4. billboards / particles
5. fog raymarch compute → fog composite (`render/mod.rs:5317`, additive into `view`)
6. wireframe overlay (`:5352`)
7. debug lines (`:5395`)
8. **UI pass** — `self.ui.encode(...)` `render/mod.rs:5497`, `LoadOp::Load`
9. timing resolve (`:5511`)
10. submit (`:5515`)

**SE insertion point:** after `ui.encode` returns (`:5505`), before timing
resolve (`:5511`). The composited frame (scene + fog + UI) is in `view`.

The roadmap calls SE "the one real cross-milestone merge … scene compositor
where M9 tonemap/fog and M10 post live." Reality: that shared compositor
target does **not** exist yet; fog composites straight into `view`. SE either
introduces a frame-resolve seam or copies `view`. See spec open questions.

## UI pass

- `pub(crate) struct UiPass` — `render/ui/mod.rs:459`; owned `ui: ui::UiPass` — `render/mod.rs:1314`.
- `encode(device, queue, encoder, view, viewport, load: LoadOp<Color>, composition)` — `render/ui/mod.rs:1012`.
- Render pass "UI Pass", color attachment `view` + `LoadOp::Load`, no depth.

## Fullscreen-pass precedent

- Fog composite: `FogPass.composite_pipeline` — `render/fog_pass.rs:73`; draw `composite.draw(0..3, 0..1)` — `render/mod.rs:5333` (fullscreen triangle, no vertex buffer).
- Shader: `src/shaders/fog_composite.wgsl`. Inclusion pattern: `const X: &str = include_str!("../shaders/…wgsl")` (`render/ui/mod.rs:149`, `render/fog_pass.rs:30`).
- Shaders dir: `crates/postretro/src/shaders/` (forward, ui_quad, fog_composite, fog_volume, …).

## Slot / snapshot system (Goal C)

- `pub(crate) struct SlotTable` — `scripting/slot_table.rs:131`; `enum SlotValue { Number(f32), Boolean(bool), String(String), Enum(String), Array(Vec<f32>) }` — `:8`.
- `SlotTable::get(&name) -> Option<&SlotRecord>` `:317`; `set_value(&name, value)` `:282`; `SlotRecord { schema, value: Option<SlotValue> }` `:53`.
- Once-per-frame snapshot: `fn build_ui_slot_snapshot(&SlotTable) -> HashMap<String, SlotValue>` — `main.rs:2850`, called `main.rs:2237` before render.
- Renderer-side: `pub(crate) struct UiReadSnapshot` — `render/ui/mod.rs:311`, field `pub slot_values: HashMap<String, SlotValue>` `:327`; published via `renderer.set_ui_snapshot(...)` `render/mod.rs:3504`; stored `ui_snapshot` `:1335`, read at `:5456`. **The SE pass reads effect slots from here — never the live table.**

## Flash precedent (the pattern SE mirrors twice)

- Reaction: `registry.register("flashScreen", …)` — `scripting/reactions/system_commands.rs:211`.
- Command: `SystemReactionCommand::FlashScreen { color: [f32;4], duration_ms: f32 }` — `system_commands.rs:31`. Sibling variants: `PlaySound`, `Rumble`, `PushTree`, `PopTree`.
- System registry: `pub(crate) struct SystemReactionRegistry` — `system_commands.rs:128`; `SystemReactionFn = Box<dyn Fn(&Value, &SystemCommandQueue) -> Result<(), ReactionError>>` `:120`.
- Decay sink: `pub(crate) struct FlashDecay` — `scripting/systems/flash_decay.rs:66`; `const FLASH_SLOT = "screen.flash"` `:13`; `start(color, duration_ms)` `:87`, `tick(dt)` `:110` writes decaying RGBA `Array` to `screen.flash` at the game-logic stage.
- `screen.flash` is written + readable today, but **no GPU consumer exists** — E renders it via a full-screen `panel` widget bound to the slot in the demo tree (the stand-in). SE is its durable consumer.

## E surfaces SE sits beside (not modified)

- `CrossingDetector` — `scripting/state_crossings.rs:36` (`onStateCrossing`, drives the demo).
- `StyleRanges` — `render/ui/style_ranges.rs:21` (continuous value→style; orthogonal to SE).

## Wave seam with G2

SE adds `vignette` / `screenShake` reactions → touches
`scripting/reactions/system_commands.rs` (SE-only), `sdk/lib/ui/reactions.{ts,luau}`
(SE-only), `scripting/typedef.rs` reaction SDK-block (shared with G2's widget
block — different sections), and `sdk/lib/index.ts` barrel (shared — different
exports). SE does **not** touch `render/ui/descriptor.rs` or the tree bridge
(those are G2-only). Regenerate typedefs after both land.

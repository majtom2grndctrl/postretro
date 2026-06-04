# M13 Goal A — Follow-ups

Deferred items surfaced by the review panel. Intentionally NOT done in Goal A
(dev-guide §2.5: don't refactor during a bugfix; §1.4: file follow-ups, don't
expand scope). Most land naturally with **Goal B** (descriptor model + text/widget
vocabulary), which already edits `render/ui/`.

- **Split `render/ui/mod.rs` (~800 lines, two responsibilities).** It owns both
  the hand-rolled quad pipeline and all glyphon text state (`FontSystem`,
  `SwashCache`, `Cache`, `Viewport`, `TextAtlas`, `TextRenderer`, `shape_text`,
  `prepare_text`). Extract the glyphon half into `render/ui/text.rs`, leaving
  `mod.rs` as the quad pass + pass orchestration. Cheapest to do as Goal B adds
  text features. (dev-guide §2.2 — responsibility seam.)

- **Call `TextAtlas::trim()` once per frame.** glyphon's atlas grows as glyphs
  rasterize. Harmless for the splash's single static line, but the field is "the
  engine default text path" — once gameplay/menus push varied text (B/BIS), the
  missing trim becomes unbounded growth. Add the per-frame trim when the first
  real multi-string text consumer lands.

- **Empty-batch encode early-out.** The gameplay path currently records an empty
  UI pass every frame (opens `begin_render_pass`, writes the uniform) to lock the
  frame-order position. Once a real per-frame gameplay UI draw list exists, add an
  early-out when both `batches` and `texts` are empty to skip the empty pass.

- **Splash text horizontal centering via measured width.** `render/ui/splash.rs`
  centers the version line using an estimated average glyph advance
  (`chars().count() * est_advance`). Fine for the short static line; mis-centers
  proportional strings. Goal B should center from glyphon's measured run width.

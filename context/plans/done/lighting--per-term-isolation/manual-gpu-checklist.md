# Per-Term Lighting Isolation — Manual GPU Checklist

This is a manual GPU verification checklist. The headless frame-capture path is
not a substitute: it does not exercise dynamic, animated, mesh, mover,
billboard, or fog lighting for this feature.

Build and launch with diagnostics:

```bash
cargo run -p xtask -- run --features dev-tools -- content/dev/maps/combat-demo.prl
```

If a referenced PRL is stale or absent, compile its source map first with
`prl-build`; do not extend the capture harness for this check.

## `combat-demo`

- With all light-term checkboxes enabled, compare against the pre-toggle scene:
  world, movers, meshes, billboards, and fog remain visually unchanged; emissive
  surfaces remain self-lit for every mask state.
- Toggle Dynamic direct off and on. Verify the dynamic-lit world, dynamic
  receivers, sprite lighting, and fog spot/point scatter change together and
  restore together.
- Toggle Ambient floor, Baked direct — static, and Baked direct — animated
  independently. Confirm every receiver path that samples the term changes,
  while emissive material remains outside the mask.

## Animated and specular scenes

Run `content/dev/maps/animated-layer-spill.prl` to inspect static versus
animated indirect and direct terms, then
`content/dev/maps/specular-shadowmask-capture.prl` to inspect the specular
term. Confirm that each relevant checkbox independently removes and restores
only its named contribution; skinned meshes have no specular contribution, so
their unchanged result is expected.

## Ordering observations

Run every ordering scenario below on the scene that exposes its affected term.
Record the adapter, map, and any discrepancy before landing the plan.

| Row | Scenario to run | Confirm |
|---|---|---|
| T1 | Clear Baked direct — static, then restore it to `ALL`. | Direct atlas recomposes when isolating and again when restoring; world and entity results agree throughout. |
| T2 | Clear Indirect — animated at runtime without reloading. | Animated indirect disappears from world, mesh, billboard, and fog on the next rendered frame. |
| T3 | Clear Baked direct — static. | World lightmap and entity composed-direct contribution change together; neither leads the other by a frame. |
| T4 | Clear both Baked direct bits in one UI update. | Static and animated direct compose passes apply together; no intermediate one-bit state appears. |
| T5 | Pause simulation, clear Dynamic direct, and issue the UI redraw. | The frozen scene updates without reload; direct compose remains render-frame-driven. |
| T6 | With Baked direct — static cleared, reload the level or force full renderer initialization. | The mask resets to `ALL` and the freshly built atlas matches that default; prior isolation does not survive. |
| T7 | Start or reload with an isolated direct mask active while direct-compose resources perform load copy-through. | First visible compose uses the current per-frame mask, never a default-mask atlas paired with isolated uniforms. |
| T8 | On a frame with world rendering disabled, clear Baked direct — animated; render the world on the next frame. | The next world frame honors the change; it was not lost during the non-world frame. |
| T9 | Arrange same-frame level/load reconstruction, active promotion weight, and a mask change. | One dispatch uses the current snapshot mask and live weights; no double dispatch or default-mask compose appears. |
| T10 | On the Dynamic direct toggle frame, compare fog spot/point scatter with world dynamic lighting. | Both retain the same snapshot for that frame and change together on the next frame; fog never leads world. |
| T11 | On a Baked direct toggle frame, compare world lightmap, direct Pass A output, and Pass B output. | Pass A, Pass B, and world use the same snapshot; static/animated direct bits land together. |

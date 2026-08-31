# Shadowmask Bake Scaling — research notes

Investigation record. Decisions live in `index.md`; this is the evidence.

## Reproduction (this session)

Fixture: `content/dev/maps/stress-warren-lit.map` — 1134 brushes, 157 static
`light`/`light_spot` entities (no dynamic/directional). Container memory ~16 GB.

- **Cached path, default dev bake** (`prl-build <map> -o out.prl`, cache enabled):
  OOM-killed at ~42 s, still at `Lightmap Bake: 0%`, ETA ~5000 s. dmesg:
  `Memory cgroup out of memory: Killed process (prl-build) anon-rss:13947392kB`
  (~14 GB). Never reached the shadowmask stage — the **cached lightmap path**
  materialized per-light layers and OOM'd first (that path is
  `lighting-scale--lightmap-bake-incremental-flush`'s territory, not this plan's).
- **Cold path** (`--no-cache`): survived to `Lightmap Bake: 2%` at the 150 s cap
  without OOM (monolithic scatter holds no per-light layers). Consistent with the
  reaching-light spike's recorded result: at ship config
  (`--sh-probe-spacing 10.0 --lightmap-density 0.25 --no-cache`) the cold bake
  completes SH and lightmap, then **SIGKILLs during the Shadowmask atlas bake
  ~405 s in** (`lighting-scale--cold-bake-reaching-light-spike/out-of-scope-findings.md`
  item 2). `--lightmap-density 1.0` completes.

Reconciliation: the OOM stage depends on cache/path, one root cause.

| Path | Lightmap stage | Shadowmask stage | OOMs at |
|---|---|---|---|
| Cached, section-miss | materializes all N layers (pipeline.rs L652/L685) | not reached | lightmap (incremental-flush) |
| Cached, section-hit | skipped (L642-649) | materializes all selected layers | shadowmask (this plan) |
| Cold `--no-cache` | monolithic scatter, no layers | materializes all selected layers | shadowmask (this plan) |

The user reaches the shadowmask stage → their lightmap is section-hit or cold →
this plan is their blocker. The cached section-miss path also needs
incremental-flush.

## Memory arithmetic

`LayerTexel` = 48 bytes (`lightmap_layer.rs` L56, static-asserted L78), dense over
covered texels. A `LightmapLayer` ≈ 48 B × covered-texel-count. The shadowmask holds
all N selected layers simultaneously (`shadowmask_bake.rs` L218 `layers.push`, or
L60-73 uncached `.iter().map().collect()`), then `build_shadowmask_from_layers`
builds an `overlap_graph` (`HashMap<usize, Vec<usize>>` over every covered texel ×
light) + a `data = vec![255; width*height*layer_count*4]` output buffer.

Two memory terms:
- **Lights-scaling** ≈ N × atlas-covered-texels × 48 B — the layers Vec. This plan
  removes it (bound to O(window)).
- **Atlas-size** ≈ width×height×layer_count×4 B — the `data` output buffer, plus the
  overlap-graph membership. Independent of light count; a function of density.
  Bounded by lightmap density, owned by `lighting-scale--lightmap-bake-scaling` /
  `--lightmap-density`. Out of this plan.

At 157 lights the lights-scaling term is ~157× the atlas-size term, so removing it is
the dominant win and the OOM fix.

## Bake substrate anchors (verified against source)

- `shadowmask_bake.rs` (1595 lines; production code ~610, tests ~985):
  `bake_shadowmask_atlas` (L28, uncached), `bake_shadowmask_atlas_cached` (L85),
  `build_shadowmask_from_layers` (L427), `overlap_graph` (L490),
  `assign_channels_with_drops` (L509) → `color_graph`/`color_order` (backtracking
  4-coloring), `SHADOWMASK_ATLAS_STAGE_ID`/`_VERSION`. Per-light layers built via
  `bake_light_layer` (L205 cached / L63 uncached).
- `lightmap_layer.rs` (2050): `bake_light_layer[_controlled]` (L207/L227) returns
  `LightmapLayer`; internally `placements.par_iter()` (L252) with
  `governor().enter()` per chart (L259), `advance(1)` per chart (L264/L303).
  `LAYER_FORMAT_VERSION = 4` (L24) — shared cache namespace `"lightmap_layer"`.
- `pipeline.rs` shadowmask stage (L1260-1294): `StageProgress::indeterminate()`
  (L1264, never publishes a total → bare spinner), `bake_shadowmask_atlas_cached`
  (L1273) gated on `delta_sections.entity_shadow_lights.is_some()`. The lightmap warm
  path publishes a real total: `warm_lightmap_total = placements.len() *
  layer_lights.len()`, `publish_total` (L593-594) — the pattern to mirror.
- Governor (`governor.rs`): `enter() -> Permit` (L58) parks while
  `paused || active >= permits`, increments `active`; `set_permits` (L71) /
  `set_paused` (L78) are what `-j` and the TUI worker call on the shared
  `Arc<Governor>`. `checkpoint` (L47) honors pause only, **not** the permit cap —
  parallel ray work must `enter()`, not `checkpoint`. Nested `enter()` consumes two
  permits per active leaf → deadlock risk at `permits == 1`.
- Reporter (`reporter.rs`): monitor renders a percentage + ETA only when
  `total_published` is set (`total()` returns `Some`); indeterminate stages get a
  drain-only bare spinner (L153/L182-193).
- `BakeControl` (`bake_control.rs`): `advance(units)` is a lone `fetch_add`;
  `publish_total` delegates to the `StageProgress`.

## Cross-plan coupling

`lighting-scale--lightmap-bake-incremental-flush` Task 2 bumps
`LAYER_FORMAT_VERSION` and re-slices the per-light `"lightmap_layer"` cache blobs to
per-layer partitions. This plan reads/writes the same cache. This plan touches the
cache only through the existing `StageCache` get/put + `LightmapLayer::from_bytes`
API, so a reslice that preserves that API is transparent; a reslice that changes read
granularity requires this plan's fill pass to align. See index.md Sequencing.

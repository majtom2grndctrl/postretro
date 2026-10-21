# fluid-depth-fog-waves

Brief · compact · reads: `context/lib/rendering_pipeline.md` §7.3a, §7.5, §7.8, §10 · `scripting.md` §2 · `player_options.md` §5 · `networking.md` §What gates, and what replicates instead · read at 6c31e7d36 · lands before `fluid-ssr-reflections`

## Problem
Owner-requested capability: murky fluids cannot hide what lies beneath them. Surface opacity hides everything or nothing, so a pond reads as a tinted sheet over a fully visible floor. The fluid-surface-motion owner review (2026-10-13) prototyped depth fog and a slow whole-texel wave, approved both, and deferred them here as `defineFluid` settings. Done: a modder declares per fluid how far you see into it from outside, and an optional slow wave. Dev water shows walls near the surface, hides anything about 1 m down, and sways in whole-texel steps. The underwater view is unchanged.

## Decisions
- **Sequencing.** Lands before `fluid-ssr-reflections`; the two never build concurrently. Both edit the fluid fragment composite, group 5, the per-volume params row, and `FluidDescriptor` with its literal sites and content twins. The SSR draft aligns to this brief before its promotion.
- **Fog model.** `fogDistance` (metres) is how far into the fluid you see from outside. Transmittance is `exp(-3·path/fogDistance)`, so 95% is hidden at that path length. Omitted: clear, drawn exactly as today. One knob covers distance and density; no separate thickness field.
- **Fog colour.** `fogColor` is the murk colour as a modder picks it, in `viewTint`'s rgb space. The engine scales it internally to an albedo lit by the surface's own lighting.
- **Fog scope.** Only fluid faces seen from outside the volume (outward normal toward the camera). Per screen pixel, along the view ray to the first opaque surface. One draw composites surface over fog over scene. A CPU fog reference pins the WGSL fog bodies.
- **Eye crossfade.** Fog factor = per-face proximity × frame weight. Murk crossfades with the tint, never flickers under head bob, and never draws for a swimmer.
  - Per-face proximity, per fragment: the camera's height above the face's own plane along its outward normal. Ramps from 0 at the plane to 1 at one tint band-width (0.08 m, §7.8) above it; 0 below the plane. The shader computes it from the camera position and the face's world position and normal; no new collision query. An eye bobbing at a plane before the tint triggers never flips between murk and clear.
  - Frame weight, one per frame: 0 whenever the eye is contained in any fluid, as `fluid_at_eye` reports; otherwise one minus the eye tint's 0..1 ramp (§7.8), so murk fades in as the tint fades out. Sent every frame like the tint, with a dry default of 1, so a recreated renderer still draws fog.
  - Accepted: where two adjacent volumes' fog doubles at their shared face, seen from above, the seam composites at T², brighter or darker as the murk is brighter or darker than the floor behind it.
- **Opacity 0 with fog draws.** The collector keeps a draw when opacity > 0 or fog is set. §7.3a's "zero opacity draws nothing" narrows to fluids without fog.
- **Underwater view unchanged.** The owner needs long underwater sight for navigation and combat. Inside a fluid the flat `viewTint` (§7.8) stays the whole effect. A future underwater fog gets its own, longer distance field, never `fogDistance`.
- **Wave fields, in texels.** `wavePeriod` is seconds per cycle of an average-length crest. `waveAmplitude` is the pattern's peak sideways shift. `waveLength` is crest to crest: a number L, meaning `[L, L]`, or a `[min, max]` pair. Amplitude and length count the face's texture texels at its texture scale — §7.3a's grain unit, and the unit of Surface Depth's texel mode (§7.3). A 64↔128 px swap keeps the wave's world size; a finer-scale face carries a finer wave.
  - Author-declared pixel motion departs deliberately from §7.3a's "pixel motion is engine-owned, not an author style": the owner deferred the wave here as per-fluid settings. §7.3a is amended at promotion.
  - The name says what moves. The surface stays flat; the wave displaces texel samples sideways in the surface plane (a UV warp). "Height" is reserved for a future real-height or voxel wave, a non-goal here.
  - Each crest hashes its own stable length from the range. All crests travel together at mean length ÷ `wavePeriod`, so crests never collide. Half-cycle arcs that start and end at zero join crests of different lengths continuously.
  - As in the prototype, `v` drives the `u` offset and `u` the `v` offset. The `wavePeriod` floor and length ceiling cap crest speed (`research.md` §Coherence and travel). The wave never reads as a current with no push behind it — the look-vs-feel split `plans/done/fluid-surface-motion` rejected when it dropped a visual-only `flow` key.
- **Validation is warn-and-drop** (`scripting.md` §2). `fogDistance` finite > 0. `fogColor` three finite values in 0..=1. `wavePeriod` finite ≥ 6 (about Quake's turb speed; the owner's cap). `waveAmplitude` finite in 0..=16; 0 means no wave. `waveLength` finite, 0 < min ≤ max ≤ 128, min ≥ 8 × `waveAmplitude`. The default length always passes, so an amplitude set without a length is never rejected on length. An explicit `waveLength` that breaks a rule rejects the fluid with a warning naming `waveLength`.
- **Wave is motion, not flash.** The 8× length minimum keeps the cross-coupled warp from folding, with margin; 6× folds (`research.md` §Coherence and travel). The wave moves light rather than adding it, so like drift it takes no 3 Hz per-cell cap (§7.3a). No reduce-motion input: reduce motion covers view feel, shake and UI tweens only (`player_options.md` §5).
  - Composes beside drift, outside the per-cell displayed offset, so the shimmer's 3 Hz and tenth-of-a-grid guards stay unchanged.
  - A manifest reload that changes wave fields may jump the pattern for one frame, accepted as drift's re-resolve is.
- **Wave stays trig-free.** The no-trig shader test bans trig textually, and a trig phase on f32 script time loses precision over a long run. The wave is a whole-texel offset per texel cell on the fixed grid; a CPU twin pins its WGSL body.
- **GPU layout.**
  - The fluid pass opens depth read-only and samples scene depth in group 5, the fluid scene-inputs group: scene depth here, SSR's target and uniform later. Group 5 rebuilds with the scene targets, never per frame.
  - The per-volume params row widens to three `vec4` (stride 48), with at least one float spare for SSR's reflectivity. A window stays within 64 KiB, the D3D constant-buffer ceiling.
  - No new storage buffer: fragment storage is at its ceiling (§10).
- **Presentation-only, client-local.** Each new field resolves locally by fluid name like `surfaceOpacity`, stays out of the mod digest and `FluidMovementTuning`, and never feeds drift resolution. Peers may differ: murk widens the per-peer visibility difference that opaque surfaces and full-strength tints already allow. Fluids are outside the compatibility digest, and co-op is not competitive.
- **Accepted:** billboards, particles and projectile bodies drawn after the fluid pass pop fully out of murk — a larger version of the existing surface-opacity limit.
- **Non-goals.**
  - Refraction: E24 deferred it (`plans/done/E24--water-and-environment-volumes`).
  - Clamping the fog path to the volume's far side: it needs per-pixel back faces or thickness, which the fluid pass lacks. A side face with open air behind it over-fogs; pools bounded by walls are exact.
  - A player option: fog costs one depth load per fluid fragment and is no cost lever.
  - Fog per texel cell. Fog stays per screen pixel, as the prototype approved.

### Scripting surface
```ts
// Proposed design
export default defineMod({
  name: "murk-canal",
  id: "example.murk-canal",
  version: "1.0",
  fluids: [
    defineFluid({
      name: "water",
      viscosity: 2.0,
      viewTint: [0.1, 0.3, 0.4, 0.35],
      muffle: 0.8,
      surfaceOpacity: 0.75,
      fogDistance: 1.0,          // metres; omitted = clear
      fogColor: [0.08, 0.2, 0.16], // rgb, viewTint's space; omitted = viewTint's rgb
      wavePeriod: 63,            // seconds; omitted = no wave
      waveAmplitude: 6,          // texels; omitted = 8; 0 = no wave
      waveLength: [48, 80],      // texels, [min, max] or one number; omitted = [max(48, 8·A), max(80, 8·A)]
    }),
  ],
});
```
The Luau mirror ships with the same names (`fogDistance = 1.0`, `fogColor = { 0.08, 0.2, 0.16 }`, `waveLength = { 48, 80 }`, …); a single `waveLength` number works in both.

## Acceptance
### Automated
- [ ] The Scripting surface example parses in TS and its Luau mirror.
- [ ] Each new field omitted resolves to its default: no fog, no wave, `fogColor` equal to `viewTint`'s rgb, amplitude 8, length `[64, 80]`.
- [ ] Boundary values are accepted: the smallest positive `fogDistance`; `fogColor` components 0 and 1; `waveAmplitude` 0.
- [ ] Each of these rejects that fluid with a warning naming the field, and the rest of the manifest stands: `fogDistance` 0, negative or NaN; a `fogColor` component outside 0..=1, or a wrong length; `wavePeriod` NaN; `waveAmplitude` negative or NaN; `waveLength` 0, NaN, a pair with min above max, or a pair of the wrong length.

Each wave limit is pinned on both sides; the rejected side warns naming the field shown:
- [ ] `wavePeriod` 6 is accepted; 5.99 is rejected (`wavePeriod`).
- [ ] `waveAmplitude` 16 with `waveLength` 128 is accepted; 16.01 is rejected (`waveAmplitude`).
- [ ] `waveLength` `[64, 128]` is accepted; `[64, 128.01]` is rejected (`waveLength`).
- [ ] A range minimum exactly 8 × `waveAmplitude` is accepted; just below it is rejected (`waveLength`).
- [ ] With `waveAmplitude` omitted, the default 8 sets the minimum at 64: `waveLength` 64 is accepted; 50, or any explicit length below 64, is rejected (`waveLength`). `waveLength` 50 with `waveAmplitude` 6 is accepted (pin P12).
- [ ] A single `waveLength` L resolves to `[L, L]`, and the pair `[48, 80]` (Luau `{ 48, 80 }`) to `[48, 80]`; TS and Luau each parse both forms to the same result. With `waveLength` omitted, amplitude 4 resolves to `[48, 80]` and amplitude 16 to `[128, 128]`, both accepted.
- [ ] `waveAmplitude` 30 with `waveLength` omitted is rejected naming `waveAmplitude`, never `waveLength`; `waveAmplitude` 17 with `wavePeriod` omitted is still rejected (`waveAmplitude`) (pin P5).
- [ ] Coherence: the wave's CPU twin, swept over A=16 `[128, 128]`, A=8 `[64, 80]`, A=6 `[48, 80]` and A=1 `[8, 128]`, keeps adjacent cells' whole-texel offsets within the bound it derives from its arc's peak slope at the range minimum, L = 8A, plus one texel for rounding. The test pins that bound at 1 texel, for an arc whose peak slope at L = 8A is at most 0.8 texel per cell (a sine's is 2π/8 ≈ 0.79; a two-parabola arc's is exactly 1, the fold point, and fails).
- [ ] The mod digest is unchanged when only the new fields differ between two manifests.
- [ ] `FluidMovementTuning` carries only name and viscosity, and drift resolution reads none of the new fields (source scan).
- [ ] A manifest reload that changes only one fluid's fog or wave fields, or only the `viewTint` rgb of a fluid whose `fogColor` is omitted (so its packed fog colour changes), re-uploads exactly that fluid's slots on the next frame's draws. A reload that changes nothing uploads nothing (pin P2).
- [ ] After a reload that changes `wavePeriod` 63 → 20, the first frame's offsets equal a fresh evaluation of the wave reference at period 20 and that frame's script time. Nothing eases across the change; the one-frame jump is accepted (pin P6).
- [ ] Committed SDK types and typedef snapshots are regenerated; the fluids-swim twins and dev manifest equality tests pass with the new dev values.
- [ ] Wave reference: the offset is a whole number of texels, constant across one cell. With amplitude 0 or no period it is exactly zero. The pattern at t + period equals the pattern at t shifted by one mean crest length, within 1e-3 texel for t up to 24 h. Every crest advances mean length ÷ `wavePeriod` texels per second, so crest spacing never changes. Each crest's length lies in the range and is the same at every time. The shape is zero at every arc join. A 64 px and a 128 px texture at the same scale get the same offset in texels.
- [ ] Wave wrap: sampled frame by frame (1/60 s apart) across every whole-period boundary for t up to 24 h, at A=6 `[48, 80]` and A=1 `[8, 128]`, each at period 6 and 63: no cell's whole-texel offset changes by more than one texel between consecutive frames, and a crest tracked by its travel keeps its length across the boundary (pin P10).
- [ ] A fluid with `wavePeriod` omitted or `waveAmplitude` 0 packs amplitude 0 whatever amplitude was declared; one with `fogDistance` omitted packs extinction 0 (pin P3).
- [ ] The no-trig shader test passes, and the wave's WGSL body is pinned to its CPU twin.
- [ ] The shimmer-and-ring flash guards (each cell's offset changes under 3 Hz; no frame changes more than a tenth of a grid) pass unchanged. The wave adds beside drift in the warped sample, outside the per-cell displayed offset.
- [ ] Fog reference: the clip-space path matches direct world-space distance on synthetic rays. Transmittance at path = `fogDistance` is 0.05 ±ε. A face seen from inside gets no fog. With fog omitted the output equals today's. Proximity is 0 at and below the face's plane, 1 at one band-width above it along the outward normal, linear between. The fog layer scales by the frame's fog weight times the face's proximity — the same two values the shader reads: full at 1, none at 0.
- [ ] Fog reference edge cases: the clip-space path matches direct world-space distance for a fragment at the near plane and for a pixel whose scene depth is the cleared far value. The output blended over a scene colour by the pipeline's straight-alpha blend equals surface over fog over scene. The same fog composited twice over one scene colour equals one composite at transmittance T² (pin P9).
- [ ] The fluid shader's fog path, transmittance, proximity and composite bodies are pinned to the CPU fog reference. The shader returns the composite's colour and alpha. Its outside-view gate and proximity read the interpolated outward normal, never the viewer-flipped shading normal.
- [ ] The frame fog weight is 0 on every frame the eye is contained in any fluid — including between the plane and the trigger depth, and during the ramp-in after a side entry — and one minus the eye's tint ramp otherwise. An eye bobbing ±0.05 m about dev water's surface after a dive keeps it at 0. A swimmer facing the shared face of a second fluid volume, same fluid or another, sees no fog there (pins P7, P8).
- [ ] An eye placed 1 m under dev water on one frame (respawn, teleport, or a level install with the eye already submerged) has frame weight 0 on that first presented frame, while the tint ramp is still near 0. So does an eye that crosses from water into an adjacent fluid with no local presentation, through the whole ramp-out (pin P11).
- [ ] An eye lowered from one band-width above dev water's surface to the plane, before the tint triggers, takes that surface's proximity from one to zero monotonically. An eye bobbing ±0.05 m across the plane at dev head bob from a standing start has proximity 0 at every sample below the plane, and between consecutive frames proximity changes by at most the height change ÷ the band width.
- [ ] Each face's proximity is its own: with the eye 0.04 m above an upper pool's surface and a lower pool in view, the upper face's proximity is 0.5 and the lower face's is 1. An eye a band-width below a floating volume's bottom face is not contained and sees that face with full fog (pin P13).
- [ ] The frame fog weight reaches the renderer every presented frame beside the fluid view tint. Before its first write the renderer's value is 1, so a renderer built or recreated mid-level draws fog on its first frame.
- [ ] A fluid with `surfaceOpacity` 0 and `fogDistance` 1 gets a draw for its visible volumes and outputs fog alone; with fog omitted it still draws nothing.
- [ ] Pipeline budgets: fragment storage exactly 8; fragment sampled textures 8 of 16.
- [ ] The params row leaves at least one float unused. A level whose draws span two params windows gives every volume its own row: every window's dynamic offset is a multiple of 256, and an offscreen GPU test draws the first window's last slot and the second window's first slot, given distinct opacities, and reads each back from the rendered pixels. CPU slot arithmetic alone never satisfies this row; the run log shows the GPU case ran, not skipped (pin P1).
- [ ] The fluid pass opens depth read-only.
- [ ] Group 5 is built at full init and rebuilt only by the scene-target rebuild, with or without a level installed; the frame after a resize binds the new depth view. The offscreen extent test reads the depth extent group 5 was last built over and finds the scene extent after a resize. A source scan finds no bind-group creation in the frame's fluid pass. The run log shows the GPU case ran, not skipped (pin P4).
- [ ] The DX12 FXC test compiles the fluid shader (Windows, ignored-gated run).

### Manual
On e24's east wing:
- [ ] From above, dev water shows walls near the surface and hides the floor about 1 m down. Sludge, with no fog or its own setting, looks as declared. A fluid with no fog fields looks exactly as before.
- [ ] Diving in shows the unchanged flat-tint view, with long visibility. Treading at the surface after a dive shows no murk flicker; climbing out, the murk fades in as the tint fades out.
- [ ] The wave sways in whole-texel steps, with crests of varied length that never collide. It keeps its world size across a 64/128 px texture swap at the same scale, and is a quarter that size on the scale-0.25 pool. Omitting `wavePeriod` stops it. Watched across at least two of dev water's 63 s periods, the pattern never jumps.
- [ ] A distant pool shows no wave crawl or seam.
- [ ] Resizing the window mid-level keeps the fog correct.
- [ ] GPU timing of the fluid pass before and after, looking down at a pool that fills the view: on the GTX 1660 Super with `POSTRETRO_GPU_TIMING=1`, and on the RDNA1 Mac via Metal System Trace where timestamps are unsupported. Report in the plan of record.

## Path
- **Prototype.** `poc-fog-wave.patch` here is the reverted fluid-surface-motion experiment (`research.md` §Prototype). Port its fog path solve and straight-alpha composite. Replace its per-frame bind group, constants and `sin` wave.
- **Seams.** Layout: `fluid_pipeline_bind_group_layout_entries`, `FluidSurfacePass::new`; group count: `FLUID_BIND_GROUP_COUNT` and the init debug assert. Group 5 rebuild: `Renderer::rebuild_scene_targets`, after `FogPass::resize`. Params row: `fluid_from_json` → `local_fluid_presentation` → `FluidSurfaceRenderCollector::collect` (which holds the opacity drop) → `FluidSurfaceSlots::set_draws`. Frame weight: `fluid_at_eye` containment with `FluidEye`'s ramp, whose accessor is test-only; it travels beside the fluid view tint, not in a params row. Proximity needs only the camera position the shader already has.
- **Wave.** The phase wrap must preserve crest identity. Drift's exact fract product drops whole periods, safe only because textures repeat. The hashed crest layout does not repeat per period, so a one-period wrap re-hashes every crest each `wavePeriod`. Wrap over a whole number of hash-repeat spans far longer than anything visible, taking the whole count and the fraction from one split. Seam: `warped_texel_uv`, beside drift. The CPU twin sits with the other motion references in render-cpu `fluid_surface/motion.rs`.
- **Params packing.** Three `vec4` per row, unpacked (`research.md` §Params row). Precompute `3/fogDistance` and `1/wavePeriod` CPU-side.
- **Parser.** `fluid_from_json` has only `EntryPath::optional_string`; add optional-number, optional-rgb and number-or-pair helpers beside it.
- **`FluidDescriptor` literals.** No `Default`, so every literal site across foundation, combat-model, netcode harnesses, postretro and scripting-core breaks. A `Default` presentation sub-struct is one way to keep that edit small.
- **Dev water.** `fogDistance: 1.0`, a `fogColor` that reproduces the prototype's murk through the chosen scale, `wavePeriod: 63`, amplitude and length at their defaults (`research.md` §Wave defaults).
- **Large file.** `renderer_render_frame.rs` is past 800 lines; this brief changes a few lines of its fluid pass, so no split.
- **Shape and rival.** Chosen: per-fragment fog in the fluid pass from sampled scene depth. Rival: extend §7.5 fog volumes. Rejected because their composite runs after the fluid pass and would fog the surface itself; they would fog the underwater view, which the owner ruled out; they resolve at quarter resolution by default, leaving blocky edges at pool walls; and a mapper would place a volume per pool instead of declaring fog once per fluid.
- **First slice.** The trig-free wave at the limit settings on the east wing's scale-1 and scale-0.25 pools, viewed near and far. It falsifies the coherence bound and the distance-seam unknown before any field plumbing.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Fog distance | `FluidDescriptor::fog_distance: Option<f32>` | n/a (local presentation, out of digest) | `fogDistance` | `fogDistance` | n/a |
| Fog color | `FluidDescriptor::fog_color: Option<[f32; 3]>` (viewTint space) | n/a | `fogColor: [r, g, b]` | `fogColor = { r, g, b }` | n/a |
| Wave period | `FluidDescriptor::wave_period: Option<f32>` | n/a | `wavePeriod` | `wavePeriod` | n/a |
| Wave amplitude | `FluidDescriptor::wave_amplitude: f32` (texels, resolved default) | n/a | `waveAmplitude` | `waveAmplitude` | n/a |
| Wave length | `FluidDescriptor::wave_length: [f32; 2]` (texels, `[min, max]`, resolved default) | n/a | `waveLength: number \| [min, max]` | `waveLength = n` or positional `{ min, max }`, e.g. `{ 48, 80 }` | n/a |

Both TS and Luau ship. `FLUID_KEYS`, the manifest typedef registration, `sdk/lib/data_script.{ts,luau}`, `docs/scripting-reference.md` and `scripting.md` §2 carry each name.

## Open questions
- Whether the wave fades at distance, and how faded and unfaded cells meet without a seam — **delegated**: decide from the first slice and report it.
- The scale from `fogColor`'s `viewTint` space to the lit albedo — **delegated**: choose it so dev water reproduces the prototype's murk; report the scale and the dev values.

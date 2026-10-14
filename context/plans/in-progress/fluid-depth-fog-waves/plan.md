# fluid-depth-fog-waves — plan of record

mode: compact
status: active
read at: afb4acaa4

## Corrections
- Brief read at 6c31e7d36 → no fluid, scripting, foundation, renderer-fluid or postretro fluid source changed through afb4acaa4 (only lightmap budget work and an unrelated `renderer_init_resources.rs` import). Decision reads reused.
- Path "frame weight travels beside the fluid view tint" → true on the CPU side (`set_fluid_fog_weight` beside `set_fluid_view_tint`), but the fluid shader cannot read the resolve's effect uniform, and `FrameUniforms` is shared by every world shader. Planning around it by giving group 5 a second binding: a small fluid scene uniform (fog weight now, SSR's uniform later, as the brief's GPU layout already names). The buffer is created once and written when the weight changes; the bind group still rebuilds only with the scene targets. A uniform spends no sampled-texture or storage slot, so the AC budgets (storage 8, sampled 8 of 16) stand.
- Params window at stride 48: 1365 slots fill 65 520 B, not a 256 multiple. `FLUID_SLOTS_PER_WINDOW` becomes 1360 (65 280 B = 255 × 256), so every window's dynamic offset is a multiple of 256 (pin P1).

## Delegated answers
- Wave distance fade — **no fade.** Adjacent cells' wave offsets differ by at most one texel (the 8× rule), so at distance the wave is a slow, sub-pixel-smooth displacement that mip filtering already integrates; nothing new aliases. A per-cell hashed fade (shimmer's scheme) would tear a multi-texel wave at each faded/unfaded boundary, and a per-pixel amplitude fade reintroduces per-pixel motion off the cell grid. The owner's "distant pool shows no wave crawl or seam" manual row is the check.
- `fogColor` scale — **0.25**, linear. `viewTint` mixes after tonemap in linear (`apply_fluid_tint`), so a plain scale maps it to a pre-tonemap albedo. Dev water `fogColor: [0.16, 0.22, 0.19]` → albedo (0.04, 0.055, 0.0475), the prototype's murk.

## Design contracts (shared seams)
- **Params row** (render-cpu `FLUID_PARAMS_STRIDE` 48, three `vec4`):
  - row0 = `[opacity, fog_extinction, spare, spare]` — extinction = 3 / `fogDistance`, 0 = no fog.
  - row1 = `[fog_albedo_r, g, b, spare]` — resolved `fogColor` (else `viewTint` rgb) × 0.25.
  - row2 = `[inv_period, amplitude, length_min, length_max]` — amplitude packs 0 when `wavePeriod` is omitted or amplitude is 0; inv_period = 1 / `wavePeriod`.
  - Unwritten rows keep the `-1` opacity sentinel so the first draw uploads.
- **Wave twin** (render-cpu `fluid_surface/motion.rs`, mirrored in WGSL): per cell, u offset = `round(A·h)` keyed on the cell's v, v offset keyed on its u (separate salts). Crests come in pairs spanning exactly 2 × mean length, the first `mean + d`, the second `mean − d`, `d` hashed per pair, so every crest lies in `[min, max]` and pair index is O(1). Each crest is a positive then a negative half-arc `h(x) = 3g + 4g²`, `g = x(1 − x)`, zero at both ends, peak slope 3.043 per unit arc → 0.761 texel per cell at L = 8A. Phase in mean lengths = `t · inv_period` split exactly (split-mantissa, as drift) into an integer count and a fraction from one split; the count wraps modulo a whole number of hash-repeat spans (pair hash keyed on `pair & (R − 1)`), so every crest keeps its length across any wrap.
- **Fog weight**: app computes `0` if `fluid_at_eye` contains the eye, else `1 − ramp`; renderer default 1.0; group 5 binding 1 uniform.

## AC-to-proof

| AC | Proof | Status |
|---|---|---|
| A1 Scripting example parses in TS and Luau | scripting-core / sim parser tests on the example | achievable as stated |
| A2 Omitted fields resolve to defaults | parser test | achievable as stated |
| A3 Boundary values accepted | parser test | achievable as stated |
| A4 Each invalid value rejects that fluid naming the field | registry/parser warn-and-drop tests | achievable as stated |
| A5 `wavePeriod` 6 / 5.99 | parser test | achievable as stated |
| A6 `waveAmplitude` 16 / 16.01 | parser test | achievable as stated |
| A7 `waveLength` [64,128] / [64,128.01] | parser test | achievable as stated |
| A8 min exactly 8A / just below | parser test | achievable as stated |
| A9 default amplitude 8 binds the 8× rule (P12) | parser test | achievable as stated |
| A10 single L → [L,L]; pair forms; defaulted lengths | TS + Luau parser tests | achievable as stated |
| A11 amplitude 30 / 17 name `waveAmplitude` (P5) | parser test | achievable as stated |
| A12 Coherence bound swept over four settings | render-cpu `motion_tests` | achievable as stated |
| A13 Mod digest unchanged by new fields | postretro digest test | achievable as stated |
| A14 `FluidMovementTuning` name + viscosity only; drift reads none (source scan) | combat-model / postretro source-scan test | achievable as stated |
| A15 Reload re-uploads exactly changed fluid's slots; no change uploads nothing (P2) | render-cpu slots test via packing from descriptors | achievable as stated |
| A16 Period 63 → 20 first frame equals fresh evaluation (P6) | render-cpu test (row → twin) | achievable as stated |
| A17 SDK types and typedef snapshots regenerated; fluids-swim twins and dev manifest tests pass | `gen-script-types` + snapshot + `fluids_swim_surface_tests` | achievable as stated |
| A18 Wave reference properties | render-cpu `motion_tests` | achievable as stated |
| A19 Wave wrap frame-by-frame to 24 h (P10) | render-cpu `motion_tests` | achievable as stated |
| A20 Packing zeroes amplitude / extinction (P3) | render-cpu packing test | achievable as stated |
| A21 No-trig test; wave WGSL pinned to twin | renderer `fluid_surface_tests` | achievable as stated |
| A22 Flash guards unchanged; wave outside displayed offset | existing `motion_tests` guards + warp composition test | achievable as stated |
| A23 Fog reference core | render-cpu fog reference tests | achievable as stated |
| A24 Fog reference edge cases (P9) | render-cpu fog reference tests | achievable as stated |
| A25 Fog WGSL bodies pinned; outward normal read | renderer `fluid_surface_tests` textual pins | achievable as stated |
| A26 Frame weight 0 when contained; bob; shared face (P7, P8) | postretro `fluid_presentation` tests | achievable as stated |
| A27 Teleported-under eye and unpresented neighbour (P11) | postretro `fluid_presentation` tests | achievable as stated |
| A28 Proximity monotone to plane; bob bound | render-cpu fog reference + postretro eye test | achievable as stated |
| A29 Per-face proximity; floating volume bottom (P13) | render-cpu fog reference test | achievable as stated |
| A30 Weight reaches renderer each frame; default 1 | renderer state test + postretro source/flow test | achievable as stated |
| A31 Opacity 0 + fog draws fog alone; no fog draws nothing | render-cpu collector test | achievable as stated |
| A32 Budgets: storage 8, sampled 8 of 16 | renderer budget tests | achievable as stated |
| A33 Spare float; two-window GPU readback (P1) | render-cpu slot test + offscreen GPU test (log shows ran) | achievable as stated |
| A34 Fluid pass opens depth read-only | renderer source-scan test | achievable as stated |
| A35 Group 5 lifecycle; resize readback; no per-frame creation (P4) | offscreen extent GPU test + source scan | achievable as stated |
| A36 DX12 FXC compiles fluid shader | ignored-gated FXC test, run on Windows | achievable as stated |
| M1 From above: walls near surface, floor hidden ~1 m; sludge; no-fog fluid unchanged | owner, in-engine | manual |
| M2 Dive: unchanged tint; treading no flicker; fade on exit | owner, in-engine | manual |
| M3 Wave: whole-texel sway, varied crests, world size, scale-0.25, no jump over two periods | owner, in-engine | manual |
| M4 Distant pool: no crawl or seam | owner, in-engine | manual |
| M5 Resize keeps fog correct | owner, in-engine | manual |
| M6 GPU timing before/after (1660 Super, RDNA1 Mac) | owner, in-engine; report here | manual |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Wave CPU twin in `motion.rs` + coherence, reference and wrap tests (A12, A18, A19, A22) — falsifies the coherence bound first | worker (Opus) | — | done: arc peak slope 0.7607 texel/cell at L = 8A, bound 1 texel; 45 `fluid_surface` render-cpu tests pass |
| 2 | Scripting surface: `FluidDescriptor` fields, parser helpers, validation, typedefs, SDK, docs, dev content (A1–A11, A13, A14, A17) | worker (Opus) | — | done: foundation `fluid` 10, scripting-core `fluid` 25, postretro `fluid` 47 / `digest` 26, sim `typedef` 43 pass; flat descriptor fields; NaN from script rejects in the shared conversion layer naming the entry and field |
| 3 | Renderer: stride-48 row + packing + collector, group 5 (depth + scene uniform), read-only depth, rebuild seam, WGSL fog + wave, CPU fog reference, budgets, GPU tests (A15, A16, A20, A21, A23–A25, A29, A31–A36) | worker (Opus) | 1 | done: render-cpu `fluid_surface` 61 pass, renderer `fluid` 33 pass; P1, P4 and fog GPU cases ran on the 1660 Super; DX12 FXC compiles every full-init pipeline. Group 5 split into `render/fluid_scene_inputs.rs` (`fluid_surface.rs` hit 663 lines); the weight uploads in the fluid pass only when changed |
| 4 | App plumbing: descriptor → row look, eye fog weight, per-frame handoff (A26–A28, A30) | worker (Opus) | 2, 3 | |
| 5 | Preflight, review panel, fix loop, full gate | integrating executor | 4 | |

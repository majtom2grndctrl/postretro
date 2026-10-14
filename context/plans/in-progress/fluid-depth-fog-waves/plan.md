# fluid-depth-fog-waves — plan of record

mode: compact
status: active (results reported; awaiting owner)
read at: afb4acaa4

## Corrections
- Brief read at 6c31e7d36 → no fluid, scripting, foundation, renderer-fluid or postretro fluid source changed through afb4acaa4 (only lightmap budget work and an unrelated `renderer_init_resources.rs` import). Decision reads reused.
- Path "frame weight travels beside the fluid view tint" → true on the CPU side (`set_fluid_fog_weight` beside `set_fluid_view_tint`), but the fluid shader cannot read the resolve's effect uniform, and `FrameUniforms` is shared by every world shader. Planning around it by giving group 5 a second binding: a small fluid scene uniform (fog weight now, SSR's uniform later, as the brief's GPU layout already names). The buffer is created once and written when the weight changes; the bind group still rebuilds only with the scene targets. A uniform spends no sampled-texture or storage slot, so the AC budgets (storage 8, sampled 8 of 16) stand.
- AC A15 "re-uploads exactly that fluid's slots" → dirty tracking uploads one contiguous range from the first to the last changed slot. When another fluid's slot sits between them it is rewritten with its identical bytes, so no row changes value and none tears (`an_interleaved_change_uploads_one_range_whose_every_row_is_current`). Read as a clarification: exactly that fluid's rows change; the byte range may cover an unchanged neighbour.
- Packed extinction is capped at `FLUID_FOG_EXTINCTION_MAX` (1e30), not `f32::MAX`, so `extinction × path` stays finite for any fog path; a period whose reciprocal overflows packs no wave.
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

| AC | Proof | Status | Result |
|---|---|---|---|
| A1 Scripting example parses in TS and Luau | scripting-core / sim parser tests on the example | achievable as stated | pass |
| A2 Omitted fields resolve to defaults | parser test | achievable as stated | pass |
| A3 Boundary values accepted | parser test | achievable as stated | pass |
| A4 Each invalid value rejects that fluid naming the field | registry/parser warn-and-drop tests | achievable as stated | pass |
| A5 `wavePeriod` 6 / 5.99 | parser test | achievable as stated | pass |
| A6 `waveAmplitude` 16 / 16.01 | parser test | achievable as stated | pass |
| A7 `waveLength` [64,128] / [64,128.01] | parser test | achievable as stated | pass |
| A8 min exactly 8A / just below | parser test | achievable as stated | pass |
| A9 default amplitude 8 binds the 8× rule (P12) | parser test | achievable as stated | pass |
| A10 single L → [L,L]; pair forms; defaulted lengths | TS + Luau parser tests | achievable as stated | pass |
| A11 amplitude 30 / 17 name `waveAmplitude` (P5) | parser test | achievable as stated | pass |
| A12 Coherence bound swept over four settings | render-cpu `motion_tests` | achievable as stated | pass |
| A13 Mod digest unchanged by new fields | postretro digest test | achievable as stated | pass |
| A14 `FluidMovementTuning` name + viscosity only; drift reads none (source scan) | combat-model / postretro source-scan test | achievable as stated | pass |
| A15 Reload re-uploads exactly changed fluid's slots; no change uploads nothing (P2) | render-cpu slots test via packing from descriptors | achievable as stated | pass |
| A16 Period 63 → 20 first frame equals fresh evaluation (P6) | render-cpu test (row → twin) | achievable as stated | pass |
| A17 SDK types and typedef snapshots regenerated; fluids-swim twins and dev manifest tests pass | `gen-script-types` + snapshot + `fluids_swim_surface_tests` | achievable as stated | pass |
| A18 Wave reference properties | render-cpu `motion_tests` | achievable as stated | pass |
| A19 Wave wrap frame-by-frame to 24 h (P10) | render-cpu `motion_tests` | achievable as stated | pass |
| A20 Packing zeroes amplitude / extinction (P3) | render-cpu packing test | achievable as stated | pass |
| A21 No-trig test; wave WGSL pinned to twin | renderer `fluid_surface_tests` | achievable as stated | pass |
| A22 Flash guards unchanged; wave outside displayed offset | existing `motion_tests` guards + warp composition test | achievable as stated | pass |
| A23 Fog reference core | render-cpu fog reference tests | achievable as stated | pass |
| A24 Fog reference edge cases (P9) | render-cpu fog reference tests | achievable as stated | pass |
| A25 Fog WGSL bodies pinned; outward normal read | renderer `fluid_surface_tests` textual pins | achievable as stated | pass |
| A26 Frame weight 0 when contained; bob; shared face (P7, P8) | postretro `fluid_presentation` tests | achievable as stated | pass |
| A27 Teleported-under eye and unpresented neighbour (P11) | postretro `fluid_presentation` tests | achievable as stated | pass |
| A28 Proximity monotone to plane; bob bound | render-cpu fog reference + postretro eye test | achievable as stated | pass |
| A29 Per-face proximity; floating volume bottom (P13) | render-cpu fog reference test | achievable as stated | pass |
| A30 Weight reaches renderer each frame; default 1 | renderer state test + postretro source/flow test | achievable as stated | pass |
| A31 Opacity 0 + fog draws fog alone; no fog draws nothing | render-cpu collector test | achievable as stated | pass |
| A32 Budgets: storage 8, sampled 8 of 16 | renderer budget tests | achievable as stated | pass |
| A33 Spare float; two-window GPU readback (P1) | render-cpu slot test + offscreen GPU test (log shows ran) | achievable as stated | pass |
| A34 Fluid pass opens depth read-only | renderer source-scan test | achievable as stated | pass |
| A35 Group 5 lifecycle; resize readback; no per-frame creation (P4) | offscreen extent GPU test + source scan | achievable as stated | pass |
| A36 DX12 FXC compiles fluid shader | ignored-gated FXC test, run on Windows | achievable as stated | pass |
| M1 From above: walls near surface, floor hidden ~1 m; sludge; no-fog fluid unchanged | owner, in-engine | manual | outstanding (owner) |
| M2 Dive: unchanged tint; treading no flicker; fade on exit | owner, in-engine | manual | outstanding (owner) |
| M3 Wave: whole-texel sway, varied crests, world size, scale-0.25, no jump over two periods | owner, in-engine | manual | outstanding (owner) |
| M4 Distant pool: no crawl or seam | owner, in-engine | manual | outstanding (owner) |
| M5 Resize keeps fog correct | owner, in-engine | manual | outstanding (owner) |
| M6 GPU timing before/after (1660 Super, RDNA1 Mac) | owner, in-engine; report here | manual | outstanding (owner) |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Wave CPU twin in `motion.rs` + coherence, reference and wrap tests (A12, A18, A19, A22) — falsifies the coherence bound first | worker (Opus) | — | done: arc peak slope 0.7607 texel/cell at L = 8A, bound 1 texel; 45 `fluid_surface` render-cpu tests pass |
| 2 | Scripting surface: `FluidDescriptor` fields, parser helpers, validation, typedefs, SDK, docs, dev content (A1–A11, A13, A14, A17) | worker (Opus) | — | done: foundation `fluid` 10, scripting-core `fluid` 25, postretro `fluid` 47 / `digest` 26, sim `typedef` 43 pass; flat descriptor fields; NaN from script rejects in the shared conversion layer naming the entry and field |
| 3 | Renderer: stride-48 row + packing + collector, group 5 (depth + scene uniform), read-only depth, rebuild seam, WGSL fog + wave, CPU fog reference, budgets, GPU tests (A15, A16, A20, A21, A23–A25, A29, A31–A36) | worker (Opus) | 1 | done: render-cpu `fluid_surface` 61 pass, renderer `fluid` 33 pass; P1, P4 and fog GPU cases ran on the 1660 Super; DX12 FXC compiles every full-init pipeline. Group 5 split into `render/fluid_scene_inputs.rs` (`fluid_surface.rs` hit 663 lines); the weight uploads in the fluid pass only when changed |
| 4 | App plumbing: descriptor → row look, eye fog weight, per-frame handoff (A26–A28, A30) | worker (Opus) | 2, 3 | done: postretro `fluid` 56 pass (9 new), render-cpu 62; `pack` clamps extinction finite for subnormal `fogDistance`; renderer default 1 asserted in the GPU fog test |
| 5 | Preflight, review panel, fix loop, full gate | integrating executor | 4 | done: preflight green; panel of 8 (no 🔴); two fix rounds; final gate fmt, clippy (default + dev-tools), `cargo test` 11 149 and dev-tools 11 229 passed, release check, crate graph, DX12 FXC all pass |

## Proof notes
- GPU cases ran on the GTX 1660 Super (log lines): `P1 two-window GPU case ran: slot 1359 (window 0) left=0, slot 1360 (window 1, offset 65280) right=188`; `P4 scene-inputs GPU case ran: group 5 at 64x48 after 4 builds`; `depth fog GPU case ran: weight 1=51.75 0.5=190.375 0=254`.
- DX12 FXC: `DX12 FXC test ran: every full-init pipeline compiled`.
- Coherence: arc peak slope 3.043 per unit arc → 0.761 texel per cell at L = 8A; adjacent rounded offsets differ by at most 1 texel in every swept setting.

## Review loop
- Panel: contract verifiers (defineFluid surface; WGSL ↔ CPU references), correctness tracers (group 5 lifecycle; eye weight → renderer; field → shader units seam), adversarial tester (wave/fog/row numerics), two hygiene + drift passes. No 🔴.
- Acted on: params-row member-offset pin; finite extinction cap; wave reciprocal guard; fog twin normalizes the outward normal; wave phase split once per fragment (CPU + WGSL); `drift_texels` pinned; interleaved-slot upload test; source scans moved beside their subjects with positive controls; requirement strings pinned to constants; comment and doc drift.
- Left for landing: flip the "decided, not yet built" markers in `rendering_pipeline.md` §7.3a/§7.8/§10 and `scripting.md` §2, and amend §7.3a **State** (output alpha is surface-over-fog coverage, not opacity × albedo alpha).

## Owner questions (Decision-level findings, not acted on)
1. **Bob flicker on other faces before the tint triggers.** The frame weight is 0 when contained, else 1 − ramp, so an eye treading ±0.05 m across a plane it entered without diving (ramp still 0) toggles the weight 0 ↔ 1 at bob rate. The bobbed face hides it (its proximity is 0 at the plane), but every other outside face in view with proximity > 0 — a split pool's seam face, a lower pool, a neighbouring fluid's side — flips between murk and clear. The code follows the Decision word for word. Recommended: make the eye gate continuous — weight = (1 − ramp) × clamp(h / band, 0, 1), where h is the eye's height above the fluid surface directly beneath it (0 when contained, 1 when no fluid lies within a band below) — and add a second-face bob test.
2. **Back faces over murk.** Within a volume, faces draw in a fixed order. On a floating or open-sided volume, a face seen from inside (no fog) can draw after the fogged front face and lay its unfogged surface over the murk. Walled pools emit no back faces and are exact. Recommended: when fog is set, scale an inside-facing fragment's alpha by (1 − frame weight) — from outside it sits behind the front face's murk; underwater (weight 0) it draws as today. Alternative: accept it as a limit beside "a side face with open air behind it over-fogs".

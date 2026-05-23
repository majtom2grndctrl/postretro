# M9 Probe-Weight-Correctness — Measurement Gate (Task 3)

> **Status: VISUALLY VERIFIED (qualitative); closed by maintainer decision
> 2026-05-23.** The fix was confirmed on a local GPU run: the maintainer
> observed the indirect-light leak through walls visibly reduced in the *after*
> vs. *before*. The quantitative fields (CPU frame-time before/after) and the
> `before.png`/`after.png` pair were captured locally but **not transcribed into
> the repo**, so no numeric before/after delta is recorded here. Spec #2 (probe
> depth/visibility atlas) was intended to read that delta to justify the atlas;
> if it needs one, re-run the procedure below to regenerate it.

## Commit anchors

| Image  | Commit    | Notes |
|--------|-----------|-------|
| Before | `7773a33` | Plan-move commit; engine code identical to the pre-Task-1 state (hardware-trilinear SH fetch, no validity bit). |
| After  | `44f8807` | Phases 1–2 + review fixes (manual 8-corner blend, validity exclusion, backface rejection in forward). |

## Procedure

1. **Compile the leak-prone map** (per `CLAUDE.md` build commands):
   ```bash
   cargo run -p postretro-level-compiler -- content/dev/maps/occlusion-test.map -o content/base/maps/occlusion-test.prl
   ```

2. **Capture BEFORE:**
   ```bash
   git stash            # if the working tree is dirty
   git checkout 7773a33
   cargo run --release -p postretro -- content/base/maps/occlusion-test.prl
   ```
   - Set isolation mode to **StaticSHOnly** (`LightingIsolation::StaticSHOnly = 6`) via the debug UI.
   - Navigate to the known through-wall bleed structure; record the exact camera
     pose (position + yaw/pitch) below so AFTER matches.
   - Screenshot → `before.png` in this directory.
   - Record the CPU frame-time (existing `FrametimeStats`, 120-sample ring) at the fixed pose.

3. **Capture AFTER:**
   ```bash
   git checkout 44f8807   # or the branch tip claude/clever-fermi-3aJtd
   cargo run --release -p postretro -- content/base/maps/occlusion-test.prl
   ```
   - Same isolation mode, same camera pose.
   - Screenshot → `after.png`.
   - Record CPU frame-time at the same pose.

4. **(Optional, dev-tools) Validity spot-check** — confirms the baked validity
   data renders correctly (AC: dev-tools validity overlay):
   ```bash
   cargo run -p postretro --features dev-tools -- content/base/maps/occlusion-test.prl
   ```
   Use the `MarkerMode::Validity` overlay to confirm a known valid probe reads
   valid and a known invalid (in-wall) probe reads invalid. This validates the
   bake (the CPU-side `ShVolume::validity` mirror), not the GPU compose-pass
   alpha propagation — there is no direct band-0-alpha readback (`decode_l0`
   returns RGB only). The packer→alpha encoding is covered by a unit test, and a
   compose-pass propagation regression would resurface as the through-wall bleed
   in steps 2–3.

## Fields to fill in

- **Map:** `content/dev/maps/occlusion-test.map` → `occlusion-test.prl`
  - (Open question from the plan: if the bleed structure is not observable from a
    single fixed pose, fall back to `campaign-test`. Note which map was used.)
- **Camera pose:** not recorded (qualitative close-out).
- **Isolation mode:** StaticSHOnly (6)
- **Before commit:** `7773a33`
- **After commit:** `44f8807`
- **Qualitative residual (through-wall bleed):** reduced/gone — confirmed
  visually by the maintainer on a local GPU run (2026-05-23).
- **Qualitative residual (near-wall darkening):** not separately recorded in
  this qualitative pass.
- **CPU frame-time before:** captured locally; not transcribed.
- **CPU frame-time after:** captured locally; not transcribed.
- **Frame-time delta:** not recorded — note: wall-clock; reflects the added
  72-fetch cost only when GPU-bound (see plan Open questions, "72-fetch cost").

## Acceptance criteria checked here

- [x] Through-wall bleed at the `occlusion-test` structure visibly reduced/gone vs. before. — confirmed visually 2026-05-23.
- [ ] Near-wall surfaces no longer darken from in-wall (invalid) probes. — not separately recorded.
- [ ] (dev-tools) `MarkerMode::Validity` overlay renders baked validity correctly at a known valid and a known invalid (in-wall) probe (validates the bake, not the GPU compose propagation). — not run.
- [ ] before.png / after.png committed in this directory. — captured locally; not committed.

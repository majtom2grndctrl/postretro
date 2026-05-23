# M9 Probe-Weight-Correctness — Measurement Gate (Task 3)

> **Status: NOT YET CAPTURED.** This gate requires running the engine with a GPU.
> It could not be executed in the headless cloud session that implemented the
> fix. Run the steps below locally, fill in the bracketed fields, and commit the
> screenshots alongside this file. Spec #2 (probe depth/visibility atlas) reads
> the before/after delta recorded here to justify the atlas.

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

4. **(Optional, dev-tools) Validity spot-check** — verifies the compose-pass
   alpha-propagation directly (AC: dev-tools band-0 alpha):
   ```bash
   cargo run -p postretro --features dev-tools -- content/base/maps/occlusion-test.prl
   ```
   Use `ShProbeReadback` or the `MarkerMode::Validity` overlay to confirm total
   band-0 alpha == 1 at a known valid probe and == 0 at a known invalid (in-wall)
   probe.

## Fields to fill in

- **Map:** `content/dev/maps/occlusion-test.map` → `occlusion-test.prl`
  - (Open question from the plan: if the bleed structure is not observable from a
    single fixed pose, fall back to `campaign-test`. Note which map was used.)
- **Camera pose:** position `[ x, y, z ]`, yaw `[ ]`, pitch `[ ]`
- **Isolation mode:** StaticSHOnly (6)
- **Before commit:** `7773a33`
- **After commit:** `44f8807`
- **Qualitative residual (through-wall bleed):** [ gone / reduced / unchanged — describe ]
- **Qualitative residual (near-wall darkening):** [ gone / reduced / unchanged ]
- **CPU frame-time before:** [ ms ]
- **CPU frame-time after:** [ ms ]
- **Frame-time delta:** [ ms ] — note: wall-clock; reflects the added 72-fetch
  cost only when GPU-bound (see plan Open questions, "72-fetch cost").

## Acceptance criteria checked here

- [ ] Through-wall bleed at the `occlusion-test` structure visibly reduced/gone vs. before.
- [ ] Near-wall surfaces no longer darken from in-wall (invalid) probes.
- [ ] (dev-tools) total band-0 alpha matches baked validity at a known invalid (0) and valid (1) probe.
- [ ] before.png / after.png committed in this directory.

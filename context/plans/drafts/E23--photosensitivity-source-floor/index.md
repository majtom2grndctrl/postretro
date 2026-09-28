# E23--photosensitivity-source-floor

Brief · compact or resumable (session decides) · Epic 23 (U1 follow-up) · reads: `context/plans/ready/E23--accessibility/index.md` (D4, I2), `context/lib/rendering_pipeline.md` §7.8, `context/lib/scripting.md` §12, `context/lib/ui.md` §3 · read at 5be7ca580

> **Seed.** Written when the owner withdrew U1's GPU frame limiter (2026-09-28, `E23--preferences-comfort-floor/plan.md` §Corrections, owner decision C). Nothing below is decided yet. Run `/draft-session` on this problem before the brief is written.

## Problem

A gap U1 leaves open by decision. U1 ships a photosensitivity floor only on the two screen-effect channels: the channel clamp limits `screen.flash` and `screen.vignette` as they pack into the resolve's effect uniform. Every other way content can strobe reaches the screen unlimited: a light animation, a script toggling a light or its intensity, a UI panel's color or visibility, an emissive, a sprite flipbook, a camera cut between bright and dark rooms, a `levelLoad` restart loop through the splash. A mod can author any of them. When this is done, every engine primitive that can make the screen flash is limited at its source by the WCAG 2.2 flash rules, with a negligible CPU cost and no GPU cost, and a player preference switches off engine-owned flash effects (muzzle and impact flash lights) outright.

## Verified facts

- **Why not a frame limiter.** U1 built and withdrew a GPU limiter over the composited frame (16×9 cell means, measure and limit compute passes). A GPU test feeding a textured, panned scene with nothing flashing (`flash_limiter_motion_test.rs`, removed with the limiter; in git at the U1 withdrawal commit) found cell means read camera motion as flashes. At full contrast a quarter-screen-per-second pan was altered on 168 of 240 frames, up to 77 of 144 cells, by up to 0.66 luminance. Turns, look flicks and shake were worse, walk bob and a still view were clean. The pre-review version (`1da7418ca`) misfired about half as much. The misfires came from both the budget hold (pans counted as flashes) and the rate cap (fast turns). A fix needs motion compensation, which no shipped limiter documents. The owner judged it high risk and unsettled for a first-person shooter.
- **Measured cost of that limiter.** Mac Metal: 0.63 vs 0.39 ms at 1080p, 1.69 vs 1.04 ms at 4K, limiter on vs off. The source floor should cost neither.
- **The rules already exist on the CPU.** `crates/render-cpu/src/flash_clamp.rs` implements the transition model (IRIS accumulation, 0.1 threshold, darker state below 0.8, red on chromaticity, six-transition window, 4.0/s rate cap, presented-frame time, hitch ceiling) for the two effect channels. The independent WCAG counter that proved it is reusable as a test oracle.
- **The setting exists.** `accessibility.flashLimiter`, its engine-panel-only toggle and its attribution (D11) landed in U1 and now switch the channel clamp. A source floor extends what that toggle switches; no new setting is needed for the floor itself.

## Candidate direction (not decided)

- Clamp at each primitive a mod can drive: light animation curves and scripted light intensity or color writes, emissive animation, UI color and visibility bindings, sprite flipbook frame rates, and the camera. Each clamp uses the channel clamp's rules. It cannot see screen area, so it assumes the worst: any strobe faster than 3 Hz whose change reaches the threshold is limited.
- Add a "reduce flashing" preference that disables engine-owned flash effects: muzzle flash light, impact flash light, and animated light radius pulses.
- Add a build-time warning in `prl-build` and the asset bake for light animations and flipbooks that strobe faster than 3 Hz.
- In `docs/`, tell mod authors to check captures with EA's IRIS or Harding FPA before shipping.

## Questions for the session

- **Combined sources.** Two sources can each be within limits and strobe together (a light and a UI panel in phase). Is a per-source floor enough, or does a shared CPU budget across primitives earn its cost?
- **Load loops.** A mod `levelLoad` loop presents gameplay ↔ splash edges at load speed. Rate-limit level restarts, or hold the splash?
- **Script camera cuts.** Can they be bounded at the camera primitive without breaking legitimate cutscenes?
- **Scope.** Is "reduce flashing" a separate preference, or the same `flashLimiter` toggle?
- **Clamp position.** Where does each clamp sit so that authored reads (`getState`) keep the authored value and only presentation is clamped (I9)?

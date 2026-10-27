# Windows measurement handoff

Branch: `claude/planar-water-spike`. Read `context/lib/index.md`, the spike contract and findings first. Implementation and Mac evidence are included on this branch. Product drafting starts at [the current drafting handoff](planar-water-spike-drafting-handoff.md); exact shipping presets remain open.

**Windows measurements collected 2026-10-23:** [initial findings](planar-water-spike-evidence/windows-2026-10-23/findings.md), [all initial tables](planar-water-spike-evidence/windows-2026-10-23/tables.md), and [bounded live release follow-up](planar-water-spike-evidence/windows-bounded-live-2026-10-23/findings.md). Initial 128 focused checks, 144 captures and four valid live probes are complete; follow-up adds 110 focused renderer tests, 16 uncapped release runs and eight accepted stills. Windows bounded live timing is complete. Remaining gaps include moving-camera/entity quality, near-water scale behavior, bounded Mac native performance, owner acceptance, mask coverage and draw counts. The checklist below is historical replay context; do not repeat completed sweeps without a new question.

## What to establish

1. Run the renderer planar GPU tests and existing fluid pins with actual adapter execution, then the Windows FXC full-init pipeline test. A skipped adapter or zero matching tests is not proof.
2. Measure planar versus SSR High and Medium on GTX 1660 Super. Record GPU spans `planar_reflection`, `planar_forward`, `fluid_reflection`, whole-frame time and CPU `mirror_walk` / `render_submit`. Confirm planar records no SSR and logs no mirror SolidCellFallback or validation error.
3. Sweep planar scale ¼, ½, 1; D and R unlimited, 30, 15, 8 m, starting with independent one-variable sweeps. Settings are live dev-panel controls, not player options. Keep resolution/pose/validation/profile fixed and alternate comparison order.
4. Test across-water and downward poses on lake, e24 and reflection-probes-test. Include mirror-only cells, movers/skinned meshes, grazing precision, seams and fade visibility. Record mask coverage and draw count where available; report missing instrumentation explicitly.

Mac answer so far: reflection GPU work is cheaper than High but costlier than Medium at one lake pose, while synchronized completion is slower. The central unanswered question is whether D/R/scale produce an acceptable image and a whole-frame win on target hardware. Capture does mirror preparation once for a static pose, so its CPU reports do not replace a live mirror-walk or submit measurement.

## Build and smoke (PowerShell, workspace root)

```powershell
git fetch origin
git switch claude/planar-water-spike
git pull --ff-only
cargo test -p postretro-render-cpu planar_reflection
cargo test -p postretro-visibility mirror
cargo test -p postretro-renderer --features dev-tools planar
cargo test -p postretro-renderer --features dev-tools fluid_
cargo test -p postretro-renderer --features dev-tools --lib dx12_fxc_compiles_every_full_init_pipeline -- --ignored --nocapture
cargo test -p postretro --features dev-tools,capture planar
cargo run -p postretro-level-compiler -- content/dev/maps/reflection-lake-test.map -o content/dev/maps/reflection-lake-test.prl --no-tui
cargo build -p postretro --features dev-tools,capture
New-Item -ItemType Directory -Force target/planar-water-measurements
$env:RUST_LOG = 'info'
$env:POSTRETRO_GPU_TIMING = '1'
$env:POSTRETRO_CPU_TIMING = '1'
$env:POSTRETRO_SH_STREAMING = 'sync-proof'
$env:POSTRETRO_PLANAR_REFLECTION = '1'
& ./target/debug/postretro.exe --capture context/plans/in-progress/planar-water-spike-evidence/windows/planar-scene.json
$env:POSTRETRO_PLANAR_REFLECTION = '0'
& ./target/debug/postretro.exe --capture context/plans/in-progress/planar-water-spike-evidence/windows/ssr-high-scene.json
& ./target/debug/postretro.exe --capture context/plans/in-progress/planar-water-spike-evidence/windows/ssr-medium-scene.json
```

Capture scenes have portable repository-relative paths, 960×540, 120 warmup and 240 sample frames. They establish the default half-scale/unlimited-D/R baseline. D/R/scale and fade widths are now explicit `planar_reflection` capture JSON fields. Use the portable matrix runner below; committed Mac scenes preserve Mac output paths and should not be replayed verbatim on Windows. Confirm the log selects the intended plane before accepting any result.

For live profiling, build the required authoring helpers first per AGENTS.md, then use the contract's authoring launch with dev-tools. Remove `POSTRETRO_SH_STREAMING=sync-proof` for ordinary streaming/live performance and label that difference. Do not compare capture wait-after-submit timing to live frame pacing. Close the engine/launcher after each run; no bakes or builds alongside measurements.

## Provenance and records

The committed lake source includes the local TrenchBroom edits used for later profiling (raised ceiling and additional light/geometry). Early short lake captures predate that fixture change; do not compare their times directly to later traces. The Mac compiled PRL hash is in `metal-2026-10-22/manifest.json`; record the Windows bake hash and compiler settings rather than claiming byte equality across platforms.

Mac evidence is versioned; latest Windows evidence and measurement source changes currently include uncommitted/untracked files in the existing spike checkout. Preserve them before changing or archiving checkouts. Raw Mac Instruments bundles/XML stay on the Mac at their recorded locations; they are not required to draft or run Windows measurements. Store new reports/scenes and findings in the evidence directory, with adapter/driver, commit, map hash, profile, validation state, pose, resolution, settings and repetition order. Add findings to the existing note; keep `context/lib/` unchanged during drafting.

## Automated knob sweep now available

After the build above, run from the workspace root with Python 3 (no third-party dependencies):

```powershell
python context/plans/in-progress/planar-water-spike-evidence/knob-sweep/run_sweep.py --binary target/debug/postretro.exe --output target/windows-planar-knob-sweep
```

The runner defaults to 120 warmup/240 measured frames to give GPU timestamp readbacks enough frames to produce windows (the Mac knob dataset used 60/120). It generates 60 matched captures: 15 configurations, across-water and over-water downward poses, two reverse-order repeats. It forces indirect validation on and refuses nonempty output folders. `--pose across` or `--pose down` narrows the run, and `--repeats` changes repeat count. See `planar-water-spike-evidence/knob-sweep/README.md` for Mac numbers, screenshots, outliers and selected Metal traces.

Prioritize confirming the D savings and whole-frame cost on Windows; quarter resolution saves GPU work on Mac but not completion time. Do not accept the fastest D15/R15 combination solely on speed: distant planar reflections visibly give way to probe fallback. Live CPU walk/submit, mover/skinned scenes and moving-camera quality remain essential.

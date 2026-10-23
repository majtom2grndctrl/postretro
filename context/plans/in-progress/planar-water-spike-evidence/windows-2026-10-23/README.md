# Windows planar reflection evidence — 2026-10-23

Results: [findings](findings.md), [all timing tables](tables.md), [compact per-run data](summary.json), [live CPU results](live/summary.json).

144 matched captures (34,560 measured frames), four accepted live probes and 128 focused checks. The GTX 1660 SUPER's large-lake Vulkan captures improve substantially over SSR High at 1080p/4K, but the live unlimited mirror walk adds about 3 ms CPU work per frame. Keep planar dev-only pending bounded live timing and owner moving-camera/entity checks.

## Evidence layout

- `lake-sweep/`: 60 captures, 15 settings × two poses × two reversed-order repeats.
- `resolution/`: 20 across-water captures at 1080p/4K, High/Medium and three planar scales.
- `coverage/`: ten 4K downward captures with the same five controls.
- `fixtures/`: 24 e24/probe-room captures, across/down, High/Medium/half planar. Mirror-only cells have clean lightmap residency counters.
- `bounds/`, `long-bounds/`: D45/D60 and D80/D100 follow-ups.
- `tiers/`: Off/Low/Very High/Ultra and planar Off/Low gates.
- `dx12/`: six matched backend controls.
- `live/`: four accepted 20-second launches, ordinary streaming, actual surface/scene 1600×900, vsync; CPU/GPU tail-window summaries.
- `windows-manifest.json`, suite manifests and telemetry CSVs: hashes, backend/profile/settings and machine state.
- `test-*.log`: focused CPU, visibility, renderer, existing fluid, FXC and app checks.
- `*-bake*.log`: bake provenance, including the failed temporary-file rename and interrupted pre-pause retry. `probes-bake-resume.log` is the successful probe bake.
- `live-invalid-start/`: excluded capsule-centre y=0 attempts; corrected launches start above the deck. Never use these as planar performance evidence.

All scene/report/log/PNG outputs are retained. Scripts run from the spike worktree root (`.claude/worktrees/fluid-ssr-reflections-0b3212`, branch `claude/planar-water-spike`). Main checkout remains on `ghost-entities`. Build/bake steps are in the Windows handoff; replay details are in findings. Fresh output directories are required.

The owner paused after the lake bake to use the computer for gaming, then resumed after closing the game and Steam. No bake, build, game or second capture ran alongside timed measurements. All launched spike processes are closed. No engine/shader source edits, shipping configuration changes or PR were made.

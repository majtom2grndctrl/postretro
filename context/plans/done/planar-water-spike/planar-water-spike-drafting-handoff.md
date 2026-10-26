# Planar + probe product drafting handoff

Status: enough evidence to draft the product feature; exact shipping presets and visual acceptance remain open. This is a handoff from a spike, not a ready implementation specification.

## Workspace

Use the existing Windows spike checkout:
`C:/Users/danhi/Projects/Personal/postretro/.claude/worktrees/fluid-ssr-reflections-0b3212`

Branch `claude/planar-water-spike`, base HEAD `1ff80ed97e60aa8cdeeb1f8463aaef352094662f`. The main project checkout is a different branch. Latest Windows evidence, notes and launch controls include uncommitted/untracked files in this checkout. A fresh branch/worktree from HEAD will omit them. Preserve this checkout; do not clean, reset, archive or delete artifacts during drafting. Stage only the eventual draft folder if committing the draft.

## Read order

1. `context/lib/index.md` and `.agents/skills/draft-plan/SKILL.md`.
2. This note, then `planar-water-spike-contract.md` (existing implementation constraints, including protected composite and source-scan boundaries).
3. `planar-water-spike-findings.md` (cross-platform synthesis).
4. `planar-water-spike-evidence/windows-bounded-live-2026-10-23/findings.md` (latest native release evidence, stills and limitations).
5. Earlier Windows `windows-2026-10-23/findings.md`, Mac `knob-sweep/README.md` and `metal-2026-10-22/README.md` as needed. Paths in items 2–5 are relative to `context/plans/in-progress/`.

Load rendering, player-options, resource-management and build/authoring library sections through the index before designing product boundaries. Ground every named code identifier in current source. Historical measurement handoffs and old recommendations are replay context; latest bounded live findings supersede their claims that Windows bounded timing is missing.

## Product direction to draft

The owner intends to combine reflection probes with either SSR or planar. Large flat water is the planar use case; nearby reflected content should remain useful when it leaves the main screen, while probes provide distant coverage. Losing distant planar detail is an acceptable design tradeoff. Do not require parity with unlimited planar as a visual acceptance condition.

Recommendation from the spike: productize bounded planar + probes as an optional mode for large flat water. Retain SSR and probes-only alternatives. Do not assume the owner has approved universal planar defaults, fixed platform policies, an authoring schema, or public distance knobs. Those decisions belong in drafting. Existing spike settings are debug tools, not a shipping UI contract.

Half scale, D15, R unlimited, 4 m fade is the first visual-test candidate; D30 is the greater-reach comparison. D limits reflected-scene path distance from the mirrored eye; R limits receiving water distance from the real eye. Bands fade over the final metres before the limit. Presets remain provisional.

## Evidence that drives the decision

| Dataset | Finding | Interpretation |
|---|---|---|
| Windows native release, 1600×900, uncapped | High 4.828 ms/frame; half unlimited 7.830; half D30 5.817; half D15 5.070 | Bounded planar is viable enough to develop; no live FPS advantage over High demonstrated at this pose |
| Same matrix | D15 mirror CPU 0.061 ms versus unlimited 2.749; R8 2.773 | Distance bounds largely remove extra traversal cost; no visibility rewrite is justified solely by the unlimited result |
| Same matrix | Quarter D15 5.027 ms, half D15 5.070 | Under 1% frame saving; not a strong reason to select quarter |
| Fresh static captures | Quarter D15 equals probes-only byte-for-byte, repeated independently; half D15 has a broad near-water contribution absent at quarter | Investigate local contribution/scale correctness before selecting settings; do not describe quarter as equally useful |
| Windows static 4K across-water | Half synchronized completion 13.846/13.700 versus High 18.318/18.300 ms | Large-water GPU promise; capture prepares mirror visibility once, so these are not native frame/FPS figures |
| Mac Metal | Reflection GPU work decreases, but synchronized completion and some bounded/downward cases trail alternatives | Do not infer a universal platform win from Windows |

Latest live matrix: 16 accepted runs, eight settings, forward/reverse order, 9,600 reduced CPU frames, 5,760 GPU readbacks; separate pilot. Ordinary streaming, release optimization, indirect validation off. Earlier Windows: 144 captures with validation on, four accepted dev/VSync live probes, 128 focused checks. Latest launch controls: 110 focused renderer tests plus feature checks/lint/format passed. Eight accepted stills have clean counters and complete GPU windows. Reports preserve binary/map/source hashes, profile, pose, settings and limitations.

Do not merge dev/VSync, synchronized capture, release live or Metal trace numbers into a single benchmark. Mac/Windows PRL bakes differ. Neither automated checks nor stills establish motion quality or owner visual acceptance.

## Decisions and checks the draft must cover

- Mode ownership and author/player control: how content opts in, how quality choices interact, defaults, fallback, persistence and SDK documentation. Decide scope explicitly; do not automatically expose all debug knobs.
- One-plane selection, transitions between planes, below/near-water behavior, nonselected water, and water without a probe. The spike supports one selected horizontal probe-bound plane; do not silently expand to arbitrary mirrors or multiple extra views.
- Useful nearby static and moving-object reflections as the camera moves and content leaves the main screen. Assess distance handoff continuity, grazing precision and waterline seams, rather than unlimited-image parity.
- Investigate quarter-D15 disappearance and the half/quarter near-water difference; unresolved cause must not become an accepted optimization.
- Mirror-only cell demand must remain coupled to mesh/mover collection, lighting residency and animated-lightmap preparation. Static lake runs have zero mirror-only cells; earlier e24/probe fixtures provide separate union/residency evidence.
- Bounded Mac native behavior and representative coverage/resolution tests. Define product performance expectations deliberately; the spike's fastest measurement is not an acceptance threshold.
- Preserve renderer ownership, frame ordering, existing reflection composition/source-scan constraints and 2 GiB target. No `unsafe` without approval. Retire diagnostic-only plumbing deliberately if appropriate.

Draft under `context/plans/drafts/` using the draft-plan workflow. Keep research in a sibling `research.md`. Do not promote, implement the feature, or update `context/lib/` during drafting. Avoid rerunning completed sweeps unless a new design question requires it. Final source review/implementation gates are future work, not satisfied by the spike's focused tests.

## Artifact retention

Inventory on 2026-10-23: all evidence about **212.32 MiB** (1,008 files, including small generated caches); rebuildable `target/debug` about **23,989 MiB**, `target/release` about **3,015 MiB**. No deletion performed by this audit.

| Keep | Purpose |
|---|---|
| Contract, synthesized findings, this handoff, corrected Windows replay handoff | Design intent, decisions, constraints and limitations |
| Entire `planar-water-spike-evidence/` tree | Original per-run reports/logs/scenes, timing summaries, manifests, telemetry, test/review records and repeatable visual evidence; about 212 MiB is small enough to retain intact |
| Fixture map sources and spike source changes, including untracked launch settings | Rebuild and ground the product draft; hashes alone cannot recover uncommitted source |
| Three measured Windows PRLs locally or in an artifact archive | Exact follow-up inputs without repeating bakes: lake 84,508,363 bytes; e24 64,352,450; probes 42,450,690. They are ignored outputs, not proposed Git additions |
| Raw Mac Instruments trace bundles/XML, one archived copy if available on Mac | Future timeline/re-extraction audit; not required for drafting and not present on this Windows host |

Safe to discard when no experiment/build is using them: this worktree's `target/debug/`, `target/release/`, empty `target/planar-bake-tmp/`, Python `__pycache__/`, and other confirmed disposable compiler caches. Binaries can be rebuilt; retaining one exact measured engine plus its script sidecar is optional for forensic replay. Deleting build outputs costs rebuild time, not measured evidence on this Windows host. This is a classification, not authorization to delete unrelated caches or another checkout's files.

Keep failed-attempt logs and their exclusion notes: they are small and distinguish bad starts from accepted measurements. Keep both D15 image repeats even though hashes match: `audit.py` checks that evidence. Do not prune individual run JSONs/logs/images from accepted suites without adapting their audits and links. Raw Mac traces are referenced outside the evidence tree (initial traces under the Mac `target/planar-spike-measurements/`; knob trace paths recorded by its Metal tooling). Their current archive status is unverified here; do not blanket-delete Mac target data on the assumption that summaries contain the original timelines.

Before deleting/archiving this spike worktree, preserve its uncommitted source and Windows evidence in a recoverable checkpoint or artifact archive. For drafting now, use the existing checkout and retained notes; no builds or raw Instruments data are needed.

# Handoff to the Windows session: mid-travel poses, Amendment 2

Written 2026-11-12 by the Mac session. The owner is moving this work to their Windows machine for a few hours while they use the Mac. Delete this file when the brief lands.

## Start-up
1. `git fetch && git switch feat/mover-light-flow--mid-travel-poses && git pull`. HEAD carries this file. The Mac session stops touching the branch once it pushes, so you are the only writer. Fetch before every push anyway.
2. Read `CLAUDE.md`, then `context/lib/index.md` (required).
3. Read the brief: `index.md` (Decisions, ACs 34–37, M9), then `plan.md`: §Owner rulings after the playtest diagnosis, §Amendment 2 with tasks 22–26, §Corrections and the AC-to-proof table.
4. Read `captures/playtest-diagnosis/consensus.md`, then the Fable and Opus write-ups beside it. The owner asks that Fable's reasoning get extra weight.
5. You run the `build-brief` skill. You are past "Report results", working an owner-approved amendment.

## Task order
1. **Task 22, AC 36.** Restate AC 36 to option 1 with the room truth at `light_poses 76`. The sweeper stays at 113. Add an assertion that no truth pose parks a mover brush plane within the enclosure band of a probe layer. Re-run under `--ignored`. Fable's arithmetic scan is in `fable-reply2.md`.
2. **Task 23, authored `light_poses`.** An authored pose whose brush plane lands within the enclosure band of a domain probe layer moves the least distance out of it. Use the bake's own enclosing rule (`Pose::encloses`, `contains_by`, `enclosure_inset` in `crates/level-compiler/src/mover_light_gate/`). Add a unit test, and a note in the `light_poses` Decision.
3. **Task 24.**
   - Draft Amendment 2's Decisions and ACs into `index.md`, with the geometry-knots amendment as the model.
   - Measure the hand-over fix candidates and the in-wall split. Present rulings A and B to the owner with captures.
   - Ask the owner before building. They can answer remotely from the Mac.
4. **Task 25.** Build what the owner rules, then capture A/B against seam-free dense truth bakes.
5. **Task 26.**
   - Review panel, fix pass, preflight.
   - The owner checks M9 in-engine on this Windows machine.
   - Note that M7 (frame cost) is Mac-only. Leave it for the Mac session if the shader changed.
6. **Land the plane** (below) only when the owner says so.

## Measuring on Windows
- Every comparison is relative (HEAD against a dense truth, captured on one machine), so redo the baselines here on DX12 or Vulkan. Don't compare against Mac numbers.
- Capture settings, as on the Mac: offscreen, release `postretro --features capture`, `POSTRETRO_SH_STREAMING=sync-proof`, `force_full_resident_sh_compose`, reflections off, one forced mover.
- The scripts in `captures/playtest-diagnosis/{opus,fable}-tools/` and `captures/motion-harness/` hard-code Mac `/private/tmp/...` scratch paths and zsh. Repoint them to a Windows scratch folder, and run them with Python 3 + numpy + Pillow.
  - `fable-tools/ocap.py` and `opus-tools/cap.py` hold the views: the owner's three shots, door ceiling, wall, sweeper east.
  - `opus-tools/prl.py` reads the probe lattice (id 47/34). Its helper `strip57lib.py` sits beside it.
- **Truth bakes:** pick `light_poses` counts that park no pose within the enclosure band of a layer (task 22's assertion). Room 177, 78 and 41 are contaminated; 178 and 76 are clean.
- **Parity baseline:** AC 1 and AC 2 parity need `prl-build` at 27c92488f. See the doc comment at the top of `crates/level-compiler/tests/mover_probe_channel_prl_parity.rs`, then point `POSTRETRO_MOVER_CHANNEL_PARITY_BASELINE_BIN` at it.
- Old `.prl` files fail to load: id 57 is at epoch 6. Rebake with `--release` and a fresh `--cache-dir`.

## House rules
- **No destructive git:** no stash, reset or checkout of others' work. Workers don't commit or edit `plan.md`; you do both.
- **Orchestrate.** Dispatch right-sized agents, and keep your own context for decisions and owner questions. Skills don't run in sub-agents, so put the implement-task discipline into worker prompts: read the context docs, examine dependent code, self-review the diff once before reporting. Background agents need acceptEdits.
- **Throttle:** the owner is away from this machine, so builds need no `-j` cap. Keep disk above 15 GB, and `cargo clean -p` the crates you churned before landing.
- **Visual ACs:** split them into automated (test-gated) and manual-visual rows. Never write a visual check as if a machine verifies it.
- **Commit trailers:** use this session's own attribution lines.

## IP rules (verbatim; paste them into every worker brief and check them in review)

From `index.md`:
> **IP rules bind unchanged.** N absolute states blended by mover position is the shape accepted for two (`ip-check.md` §Piece 7, §Tencent US12293451B2). Depth blends moment sets, the domain and masks are storage sparsity, and nothing sets a per-probe flag, zeroes a weight or excludes a probe (`ip-372` levers 2, 7). The counsel packet gains one line: states per pose, not per terminus; an enclosed terminus reads the base; an enclosed or skipped mid pose blends across. The direct block's across marks are storage sparsity: compose blends between neighbouring stored states and never zeroes a weight.

From `context/research/gi-rtx-lite/ip-372-direction-aware-visibility.md`:
- Lever 2: "Never precompute, per probe or per texel, a "blocked" flag, a "visible" bit, a threshold distance turned into a bit, or a per-direction yes/no mask, even as an optimisation."
- Lever 3: "Floor: keep 0.03 until counsel answers. Never 0."
- Lever 5: the backface form is the owner's decision.
- Lever 7: "Mover tiles (the probe channel) blend *moment sets* by mover position, never a per-probe "occluded" flag or a weight forced to zero when a door closes."
- Read question 9 (the owner-approved mover-tile wording, with its 2026-11-12 correction) and question 6 (backface half-space). Never paraphrase question 9.

## Land the plane (only when the owner says so)
1. `context/lib/build_pipeline.md`:
   - id-57 section to epoch 6;
   - step 12: Poses, Bounce, Depth, Skipping, plus geometry knots and the authored-pose band rule;
   - the trim note;
   - §Build Cache fidelity markers;
   - the `--bake-fidelity` CLI row (finishes AC 33).
2. `context/lib/rendering_pipeline.md` §4: Poses, the compose bullets, the mean-and-variance depth rule, the t′ block with its bracket, the IP masks, and Amendment 2's receiver change.
3. `context/lib/index.md`: router keys `light_poses` and `--bake-fidelity`.
4. IP docs: the Piece 7 row in `ip-check.md`; the brief's original counsel-packet line ("states per pose…"); any lever-5 change Amendment 2 rules.
5. File follow-up drafts:
   - a static patch grid at posed mover faces (AC 7);
   - per-mover recompose under 0.2 ms;
   - direct interpolation over long intervals plus the skip bar's reach on dark surfaces;
   - the zigzag skip worst case (40.6 s at 140 poses × 8k);
   - probes that ride the mover (the root fix for lattice hand-over), if Amendment 2 leaves residue.
6. Move the brief to `context/plans/done/` and mark `roadmap.md`. Delete this file.
7. Commit, fetch, push. Open the PR against `main` with the 🤖 footer and session link. Report the link.

## Owner reminders
- About 25 GB of leaked test temp folders sit under the Mac's `/var/folders/2d/gsn5t4bx2vsfbf_vybxvc86m0000gn/T/.tmp*` (the leak itself was fixed earlier on this branch). They're the owner's to clear.

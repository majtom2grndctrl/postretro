# Weapon Mount Frame Solver

## Goal
Replace the trial-and-error `--rotate-euler` loop for mounting weapons/props on
skeletal sockets with a deterministic design-time solver. An author declares the
weapon's forward/up axes in their own authoring frame; the tool computes the
engine-correct corrective rotation from the real socket joint frame, emits the
`prop_to_gltf.py` bake command, and verifies the mounted result — so a correct
mount is confirmed against the engine frame, never eyeballed in Blender.

## Preconditions
This spec extends tooling already merged to `main` and present in this tree — it
does not depend on an open branch. The extraction source and fixtures exist now:
- `crates/model/examples/socket_dump.rs` — the throwaway diagnostic whose
  engine-frame solve (load, socket-frame sample, geometric barrel/up detection,
  the corrective delta `D = S^T·G^T`, the verify metrics) Task 1 promotes into
  `postretro-model`. This is the extraction source.
- `tools/prop_to_gltf.py` — the Blender mesh-baking authority (`--grip`,
  `--scale`, `--rotate-euler`/`rotate_mesh`, `--socket NAME=NODE` extras); the
  solver emits a `--rotate-euler` for it and never reimplements vertex baking.
- `tools/mixamo_to_gltf.py` — the skeleton importer; its `--yaw` rotates the
  whole rig (including the hand joint's model-space frame), the origin of the
  "how many bakes per weapon" question (§Open questions).
- `content/dev/scripts/limitator.ts` — documents the AR_4's re-tuned grip
  orientation for this skeleton bake and the unwired wrist-reorienting reload.
- Fixtures: `content/dev/models/limitator/model.gltf` (skinned holder, sockets
  `hand_l`/`hand_r`) and `content/dev/models/ar_4/model.gltf` (the AR_4 prop,
  node `AR_4`, no socket tags).

## Scope

### In scope
- A shared engine-frame solve in the `postretro-model` crate: geometric barrel/up
  detection, socket-joint frame resolution at a reference `(clip, time)`, the
  glTF-space corrective delta, and mount verify metrics. Promoted from the
  throwaway `crates/model/examples/socket_dump.rs`.
- An xtask subcommand (`solve-weapon-mount`) that loads the skeleton and weapon
  glTF via the engine loader, runs the solve, converts the corrective to a Blender
  XYZ euler, and emits a ready-to-run `prop_to_gltf.py --rotate-euler …` command.
- An authoring-intent contract: the author declares weapon-local barrel and up
  axes; geometric detection is a labelled assist/fallback, never a silent source
  of truth.
- A check mode on the same subcommand: mount the baked weapon, report `barrel·+Z`,
  `barrel·+Y`, `up·+Y`, exit non-zero when outside tolerance. This is the
  no-Blender acceptance path.
- Compose-with-current: refine an already-`--rotate-euler`-baked weapon by
  emitting the TOTAL euler to re-bake from the raw source.
- Re-point `socket_dump.rs` at the shared solve so the extracted logic has a live
  consumer.

### Out of scope
- Any engine-loader axis/bone/vertex conversion. The loader stays glTF-spec-correct
  and reads raw joint matrices (`resource_management.md` §7). Non-negotiable.
- A runtime/descriptor per-attachment corrective rotation (a rotation field on
  `mesh.attachments`). Rejected in Direction.
- Reimplementing mesh-vertex rotation and glTF re-export in Rust. Blender
  (`tools/prop_to_gltf.py`) stays the mesh-transform authority; the tool feeds it
  a euler.
- Fixing wrist-reorienting poses (e.g. the limitator crouching reload) for rigid
  mounts. A rigid bake is exact only at the reference pose; poses that reorient
  the hand need a skinned weapon. Documented degrade, not fixed here.
- Persisting the declared axes as weapon-glTF extras (self-describing weapons).
  Possible follow-up; the MVP declares axes as tool arguments.
- Automatic build-hook integration. Like `prl-build` and `bake-model-textures`,
  this is a manual design-time command.
- `context/lib/` doc capture. Happens at promotion, not in this plan (§Open
  questions notes the target).

## Direction

**Problem.** The engine mounts a skinned attachment at `holder_transform *
socket_matrix` with no per-socket offset (`attachments.rs` `emit_for_holder`),
and `socket_matrix` is the socket joint's spec-correct glTF world matrix
(`compose_world_pose`: `world[parent] * local`). So a weapon points forward only
if its own baked local frame already compensates for the socket joint's frame.
Today the author guesses that compensation as raw `--rotate-euler` degrees, bakes
via `prop_to_gltf.py`, verifies in the engine frame via `socket_dump.rs`, and
repeats. The cause is that the author is asked to supply a value (a corrective
Euler across two mismatched frames) they cannot observe, instead of intent they
can.

**Prior commitments.**
- Engine loader reads raw glTF matrices — "no import-time normalization, no
  node-transform bake into vertices" (`resource_management.md` §7). The correction
  cannot live in the loader; it must be baked into weapon vertices upstream.
- Coordinate/frame translation belongs at a tooling/compile boundary, not the
  runtime: "each input format translates its own coordinate axes … before reaching
  shared compiler logic. Format-specific helpers belong in the format adapter, not
  shared code" (`build_pipeline.md`, Supported Map Formats). This spec places the
  authoring-frame → engine-frame conversion at exactly such a boundary (an xtask
  design-time tool), and keeps the Blender-euler mapping in the tool layer while
  the engine-frame math lives in the model crate.
- Design-time asset tooling is a manual command in this project (`prl-build`,
  `bake-model-textures`), not a build hook. This tool follows that shape.
- `resource_management.md` §7 already frames the correction as baked via
  `prop_to_gltf.py --rotate-euler` and names `socket_dump` as the mandatory engine-
  frame verifier. This spec keeps that division and automates the solve between
  them; it does not diverge from the baked-into-the-weapon contract.
- `context/plans/done/E21--bone-sockets-attachments/index.md` (line 25) already
  ruled per-attachment local offset/rotation out of scope for this subsystem: "The
  prop's own authored origin is the grip point; the socket joint poses the prop.
  Art fixes placement in the prop or the socket joint, not in data." This spec
  keeps placement in the prop (baked orientation) and adds the tooling that
  computes it — it does not add the offset/rotation-in-data that E21 foreclosed.

**Alternatives rejected.** The strongest rival is a runtime per-attachment
corrective rotation: add a rotation to `mesh.attachments`, mount at `holder *
socket * corrective`. Rejected because (a) it forks the deliberate zero-offset
mount contract and pushes a coordinate-frame concern into every descriptor and
consumer of `attachments`; (b) this subsystem already foreclosed offset/rotation-in-data
(E21 line 25, cited above), and the project's stated placement for frame
translation is the tooling boundary, not the runtime (`build_pipeline.md` above);
(c) the decoupling it buys — one weapon glTF, per-mount correction — is not needed
now, since the correction is cheap to recompute and re-bake per target with this
tool, and all current humanoids share one rig. It is not rejected on a "one bake
serves all characters" claim — that is left empirical (§Open questions).

## Acceptance criteria
- [ ] Given the limitator skeleton (`content/dev/models/limitator/model.gltf`,
  socket `hand_r`, clip `idle_aiming`, t=0), the AR_4 weapon
  (`content/dev/models/ar_4/model.gltf`), and declared weapon-local barrel/up
  axes, `solve-weapon-mount` emits a Blender XYZ euler and a copy-paste
  `prop_to_gltf.py --rotate-euler …` command line.
- [ ] Check mode on a weapon baked with that euler reports `barrel·+Z ≥ 0.999`,
  `|barrel·+Y| ≤ 0.02`, and `up·+Y ≥ 0.999` for the limitator/`hand_r`/`idle_aiming`
  case, and exits zero.
- [ ] Check mode exits non-zero, and names which metric failed, when a weapon is
  baked with a deliberately wrong euler (e.g. barrel facing −Z or rolled 90°).
- [ ] When barrel/up axes are NOT declared, the tool runs geometric detection,
  labels the result as an assist (with the detected axes and a confidence signal),
  and emits any resulting euler flagged UNVERIFIED — never a trusted euler, never
  a geometry-only guess presented as authoritative.
- [ ] A geometrically ambiguous weapon is reported as low-confidence in assist
  mode rather than silently resolved. Ambiguity is pinned by two reproducible
  cutoffs on the detection outputs: the end cross-section radii are within a
  factor of 1.5 (`max(ra,rb)/min(ra,rb) < 1.5`, near-symmetric cross-section), or
  the long-axis length is under twice the larger end diameter
  (`len / (2·max(ra,rb)) < 2.0`, not clearly elongated). The declared-axis path
  is unaffected by geometry.
- [ ] Compose-with-current: given a weapon already baked with euler `E0` and a
  residual solve, the tool emits a TOTAL euler `E1` such that re-baking the raw
  source with `E1` passes check mode; solving the same case from the unrotated
  source yields a euler that passes check mode to the same tolerance.
- [ ] Running at a `(clip, time)` other than the reference (`idle_aiming`, t=0) —
  e.g. the limitator `reloading` clip — prints a distinct NOTE naming the actual
  `(clip, time)` and the limitator `reloading` clip, stating that a rigid bake is
  exact only at its solve pose and that wrist-reorienting poses need a skinned
  weapon; the tool does not silently claim a single euler satisfies both poses.
- [ ] `socket_dump.rs` builds and runs against the shared solve; its output for
  the reference invocation (research.md §Fixtures) is diffed against a golden
  baseline captured before extraction, and the `MAT` socket-frame line and all
  verify metrics are byte-identical (no drift).
- [ ] The shared solve reuses `postretro_model::gltf_loader::load_model` and
  `sample_clip_looped_world_modified` with `&model.pose_stack` and `inputs = None`
  — not a reimplementation — so the socket frame it solves against is identical to
  the frame the engine mounts at, at the reference `(clip, time)` with
  `inputs = None` (a modified or differently-looped runtime pose diverges by
  design; see Invariants).

## Tasks

### Task 1: Extract engine-frame mount solve into `postretro-model`
Promote the engine-frame solve out of `crates/model/examples/socket_dump.rs` into
a new public module `crates/model/src/mount.rs`. It owns, in engine (glTF) frame
only: (a) geometric barrel/up/side detection over a loaded weapon's mesh vertices
— long axis from the extreme vertex pair refined by end-region centroids, muzzle =
the thin (smaller cross-section radius) end, up = opposite the mean lateral mass
offset from the bore — returning the right-handed frame `[side, up, barrel]` plus
a confidence/ambiguity signal built from the detection outputs: the two end
cross-section radii `ra`/`rb` and the long-axis length `len`. The signal reports
LOW confidence when `max(ra,rb)/min(ra,rb) < 1.5` (near-symmetric cross-section —
muzzle vs. stock indistinguishable) or `len / (2·max(ra,rb)) < 2.0` (not clearly
elongated — long-axis direction unreliable); these are the reproducible cutoffs
AC #5 checks. (b) resolution of a named socket joint's world matrix at a given
`(clip, time)`, reusing `load_model` and `sample_clip_looped_world_modified` with
`&model.pose_stack` and `inputs = None` (the neutral, modifier-free path — with
`inputs = None` the sampler short-circuits to `sample_clip_looped_world`, the same
composition `attachments.rs::sample_modified_world_pose` reaches at the neutral
reference), so there is no frame drift from the runtime mount at that reference
(see Invariants for the scope of this equality); (c) the glTF-space corrective
delta `D = S^T · G^T`
(`S` = normalized socket rotation from the joint world matrix, `G` = the weapon
frame) that maps barrel→+Z and up→+Y; (d) the verify metrics `barrel·+Z`,
`barrel·+Y`, `up·+Y` given a socket matrix and a weapon frame. Do NOT include any
Blender/authoring-tool euler mapping here — that stays at the tool layer (Task 2).
Take declared axes as an input to the corrective/verify path so callers can bypass
geometric detection. Re-point `socket_dump.rs` to call this module for its socket
dump, its geometric barrel/up detection, its corrective delta, and its verify
metrics, deleting that duplicated engine-frame math from the example. The example
keeps its own CLI and its inline Blender-XYZ-euler decomposition (that mapping is
example-local and is not what Task 2 promotes to the tool layer); its emitted
socket-frame and verify numbers must not change. Before deleting the duplicated
math, capture the pre-extraction output of the reference invocation (the
`socket_dump` command line in research.md §Fixtures) to a golden text file (a
scratch/temp file, not committed); after re-pointing, diff the new output against
it — the `MAT` socket-frame line and every verify metric must be byte-identical.
This golden diff is the check AC #8 names. `gltf_loader.rs` is large (~4400
lines incl. tests) but is not extended here; the new logic is a separate module.

### Task 2: `solve-weapon-mount` xtask subcommand — solve and emit
Add a `solve-weapon-mount` subcommand to `crates/xtask/src/main.rs`, mirroring the
`bake-model-textures` shape: one `if command == "solve-weapon-mount"` branch in
`try_main()`, a handler fn, a `parse_solve_weapon_mount_args` helper, and a
`print_help()` entry. It takes the skeleton model path, the mount-joint selector
`--mount-joint NAME` (default `hand_r` — this is the solver's socket-joint
selector, NOT to be confused with `prop_to_gltf.py`'s `--socket NAME=NODE`
extras flag, which the tool only passes through), the reference clip
`--clip` (default `idle_aiming`) and `--time` (default 0), the weapon glTF path
`--weapon`, and the authoring-intent declaration of the weapon-local barrel and up
axes `--barrel X Y Z` / `--up X Y Z` (see Boundary inventory for the exact
argument surface and axis convention). It loads both models via `postretro_model::gltf_loader::load_model`, calls the
Task 1 module to resolve the socket frame and compute the glTF-space corrective
delta from the DECLARED axes (geometric detection runs only as a labelled assist
when axes are omitted — see Task 3's mode boundary; this task wires the assist as
advisory output, not as the emitted euler), then converts the corrective delta to
a Blender XYZ euler here in the tool layer: apply the Blender-frame change of
basis `C: (x,y,z) → (x,−z,y)` and decompose `R = Rz·Ry·Rx` to XYZ degrees. It
prints the euler and a copy-paste `prop_to_gltf.py --rotate-euler X Y Z` command
line (preserving any `--grip`/`--scale`/`--socket NAME=NODE` the author passes
through for convenience — `--socket` here is the prop_to_gltf extras flag, verbatim
pass-through). It supports compose-with-current via `--current-euler X Y Z` (the
Blender XYZ `--rotate-euler` degrees already baked into the weapon): emit the TOTAL
euler to re-bake from the raw source, composed as `D_blender · R_current` before
decomposition — the same composition `socket_dump.rs` does with its positional args
6-8. `--current-euler` is the single "euler already baked in" input; Task 3's
check mode consumes the same flag as its frame bridge. The Blender-adapter math
(the change of basis and the euler decomposition) lives only in this tool layer,
per the format-adapter placement in Direction; the engine crate stays euler-free.

### Task 3: Check mode and the assist/trust boundary
Add a `--check` mode to `solve-weapon-mount` (same subcommand file, so sequenced
after Task 2). In check mode the tool takes a baked weapon (already carrying its
corrective in its vertices) plus `--current-euler X Y Z` (the `--rotate-euler`
that bake applied). Frame handling — the decided resolution of the check-mode
axis-frame ambiguity: the declared `--barrel`/`--up` are ALWAYS in the raw-source
weapon frame (one intent, shared verbatim with solve mode); because check loads
the already-rotated BAKED weapon, the tool composes the applied euler onto the
declared axes — it rotates the declared source-frame barrel/up by the glTF-space
rotation equivalent of `--current-euler` to obtain the baked-frame axes, then
feeds those to the Task 1 verify metrics at the resolved socket frame. Applying
raw-source-frame axes directly to baked geometry would measure the wrong
direction, so `--current-euler` is REQUIRED in check mode. The declared path is
thus an analytic verification of the euler against declared intent (it trusts
`prop_to_gltf.py` to have baked `--current-euler` faithfully — the same trust the
whole pipeline places in the mesh authority); the baked weapon is still loaded so
that the assist path (axes omitted) can run geometric detection on the baked mesh. (This reinforces the
args-vs-`extras` open question: a self-describing weapon that recorded its applied
euler in `extras` would remove both re-supplied inputs. See Open questions.) It
prints both the declared source-frame axes and the composed baked-frame axes it
validated against (so a mismatch between the solve run's axes and the check run's
axes is visible), then prints `barrel·+Z`, `barrel·+Y`, `up·+Y`, and exits
non-zero when any is outside tolerance, naming the failed metric. Tolerances
default to `barrel·+Z ≥ 0.999`, `|barrel·+Y| ≤ 0.02`, `up·+Y ≥ 0.999`, each
overridable via `--min-barrel-dot` (default 0.999), `--max-barrel-y` (default
0.02), and `--min-up-dot` (default 0.999). Check mode uses the DECLARED (composed)
barrel/up axes to know what "forward" was supposed to be, so a mis-identified
barrel cannot pass as correct (the false-positive class from the brief). Also
implement the assist/trust boundary shared with Task 2: when the author omits
declared axes, run geometric detection, print the detected `[side, up, barrel]`
with its confidence signal, and mark any emitted euler (solve mode) or any pass
(check mode) as UNVERIFIED — the tool must never present a geometry-only result as
authoritative. A low-confidence geometric result (from the end-radii/axis-length
cutoffs pinned in Task 1) prints a distinct warning. Non-reference-pose reporting
(AC #7): whenever the resolved `(clip, time)` is not the reference (`idle_aiming`,
t=0) — in either mode — the tool prints a NOTE naming the actual `(clip, time)`,
stating the euler is exact only at the pose it is solved/checked at, that a rigid
bake cannot satisfy both this pose and the reference, and that wrist-reorienting
poses (e.g. the limitator `reloading` clip) need a skinned weapon rather than a
re-solve; in check mode the tolerance exit code still reflects the metrics at the
given pose, but the NOTE marks degraded non-reference metrics as expected. This
task owns the tolerance defaults and their override flags, the exit-code contract,
the check-mode frame composition, and the labelling; Task 2 owns the solve/emit
surface it plugs into.

### Task 4: Author-facing workflow doc
Add a short authoring-loop doc for weapon/prop mounting (human-facing, under
`docs/` if a weapon/model authoring page exists there; otherwise a new page under
`docs/`), describing the deterministic loop: bake the weapon grip/scale-only with
`prop_to_gltf.py` → declare barrel/up axes (`--barrel`/`--up`) and run
`solve-weapon-mount --mount-joint hand_r` to get the euler → re-bake with
`--rotate-euler` → `solve-weapon-mount --check --current-euler <the baked euler>`
to confirm in the engine frame. Explain that `--check` needs `--current-euler`
because the declared axes are raw-source-frame while the checked weapon is already
baked, and that `--mount-joint` (the socket-joint selector) is not
`prop_to_gltf.py`'s `--socket NAME=NODE` extras flag. State the reference-pose
caveat (a rigid bake is exact at the solve pose; wrist-reorienting poses need a
skinned weapon) and the assist-vs-declared distinction. Do NOT edit `context/lib/` here — that capture is a promotion step
(§Open questions records the `resource_management.md` §7 target).

## Sequencing

**Phase 1 (sequential):** Task 1 — thin slice. Extracts the engine-frame solve and
re-points `socket_dump.rs`, falsifying the boundary assumption that the engine-frame
math separates cleanly from the Blender-euler mapping. Blocks everything.
**Phase 2 (sequential):** Task 2 — builds the xtask solve/emit on the Task 1 module.
**Phase 3 (sequential):** Task 3 — adds check mode and the assist/trust boundary to
the same subcommand file Task 2 creates (shared file → sequential).
**Phase 4 (concurrent):** Task 4 — author doc, once the tool's argument surface and
`--check` contract are settled by Tasks 2-3.

## Boundary inventory

Crosses Rust (model crate, xtask) ↔ Python CLI (`prop_to_gltf.py`) ↔ engine-frame
vs Blender-frame. Pinned once:

| Name | Rust (engine crate) | xtask tool layer | Python (`prop_to_gltf.py`) |
|---|---|---|---|
| Corrective rotation | glTF-space delta `Mat3`/quat (`D = S^T·G^T`), euler-free | Blender XYZ euler degrees, via `C:(x,y,z)→(x,−z,y)` then `R=Rz·Ry·Rx` | `--rotate-euler X Y Z` (degrees, XYZ, applied after `--grip`) |
| Declared barrel axis | unit `Vec3` in weapon-local (glTF) frame | `--barrel X Y Z`, raw-source weapon-local frame (glTF-frame components of the raw model as loaded) | n/a |
| Declared up axis | unit `Vec3` in weapon-local (glTF) frame | `--up X Y Z`, raw-source weapon-local frame | n/a |
| Engine forward / up targets | `+Z` forward, `+Y` up (mount target) | same | n/a |
| Mount-joint selector | `SocketBinding::SkinnedJoint`, sampled at `(clip, time)` | `--mount-joint NAME` (default `hand_r`), `--clip` (default `idle_aiming`), `--time` (default 0) — the solver's socket-joint selector | n/a |
| Current bake euler (compose + check bridge) | n/a (engine crate euler-free) | `--current-euler X Y Z` (Blender XYZ degrees, the euler already baked into the weapon); mirrors the prior `--rotate-euler` | reads a `--rotate-euler` previously applied |
| Verify tolerances | metric thresholds consumed by the verify path | `--min-barrel-dot` (0.999), `--max-barrel-y` (0.02), `--min-up-dot` (0.999) | n/a |
| Prop socket extras (pass-through) | n/a | `--socket NAME=NODE` forwarded verbatim into the emitted command (distinct from `--mount-joint`) | `--socket NAME=NODE` (append; writes node `extras`) |

Axis-declaration convention: the author states the barrel/up axes in the weapon's
own local frame **as the engine loads it** (raw glTF vertex frame), not in Blender's
viewport frame — the tool loads the weapon through `load_model`, so the declared
axes and the geometry it solves against share one frame. The doc (Task 4) states
this explicitly with the AR_4 as the worked example.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Solver's socket frame == the engine's mount frame **at the reference `(clip, time)` with `inputs = None`** (neutral, modifier-free). The solver samples `sample_clip_looped_world_modified(clip, skeleton, time, Loop::Clamp, &model.pose_stack, None, …)`; the runtime `sample_primary_world_pose` passes `pose_inputs = Some(...)` and the state's real loop policy, so the frames coincide only at that neutral reference — a modified or differently-looped runtime pose diverges by design (the reference-pose caveat below, not drift). | Task 1 (reuse `load_model` + `sample_clip_looped_world_modified` with `inputs = None`) | Any reimplementation of load or sampling in the tool would drift it; broadening the claim past the neutral reference overstates it | AC "reuses loader/sampler" (scoped to reference/`inputs = None`), AC "socket_dump numbers unchanged" |
| Engine-frame math carries no Blender/authoring-tool mapping | Task 1 (module is euler-free) | Task 2 adds the Blender mapping only in the tool layer | Direction (format-adapter placement); AC on emit living in xtask |
| A geometry-only result is never presented as authoritative | Task 3 (assist/trust labelling) | Task 2 emit path must route geometry through the assist label, not the trusted euler | AC on undeclared-axis labelling; AC on ambiguous weapon |
| Corrective is exact only at the solve `(clip, time)` | inherent (socket frame is pose-dependent) | Task 3 prints the non-reference NOTE when `(clip, time)` ≠ (`idle_aiming`, 0), in solve or check mode; Task 4 doc restates the caveat | AC on non-reference-pose reporting (AC #7) |

## Script syntax examples

No script/descriptor change — `mesh.attachments` stays `hand_r: "models/ar_4/model.gltf"`.
The author-facing surface is the CLI loop:

```bash
# 1. Grip/scale-only bake (no orientation yet):
blender --background --python tools/prop_to_gltf.py -- \
  --input raw/ar_4.glb --output content/dev/models/ar_4/model.gltf \
  --grip 0 -0.05 0.12 --scale 0.68

# 2. Solve the corrective from declared axes + the real socket frame:
cargo run -p xtask -- solve-weapon-mount \
  content/dev/models/limitator/model.gltf \
  --mount-joint hand_r --clip idle_aiming --time 0 \
  --weapon content/dev/models/ar_4/model.gltf \
  --barrel 0 1 0 --up 0 0 1
# -> emits:  --rotate-euler <X> <Y> <Z>   and a ready prop_to_gltf.py command

# 3. Re-bake with the emitted euler (or add --bake to have step 2 drive it):
blender --background --python tools/prop_to_gltf.py -- \
  --input raw/ar_4.glb --output content/dev/models/ar_4/model.gltf \
  --grip 0 -0.05 0.12 --scale 0.68 --rotate-euler <X> <Y> <Z>

# 4. Confirm in the engine frame (non-zero exit == mount is wrong).
#    --current-euler is the euler step 3 baked; check composes it onto the
#    declared source-frame axes to reach the baked frame.
cargo run -p xtask -- solve-weapon-mount \
  content/dev/models/limitator/model.gltf \
  --mount-joint hand_r --clip idle_aiming --time 0 \
  --weapon content/dev/models/ar_4/model.gltf \
  --barrel 0 1 0 --up 0 0 1 --check --current-euler <X> <Y> <Z>

# Variant A — compose-with-current: refine a weapon already baked with E0,
#   emitting the TOTAL euler to re-bake from the raw source:
cargo run -p xtask -- solve-weapon-mount \
  content/dev/models/limitator/model.gltf \
  --mount-joint hand_r --weapon content/dev/models/ar_4/model.gltf \
  --barrel 0 1 0 --up 0 0 1 --current-euler <E0x> <E0y> <E0z>

# Variant B — loosen a tolerance for a check run:
#   --min-barrel-dot 0.995 --max-barrel-y 0.05 --min-up-dot 0.995
```

(The `--barrel`/`--up` values above are illustrative, not the AR_4's actual axes.)

## Open questions
- **How many bakes per weapon?** Whether one AR_4 bake serves the player and every
  enemy on the shared Mixamo rig, or each character/yaw bake needs its own, is left
  for the tool to answer empirically (solve against each target socket; compare the
  emitted eulers). Not assumed. If they diverge, the descriptor-corrective
  alternative (rejected in Direction) may deserve revisiting — a decision for the
  owner, not this plan.
- **Declared axes: CLI args vs glTF `extras`.** The barrel/up axes are intrinsic to
  the weapon, but as CLI args (the MVP) they must be re-supplied at both solve and
  check time — a check run given different axes than its solve run silently
  validates against a *different* intent, defeating the false-positive guard. The
  check-mode frame resolution (Task 3, finding 4) sharpens this: check now also
  requires `--current-euler` (the applied bake) to bring the declared source-frame
  axes into the baked frame, so an arg-only workflow re-supplies BOTH the axes and
  the applied euler — two chances for solve/check to diverge. The `extras` channel
  (the project's existing idiom for per-node authored intent — sockets, hit-zones,
  pose-masks) would give solve and check one source of truth and make the weapon
  self-describing; if `prop_to_gltf.py` also recorded the applied euler in `extras`
  at bake time, check would need neither input re-supplied. Deferred to keep the MVP
  arg-only, but this solve/check drift class is the reason to revisit `extras`
  before the CLI surface hardens. Owner call; kept arg-only for now, and Task 3's
  check echoes both the source-frame axes and the composed baked-frame axes it
  validated against so a mismatch is visible.
- **Optional `--bake` drive step.** Should `solve-weapon-mount` optionally shell out
  to Blender/`prop_to_gltf.py` with the computed euler for a one-command experience,
  or stay emit-only (author runs the bake)? Emit-only keeps the tool's dependency
  surface Blender-free for solve/check; a `--bake` flag adds a Blender dependency to
  that path. Recommended default: emit-only, `--bake` as a later convenience. Owner
  call.
- **Doc capture at promotion.** `resource_management.md` §7 currently names
  `socket_dump` + raw `--rotate-euler` trial-and-error as the mount-verify path.
  Update it at promotion to name `solve-weapon-mount` and the declare-axes loop, and
  demote the trial-and-error description. Not done in this plan (drafting does not
  edit `context/lib/`).

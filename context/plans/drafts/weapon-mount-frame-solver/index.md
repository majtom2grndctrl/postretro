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
  whole rig about vertical Z (including the hand joint's model-space frame), the
  origin of the "how many bakes per weapon" question that Decision 3 settles.
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
  XYZ euler, and emits a COMPLETE, ready-to-run
  `blender … prop_to_gltf.py -- --input <raw-source> --output <out> … --rotate-euler …`
  command. Emit-only: it prints the command text, it does not shell out to Blender.
- An authoring-intent contract: the author declares weapon-local barrel and up
  axes; geometric detection is a labelled assist/fallback, never a silent source
  of truth.
- Declared axes (and the applied corrective euler) persist in the weapon glTF's
  per-node `extras` under a `mount` key, so SOLVE and CHECK read one persisted
  source of truth instead of re-supplied CLI args (Decision 1). `prop_to_gltf.py`
  writes them at bake time (extending its existing `--socket` node-`extras`
  write); the model crate reads them (a new `gltf_extras.rs` reader).
  `--barrel`/`--up`/`--current-euler` remain as an optional first-author/override
  path.
- A check mode on the same subcommand: mount the baked weapon, report `barrel·+Z`,
  `barrel·+Y`, `up·+Y`, exit non-zero when outside tolerance. This is the
  no-Blender acceptance path.
- Two emit paths: the DECLARED-axes path emits the FULL from-raw euler directly
  and is stateless (no compose); the geometric-ASSIST path (axes omitted, run on
  an already-baked weapon) composes its residual onto the current
  `--rotate-euler` to emit the TOTAL euler for re-baking from the raw source.
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
- Shelling out to Blender to run the bake (a `--bake` drive step). The tool stays
  emit-only; it prints the complete command but does not run it (Decision 2). May
  be a fast-follow convenience.
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
serves all characters" claim — Decision 3 makes that default empirically
verifiable and keeps a per-character corrective as the documented escape hatch.

## Decisions

Three questions earlier review left open are settled here as owner calls. The
task paragraphs, ACs, Boundary inventory, and Invariants below are written against
these outcomes; this section carries the rationale.

### Decision 1 — declared axes persist in glTF `extras`, not CLI args
The weapon's declared barrel/up axes, and the applied corrective euler, are the
single source of truth in the weapon glTF's per-node `extras` — not values
re-supplied on the command line. This removes the solve/check drift class: an
arg-only workflow re-supplies the axes, the applied euler, and the reference
`(clip, time)` at both solve and check, and any mismatch silently validates
against a *different* intent, defeating the false-positive guard.

**Contract.** On the weapon's single mesh node (e.g. `AR_4` — the node
`postprocess_gltf` already locates for its glTF summary), per-node `extras` carries
a `mount` object:

```json
"extras": { "mount": { "barrel": [bx,by,bz], "up": [ux,uy,uz], "euler": [ex,ey,ez] } }
```

- `barrel`/`up`: unit `Vec3` in the RAW-SOURCE weapon-local (glTF) frame — the
  frame-invariant author intent, unchanged by any applied `--rotate-euler`. (These
  describe the raw geometry, so they persist correctly even on a rotated bake;
  CHECK composes the applied euler onto them to reach the baked frame.)
- `euler`: the Blender XYZ `--rotate-euler` degrees that baked THIS weapon from
  its raw source. Makes the baked weapon self-describing so CHECK reads the applied
  euler from `extras` instead of `--current-euler`.

**Written by `prop_to_gltf.py`** at bake time, extending its existing
`postprocess_gltf` node-`extras` write — the same JSON post-export step that today writes `--socket NAME=NODE` onto a node (`postprocess_gltf`'s `node["extras"]["socket"] = socket_name` write); the `--mount-axes` write applies that same node-`extras` pattern to the mesh node `postprocess_gltf` already locates for its summary. A new
`--mount-axes` flag carries barrel/up; `euler` is the `--rotate-euler` it already
applied. SOLVE emits this flag into the re-bake command, so a single run both bakes
the correction and persists the intent.

**Read by the model crate**: a new `pub(crate)` `gltf_extras.rs` reader
`read_mount_axes` (mirroring `read_socket_name` — never fail the load), surfaced on
a public `LoadedModel.mount` field the same way `read_socket_name` feeds the public
`LoadedModel.sockets`, so the xtask reads it across the crate boundary without the
reader being public. `barrel`/`up` are the core pair (either absent/malformed →
whole `mount` `None`); `euler` degrades independently to its own `None`. SOLVE reads
raw-source-frame barrel/up to build the FULL from-raw `D`; CHECK reads barrel/up
plus `euler` and composes them into the baked frame. Neither re-supplies axes on the
CLI once the weapon is self-describing.

**Persisting the euler is adopted** (rather than keeping `--current-euler` the only
source): without it, CHECK still needs the applied euler re-supplied — one of the
two drift chances survives. Since `prop_to_gltf.py` already applies
`--rotate-euler`, writing it into `extras` in the same postprocess pass costs only one added `postprocess_gltf` parameter (it currently receives just the socket list) plus one more extras key, and closes the gap.

**CLI axes/euler are the first-author + override path.** On the first solve the
raw/grip-only weapon carries no `mount` extras, so `--barrel`/`--up` are supplied
once; the emitted bake persists them. Afterward they are optional overrides
(re-persisted on the next bake), and `--current-euler` is an optional fallback for
a weapon baked before `extras` carried `euler`. When both a CLI value and a
persisted `extras` value are present, the CLI value wins and is re-persisted — the
override path, decided so re-authoring is never blocked by stale extras.

### Decision 2 — emit-only, not `--bake`
`solve-weapon-mount` prints the COMPLETE runnable `blender … prop_to_gltf.py …`
command; it does not shell out to Blender. Emit-only already assembles the full
command, so a `--bake` flag would only auto-RUN it — and would add a Blender
dependency to the solve/check path, which otherwise needs none. `--bake` is
declined for this spec; it may be a fast-follow convenience.

### Decision 3 — one bake per weapon by default; per-character correction is the escape hatch
The default assumption is that a single weapon bake serves the player and every
humanoid on the shared Mixamo rig (memory: character-model-provenance). This stays
empirically verifiable — solve against each target socket and compare the emitted
eulers — but is no longer called "open." If socket frames diverge (e.g.
`tools/mixamo_to_gltf.py --yaw` rotates a character's whole skeleton, including the
hand joint's model-space frame, about vertical Z), the documented escape hatch is a
per-character corrective: a separate baked weapon file per target, each carrying
its own `extras.mount.euler`. The rejected descriptor-corrective alternative
(Direction) is revisited only if that escape hatch proves too costly — an owner
call, not this plan's.

## Acceptance criteria
- [ ] Given the limitator skeleton (`content/dev/models/limitator/model.gltf`,
  socket `hand_r`, clip `idle_aiming`, t=0), the AR_4 weapon
  (`content/dev/models/ar_4/model.gltf`), declared weapon-local barrel/up axes
  (from the weapon's `extras.mount`, or from `--barrel`/`--up` on a first author),
  and `--raw-source`/`--out` paths, `solve-weapon-mount` emits a Blender XYZ euler
  (the FULL from-raw `D`) and a copy-paste, COMPLETE
  `blender … prop_to_gltf.py -- --input <raw-source> --output <out> … --rotate-euler … --mount-axes …`
  command line that re-bakes the mount AND persists `extras.mount` in one run.
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
- [ ] Declared solve is stateless: with declared axes the tool emits the FULL
  from-raw euler `D` and does not consume `--current-euler` for the solve; baking
  the raw source with `D` passes check mode. Geometric-assist compose: with axes
  omitted, a weapon already baked with euler `E0` (supplied via `--current-euler`,
  or read from `extras.mount.euler` when present), the tool detects the baked
  barrel/up and emits a TOTAL euler `E1` (its residual `D_geom` composed onto `E0`)
  such that re-baking the raw source with `E1` passes check mode.
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
  `postretro_model::anim::sample_clip_looped_world_modified` with `&model.pose_stack` and `inputs = None`
  — not a reimplementation — so the socket frame it solves against is identical to
  the frame the engine mounts at, at the reference `(clip, time)` with
  `inputs = None` (a modified or differently-looped runtime pose diverges by
  design; see Invariants).
- [ ] First-author persistence (Decision 1): on a weapon whose mesh node carries
  no `mount` extras, solving with `--barrel`/`--up` emits a bake command whose
  `--mount-axes` writes the declared barrel/up (raw-source frame) and whose
  `--rotate-euler` value is written as `extras.mount.euler` onto the mesh node;
  loading the re-baked weapon surfaces `mount.barrel`/`up`/`euler` with no CLI
  re-supply.
- [ ] Drift elimination (Decision 1), for the weapon file as baked — the single
  path that is solve's `--out` and check's `--weapon`: a weapon baked through the
  declared path (mesh node carries `extras.mount = {barrel, up, euler}`) checks
  correctly with NO `--barrel`/`--up`/`--current-euler` supplied — CHECK reads all
  three from `extras.mount`, composes the persisted euler onto the persisted axes,
  and reports the same metrics as an equivalent CLI-supplied check, so SOLVE and
  CHECK cannot validate against divergent intent. Decision 3's per-character
  escape hatch (a separate baked file per target) is the deliberate multi-file
  case and is exempt from this single-file guarantee.
- [ ] Mount-extras degradation is granular and never fails the load. If EITHER
  `barrel` or `up` is absent or malformed, `LoadedModel.mount` reads `None` (the
  core pair is all-or-nothing) and the tool treats axes as undeclared — CLI
  `--barrel`/`--up` if supplied, else geometric assist. If only `euler` is absent,
  the axes still surface and `MountAxes.euler` is `None` (its independent
  optionality), so a pre-euler weapon reads its declared axes while check mode
  falls back to `--current-euler` for the applied euler.
- [ ] In DECLARED check mode, when the weapon's `extras.mount` carries no `euler`
  and no `--current-euler` is supplied, the tool exits non-zero with a message
  naming the missing applied euler and computes no metrics — it never assumes an
  identity euler (which would compose the wrong baked-frame axes and false-pass or
  false-fail the check).

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
`(clip, time)`, reusing `gltf_loader::load_model` and
`postretro_model::anim::sample_clip_looped_world_modified` with
`&model.pose_stack` and `inputs = None` (the neutral, modifier-free path — with
`inputs = None` the sampler short-circuits to `sample_clip_looped_world`, the same
composition `attachments.rs::sample_modified_world_pose` reaches at the neutral
reference), so there is no frame drift from the runtime mount at that reference
(see Invariants for the scope of this equality); the tool always samples with
`Loop::Clamp` (no `--loop` flag), so a `--time` past the clip duration clamps
to the clip end; (c) the glTF-space corrective
delta `D = S^T · G^T`
(`S` = normalized socket rotation from the joint world matrix, `G` = the weapon
frame) that maps barrel→+Z and up→+Y; (d) the verify metrics `barrel·+Z`,
`barrel·+Y`, `up·+Y` given a socket matrix and a weapon frame — the verify
metrics use the RAW (un-normalized) socket matrix (`Mat3::from_mat4(m)`), while
the corrective delta in (c) uses the per-column-normalized socket rotation; both
`socket_dump.rs` and check mode call this same verify with the raw matrix so
their metrics match. Do NOT include any
Blender/authoring-tool euler mapping here — that stays at the tool layer (Task 2).
Take declared axes as an input to the corrective/verify path so callers can bypass
geometric detection. Also add a `mount`-extras reader to `gltf_extras.rs`
(`read_mount_axes`, `pub(crate)` like its sibling readers, backed by a private
`MountExtras` serde deserialize shape and returning a public `MountAxes` value —
the typed `read_joint_zone` → public `JointZone` pattern). It reads a weapon mesh
node's per-node `extras.mount` and returns the raw-source-frame `barrel`/`up` unit
vectors plus the optional applied `euler`. Degradation is granular, so "mirrors
`read_socket_name`" does not collide with the euler's independent optionality:
`barrel` and `up` are the CORE PAIR — if EITHER is absent or malformed the whole
`mount` reads `None`; `euler` degrades INDEPENDENTLY to its own `None`
(`MountAxes.euler: Option<[f32;3]>`), so a pre-euler weapon still surfaces its
axes. As with every existing reader, malformed metadata never fails the load.
Then surface the result on a NEW PUBLIC `LoadedModel.mount: Option<MountAxes>`
field, populated inside `load_model` from the selected mesh node's `extras`
(`SelectedModel.mesh_node`, the same node `build_rigid_sockets` and
`prop_to_gltf.py`'s summary already target) and assembled into the returned
`LoadedModel` — the same plumbing by which the `pub(crate)` `read_socket_name`
feeds the PUBLIC `LoadedModel.sockets` field (`build_skinned_sockets`/
`build_rigid_sockets` call the reader; `load_model` places the result in the
`LoadedModel` struct literal). `read_mount_axes` stays `pub(crate)` and is NOT
callable across the crate boundary; the xtask (Tasks 2 and 3) reads the public
`LoadedModel.mount` field instead, so the declared axes (and the applied euler)
come from the persisted weapon `extras`, not re-supplied CLI args — Decision 1;
the corrective/verify math itself is unchanged and stays euler-free. Re-point `socket_dump.rs` to call this module for its socket
dump, its geometric barrel/up detection, its corrective delta, and its verify
metrics, deleting that duplicated engine-frame math from the example. The example
keeps its own CLI and its inline Blender-XYZ-euler decomposition (that mapping is
example-local and is not what Task 2 promotes to the tool layer); its emitted
socket-frame and verify numbers must not change. Before deleting the duplicated
math, capture the pre-extraction output of the reference invocation (the
`socket_dump` command line in research.md §Fixtures) to a golden text file (a
scratch/temp file, not committed); after re-pointing, diff the new output against
it — the `MAT` socket-frame line and every verify metric must be byte-identical.
This golden diff is the check AC #8 names. `gltf_loader.rs` is large (~4800
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
axes — read from the weapon's `extras.mount` when present (via the public
Task 1 `LoadedModel.mount` field; the `read_mount_axes` reader itself is
`pub(crate)` and not reachable from xtask), with `--barrel X Y Z` / `--up X Y Z` as the first-author/
override path (required only when `extras.mount` is absent; a supplied CLI value
overrides the persisted axes and is re-persisted by the emitted bake — Decision 1).
It also takes the bake endpoints `--raw-source <path>`
(the raw glb/gltf `prop_to_gltf.py` bakes FROM) and `--out <path>` (the baked
output weapon glTF), forwarded verbatim as `--input`/`--output` into the emitted
command (see Boundary inventory for the exact argument surface and axis
convention). It loads both models via `postretro_model::gltf_loader::load_model`, calls the
Task 1 module to resolve the socket frame and compute the glTF-space corrective
delta from the DECLARED axes (geometric detection runs only as a labelled assist
when axes are omitted — see Task 3's mode boundary; this task wires the assist as
advisory output, not as a trusted euler), then converts the corrective delta to
a Blender XYZ euler here in the tool layer: apply the two-sided similarity
`D_blender = C · D_gltf · Cᵀ` with `C = [X, Z, −Y]` columns (the glTF→Blender
change of basis `C: (x,y,z) → (x,−z,y)`; a one-sided `C · D_gltf` would be wrong,
because `D` is a rotation operator, not a vector), then decompose `R = Rz·Ry·Rx`
to XYZ degrees. It prints the euler and a copy-paste, COMPLETE runnable command
`blender --background --python tools/prop_to_gltf.py -- --input <raw-source>
--output <out> [--grip GX GY GZ] [--scale S] [--socket NAME=NODE] --rotate-euler X Y Z
--mount-axes BX BY BZ UX UY UZ`
— `<raw-source>`/`<out>` from `--raw-source`/`--out`, and the emitted euler is the
full from-raw `D`, so the single command re-bakes correctly from the raw source.
The emitted `--mount-axes BX BY BZ UX UY UZ` carries the raw-source-frame declared
barrel/up so the bake persists the intent (Decision 1): `prop_to_gltf.py` gains a
`--mount-axes` flag whose `postprocess_gltf` step writes
`extras.mount = { barrel, up, euler }` onto the mesh node — the same JSON node-
`extras` write it already performs for `--socket NAME=NODE`
(`node["extras"]["socket"] = …`) — with `euler` set to the `--rotate-euler` degrees
that same bake applied. `--mount-axes` and `--rotate-euler` are INDEPENDENT flags:
on an unrotated bake (`--mount-axes` supplied, no `--rotate-euler`),
`postprocess_gltf` writes `extras.mount.euler = [0, 0, 0]` — a truthful "no
rotation applied" record, not an omitted key — so the persisted euler always
describes the bake and CHECK never falls back to `--current-euler` on a
declared-path weapon. This is EMIT-ONLY: the tool prints the command text; it does
not shell out to Blender (Decision 2 declines the `--bake` drive step). Any
`--grip`/`--scale`/`--socket NAME=NODE` the author passes through is forwarded
verbatim — `--socket` here is the prop_to_gltf extras flag, distinct from
`--mount-joint`. Emit-path semantics: the DECLARED-axes path emits the FULL
from-raw euler `D`
and is stateless — `D` depends only on the socket frame and the declared axes, so
`--current-euler` is NOT an input to the declared solve (in solve mode it is at
most an optional validation that the already-baked euler — read from
`extras.mount.euler`, or supplied via `--current-euler` — matches the freshly
solved `D`). Compose belongs to the geometric-ASSIST path only: when axes are
omitted, detection measures the loaded, already-baked mesh, so its delta `D_geom`
is a RESIDUAL; given the already-baked euler (`extras.mount.euler` when present,
else `--current-euler X Y Z` — the Blender XYZ `--rotate-euler`
already baked in) the assist path emits the TOTAL euler by composing
`D_geom_blender · R_current` before decomposition — the same composition
`socket_dump.rs` does with its positional args 6-8, which is itself a
geometric-detection path. Task 3's check mode consumes `--current-euler` as its
Blender→glTF frame bridge. The Blender-adapter math (the change of basis and the
euler decomposition) lives only in this tool layer, per the format-adapter
placement in Direction; the engine crate stays euler-free.

### Task 3: Check mode and the assist/trust boundary
Add a `--check` mode to `solve-weapon-mount` (same subcommand file, so sequenced
after Task 2). In check mode the tool takes a baked weapon (already carrying its
corrective in its vertices); it reads the declared barrel/up axes and the applied
euler from the weapon's `extras.mount` (Decision 1), with `--barrel`/`--up` and
`--current-euler X Y Z` (the `--rotate-euler` that bake applied) as optional
fallbacks/overrides for a weapon baked before `extras` carried them. Frame
handling — the decided resolution of the check-mode axis-frame ambiguity: the
declared barrel/up (from `extras.mount`, or the `--barrel`/`--up` overrides) are
ALWAYS in the raw-source weapon frame (one intent, shared verbatim with solve
mode); because check loads the already-rotated BAKED weapon, the tool composes the
applied euler onto the declared axes — it rotates the declared source-frame
barrel/up by the glTF-space rotation equivalent of the applied euler
(`extras.mount.euler`, else `--current-euler`) to obtain the baked-frame axes, then
feeds those to the Task 1 verify metrics at the resolved socket frame. That
glTF-space rotation is `R_gltf = Cᵀ · R_blender · C` (`C = [X, Z, −Y]` columns;
`R_blender = Rz·Ry·Rx` built from the applied euler degrees) — the inverse of
the emit-path similarity, taking the Blender-frame applied rotation back to glTF
frame; the axes then rotate by FORWARD application `v_baked = R_gltf · v_declared`
(the same forward direction `socket_dump.rs` uses for `barrel_w = rot · barrel_l`).
The composition direction is pinned because a reversed one would silently
false-pass or false-fail check mode. Applying
raw-source-frame axes directly to baked geometry would measure the wrong
direction, so the applied euler is REQUIRED in DECLARED check mode — normally read
from `extras.mount.euler`, with `--current-euler` supplying it only when the weapon
lacks that persisted value; the assist check path (axes omitted) measures the baked
mesh directly and does not consume it. When NEITHER is available — a pre-euler
weapon whose `extras.mount` carries no `euler` and no `--current-euler` supplied —
DECLARED check ERRORS: it prints a message naming the missing applied euler and
exits non-zero WITHOUT computing the metrics. It MUST NOT assume an identity
euler; a silent identity would compose the wrong baked-frame axes and let check
false-pass or false-fail against a frame the euler was never solved for. The declared path is
thus an analytic verification of the euler against declared intent (it trusts
`prop_to_gltf.py` to have baked the euler faithfully — the same trust the
whole pipeline places in the mesh authority); the baked weapon is still loaded so
that the assist path (axes omitted) can run geometric detection on the baked mesh.
This is exactly why Decision 1 persists both the declared axes and the applied
euler in `extras.mount`: a self-describing weapon removes both re-supplied inputs,
so a normal declared check supplies neither `--barrel`/`--up` nor `--current-euler`
and cannot diverge from the solve run's intent. It
prints both the declared source-frame axes and the composed baked-frame axes it
validated against (so a mismatch between the solve run's axes and the check run's
axes is visible), then prints `barrel·+Z`, `barrel·+Y`, `up·+Y`, and exits
non-zero when any is outside tolerance, naming the failed metric. A gating
check must sample the same `(clip, time)` the euler was solved at; a check at
a different pose measures a frame the euler was not solved for. In check mode
`--weapon` is the baked weapon (the solve's `--out`); `--raw-source`/`--out`
are not consumed (check emits no command). The default single-bake loop is a
same-file identity: solve's `--weapon`, solve's `--out`, and check's `--weapon`
are ONE path, so the drift-elimination guarantee is scoped to that one weapon
file as baked. Decision 3's per-character escape hatch — a separate baked weapon
file per target, each with its own `extras.mount.euler` — is the deliberate
multi-file case and is exempt from the single-file identity. Tolerances
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
`prop_to_gltf.py` → declare barrel/up axes once (`--barrel`/`--up`, the first
author) and run `solve-weapon-mount --mount-joint hand_r` to get the euler →
re-bake with the emitted command, which applies `--rotate-euler` AND persists the
declared axes and applied euler into the weapon's `extras.mount` (Decision 1) →
`solve-weapon-mount --check` to confirm in the engine frame. Explain that a normal
`--check` re-supplies nothing — it reads the declared axes (raw-source frame) and
the applied euler from `extras.mount`, and composes them into the baked frame — and
that `--barrel`/`--up`/`--current-euler` are only first-author/override or fallback
inputs for a weapon baked before `extras.mount` existed. Explain why the euler
matters at check time (the declared axes are raw-source-frame while the checked
weapon is already baked), and that `--mount-joint` (the socket-joint selector) is
not `prop_to_gltf.py`'s `--socket NAME=NODE` extras flag. State the reference-pose
caveat (a rigid bake is exact at the solve pose; wrist-reorienting poses need a
skinned weapon), the assist-vs-declared distinction, and the one-bake-per-weapon
default with the per-character escape hatch (Decision 3). Do NOT edit
`context/lib/` here — that capture is a promotion step (§Open questions records the
`resource_management.md` §7 target).

## Sequencing

**Phase 1 (sequential):** Task 1 — thin slice. Extracts the engine-frame solve and
re-points `socket_dump.rs`, falsifying the boundary assumption that the engine-frame
math separates cleanly from the Blender-euler mapping. Blocks everything.
**Phase 2 (sequential):** Task 2 — builds the xtask solve/emit on the Task 1 module,
and extends `prop_to_gltf.py` with the `--mount-axes` write the emitted command uses.
**Phase 3 (sequential):** Task 3 — adds check mode and the assist/trust boundary to
the same subcommand file Task 2 creates (shared file → sequential).
**Phase 4 (concurrent):** Task 4 — author doc, once the tool's argument surface and
`--check` contract are settled by Tasks 2-3.

## Boundary inventory

Crosses Rust (model crate, xtask) ↔ Python CLI (`prop_to_gltf.py`) ↔ engine-frame
vs Blender-frame. Pinned once:

| Name | Rust (engine crate) | xtask tool layer | Python (`prop_to_gltf.py`) |
|---|---|---|---|
| Corrective rotation | glTF-space delta `Mat3`/quat (`D = S^T·G^T`), euler-free | Blender XYZ euler degrees, via two-sided similarity `D_b = C·D_gltf·Cᵀ` (`C = [X, Z, −Y]` cols, glTF→Blender; NOT one-sided) then `R=Rz·Ry·Rx` | `--rotate-euler X Y Z` (degrees, XYZ, applied after `--grip`) |
| Declared barrel axis | unit `Vec3` in weapon-local (glTF) frame; surfaced on the public `LoadedModel.mount.barrel` field (the `read_mount_axes` reader stays `pub(crate)`) | persisted in `extras.mount.barrel` (raw-source weapon-local glTF frame); `--barrel X Y Z` is the first-author/override — overrides extras, re-persisted at the next bake | written to `extras.mount.barrel` by `postprocess_gltf` from `--mount-axes` |
| Declared up axis | unit `Vec3` in weapon-local (glTF) frame; surfaced on the public `LoadedModel.mount.up` field (reader `pub(crate)`) | persisted in `extras.mount.up` (raw-source weapon-local frame); `--up X Y Z` is the first-author/override | written to `extras.mount.up` by `postprocess_gltf` from `--mount-axes` |
| Mount declaration (`extras.mount`) | `read_mount_axes` (`pub(crate)`) → `Option<MountAxes>` surfaced on the public `LoadedModel.mount` field; `barrel`/`up` are the core pair (either absent/malformed → whole `mount` `None`), `euler: Option<[f32;3]>` degrades independently; never fails the load | single persisted source of truth read for solve and check via `LoadedModel.mount`; the emitted `--mount-axes` persists barrel/up and the bake writes `euler` | `extras.mount = { barrel, up, euler }` on the mesh node, written by `postprocess_gltf` — the same node-`extras` write as `--socket` (precedent) |
| Engine forward / up targets | `+Z` forward, `+Y` up (mount target) | same | n/a |
| Mount-joint selector | `SocketBinding::SkinnedJoint`, sampled at `(clip, time)` | `--mount-joint NAME` (default `hand_r`), `--clip` (default `idle_aiming`), `--time` (default 0) — the solver's socket-joint selector | n/a |
| Raw source weapon | n/a | `--raw-source <path>` (raw glb/gltf the bake reads FROM); forwarded as `--input` in the emitted command | `--input <path>` (required) |
| Baked output path | n/a | `--out <path>` (baked weapon glTF the bake writes TO); forwarded as `--output` | `--output <path>` (required) |
| Current bake euler (assist-compose + check bridge) | engine-frame math euler-free; the public `LoadedModel.mount.euler` surfaces `extras.mount.euler` as raw `Option<[f32;3]>` metadata for the tool | read from `extras.mount.euler`; `--current-euler X Y Z` is an optional fallback (weapon baked before `extras` carried it). The geometric-ASSIST residual-compose input and check's Blender→glTF bridge `R_gltf = Cᵀ·R_blender·C`; NOT an input to the declared solve (declared emits the full `D`) | writes `extras.mount.euler` = the `--rotate-euler` it applied; also reads a `--rotate-euler` previously applied |
| Verify tolerances | metric thresholds consumed by the verify path | `--min-barrel-dot` (0.999), `--max-barrel-y` (0.02), `--min-up-dot` (0.999) | n/a |
| Prop socket extras (pass-through) | n/a | `--socket NAME=NODE` forwarded verbatim into the emitted command (distinct from `--mount-joint` and from `--mount-axes`) | `--socket NAME=NODE` (append; writes node `extras.socket`) |

This table pins the frame-crossing arguments; the skeleton model path
(positional), `--weapon`, and the `--grip`/`--scale`/`--check` flags are named
in Tasks 2-3.

Axis-declaration convention: the author states the barrel/up axes in the weapon's
own local frame **as the engine loads it** (raw glTF vertex frame), not in Blender's
viewport frame — the tool loads the weapon through `load_model`, so in solve mode the declared
axes and the geometry it solves against share one frame (in check mode the
loaded geometry is the already-baked frame, which is why check composes the applied
euler — `extras.mount.euler`, else `--current-euler` — onto the declared
source-frame axes). The doc (Task 4) states this explicitly with the AR_4 as the
worked example.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| Solver's socket frame == the engine's mount frame **at the reference `(clip, time)` with `inputs = None`** (neutral, modifier-free). The solver samples `sample_clip_looped_world_modified(clip, skeleton, time, Loop::Clamp, &model.pose_stack, None, …)`; the runtime `sample_primary_world_pose` passes the mesh's `pose_inputs` (`Some(..)` for a posed humanoid) and the state's real loop policy. At the reference `t=0` both `Loop::Wrap` and `Loop::Clamp` resolve to time 0, so loop policy is a no-op there; the frames coincide whenever the runtime playhead resolves to the reference time with no active pose modifier, and diverge once the clip advances (`t≠0`, where loop policy bites), a pose modifier engages (`Some` inputs over a non-empty stack), or a fade/crossfade is active (`params.fade = Some`) — by design (the reference-pose caveat below, not drift). | Task 1 (reuse `load_model` + `sample_clip_looped_world_modified` with `inputs = None`) | Any reimplementation of load or sampling in the tool would drift it; broadening the claim past the neutral reference overstates it | AC "reuses loader/sampler" (scoped to reference/`inputs = None`), AC "socket_dump numbers unchanged" |
| Engine-frame math carries no Blender/authoring-tool mapping | Task 1 (module is euler-free) | Task 2 adds the Blender mapping only in the tool layer | Direction (format-adapter placement); AC on emit living in xtask |
| A geometry-only result is never presented as authoritative | Task 3 (assist/trust labelling) | Task 2 emit path must route geometry through the assist label, not the trusted euler | AC on undeclared-axis labelling; AC on ambiguous weapon |
| Corrective is exact only at the solve `(clip, time)` | inherent (socket frame is pose-dependent) | Task 3 prints the non-reference NOTE when `(clip, time)` ≠ (`idle_aiming`, 0), in solve or check mode; Task 4 doc restates the caveat | AC on non-reference-pose reporting (AC #7) |
| SOLVE and CHECK read the declared axes and applied euler from one persisted source — the weapon file as baked (solve's `--out`, which is check's `--weapon`) via its `extras.mount` — never from separately re-supplied CLI values, so the intent CHECK validates against cannot diverge from the intent SOLVE used. Scoped to that single file; Decision 3's per-character escape hatch (a separate baked file per target) is the deliberate multi-file case and is exempt. | Task 2 (emitted `--mount-axes` persists `extras.mount` via the bake), Task 1 (`read_mount_axes` `pub(crate)` → public `LoadedModel.mount`) | Task 3 reads the same `LoadedModel.mount`; a CLI `--barrel`/`--up`/`--current-euler` override must re-persist at the next bake, not silently diverge | AC on drift elimination, AC on first-author persistence |

## Script syntax examples

No script/descriptor change — `mesh.attachments` stays `hand_r: "models/ar_4/model.gltf"`.
The author-facing surface is the CLI loop:

```bash
# 1. Grip/scale-only bake (no orientation yet):
blender --background --python tools/prop_to_gltf.py -- \
  --input raw/ar_4.glb --output content/dev/models/ar_4/model.gltf \
  --grip 0 -0.05 0.12 --scale 0.68

# 2. Solve the corrective. --barrel/--up are the FIRST-AUTHOR declaration (the
#    grip/scale-only weapon carries no extras.mount yet); later solves read the
#    axes from extras.mount and omit them.
cargo run -p xtask -- solve-weapon-mount \
  content/dev/models/limitator/model.gltf \
  --mount-joint hand_r --clip idle_aiming --time 0 \
  --weapon content/dev/models/ar_4/model.gltf \
  --barrel 0 1 0 --up 0 0 1 \
  --raw-source raw/ar_4.glb --out content/dev/models/ar_4/model.gltf
# -> emits the FULL from-raw --rotate-euler <X> <Y> <Z>, and a COMPLETE, ready-to-run
#    `blender … prop_to_gltf.py -- --input raw/ar_4.glb --output …/model.gltf
#     --grip … --rotate-euler <X> <Y> <Z> --mount-axes 0 1 0 0 0 1` command
#    (emit-only; it does not run Blender — Decision 2).

# 3. Re-bake with the emitted command. Besides applying --rotate-euler, the
#    --mount-axes term persists extras.mount = {barrel, up, euler} onto the mesh
#    node, so the baked weapon self-describes (Decision 1):
blender --background --python tools/prop_to_gltf.py -- \
  --input raw/ar_4.glb --output content/dev/models/ar_4/model.gltf \
  --grip 0 -0.05 0.12 --scale 0.68 --rotate-euler <X> <Y> <Z> --mount-axes 0 1 0 0 0 1

# 4. Confirm in the engine frame. A normal check re-supplies NOTHING — it reads the
#    declared axes and the applied euler from extras.mount and composes them into
#    the baked frame. At the reference (clip,time), non-zero exit == mount is wrong;
#    at a non-reference pose a rigid bake is inexact by design, so a non-zero exit
#    there does not by itself mean a wrong mount (see the NOTE).
cargo run -p xtask -- solve-weapon-mount \
  content/dev/models/limitator/model.gltf \
  --mount-joint hand_r --clip idle_aiming --time 0 \
  --weapon content/dev/models/ar_4/model.gltf --check
# (For a weapon baked before extras.mount existed, supply the fallbacks:
#  --barrel 0 1 0 --up 0 0 1 --current-euler <X> <Y> <Z>.)

# Variant A — geometric-ASSIST compose: refine a weapon already baked with E0
#   WITHOUT declared axes. Detection runs on the baked mesh; its residual is
#   composed onto E0 (read from extras.mount.euler when present, else supply
#   --current-euler) to emit the TOTAL euler to re-bake from raw. The emitted
#   euler is flagged UNVERIFIED (geometry-only). The DECLARED path never composes
#   — it emits the full from-raw euler directly (steps 2-3 above).
cargo run -p xtask -- solve-weapon-mount \
  content/dev/models/limitator/model.gltf \
  --mount-joint hand_r --weapon content/dev/models/ar_4/model.gltf \
  --raw-source raw/ar_4.glb --out content/dev/models/ar_4/model.gltf \
  --current-euler <E0x> <E0y> <E0z>

# Variant B — loosen a tolerance for a check run:
#   --min-barrel-dot 0.995 --max-barrel-y 0.05 --min-up-dot 0.995
```

(The `--barrel`/`--up` values above are illustrative, not the AR_4's actual axes.)

## Open questions

The three design questions earlier review left open — declared axes via `extras`
vs CLI args, the optional `--bake` drive step, and how many bakes per weapon — are
now settled in Decisions (1, 2, 3). The `extras` adoption surfaced no new owner
question: the CLI-vs-`extras` precedence (CLI overrides, re-persisted at the next
bake) and euler persistence are decided inline in Decision 1. One promotion note
remains:

- **Doc capture at promotion.** `resource_management.md` §7 currently names
  `socket_dump` + raw `--rotate-euler` trial-and-error as the mount-verify path.
  Update it at promotion to name `solve-weapon-mount` and the declare-axes loop, and
  demote the trial-and-error description. Not done in this plan (drafting does not
  edit `context/lib/`).

# M10 Model Pipeline Slice — Findings (Task 6)

Two measured tripwires, the coordinate-system read, and recommendations on the
two deferrals the roadmap already leans toward. **Measure-and-report, not pass/fail
gates** — nothing below blocks the slice; these inform the broadening tasks.

> **Honesty note.** Per the honest-visual-acceptance-criteria principle, this note
> separates what a machine actually measured from what is run-pending or
> manual-visual. The CPU pose-sampling figure is **measured** here (no GPU needed).
> The runtime parse+upload figure and all in-engine / visual confirmations are
> **run-pending** — a human must run the engine to fill them in.

---

## Tripwire 1 — runtime glTF parse + upload time

**Status: RUN-PENDING (needs a GPU engine run; not measurable in this environment).**

### What was instrumented
`load_skinned_model` (renderer) wraps `load_model` (glTF parse, CPU) + `set_model`
(GPU buffer upload). A new level-load timing stage `model_load` is recorded
immediately after `load_skinned_model` returns, at the hardcoded mesh spawn seam
(`crates/postretro/src/main.rs`, just after line 1620). It joins the existing
`level_timings` stages (`geometry_upload`, `texture_upload`, …) and is emitted in
the single `[Startup] …` summary line logged when the first level frame presents.

### How to fill in the number
```
RUST_LOG=info cargo run --release -p postretro -- content/dev/maps/campaign-test.prl
```
Read the `[Startup] …` summary line printed at the first level frame and find the
`model_load=Xms` field. (`--release` is the figure to report; debug parse time is
not representative.) Also note the one-shot `[Model] skinned model uploaded: N
clip(s) parsed` and `[Model] skinned model animation: …` lines that confirm the
load path ran.

| Value | Result |
|---|---|
| `model_load` (parse + GPU upload), **debug** | **≈20.4 ms** (measured on the user's macOS machine during panic-fix verification; `first_level_frame=14.3 ms`). Debug build — not the representative figure. |
| `model_load` (parse + GPU upload), release | **TODO: fill from a `--release` run — `model_load=Xms`.** Expected lower than the 20.4 ms debug figure. |

### Asset shape (measured, frames the expectation)
- `scene.gltf` 46 KB + `scene.bin` 2.16 MB; 26-joint skin; one clip (`mixamo.com`);
  one mesh primitive; one external-PNG baseColor (`.prm` 350 KB on disk).
- This is a low-poly retro asset — exactly the poly budget the roadmap's "no
  offline mesh bake at this poly count" deferral targets.

### Read against the near-instant-boot northstar
The northstar (`context/lib/index.md` §1) is *near-instant boot*. A ~2 MB buffer
blob parsed by the `gltf` crate into already-engine-shaped structs (positions,
quantized UV/normal/tangent, joint/weight quads) plus one GPU buffer upload is
expected to land in the **single-digit-to-low-tens of ms** range in release —
small relative to the existing level-load stages (PRL parse, texture decode,
geometry upload) that already clear the northstar bar. **Confirm with the run.**

### Confirms or refutes "no offline mesh bake at this poly count"
**CONFIRMED (debug data point; release pending).** The debug `model_load≈20.4 ms`
already sits within a near-instant-boot budget for a one-shot level-load stage, and
release is expected lower — so the deferral holds at this poly count. If `model_load`
is a small fraction of total level load and within the boot budget, a per-mesh
offline bake buys negligible load-time at this poly count and is not worth the
pipeline complexity — matching the roadmap's stance (M10 asset-format note: "at
low-poly scale baking geometry buys negligible load-time or VRAM"). A mesh bake
stays deferred, additive via `format_tag`-style sidecars, added only against a
measured need. **If** the measured `model_load` is surprisingly large (e.g. tens
of ms scaling badly with vertex count), that refutes the deferral and a bake earns
its place — record the actual number before concluding.

---

## Tripwire 2 — per-frame CPU pose-sampling cost

**Status: MEASURED (CPU-only, no GPU) via a release micro-benchmark on the real
shipped skeleton + clip. The in-engine periodic log is additionally wired for an
on-device cross-check.**

### CPU side ONLY — explicit scope
This measures **only** the CPU cost of `sample_clip` (one skeleton's clip →
bone-matrix palette: local TRS sample, hierarchy compose, inverse-bind multiply).
It does **not** measure the GPU vertex-skinning throughput or the bone-palette
upload bandwidth at N instances. **That GPU cost is the ACTUAL wave risk and is
UNMEASURED here** — it is owned by the *Mesh render pass + MeshComponent* /
many-instance broadening task's measurement, not this slice. Do not read the green
light below as "waves are cheap"; it only says the CPU pose math is cheap.

### What was instrumented
1. **In-engine (run-pending cross-check):** `render_frame_indirect` times the live
   `sample_clip` call with `std::time::Instant`, folds each frame's duration into a
   `PoseSampleStats` accumulator, and logs a **min/mean/max** `[Model]` summary
   once per `POSE_SAMPLE_LOG_INTERVAL` (600) samples — **never per frame** (hot-path
   spam is forbidden per `development_guide.md` §6). At 60–144 FPS that is roughly
   one line every few seconds.
2. **CPU micro-benchmark (measured here):** `model::anim::tests::
   sample_clip_cpu_cost_on_real_model` (`#[ignore]`d) loads the real model via
   `load_model` and times 100,000 `sample_clip` calls over the clip, with a warm-up.
   No GPU. This is the reported figure.

### Measured value
```
cargo test -p postretro --release sample_clip_cpu_cost -- --ignored --nocapture
```
| Field | Value |
|---|---|
| Skeleton | 26 joints (the real shipped asset) |
| Samples | 100,000 (release) |
| Per-skeleton CPU cost | **min 3.34 µs · mean 3.64 µs · max 47.2 µs** |
| Machine | Intel Core i9-9980HK @ 2.40 GHz (x86_64, macOS) |
| Toolchain / profile | rustc 1.92.0, `--release` |

The `max` outlier (~47 µs) is a single-sample scheduler/cache blip across 100k
iterations; the steady-state cost is the **mean ~3.6 µs**. Steady-state sampling
is allocation-free (Task 4: `out` is reused, world-pose sweep uses a thread-local
scratch) — confirmed by the tight min/mean spread.

> Fill the in-engine cross-check from the `RUST_LOG=info cargo run --release`
> command above: read a `[Model] pose sample (CPU, 1 skeleton, 26 joints):
> min=… mean=… max=…` line. Expect it near the micro-benchmark mean; a large gap
> would point at frame-rate-coupled cache effects worth noting.
> In-engine value: **TODO: fill from the run (`[Model] pose sample …` line).**

### Projection to wave scale
Wave size **N = 200** (see "Wave size N" below). CPU pose sampling is linear in
instance count (one independent `sample_clip` per skeleton):

```
mean 3.64 µs/skeleton × 200 = ~727 µs/frame = ~0.73 ms/frame
```

At a 16.6 ms (60 FPS) frame budget that is **~4.4% of the frame on CPU pose math**
for a full 200-instance wave, single-threaded, before any animation time-slicing
(distant/off-screen agents sampling at a reduced rate — a named M10 optimization
that would cut this further). On the higher-budget side (≤8.3 ms at 120 FPS) it is
~8.7% — still comfortably affordable.

### Confirms or refutes "no `ozz`-style baked pose buffer"
**CONFIRMS the deferral (CPU side).** At ~0.73 ms/frame for a 200-instance wave —
shrinkable further by time-slicing — naive per-frame CPU sampling does not warrant
a baked pose buffer or `ozz`-style precomputation **on the CPU-cost axis**. An
`ozz` kernel may still earn its place for state-machine blending ergonomics or for
the GPU-skinning side; that decision belongs to the *Skinned animation runtime*
task, informed by the GPU measurement this slice does not take. The roadmap's
stance ("the slice's pose-buffer measurement decides") is satisfied: the CPU
measurement says no baked pose buffer is needed for CPU reasons.

---

## Wave size N

The roadmap (`context/plans/roadmap.md`, Milestone 10) states enemies "arrive in
waves" and that the pass/palette are built instance-friendly, but **does not
specify a numeric wave size**. Per this task's instruction, **N = 200 is assumed**
and used for both projections above. If the *Mesh render pass + MeshComponent* task
fixes a different wave target, rescale linearly (the CPU figure is per-skeleton, so
cost = mean × N).

---

## Coordinate-system / orientation read

### Analysis (Task 3 determination — static, no run needed)
The glTF → engine basis conversion is the **identity**. Both spaces are **Y-up,
right-handed, in meters**. The loader stores positions **verbatim** (no axis swap,
no negation, no scale) — see `gltf_loader.rs` `load_mesh`, where `read_positions()`
output is pushed straight into `SkinnedVertex.position`. Front faces are **CCW with
back-face culling**, matching the world geometry path. Inverse-bind matrices,
joint rest TRS, and animation TRS are likewise carried straight from glTF into glam
column-major matrices (`build_skeleton`, `load_clip`) with no basis change, and the
sampler composes them in glTF's `T * R * S` convention (`anim.rs`
`sample_local_pose`). So **no coordinate conversion is applied or needed**; the
model should appear upright, un-mirrored, and correctly scaled with no transform
fix-up.

### Manual-visual confirmations — PENDING A HUMAN RUN
A machine did **not** verify the following; they require running the GPU engine and
looking at the screen. Per honest-visual-acceptance-criteria these are not
machine-checked ACs.

Run:
```
RUST_LOG=info cargo run --release -p postretro -- content/dev/maps/campaign-test.prl
```
When the map has a `player_spawn` entity the model is planted
`MESH_SPAWN_DISTANCE` (3 m) straight ahead of `player_start`, facing back toward
the camera (yaw + π), then nudged `MESH_SPAWN_Y_OFFSET = -1.0` m vertically so
its feet sit near the floor. When there is **no** `player_spawn` the model falls
back to the level geometry center (`spawn_position`), same nudge applied. Both
paths are in the hardcoded spawn seam in `main.rs`. A human should confirm, in
order:

1. **Upright, un-mirrored, correctly scaled.** The model stands the right way up,
   is not flipped left/right (text/asymmetry reads correctly), and is roughly
   human-scale relative to the level — confirming the identity basis read above.
   *(If it appears on its side or mirrored, the identity-basis analysis is wrong —
   record that, it refutes Task 3.)*
2. **Animation plays forward.** Visible skeletal motion from the one clip
   (`mixamo.com`), playing **forward**, not frozen in bind pose and not running
   mirrored or backward.
3. **Portal cull disappear.** Walking the camera so a closed portal occludes the
   model's cell makes it disappear, and it reappears when the cell is visible again
   — portal/frustum culling reads visually correct.

The provisional spawn position (`MESH_SPAWN_Y_OFFSET`) is a manual-visual knob; if
the model is half-sunk in the floor or floating, adjust it and re-run — this is
tuning, not a correctness failure.

---

## Recommendations on the two deferrals

| Deferral | Recommendation | Basis |
|---|---|---|
| **No offline mesh bake at this poly count** | **Keep deferred** (pending the run-pending `model_load` number confirming it is within boot budget). Add a bake only against a measured need, additive via `format_tag` sidecars. | Tripwire 1 (run-pending) + asset is exactly the low-poly budget the deferral targets. |
| **No `ozz`-style baked pose buffer** | **Keep deferred on the CPU-cost axis.** ~0.73 ms/frame for N=200 single-threaded, further reducible by time-slicing. Revisit only if the *Skinned animation runtime* task wants `ozz` for blending ergonomics or if the **GPU** side (unmeasured here) demands it. | Tripwire 2 (measured: 3.64 µs/skeleton × 200). |

Both recommendations are **CPU-axis** (and, for Tripwire 1, load-time-axis)
conclusions. The wave's GPU vertex-skinning + palette-upload cost is the dominant
unknown and is **deliberately not measured by this slice**.

---

## Model `.prm` reproducibility note

The model's baseColor material resolves through a **pre-staged offline-baked**
`.prm` whose blake3 cache key is the constant
`581e80bb91c2d2e6fbed2aca5ba8fc0252aa7485579ea21376eeb294e972f0f1` (staged in
`gltf_loader.rs` `STAGED_MATERIAL_KEYS`, keyed by the glTF image URI). That
`.prm` lives at:

```
.build-caches/prm-cache/581e80bb91c2d2e6fbed2aca5ba8fc0252aa7485579ea21376eeb294e972f0f1.prm
```

This directory is **gitignored** — the `.prm` is a **local build artifact**, not
committed. **A fresh checkout must re-bake it** (run the production baker /
`prl-build` over the model's baseColor PNG once, offline) before the model renders
with its texture; without it, `load_textures` degrades to a **visible magenta/black checkerboard**
placeholder (`placeholder_loaded_texture` in `render/loaded_texture.rs`). This
is actually the better degrade for the manual-visual acceptance check — a missing
`.prm` is immediately obvious on screen rather than silently invisible.
This is the **baked-over-computed** invariant in action: the runtime never hashes
the PNG. Automating model-PNG → `.prm` baking (so a fresh checkout self-heals) is
the deferred **glTF mesh loading** broadening task, not this slice.

---

## What this slice proved / what remains provisional

Tied to the plan's three deliverables:

1. **A proven path.** One real skinned glTF is loaded → posed → portal/frustum-
   culled → drawn flat-lit at the entity transform, in production code, in the
   durable module layout (`model/`, `render/mesh_pass.rs`,
   `scripting/components/mesh.rs`, `scripting/systems/mesh_render.rs`). Automated
   gates pass: malformed-input graceful degrade, Pod/Zeroable layout round-trip,
   tangent attribute present, rigid single-bone degenerate case, point→leaf cull
   exclusion, real-model load (26 joints, 1 clip, staged material key). **The
   live-render visual confirmation remains the run-pending manual-visual checklist
   above** — the path is proven structurally and by unit tests; the on-screen
   result is a human run away.

2. **A reversibility-tiered contract proposal.** Committed (art-budget-bound):
   skinned vertex attribute set + widths, bone-palette entry, `ComponentKind::Mesh`.
   Provisional (consumer-bound, named not frozen): the indirect/per-instance
   alignment, the depth-only skinned variant shape, and the lighting bind group —
   each locks when its consumer (many-instance task, shadow task, lighting rewrite)
   arrives. The slice does **not** claim the multi-instance shared-palette layout
   is validated at N=1.

3. **Two measured tripwires.** Tripwire 2 (CPU pose sampling) is **measured**:
   3.64 µs/skeleton, ~0.73 ms projected at N=200 → no baked pose buffer needed on
   CPU grounds. Tripwire 1 (parse+upload) is **instrumented and run-pending** —
   the `model_load` stage and the run command are in place; a human fills the
   number. Both are informational; neither gates the slice. The dominant wave risk
   — GPU skinning + palette upload at N instances — is **explicitly out of scope**
   and owned by the many-instance broadening task.

### Provisional / outstanding for the orchestrator
- **Run-pending numbers:** `model_load=Xms` (Tripwire 1) and the in-engine
  `[Model] pose sample …` cross-check line (Tripwire 2), both from the
  `RUST_LOG=info cargo run --release` command.
- **Manual-visual confirmations the human still owes:** (1) upright/un-mirrored/
  correctly-scaled, (2) animation plays forward, (3) portal-cull disappear.
- **Fresh-checkout prerequisite:** re-bake the model `.prm` into the gitignored
  `prm-cache/` (offline-bake automation is the deferred glTF mesh loading task).

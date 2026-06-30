# Render-Stack Decomposition (Epic)

> **Status:** draft (epic). Sub-specs in this folder (`s0`–`s8`). Source-grounded findings in `research.md`.
> **Related:** `context/lib/scripting.md §12` (the data-floor precedent this mirrors) · `context/lib/rendering_pipeline.md` · `context/lib/development_guide.md` · `context/lib/index.md §2` (architectural invariants) · supersedes `context/plans/drafts/compile-time-reduction/`.

## Goal

Decompose the `postretro` binary's rendering runtime and the heavy CPU/GPU modules around it into a correct, one-way crate graph, so routine engine edits stop recompiling the wgpu stack and the renderer becomes a real boundary. End-state-first: define the whole target graph, then extract in dependency order with hard verification gates. This is the render-side analog of the `engine-data-floor`/`scripting-core` floor and the reference decomposition the upcoming combat crate mirrors.

## Scoping philosophy — build more right faster

Scope to the **correct end-state crate graph**, not a locally-safe first slice. Keep only ordering the compiler/dependency graph forces; everything else fans out in parallel (worktree-isolated). Because incremental human checkpoints are removed, **replace them with verification**: every sub-spec proves correctness by construction (`cargo tree` isolation, acyclicity-by-compile, typedef-drift byte-identity, WGSL byte-layout tests, behavior-preservation), not by reviewer trust. A split that does not measurably improve its targeted edit loop pauses the structural phases for re-evaluation.

## Scope

### In scope
- A baseline + dev Cargo-config harness (folds `compile-time-reduction` Tasks 1–2).
- Eight new workspace crates forming the render stack (see **Target crate graph**): `postretro-geometry`, `postretro-material`, `postretro-level-loader`, `postretro-visibility`, `postretro-lighting`, `postretro-ui`, `postretro-render-cpu`, `postretro-renderer`.
- Restoring the **"Renderer owns GPU"** invariant *within one crate*: the GPU renderer crate absorbs the stray GPU modules (`compute_cull`, `candidate_cull`, `shadow_cull`, `lighting` GPU pools).
- An **opaque present-handle** contract that hides `wgpu::SurfaceTexture` from engine-facing APIs.
- Breaking the inbound `scripting → render` CPU edges by sinking shared CPU types below both.
- Leaf hygiene: delete verified-dead duplicate files; relocate `UiCaptureMode`; widen `Frustum`/`FrustumPlane`.

### Out of scope
- **Pass-level GPU sub-crates** (fog/mesh/sh as separate crates). Requires converting `FullRenderer`'s `pub(super)` reach-in to owned-handle constructors — the renderer-side layered-floor refactor. Deferred unless a concrete need appears (see **Deferred**).
- **`render-diagnostics` extraction** — cross-cutting dev-tools reader; deferred (see **Deferred**).
- Runtime/behavior changes, PRL wire-format changes, map-bake cache changes, scripting-semantics or SDK-typedef changes.
- Removing wgpu/winit/glyphon/kira/mlua/rquickjs/SWC from supported builds; final binary slimming; cold-build reduction beyond what the warm-edit firewall yields.
- Forcing one linker on every platform.

## Target crate graph

One-way edges, top depends on bottom. New crates marked `*`. Existing data floor (`level-format`, `foundation`, `entities`, `scripting-core`) shown for placement.

```
                         postretro (binary)
                              │  drives present loop via opaque handle; owns Session::build wiring
                              ▼
                     postretro-renderer*  (GPU; the only wgpu crate)
        absorbs: compute_cull, candidate_cull, shadow_cull,
                 lighting::{spot_shadow,cube_shadow,lightmap,chunk_list}
            │        │        │         │          │         │
            ▼        ▼        ▼         ▼          ▼         ▼
        ui*   render-cpu*  visibility*  lighting*   model     (wgpu, winit,
         │        │            │       (cpu-math)  (cpu)       glyphon, …)
         │        │            ▼
         │        │       level-loader*  (prl)
         │        ▼            │
         │   level-loader*◄────┤
         ▼        ▼            ▼
   scripting-core   geometry* + material*  ◄──── level-format
   entities / foundation
```

- **`postretro-ui*`** (cpu-only) — `render::ui` CPU subtree + `UiTexture`. Depends on `scripting-core` (descriptor model), `entities` (only if tree bindings reference entity handles — confirm), `taffy`, `glyphon` (`FontSystem` only). No `input`, no wgpu.
- **`postretro-render-cpu*`** (cpu-only) — the harvest: CPU islands from `render/` (`frame_uniforms`, `mesh_instances`, `material_plan` CPU half, `fog_mask`, the CPU halves of `loaded_texture`/`sdf_*`/`sh_volume`/`sh_compose`/`animated_lightmap`/`screen_effects`/`splash`), the `fx::{smoke,fog_volume}` data, and the mesh/SH CPU types scripting imports. Carries WGSL binding constants with their packers.
- **`postretro-visibility*`** (cpu-only) — `visibility.rs` + `portal_vis.rs`.
- **`postretro-lighting*`** (cpu-only) — `lighting::{mod,influence,spec_buffer,cone_frustum}` (light packing, cone geometry). `script_primitives` placement per §12 (open question).
- **`postretro-level-loader*`** (cpu-only) — `prl.rs` + `prl_loader.rs`.
- **`postretro-geometry*` + `postretro-material*`** (cpu-only) — leaf data types under the loader.
- **`postretro-renderer*`** (gpu) — everything wgpu: `Renderer`/`FullRenderer`, all `renderer_*.rs`, all passes, + absorbed GPU modules. Public surface ≈ `{Renderer, opaque present handle, dev-tools setter API}` — `FullRenderer` stays private.

## Global acceptance criteria (every sub-spec inherits)

- [ ] `cargo build --workspace` and `cargo test --workspace` pass after each sub-spec.
- [ ] Dependency graph is acyclic and one-way (proven by `cargo build --workspace`); no lower crate depends on `postretro-renderer` or the binary.
- [ ] Each new **cpu-only** crate's `cargo tree` (default features) shows **no** `wgpu`, `winit`, `glyphon` (except `postretro-ui`, which uses `glyphon` `FontSystem` for CPU text measurement), `kira`, `mlua`, or `rquickjs`. (Manual/CI gate.)
- [ ] Editing a cpu-only crate and running *its* tests does not recompile `wgpu`/`naga`/`winit`/`kira`, nor (except `postretro-ui`/`scripting-core` consumers) `mlua`/`rquickjs`.
- [ ] Behavior-preserving: no PRL wire, runtime, scripting-semantics, or SDK-typedef change. The typedef drift test (`scripting/typedef/tests/committed.rs`) stays byte-identical.
- [ ] WGSL byte-layout contracts hold: `group3_shader_bindings`, `uniform_tests`, `shader_tests` pass — any moved packer carries its binding-index/stride constants.
- [ ] No net-new `unsafe` (pre-existing `unsafe` travels with moved code).
- [ ] No `wgpu` call lands outside `postretro-renderer` (and the binary's thin present driver). After the terminal sub-spec, `rg wgpu` over every non-renderer crate is empty.
- [ ] Each extraction PR quotes before/after warm-edit timings vs. the `s0` baseline for its targeted loop. A split that fails to improve its loop meaningfully pauses later structural phases.

## Sub-spec roster

| ID | Crate / unit | Layer | Risk | Folds from old draft |
|---|---|---|---|---|
| `s0` | Baseline + dev Cargo config | tooling | low | Tasks 1–2 |
| `s1` | Leaf hygiene & boundary prep | refactor | low | — |
| `s2` | `postretro-geometry` + `postretro-material` | cpu | low | (Task 4 type-homes) |
| `s3` | `postretro-level-loader` | cpu | medium | Tasks 4–5 |
| `s4` | `postretro-visibility` | cpu | medium | Task 3 |
| `s5` | `postretro-lighting` (cpu-math) | cpu | medium | — |
| `s6` | `postretro-ui` | cpu | medium | Task 6 (superseded) |
| `s7` | `postretro-render-cpu` | cpu | medium | (audit Tasks 6–7) |
| `s8` | `postretro-renderer` | gpu | high | the deferred renderer split |

## Cross-sub-spec sequencing

Dependency-forced ordering only; within a phase, sub-specs fan out in parallel worktrees.

**Phase 1 (parallel):** `s0` (baseline — gates all timing claims), `s1` (hygiene/prep — unblocks `s4` and `s6`).
**Phase 2 (parallel):** `s2` (geometry+material), `s5` (lighting-cpu), `s6` (ui — needs `s1`).
**Phase 3 (sequential within track):** `s3` (level-loader — needs `s2`), then `s4` (visibility — needs `s3`+`s2`).
**Phase 4:** `s7` (render-cpu — needs `s2`, `s3`, `s5`; breaks `scripting→render` edges).
**Phase 5 (terminal):** `s8` (renderer — needs `s2`–`s7` + `model`); absorbs GPU modules; lands the present handle. Single verification gate runs here and after every prior sub-spec.

## Cross-boundary contracts

- **Opaque present handle.** The renderer returns an opaque handle (or takes a present closure); the binary calls `renderer.present(handle)`, never `surface_texture.present()`. The handle encapsulates surface acquire (Success/Suboptimal/Outdated/Lost/Timeout/Validation), the surface `TextureView`, encoder completion, and `present()`. Unifies the gameplay and splash present paths. No consumer names `wgpu`. (Detail: `s8`.)
- **`UiCaptureMode` inversion.** `postretro-ui` uses `scripting-core`'s `descriptor::CaptureMode` directly; the `From<CaptureMode> → input::UiCaptureMode` conversion moves to the binary. `UiReadSnapshot` carries `descriptor::CaptureMode`. (Detail: `s1`/`s6`.)
- **WGSL byte-layout.** Binding-index/stride constants travel **with** their CPU packers into `postretro-render-cpu`; the `group3_shader_bindings`/`uniform_tests`/`shader_tests` guards must pass unchanged. Pin constants, not offsets (per `context_style_guide.md` — state the constraint, not the layout).
- **`script-ffi` / handler placement.** New cpu crates keep any VM marshalling behind an optional `script-ffi` feature per `scripting.md §12`; script-primitive *wiring* co-locates with its subsystem and is invoked from `Session::build`.

## Relationship to existing plans

- **Supersedes `compile-time-reduction`.** Its baseline methodology (T1), dev Cargo config (T2), visibility crate (T3), PRL split + loader crate (T4–5), and CPU UI-model crate (T6) are folded into `s0`/`s2`/`s3`/`s4`/`s6`. On promotion, `compile-time-reduction` is retired (moved to `done/` as superseded or deleted) so the work is single-owned.
- **Mirrors `scripting.md §12`.** Same one-way-floor discipline, `script-ffi` orphan-rule features, and `cargo tree` firewall AC, applied to the render stack.

## Deferred (documented non-goals, revisitable)

- **`s9` — `FullRenderer` encapsulation refactor** (pass-level GPU sub-crates). Convert `pub(super)` field reach-in to owned `device/queue` + explicit BGL-handle constructors. Large; only justified if pass-level crates become a goal. Not required for the single renderer crate.
- **`s10` — `postretro-render-diagnostics`** (dev-tools CPU behind a `LineSink` trait). Cross-cutting reader of `FullRenderer`/`nav`/`prl`/`geometry`/`visibility`; bundling it earlier would dominate risk.

## Open questions (decisions to lock before promotion)

1. **`lighting/script_primitives.rs` placement** — stays binary-side per §12 handler rule, or moves into `postretro-lighting` behind `script-ffi`? (1223 lines; calls scripting-core marshalling + lighting fns.)
2. **`postretro-geometry` + `postretro-material`** — one crate or two? (Tiny; combine as `postretro-render-data`?)
3. **`model/` as a crate** — extract `postretro-model` (CPU loader, already correctly layered) now, or leave in the binary as a renderer dep? Affects whether `s8` depends on a `model` crate or a binary module.
4. **`ui_texture` home** — confirm `UiTexture` lands in `postretro-ui` (renderer depends on ui for splash) vs. a lower shared crate.
5. **`postretro-render-cpu` membership ruling** — per-function: `frame_uniforms`/`mesh_instances`/`fog_mask`/`material_plan`-CPU are clean; the `sh_volume`/`sdf_*`/`animated_lightmap` CPU halves are entangled with binding constants + per-frame `FullRenderer` state. Which truly leave?
6. **Visibility boundary shape** — depend on `LevelWorld` directly, or introduce the old draft's borrowed portal-world view? (Affects `s4` coupling and whether `Frustum` widening suffices.)
7. **Stray-GPU-module staging** — do `compute_cull`/`candidate_cull`/`shadow_cull` + lighting GPU pools move into `render/` first (in-binary), or directly into `postretro-renderer` at cut time? (Churn vs. one-step transplant.)
8. **`compile-time-reduction` retirement mechanics** — supersede-in-place (move to `done/`) vs. delete on promotion.

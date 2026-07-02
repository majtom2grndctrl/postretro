# Render-Stack Decomposition (Epic)

> **Status:** draft (roadmap Epic 19). Ten per-spec folders, grouped into three milestones. Source-grounded findings in `research.md`.
> **Layout:** this folder is the epic hub (index + shared `research.md`). The ten specs live in sibling `E19--*` folders, each with its own `index.md`. Each is independently reviewable (`review-draft-spec`) and promotable on its own (`git mv drafts/E19--<spec> ready/E19--<spec>`).
> **Related:** `context/lib/scripting.md §12` (the data-floor precedent this mirrors) · `context/lib/rendering_pipeline.md` · `context/lib/development_guide.md` · `context/lib/index.md §2` (architectural invariants) · supersedes `context/plans/drafts/compile-time-reduction/`.

## Goal

Decompose the `postretro` binary's rendering runtime and the heavy CPU/GPU modules around it into a correct, one-way crate graph, so routine engine edits stop recompiling the wgpu stack and the renderer becomes a real boundary. End-state-first: define the whole target graph, then extract in dependency order with hard verification gates. This is the render-side analog of the `engine-data-floor`/`scripting-core` floor and the reference decomposition the upcoming combat crate mirrors.

## Scoping philosophy — build more right faster

Scope to the **correct end-state crate graph**, not a locally-safe first slice. Build the specs **sequentially in dependency order** — one spec per `/orchestrate` run, lowest crate first. Each extraction re-points shared call sites and edits the workspace, so concurrent extractions conflict; and each landed spec moves files and re-points imports, so the next spec must be **re-grounded against the live tree** right before it's built (see **Execution model**). Because incremental human checkpoints are removed, **replace them with verification**: every spec proves correctness by construction (`cargo tree` isolation, acyclicity-by-compile, typedef-drift byte-identity, WGSL byte-layout tests, behavior-preservation), not by reviewer trust. A split that does not measurably improve its targeted edit loop pauses the structural phases for re-evaluation.

## Scope

### In scope
- A baseline + dev Cargo-config harness (folds `compile-time-reduction` Tasks 1–2).
- Eight new workspace crates forming the render stack (see **Target crate graph**): `postretro-render-data`, `postretro-level-loader`, `postretro-visibility`, `postretro-lighting`, `postretro-model`, `postretro-ui`, `postretro-render-cpu`, `postretro-renderer`.
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
                 lighting::{spot_shadow,cube_shadow,lightmap}
            │        │        │         │          │         │
            ▼        ▼        ▼         ▼          ▼         ▼
        ui*   render-cpu*  visibility*  lighting*   model*    (wgpu, winit,
         │        │            │       (cpu-math)  (cpu)       glyphon, …)
         │        │            ▼                       │
         │        │       level-loader*  (prl)         │
         │        ▼            │                        │
         │   level-loader*◄────┤                        │
         ▼        ▼            ▼                         ▼
   scripting-core    render-data*  ◄──── level-format ◄─┘
   entities / foundation   (geometry + material + cone_frustum/frustum math)
```

`model`/`weapon` depend *down* on `render-data` for `cone_frustum`/`Aabb`, not on `lighting`. The shared frustum-plane row-math lives in `render-data` too, so the renderer's GPU cull path and the CPU cone path both call into it — no `lighting → renderer` reach-across.

- **`postretro-ui*`** (cpu-only, `E19--ui`) — `render::ui` CPU subtree + `UiTexture`. Depends on `scripting-core` (descriptor model), `entities` (only if tree bindings reference entity handles — confirm), `taffy`, `glyphon` (`FontSystem` only). No `input`, no wgpu.
- **`postretro-render-cpu*`** (cpu-only, `E19--render-cpu`) — the harvest: CPU islands from `render/` (`frame_uniforms`, `mesh_instances`, `material_plan` CPU half, `fog_mask`, `lighting/chunk_list`, the CPU halves of `loaded_texture`/`sdf_*`/`sh_volume`/`sh_compose`/`animated_lightmap`/`screen_effects`/`splash`), the `fx::{smoke,fog_volume}` data, and the mesh/SH CPU types scripting imports. Carries WGSL binding constants with their packers.
- **`postretro-visibility*`** (cpu-only, `E19--visibility`) — `visibility.rs` + `portal_vis.rs`.
- **`postretro-lighting*`** (cpu-only, `E19--lighting-cpu`) — `lighting::{mod,influence,spec_buffer}` (light packing). `cone_frustum` does **not** live here — it homes in `postretro-render-data` (it delegated to `compute_cull`, a GPU module, so lighting would have caught a `lighting → renderer` cycle). The `script_primitives` wiring descends here whole behind an optional `script-ffi` feature (off by default) per §12; the marshalling substrate stays in `scripting-core`, and the registrar is invoked from `Session::build`. Moving the file intact carries its cross-domain world-query registrations along as acknowledged debt (Decision 1).
- **`postretro-model*`** (cpu-only, `E19--model`) — `model/` (CPU glTF loader: `ModelHandle`, `SkinnedMesh`, skeleton/anim/`sample_params`, `gltf_loader`). Depends on `postretro-render-data` (for the `Aabb`/`cone_frustum` types its mesh bounds carry), not on `postretro-lighting`.
- **`postretro-level-loader*`** (cpu-only, `E19--level-loader`) — `prl.rs` + `prl_loader.rs`.
- **`postretro-render-data*`** (cpu-only, `E19--render-data`) — `geometry.rs` + `material.rs` + `cone_frustum.rs` (geometry/AABB math) leaf data types under the loader, in one crate. Also carries the shared frustum-plane row-math (relocated out of `compute_cull.rs`): the universal lower leaf the CPU cone path and the GPU cull pipelines both call *down* into. Still wgpu-free.
- **`postretro-renderer*`** (gpu, `E19--renderer-gpu`) — everything wgpu: `Renderer`/`FullRenderer`, all `renderer_*.rs`, all passes, + absorbed GPU modules. Public surface ≈ `{Renderer, opaque present handle, dev-tools setter API}` — `FullRenderer` stays private.

## Global acceptance criteria (every spec inherits)

- [ ] `cargo build --workspace` and `cargo test --workspace` pass after each spec.
- [ ] Dependency graph is acyclic and one-way (proven by `cargo build --workspace`); no lower crate depends on `postretro-renderer` or the binary.
- [ ] Each new **cpu-only** crate's `cargo tree` (default features) shows **no** `wgpu`, `winit`, `glyphon` (except `postretro-ui`, which uses `glyphon` `FontSystem` for CPU text measurement), `kira`, `mlua`, or `rquickjs`. (Manual/CI gate.)
- [ ] Editing a cpu-only crate and running *its* tests does not recompile `wgpu`/`naga`/`winit`/`kira`, nor (except `postretro-ui`/`scripting-core` consumers) `mlua`/`rquickjs`.
- [ ] Behavior-preserving: no PRL wire, runtime, scripting-semantics, or SDK-typedef change. The typedef drift test (`scripting/typedef/tests/committed.rs`) stays byte-identical. **Sanctioned exception:** `E19--leaf-hygiene-and-boundary-prep` corrected the shared cull near-plane row (`r3+r2` → `r2`) to the WebGPU `[0,1]` depth contract while relocating `extract_frustum_planes_for_gpu`. The old plane was conservative (over-included candidates), so the fix tightens culling with no visual regression; it is test-pinned. A documented correctness fix, not behavior drift.
- [ ] WGSL byte-layout contracts hold: `group3_shader_bindings`, `uniform_tests`, `shader_tests` pass — any moved packer carries its binding-index/stride constants.
- [ ] No net-new `unsafe` (pre-existing `unsafe` travels with moved code).
- [ ] No `wgpu` call lands outside `postretro-renderer` (and the binary's thin present driver). After the terminal spec, `rg wgpu` over every non-renderer crate is empty.
- [ ] Each extraction PR quotes before/after warm-edit timings vs. the `E19--baseline-and-cargo-config` baseline for its targeted loop. A split that fails to improve its loop meaningfully pauses later structural phases.

## Milestones

Three milestones, each a shippable checkpoint with a developer-facing testable outcome — the **safety** boundary: the build stays green and behavior-preserving at every milestone, so the epic can pause after any one without a half-migrated tree. Within a milestone, specs are built **one at a time in dependency order**, each re-grounded against the live tree just before it's built (see **Execution model**). Milestones are seam-first, not a strict chain: Milestone 2's lighting/UI tracks may start once their deps land, overlapping Milestone 1's tail.

### Milestone 1 — CPU runtime floor
**Specs:** `E19--baseline-and-cargo-config`, `E19--leaf-hygiene-and-boundary-prep`, `E19--render-data`, `E19--level-loader`, `E19--visibility`, `E19--model`.
**Order:** `E19--baseline-and-cargo-config` + `E19--leaf-hygiene-and-boundary-prep` (may pair only if file-disjoint, e.g. baseline + leaf-hygiene); then `E19--render-data`; then `E19--level-loader` (needs `E19--render-data`); then `E19--visibility` (needs `E19--level-loader`; `E19--render-data` is a dev-dependency only — test-side). `E19--model` is an independent low-risk CPU prerequisite, slotted in where the order allows (needs `E19--render-data` for the `Aabb`/`cone_frustum` types its mesh bounds carry). `E19--leaf-hygiene-and-boundary-prep` also unblocks `E19--render-data` (it severs the `cone_frustum → compute_cull` import and widens the cone symbols) and `E19--ui`.
**Testable outcome:** `postretro-render-data`, `postretro-level-loader`, `postretro-visibility`, `postretro-model` are workspace crates; editing any and running its tests recompiles no `wgpu`/`naga`/`winit`/VM crate; the `E19--baseline-and-cargo-config` baseline shows the warm-edit win on a `prl.rs`/`portal_vis.rs` touch; `cargo build --workspace` + `cargo test --workspace` green.

### Milestone 2 — Sever scripting / UI / CPU-math from the renderer
**Specs:** `E19--lighting-cpu`, `E19--ui`, `E19--render-cpu`.
**Order:** `E19--lighting-cpu` and `E19--ui` are independent (`E19--ui` needs `E19--leaf-hygiene-and-boundary-prep`) — build either order, or pair only if file-disjoint; `E19--render-cpu` after `E19--render-data` / `E19--level-loader` / `E19--lighting-cpu`. May overlap Milestone 1's tail.
**Testable outcome:** `rg "use crate::render" crates/postretro/src/scripting` is empty — the `scripting → render` edge is gone; `postretro-ui`/`-lighting`/`-render-cpu` are crates whose tests recompile no `wgpu`/`naga`; the WGSL byte-layout guards and the typedef drift test stay green.

### Milestone 3 — Renderer crate (invariant restored)
**Spec:** `E19--renderer-gpu`. **Built last, solo** as the integration surface — never paired with another spec.
**Testable outcome:** `rg wgpu` is empty across every crate except `postretro-renderer` (and the binary's thin present driver); no consumer of the renderer crate imports `wgpu`; `wgpu::SurfaceTexture` is absent from every engine-facing signature; behavior-preserving; the full verification gate (cargo-tree isolation, acyclicity, typedef drift, WGSL) is green.

## Execution model

Build the specs **sequentially in dependency order** — one spec per `/orchestrate` run, not waves. The cadence per spec:

1. **Re-ground.** Run `review-draft-spec`'s codebase-anchor (source-grounding) lens against the *then-current* tree. Discovery (`research.md`) was a point-in-time snapshot; every landed extraction moves files and re-points imports, so a spec's named identifiers and call-sites drift stale until re-grounded.
2. **Update the spec** from the anchor findings.
3. **Orchestrate** that one spec.
4. **Verification gate** — the full Global ACs (cargo-tree isolation, acyclicity, typedef drift, WGSL) must hold before the next spec opens.

Then the next spec, lowest crate first. **Don't deep-ground the later specs now** — they change as the lower crates land; ground each just before it's built (detail-on-open).

**Parallelism is the exception.** It applies to the discovery/review phase (where it genuinely did) and to genuinely file-disjoint specs (e.g. baseline + leaf-hygiene). The renderer spec (`E19--renderer-gpu`) is built last and solo. Default is one spec at a time.

## Spec roster

| Folder | Crate / unit | Milestone | Layer | Risk | Folds from old draft |
|---|---|---|---|---|---|
| `E19--baseline-and-cargo-config` | Baseline + dev Cargo config | 1 | tooling | low | Tasks 1–2 |
| `E19--leaf-hygiene-and-boundary-prep` | Leaf hygiene & boundary prep | 1 | refactor | low | — |
| `E19--render-data` | `postretro-render-data` (geometry + material) | 1 | cpu | low | (Task 4 type-homes) |
| `E19--level-loader` | `postretro-level-loader` | 1 | cpu | medium | Tasks 4–5 |
| `E19--visibility` | `postretro-visibility` | 1 | cpu | medium | Task 3 |
| `E19--model` | `postretro-model` (CPU glTF loader) | 1 | cpu | low | — |
| `E19--lighting-cpu` | `postretro-lighting` (cpu-math) | 2 | cpu | medium | — |
| `E19--ui` | `postretro-ui` | 2 | cpu | medium | Task 6 (superseded) |
| `E19--render-cpu` | `postretro-render-cpu` | 2 | cpu | medium | (audit Tasks 6–7) |
| `E19--renderer-gpu` | `postretro-renderer` | 3 | gpu | high | the deferred renderer split |

The full verification gate (Global ACs) runs after every spec; the milestone testable outcomes above are the checkpoints where it must hold cleanly before the next milestone opens.

## Cross-boundary contracts

- **Opaque present handle.** The renderer returns an opaque handle (or takes a present closure); the binary calls `renderer.present(handle)`, never `surface_texture.present()`. The handle encapsulates surface acquire (Success/Suboptimal/Outdated/Lost/Timeout/Validation), the surface `TextureView`, encoder completion, and `present()`. Unifies the gameplay and splash present paths. No consumer names `wgpu`. (Detail: `E19--renderer-gpu`.)
- **`UiCaptureMode` inversion.** `postretro-ui` uses `scripting-core`'s `descriptor::CaptureMode` directly; the `From<CaptureMode> → input::UiCaptureMode` conversion moves to the binary. The mode lives on `UiTreeEntry.capture_mode`; `UiReadSnapshot` carries it transitively, via its `trees` entries (it has no `capture_mode` field of its own). (Detail: `E19--leaf-hygiene-and-boundary-prep` / `E19--ui`.)
- **WGSL byte-layout.** Binding-index/stride constants travel **with** their CPU packers into `postretro-render-cpu`; the `group3_shader_bindings`/`uniform_tests`/`shader_tests` guards must pass unchanged. Pin constants, not offsets (per `context_style_guide.md` — state the constraint, not the layout).
- **`script-ffi` / handler placement.** Script-primitive *wiring* descends into its subsystem crate behind an optional `script-ffi` feature (off by default, `script-ffi = ["dep:rquickjs","dep:mlua", ...]` per `scripting.md §12`) only when it is subsystem-owned. The marshalling substrate stays in `scripting-core`; the registrar is invoked from `Session::build`. `postretro-lighting` moves the whole `script_primitives.rs` file, not a domain split: its `register_shared_types` also registers cross-domain world-query types (`WorldQueryComponent`, `WorldQueryFilter`, `Entity`, `EmitterEntity`), which ride along as acknowledged debt because typedef registration order blocks carving them out (Decision 1). This is the precedent the Epic 16 combat crate mirrors.

## Relationship to existing plans

- **Supersedes `compile-time-reduction`.** Its baseline methodology (T1), dev Cargo config (T2), visibility crate (T3), PRL split + loader crate (T4–5), and CPU UI-model crate (T6) are folded into `E19--baseline-and-cargo-config`, `E19--render-data`, `E19--level-loader`, `E19--visibility`, and `E19--ui`. **At Epic 19 promotion, `context/plans/drafts/compile-time-reduction/` is deleted** (not moved to `done/`: `done/` is for shipped plans and it never shipped). Provenance survives in this epic's `research.md` and the "folds from" column. Not deleted now — only at promotion.
- **Mirrors `scripting.md §12`.** Same one-way-floor discipline, `script-ffi` orphan-rule features, and `cargo tree` firewall AC, applied to the render stack.

## Deferred (documented non-goals, revisitable)

- **The deferred FullRenderer-encapsulation spec** (pass-level GPU sub-crates). Convert `pub(super)` field reach-in to owned `device/queue` + explicit BGL-handle constructors. Large; only justified if pass-level crates become a goal. Not required for the single renderer crate. No folder (no `E19--*` spec).
- **The deferred render-diagnostics spec** (`postretro-render-diagnostics`: dev-tools CPU behind a `LineSink` trait). Cross-cutting reader of `FullRenderer`/`nav`/`prl`/`render-data`/`visibility`; bundling it earlier would dominate risk. No folder (no `E19--*` spec).

## Decisions

These were the open questions; all are now settled. Each states the decision and the one principle that settled it.

1. **`lighting/script_primitives.rs` placement** — the whole file descends into `postretro-lighting` behind an optional `script-ffi` feature (off by default), not a domain split. The split is blocked by typedef registration order (verified against source — `primitives_registry.rs`, `register_shared_types`): the file's `register_shared_types` registers the cross-domain world-query types (`WorldQueryComponent`/`WorldQueryFilter`/`Entity`/`EmitterEntity`) before `LightEntity`, and the typedef generator emits in registration order, so carving them out would reorder emission and break the drift test. Those cross-domain registrations ride along as acknowledged debt for a future deliberate refactor that rewrites the snapshot on purpose. The marshalling substrate stays in `scripting-core`; the registrar is still invoked from `Session::build`. Principle: `scripting.md §12` handler-placement spirit applies to subsystem-owned wiring, but a boundary blocked by source registration order is deferred, not forced. This is the precedent the Epic 16 combat crate mirrors. (`E19--lighting-cpu`.)
2. **geometry + material crate count** — one crate, `postretro-render-data`, holding both `geometry.rs` and `material.rs`. Principle: lean — two dependency-free leaf modules gain no recompile isolation from splitting. (`E19--render-data`.)
3. **`model/` as a crate** — extract a new `postretro-model` CPU crate. Principle: clean one-way boundaries + the firewall goal — the renderer crate can't depend up into the binary, and a CPU glTF loader must not live in the GPU crate or model edits rebuild the renderer compile unit. (`E19--model`.)
4. **`ui_texture` home** — `UiTexture` lives in `postretro-ui` (renderer already depends on it for splash). Principle: lean — no 12-line crate. (`E19--ui`.)
5. **`postretro-render-cpu` per-function membership** — settled by reading current source (ruling in `E19--render-cpu` Task 1): every candidate helper descends except the GPU-recording halves of `mesh_pass.rs` and `loaded_texture.rs`, which split; WGSL binding constants travel with their packers. The source pass corrected one investigator mis-classification — `mesh_visible` is a pure `LevelWorld`+`VisibleCells` predicate, so it descends, adding a `postretro-visibility` dependency to `E19--render-cpu`. Principle: the descent rule applied to real code. (`E19--render-cpu`.)
6. **Visibility boundary shape** — depend on `LevelWorld` directly; the borrowed portal-world view is a deferred optimization, added only on a measured rebuild problem. Principle: lean — the crate boundary already cuts the recompile coupling, so the view would be speculative abstraction. (`E19--visibility`.)
7. **Stray-GPU-module staging** — move `compute_cull`/`candidate_cull`/`shadow_cull` + the lighting GPU pools directly into `postretro-renderer` at cut time (one transplant, no in-binary staging). `chunk_list` is not part of this set. Principle: efficiency + behavior-preserving + gated — the `E19--renderer-gpu` verification gate provides the safety staging would. (`E19--renderer-gpu`.)
8. **`compile-time-reduction` retirement mechanics** — delete `context/plans/drafts/compile-time-reduction/` at Epic 19 promotion (not now). Principle: documentation lifecycle — `done/` is for shipped plans and it never shipped; provenance lives in this epic's `research.md` + the "folds from" columns. (See **Relationship to existing plans**.)
9. **`cone_frustum` + shared frustum-plane math placement** — live in `postretro-render-data`, not `postretro-lighting`. Principle: clean one-way boundaries — `cone_frustum` delegated to `compute_cull` (a GPU/renderer module), so homing it in lighting created a `lighting → renderer` cycle; it is geometry math with the widest fan-out (model/weapon/cull/renderer), so render-data is the correct leaf. (Found by the architecture review; `E19--render-data`/`E19--lighting-cpu`/`E19--leaf-hygiene`.)
10. **`LightInfluence` struct placement** — the bare `LightInfluence` (`{ center, radius }`) sinks from `lighting/influence.rs` into `postretro-render-data`; the GPU `pack_influence` packer stays in `postretro-lighting`. Principle: same as Decision 9 — it is a bounding-sphere cull volume in the `Aabb`/`cone_frustum` family, produced by the CPU loader and consumed by the GPU light-cull path. `LevelWorld.light_influences` carries it, and `level-loader` sits *below* `lighting`, so homing the struct in `lighting` would force a `level-loader → lighting` up-edge; render-data is the leaf both depend down on. (Found by the `E19--level-loader` re-grounding review; affects `E19--level-loader`/`E19--render-data`/`E19--lighting-cpu`.)
11. **`ChunkGrid` placement** — `lighting/chunk_list.rs` (`ChunkGrid`, `CHUNK_GRID_UNIFORM_SIZE`) lives in `postretro-render-cpu`, not `postretro-lighting` or `postretro-renderer`. Principle: it is CPU byte-payload planning for renderer buffers. Renderer owns the upload/resources; render-cpu owns the packer and layout tests. (Found by source-grounding `chunk_list.rs`; affects `E19--lighting-cpu`/`E19--render-cpu`/`E19--renderer-gpu`.)

## Open questions

None remaining — all eleven settled (see **Decisions**). Residual implementation-time confirmations (not design questions) live in the specs: whether `postretro-ui` needs an `entities` dep, and whether any remaining `postretro-lighting` packer references `render-data` geometry types. (`cone_frustum`'s home was a real design question — now decided: `render-data`, Decision 9.)

# Research Brief — M10 "glTF mesh loading" spec prep

> **Status:** pre-draft research. Not a spec. Inputs for a `/draft-spec` session.
> **Target task:** Milestone 10 render-foundation track, item 2 — "glTF mesh loading"
> (generalize the just-shipped thin slice `M10--model-pipeline-slice` *in place*).
> **Method:** parallel research — codebase crawl (as-built contracts + texture/resource
> pipeline) plus library/web research (the `gltf` crate API + glTF conventions).
> Every claim below is file:line- or source-anchored in the agent reports that fed this brief.

---

## 0. The reframe that matters most

The roadmap one-liner reads as "build the loader." **The loader already exists** — the slice
(`crates/postretro/src/model/gltf_loader.rs`) already reads arbitrary mesh geometry, all vertex
attributes (positions, UVs, normals, tangents, JOINTS_0, WEIGHTS_0), merges multi-primitive
meshes, validates topology/joint-count, and returns a `LoadedModel` with per-primitive material
keys. It even loads the skeleton and all animation clips (narrowed to first-clip use).

So this spec is **not** "write a glTF reader." It is "**replace the slice's hardcoded/stubbed
seams with the real generalized versions**," in place, against the locked contracts. The scope is
defined by the *gaps*, enumerated in §3. Drawing that scope line crisply — especially vs. the
sibling "glTF skeleton + clip loading" task — is the first job of the draft.

---

## 1. Locked contracts the spec must honor (build *in place*, do not redesign)

From `M10--model-pipeline-slice` (done) and durable decisions migrated to `context/lib/`:

- **GPU vertex layout — `SkinnedVertex`, 32 B, LOCKED.** `model/mesh.rs:19–35`.
  `position [f32;3]` · `base_uv [u16;2]` (Unorm16) · `normal_oct [u16;2]` · `tangent_packed [u16;2]`
  (octahedral u + v-with-bitangent-sign-in-bit-15) · `joints [u8;4]` · `weights [u8;4]`.
  Vertex-attr layout at `render/mesh_pass.rs:166–208`. **Rigid = degenerate single-bone case**
  (`joints=[0,0,0,0]`, `weights=[255,0,0,0]`), `model/mesh.rs:44–58`. One struct, not two tiers.
- **Joint index width — `[u8;4]`, max 256 joints (`MAX_JOINTS`).** Errors early if exceeded.
- **Weight encoding — `[u8;4]` normalized (0..255 → 0..1 in shader).**
- **Bone palette — `BonePaletteEntry { matrix: [[f32;4];4] }` (64 B), `model/mod.rs:41–45`.**
  Single shared palette buffer sized `MAX_JOINTS`, instance addresses joints via a `base_index`
  (`render/mesh_pass.rs:78–79`). Scheme locked; *scale* (many instances) is a later task's problem.
- **Module layout — CPU/GPU split is a contract.** `model/` is wgpu-free
  (`mesh.rs`, `mod.rs`, `skeleton.rs`, `gltf_loader.rs`, `anim.rs`); the renderer
  (`render/mesh_pass.rs`) owns all GPU. "Renderer consumes handles, never raw glTF." Broadening
  tasks fill these modules in place — no dump-and-split.
- **Loader returns `Result`, never panics** on malformed glTF (`ModelLoadError` enum).

---

## 2. The texture/`.prm` pipeline this spec plugs into (the "runtime-loader half")

The spec must resolve glTF material PNGs "through the existing texture pipeline (external PNG →
blake3 cache)." That pipeline is two-phase and the new work touches **both** halves:

- **Runtime half (mirror this, no new code shape):** `render/loaded_texture.rs::load_textures`
  (`:295–370`) takes parallel `texture_names` + `[u8;32]` blake3 keys, opens
  `<prm_cache_root>/<hex>.prm` via `cache_filename_for_key` (`level-format/src/prm.rs:46–59`),
  parses (`PrmFile::from_bytes_partial`), uploads per-slot with placeholder fallback
  (`upload_texture_data`, `:59–128`), yielding `LoadedTexture` GPU handles (`:26–43`).
- **`.prm` format:** material bundle, slot_mask (diffuse/specular/normal), per-slot `format_tag`
  (`Rgba8UnormSrgb`/`Rgba8Unorm`/`R8Unorm`/`Bc5RgUnorm`), mip chain. `level-format/src/prm.rs`.
- **Build half (the bake) — `level-compiler/src/texture_mips.rs::bake_texture_mips` (`:632–794`).**
  Resolves a texture *name* → PNG path (`build_name_to_path_map`), hashes **raw PNG bytes** →
  blake3 filename key (`filename_key_for`, `:197–217`), bakes mips/BC5, writes `<hex>.prm`,
  returns `name → key`. Keys are emitted into the PRL `TextureCacheKeysSection`.

**The integration gap (see §3.1):** today this bake is driven by **map texture names** flowing
out of `.map`/PRL. A glTF model's PNGs are *not* map textures and are *not* in the PRL texture
sections. The slice papers over this with a **hardcoded `STAGED_MATERIAL_KEYS` table**
(`gltf_loader.rs:93–114`) — someone pre-baked the model PNG offline by hand. Generalizing that is
the single biggest design decision in this spec.

---

## 3. The actual scope — gaps between the slice and the task

| # | Gap | As-built (slice) | What the spec must decide/build |
|---|-----|------------------|----------------------------------|
| 3.1 | **Model PNG → `.prm` at build time** | Hardcoded `STAGED_MATERIAL_KEYS` (one entry) | **DECISION.** How does prl-build *discover* model PNGs to bake? Models aren't map-referenced. Options: scan a models dir, drive off a model manifest, bake-on-first-load, or extend prl-build to walk glTF material URIs. Milestone text says "conversion is implicit during prl-build." |
| 3.2 | **Runtime material resolution** | Renderer resolves only `material_keys.first()` (`render/mod.rs:2689–2722`); **`UploadedModel` carries exactly one `material_bind_group` for the whole model** (`mesh_pass.rs:64–69`, verified) | Per-primitive material → per-submesh draw range + bind group. Decide multi-material support depth. Genuinely absent at the render level, not just narrowed. |
| 3.3 | **Model handle / cache** | **VERIFIED single-model:** `MeshPass.model: Option<UploadedModel>` (`mesh_pass.rs:86`); `set_model` "Replaces any previously uploaded model (the slice carries one)" (`:266`). `MeshComponent { model: String }`; loaded via a one-shot renderer seam at level-install (`main.rs:107–136`, `1804–1821`). No model handle table, no dedup, no multi-model | A model resource handle + cache parallel to the texture handle convention (`Vec<LoadedTexture>`/`GpuTexture`). Mint/dedup/lookup. **Net-new architecture, not generalize-in-place.** |
| 3.4 | **Image-decode avoidance** | Loader calls `gltf::import`, which **decodes all PNGs then discards them** (`gltf_loader.rs:147`) | **CONTRACT.** Switch to `gltf::Gltf::open` + `gltf::import_buffers` to read geometry/skin buffers *without* decoding images; resolve PNG URIs through the engine cache instead. (Source-confirmed against gltf 1.4.1.) |
| 3.5 | **`extras` metadata** | None read. **Also: the `gltf` `extras` cargo feature is OFF** — `extras()` returns the empty `Void` type today | **CONTRACT + DECISION.** Must enable `gltf = { features = ["extras"] }` or the task is dead on arrival. Then decide the entity-facing surface: `extras` is raw JSON (`RawValue`); where does it land (`MeshComponent`? a descriptor? scripting primitive)? `MeshComponent` is currently just `{model}`. |
| 3.6 | **Coordinate convention** | Positions stored **verbatim**; basis conversion is the **identity** — **VERIFIED correct by construction**: engine is Y-up, right-handed, meters (`camera.rs:98` `look_at_rh(_, _, Vec3::Y)` + `perspective_rh`; `Vec3::Y` up everywhere), glTF matches, winding matches (`Ccw` + cull `Back`). Documented at `mesh_pass.rs:19–30` | **Not an architectural risk.** Only the manual-visual "upright/un-mirrored" gate is GPU-run-pending — run it once to confirm. No loader basis change needed unless a future asset uses a different convention. |
| 3.7 | **Tangent generation** | Stubbed `+X` placeholder when absent (`gltf_loader.rs:385–389`); fine only because slice is flat-lit | Decide: generate tangents (MikkTSpace) now, or defer to the SH-lit/normal-mapped "Mesh render pass" task. glTF spec: generate only when a normal map is present. |
| 3.7b | **Embedded textures (`Source::View` / `.glb`)** | Skipped — only `Source::Uri` handled | Decide whether `.glb`/embedded images are in scope or explicitly a non-goal. |
| 3.8 | **Multi-mesh** | Reads only `document.meshes().next()` | Decide: one mesh per model (likely fine at this scale) vs. multi-mesh. |
| 3.9 | **Classname spawning** | Absent — single hardcoded spawn seam | Likely belongs to the *next* task ("Mesh render pass + MeshComponent"). **Draw the line.** |

---

## 4. `gltf` crate (1.4.1, pinned in `Cargo.lock`) — API anchors for the spec

- **Default features:** `["import","utils","names"]`. `extras` and `extensions` are **off** (see 3.5).
- **Geometry:** `Document → meshes() → primitives()`; `primitive.reader(|b| buffers.get(b.index())…)`
  gives typed `read_positions/normals/tangents/tex_coords(0)/indices`. `Reader` resolves
  buffer-views, strides, normalized flags, and sparse accessors for you. `primitive.bounding_box()`
  gives POSITION min/max for a free AABB.
- **Skinning:** `read_joints(0)` → `ReadJoints::{U8,U16}` (use `.into_u16()`); `read_weights(0)` →
  `ReadWeights::{U8,U16,F32}` (`.into_u8()` rescales the *type domain*, **not** the sum — still
  renormalize the 4-lane sum). 4 influences/set; >4 needs multiple sets. **JOINTS_0 values index the
  skin's joint list, not glTF node indices** — the #1 skinning bug; keep the skin-joint→bone remap.
- **Skin seam:** `document.skins()` → `Skin::joints()` (ordered), `read_inverse_bind_matrices()`
  (identity if absent). The *vertex-data* task needs only the joint **count** + order for validation
  and remap; IBMs/hierarchy/pose eval belong to the separate skeleton+clip task.
- **Materials/URIs (no decode):** `primitive.material().pbr_metallic_roughness()
  .base_color_texture().texture().source().source()` → `gltf::image::Source::Uri { uri, .. }`;
  `uri` is the raw relative path as written — resolve against the glTF's parent dir, feed to the
  cache. `Source::View` = embedded (handle/skip).
- **`extras`:** `.extras()` on Mesh/Primitive/Node/Material/Skin → `&Option<Box<RawValue>>` **once the
  feature is on**; parse `raw.get()` with `serde_json` (already a workspace dep). Distinct from
  `extensions`.
- **Conventions:** right-handed, **Y-up**, meters, **CCW front face**, quaternion `[x,y,z,w]`.
  Handedness flip ⇒ invert winding + flip tangent `w`. Normalized ints are pre-rescaled by `Reader`.

---

## 5. Open questions for the `/draft-spec` session (need a decision)

1. **(3.1) How are model PNGs discovered + baked into `.prm` at build time?** — the central design
   question. Determines whether prl-build grows a model-asset walk, a manifest, or a per-mod scan.
2. **(3.5) Where does `extras` land on the entity?** — `MeshComponent` field, a descriptor, or a
   scripting primitive? (Note the "primitive surface is a contract" invariant if it reaches scripts.)
3. **(3.7) Tangent generation — this task or deferred** to the SH-lit mesh-pass task?
4. **(3.2/3.9) Scope line vs. neighboring tasks** — how much material/submesh breadth and which (if
   any) of classname spawning belongs here vs. "Mesh render pass + MeshComponent" and the skeleton/
   clip task. Avoid double-owning.
5. **(3.3) Model handle/cache shape** — mirror the texture handle convention; confirm dedup needs.

> **Resolved during research (no longer open):** (3.6) coordinate convention — engine is Y-up RH
> meters, identity conversion, verified in-code; only the visual confirmation gate remains to run.

---

## 6. Source map (where to read for the draft)

- As-built mesh path: `model/{mesh,mod,skeleton,gltf_loader,anim}.rs`, `render/mesh_pass.rs`,
  `render/mod.rs:2620–2722`, `scripting/components/mesh.rs`, `scripting/systems/mesh_render.rs`,
  `main.rs:107–136 / 1745–1821`. Done plan: `context/plans/done/M10--model-pipeline-slice/`.
- Texture pipeline: `level-format/src/prm.rs`, `level-format/src/texture_cache_keys.rs`,
  `level-compiler/src/texture_mips.rs`, `render/loaded_texture.rs`, `render/mod.rs:2539–2615`.
  Done plans: `context/plans/done/baked-texture-mips/`, `prm-bc5-normals/`.
- Context lib: `rendering_pipeline.md`, `build_pipeline.md`, `entity_model.md`,
  `resource_management.md`.
- Library: `gltf 1.4.1` (docs.rs / gltf-rs GitHub); glTF 2.0 spec (Khronos) for conventions.

# Retire Runtime BSP — Cells + CellLocator

## Goal

The runtime stops loading the BSP node/leaf tree as its spatial contract. It loads a first-class `Cells` section (the visibility units it already reasons about) plus an explicit `CellLocator` (point → cell). BSP stays a compiler intermediate. This decouples runtime systems from how cells were produced, consolidates per-cell data that is currently scattered across parallel arrays, and names the point-locator as a swappable component so a future non-plane-tree locator lands without touching render/fog/particles.

Clean format cut: pre-1.0 foundation, all maps are ours. Compiler emits the new sections **instead of** BSP, the PRL version bumps, every map recompiles, no runtime fallback.

## Scope

### In scope
- New `Cells` PRL section: per-cell bounds, baked solid/drawable/exterior flags, and a baked portal-adjacency CSR. Replaces `BspLeaves` and the load-time adjacency build.
- New `CellLocator` PRL section: point → cell lookup. v1 implementation **is** the existing plane tree, renamed and owned as a locator. Replaces `BspNodes`.
- Runtime `locate_cell(position) -> cell` API replacing `find_leaf`, and `CellData` replacing `LeafData` where semantically correct.
- `current_cell` caching on camera and mesh entities — full locator descent only on cache miss / cell crossing.
- Compiler emits `Cells` + `CellLocator` **instead of** `BspNodes` + `BspLeaves`. PRL version bumps; all committed fixtures and content maps recompile; pre-bump maps rejected at load.
- Compiler-side BSP unchanged (still discovers cells, portals, and the plane tree).

### Out of scope
- Swapping the `CellLocator` implementation away from a plane tree (grid / temporal-coherence / hybrid). The named section makes this a later, self-contained change; this spec ships the plane-tree v1 only.
- Runtime-mutable cells or portals (kinematics, latent/toggleable portals, doors-affect-visibility). Independent concern.
- Inlining `CellDrawIndex` spans or `FogCellMasks` into the `Cells` record — both stay separate sections, already keyed by cell id (the shared key is enough; no duplication).
- Removing or restructuring compiler-side BSP.
- Splitting `prl.rs` / `mesh_pass.rs` (oversized — noted under Open questions, not blocking).
- Any GPU-timing-based acceptance.

## Acceptance criteria

- [ ] A map compiled by `prl-build` lists `Cells` and `CellLocator` in its section table and omits `BspNodes` and `BspLeaves`.
- [ ] Loading a pre-bump (current-version) `.prl` fails fast with an unsupported-version error naming "recompile"; it does not load or silently degrade.
- [ ] On a freshly compiled map, the engine produces identical visible-cell sets, camera-cull submission, fog assignment, and particle culling as the pre-change build (golden behavior comparison on a committed test map).
- [ ] Point location stays total and deterministic: every sampled position — interior, solid, exterior, and out-of-bounds — resolves to exactly one cell, and a position on a locator split plane resolves to the front side. A migration-time equivalence check confirms `locate_cell` returns, for every sampled position, the same cell the BSP descent returned on the same geometry.
- [ ] Each cell's solid / drawable / exterior classification read from baked flags equals the prior runtime derivation (`is_solid`; `!is_solid && face_count > 0`; `!is_solid && face_count == 0`) for every cell in a recompiled map.
- [ ] Portal traversal reads adjacency from the baked CSR; the runtime no longer builds a per-cell adjacency list at load. Visible-cell sets are unchanged (covered by the golden comparison).
- [ ] A stationary camera performs no locator descent after the frame that first resolves its cell; moving across a portal updates the cached cell. Same for a stationary vs. moving mesh entity.
- [ ] No runtime code path reads BSP node or leaf data; `find_leaf` is gone. (`rg` for the symbols returns only compiler-side hits.)
- [ ] Full `postretro` bin suite green on recompiled maps; `cargo fmt --check` and `clippy --workspace -D warnings` clean.

## Tasks

### Task 1: Cells + CellLocator wire format
Add two sections to `postretro-level-format`, each in its own module mirroring the existing `bvh.rs` / `bsp.rs` / `cell_draw_index.rs` shape (record struct, `to_bytes` / `from_bytes`, round-trip + truncation tests). No compiler emission, no runtime consumption yet — this task pins the byte contract only. Add the `SectionId` variants (next free ids; 38/39 at time of writing). See **Wire format**. This is the hard-to-reverse contract; design it to the endpoint.

### Task 2: Compiler emission
`prl-build` emits `Cells` + `CellLocator` **instead of** `BspNodes` + `BspLeaves`. Derive each cell (1:1 with a BSP leaf) into the consolidated record: bounds and `is_solid` from the leaf; bake the `drawable` / `exterior` flags from `is_solid` + `face_count`; bake the portal-adjacency CSR (the same front/back-leaf fan-out the runtime builds at load today). Emit `CellLocator` from the BSP node tree — same plane/child data, child leaf-refs reinterpreted as cell ids (identical values, cells being leaves). Stop emitting the two BSP sections. Leave the compiler's internal BSP pass untouched. Validate by decoding the compiler's own output and checking cell count, flag derivation, and adjacency against the BSP intermediate.

### Task 3: Runtime load + `locate_cell`
`LevelWorld` loads `Cells` + `CellLocator`; remove the `BspNodes` / `BspLeaves` decode and the load-time `Vec<Vec<usize>>` adjacency build (adjacency now comes from the baked CSR). Introduce `CellData` (bounds, flags, portal CSR slice) replacing `LeafData`, and `locate_cell` (plane-tree descent over `CellLocator`) replacing `find_leaf`. Update internal consumers forced by the struct change (`portal_vis` adjacency access → CSR slice; `visibility` flag reads → baked flags) behavior-identically. Keep `find_leaf` as a thin wrapper delegating to `locate_cell` so external callers compile unchanged this task. Bump `CURRENT_VERSION`; reject older maps with a recompile error mirroring the OctahedralShVolume precedent. Before dropping leaf `face_start` / `face_count` from the runtime, confirm no consumer reads them beyond flag derivation; if one does, carry the needed field on `CellData`.

### Task 4: Migrate callers to cell vocabulary
Migrate the four `find_leaf` call sites — camera visibility, per-entity mesh cell lookup, per-particle cull, SH diagnostics — to `locate_cell`, and switch solid/drawable/exterior consumers (visibility path selection, `spawn_position`) to read baked cell flags instead of deriving from `is_solid` / `face_count`. Remove the `find_leaf` wrapper. Rename `LeafData`-flavored locals to cell vocabulary where it reads clearer. Behavior-preserving.

### Task 5: `current_cell` caching
Add a cached current cell to the camera and to mesh entities. Each frame, verify the cached cell still contains the object (cheap point-in-cell / unchanged-position check); run a full `locate_cell` descent only on miss or cell crossing. Particles stay uncached (scattered cold queries). Expose a small instrument (descent count) so the "stationary ⇒ no descent" AC is testable.

### Task 6: Recompile + golden integration + docs
Recompile every committed `.prl` fixture and content map to the new version with `prl-build`. Run the golden behavior-equivalence comparison on a committed test map (visible cells, cull submission, fog, particles). Update `build_pipeline.md` (section registry, the cell/locator description, the "runtime consumes cells, portals, BVH" line that is now literally true) and `index.md` §3. Do not compile `stress-warren*` / `campaign-test` in routine tests.

## Sequencing

**Phase 1 (sequential):** Task 1 — the wire contract; blocks everything.
**Phase 2 (concurrent):** Task 2, Task 3 — compiler emit and runtime load both consume Task 1's format across different crates. Load-time cross-validation guards the shared contract against divergence (precedent: the candidate-cull baker/validator reconciliation). Each validates with synthetic / self-round-trip unit tests.
**Phase 3 (sequential):** Task 4 — consumes Task 3's `locate_cell` and baked flags.
**Phase 4 (sequential):** Task 5 — layers caching on Task 4's migrated call sites.
**Phase 5 (sequential):** Task 6 — recompiles maps and runs the real-map golden integration. The integration suite that loads committed `.prl` maps is intentionally red from the Phase-2 version bump until this phase recompiles them; phases 2–4 gate on unit/synthetic tests, and this phase closes the integration gate.

## Rough sketch

- New format modules sit beside `crates/level-format/src/bvh.rs`; the runtime decode block sits beside the existing node/leaf decode in `prl.rs` (`LevelWorld` build). Most net-new code is new files plus a contained load block — it does not deepen `prl.rs`'s tangled functions.
- `CellLocator` v1 reuses the exact `find_leaf` descent (`side = n·p − d; side >= 0 → front`), so totality/determinism transfer with zero re-derivation.
- The consolidation that earns the `Cells` section: it absorbs BSP-leaf bounds + `is_solid`, the previously-*derived* drawable/exterior bits, and the previously-*load-built* portal adjacency — collapsing one section, one per-frame derivation, and one load-time allocation into one baked record.

## Boundary inventory

Rust ↔ wire (PRL) only; no JS/Lua/FGD.

| Name | Rust (`SectionId`) | Wire id | Doc name |
|---|---|---|---|
| Cells | `SectionId::Cells` | 38 (next free) | `Cells` |
| Cell locator | `SectionId::CellLocator` | 39 (next free) | `CellLocator` |

Retired from the runtime (compiler no longer emits): `BspNodes` (12), `BspLeaves` (13).

## Wire format

Both sections little-endian, mirroring existing PRL sections (`bvh.rs`, `bsp.rs`, `cell_draw_index.rs`). Stated as contract, not byte offsets — the implementor fixes layout.

**Cells.** Header: cell count, plus the total length of the portal-index pool. Then one fixed-stride record per cell carrying: `bounds_min` / `bounds_max` (three `f32` each); a flags field encoding `solid`, `drawable`, `exterior`; and `portal_start` / `portal_count` (`u32`) into a trailing flat portal-index pool. Then the pool: a flat `u32` array of portal indices, CSR-style — cell *c*'s adjacency is `pool[start .. start+count]`. Invariants: `drawable` and `exterior` are mutually exclusive; either implies `!solid`; a solid cell has neither. Empty map → zero cells, empty pool. A cell with no portals has `portal_count == 0` (valid, not a sentinel).

**CellLocator.** Mirrors the current `BspNodes` layout: a node count and a root reference, then one fixed-stride record per node carrying `plane_normal` (three `f32`), `plane_distance` (`f32`), and `front` / `back` child references using the existing signed-int encoding (non-negative = node index; negative = cell id via the established `-1 - id` form). Child leaf-refs denote cell ids; values are identical to the BSP encoding since cells are leaves 1:1. Single-cell map → root references that cell directly, zero nodes.

`CellDrawIndex` (37) and `FogCellMasks` (31) are unchanged and remain keyed by cell id — not folded into `Cells`.

## Open questions

- **Oversized files.** `prl.rs` (3451), `mesh_pass.rs` (3531), `portal_vis.rs` (2274) are all well past the ~800-line smell. This change touches each only lightly (new decode block; one-line caller swaps; CSR access). A split is *not* taken here to keep the cut focused — flagged as a separate follow-up so the diff stays legible.
- **Cell record growth.** If a future locator or a consumer needs per-cell data beyond bounds/flags/portal-CSR, it extends the `Cells` record — the stride is the contract, so additions are a deliberate version event. Worth confirming during Task 1 that the v1 field set is the intended endpoint for *this* spec's scope.

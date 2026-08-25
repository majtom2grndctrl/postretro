# Cell-Visibility Relation — research notes

Investigation backing `index.md`. Not the spec. Decisions live in the spec; this is why they hold.

## Source grounding (verified this session)

### Leaf ↔ Cell identity — the load-bearing fact

`CellId == BSP leaf index`, 1:1, no remap, no compaction. `pack::encode_cells`
(`crates/level-compiler/src/pack.rs`) iterates `BspLeavesSection.leaves` once with
`enumerate()` and emits one `CellRecord` per leaf at the same index. Solid and exterior
leaves are **not** dropped — they become cells carrying `CELL_FLAG_SOLID` /
`CELL_FLAG_EXTERIOR` with `face_count = 0`. Doc comment in `encode_cells`: *"Cell ids stay
one-to-one with BSP leaf ids … all downstream consumers index by leaf id directly."*
Corroborated by `cell_draw_index_bake` (`BvhLeaf.cell_id` indexes `bsp_leaves[cell_id]`),
`FaceMeta.leaf_index` ("the value is a runtime cell id"), and `encode_cell_locator`
("preserves the BSP leaf id space").

Consequence: a bake computing over BSP leaves emits its Cell-keyed relation with no
translation. `cell_count == tree.leaves.len()`, dense `0..N`.

### Portalization is complete — no BVH needed for the sightline test

`generate_portals` / `distribute_portal` (`crates/level-compiler/src/portals.rs`) emits a
portal only when a splitting plane separates two children **both** `!is_solid`
(`if !front_leaf.is_solid && !back_leaf.is_solid`). Solid geometry = portal absence; a solid
leaf never appears as a portal endpoint. `find_exterior_leaves`
(`crates/level-compiler/src/visibility/mod.rs`) flood-fills only through portal adjacency,
treating solid leaves as barriers. So connectivity between non-solid cells is fully carried
by the portal graph, and a separating-plane sightline test over the portal chain is a
conservative visibility oracle without raycasting solid brush geometry. The compile-time
`bvh` (built at `StageId::BvhBuild`) is a render/trace accelerator, not the connectivity
oracle — the relation bake does not need it.

Caveat carried into the spec: exterior cells are non-solid and DO get portals among
themselves and to interior leak points. That is a classification choice on
`CELL_FLAG_EXTERIOR`, not a raycasting need.

### Bake ordering / data availability

Pipeline (`crates/level-compiler/src/pipeline.rs`): `generate_portals` →
`find_exterior_leaves` → `encode_vis` (produces the `BspLeavesSection` that `encode_cells`
iterates) → BVH build → `cell_draw_index_bake` → `pack_and_write_portals` (where
`encode_portals` / `encode_cells` / `encode_cell_locator` serialize). Everything the relation
bake needs — `result.tree`, `generated_portals` (leaf-keyed), `exterior_leaves`, and the final
cell count `= vis_result.leaves_section.leaves.len()` — is live from just after `encode_vis`.
`cell_draw_index_bake` is the template: a leaf-keyed bake that runs before packing and hands
pre-serialized bytes into the pack step.

### PRL section mechanism

`SectionId` enum + `from_u32` in `crates/level-format/src/lib.rs`; highest live id is 45
(`AnimatedDirectShDeltaVolumes`); id 14 (`LeafPvs`) is a **retired hole** — do not reuse it.
Next free id is **46**. Section module template: `crates/level-format/src/cell_locator.rs`
(graph-shaped, carries a per-section `CELL_LOCATOR_VERSION: u32` checked in `from_bytes`, plus
structural validation) — the closest analog. Writer: an `encode_*` in `pack.rs` pushes a
`SectionBlob { section_id, version: 1, data }`. Loader: `prl_loader.rs`
`read_section_data(...)` → `Section::from_bytes` → a `convert_*` lowering → field on
`LevelWorld` (`crates/level-loader/src/prl.rs`). Optional sections gate `Some(data) => … /
None => …`; mandatory ones `stale_section(...)`. File-wide `CURRENT_VERSION: u16 = 4`. Adding
an optional section needs no file-version bump (old maps lack it → fallback); the project's
recompile-everything convention still recompiles content.

### Neutral crate home — decided: `crates/level-loader`

Crate graph (`context/lib/crate-graph.md`): `visibility` is **Layer 2**, described as runtime
portal traversal + frustum visibility, dependents only `postretro` / `render-cpu` / `renderer`
— a render-side camera-cull crate. `net` is a **Layer-0 leaf** (wire transport); the network-
*relevance decision* runs in `postretro`, not `net`. There is **no `audio` crate** — audio is a
`postretro` module. AI perception, VFX, and E17-F doors are all in `postretro` / the render
stack. So every real consumer sits at `postretro` (L5) or the render layers, all of which
already depend on `level-loader` (L1).

Decision: home the runtime query on `LevelWorld` in a new `crates/level-loader` submodule,
beside `LevelWorld::locate_cell` and `cell_count()` — the same shape (a CellId lookup over
baked level data). This keeps `crates/visibility` single-purpose, adds no new crate, sits at the
neutral data layer reachable by all real consumers, and stays a pure CellId-only function
(neutrality contract satisfied). The future dynamic-mask widening still works — `portal_state`
is passed in as a param, never owned by the loader. This serves the substrate doc's *intent*
("neutral crate, not a consumer's crate") better than its literal example ("`crates/visibility`
or a sibling"): `level-loader` is the data layer, not a consumer, and holds the query precedent.
`crates/visibility`'s deps (`glam`, `log`, `postretro-level-loader`; dev `render-data`,
`level-format`) were the other candidate; rejected as concern-crowding a render-side cull crate.

### Observability runner

`crates/postretro/src/observability/` (feature `observability`). `driver::run_headless`
(entry via `--headless`), `RunSpec` (`runspec.rs`, `#[serde(deny_unknown_fields)]`, fields
`map`/`ticks`/`commands`/`dump: DumpSpec`), `to_deterministic_json` + `sort_json_maps`
(recursive key sort; **arrays left in data order** — pair lists must be pre-sorted by the
producer). `build_output_document(map, ticks_run, registry, dump, events, player)` is
**registry-only** — it never reads `world`, so surfacing a baked relation requires widening
its signature to carry the loaded relation and adding an `OutputDocument` field + record type
in `document.rs`. Driver already has `world` in scope, so the call-site change is trivial.
No precedent exists for dumping a static/baked query — this establishes the first.
`.map` fixtures: `content/dev/maps/` (`campaign-test.map`, `occlusion-test.map`,
`stress-warren*`, `wedge-shared-plane.map`). Test compile path:
`crates/level-compiler/tests/compiler_cli_contract.rs` `compile_fixture` spawns
`CARGO_BIN_EXE_prl-build` (`<in.map> -o <out.prl> --no-cache --no-tui`).

### AI target-selection seam (paper-check consumer, not wired here)

`select_target` in `crates/postretro/src/scripting/systems/ai/targeting.rs` takes
`candidate_perception: &mut dyn FnMut(TargetPawn) -> Option<RawTargetPerception>` — the
injectable per-candidate visibility gate, filtered on `perception.visible`. Concrete closure
at `ai/mod.rs` wraps `perception::raw_target_perception` (the exact-LOS raycast via
`collision::line_of_sight`). A cell perceptibility pre-filter would wrap that closure and
early-return `None` when the two cells are not mutually perceivable. **Gap:** the AI tick
carries `EntityRegistry`/`NavGraph`/`CollisionWorld` but **no `LevelWorld`**, so no
`locate_cell` is reachable there today. Wiring this consumer needs a cell-locator threaded
into the AI tick — real plumbing, not a drop-in. This is why the AI broad-phase is a
*paper-check* consumer here, not a built one.

### Superseded prior art — `context/plans/drafts/perf-anti-penumbra-pvs`

Sketched the anti-penumbra separating-plane portal flood (Teller 1992 §4; Quake `vis`
`ClipToSeperators` / `FindPassages`): per adjacent portal pair build separating planes (plane
through an edge of P_i and a vertex of P_j, normal so P_i is front, kept iff all of P_j is
front), intersect wedges along the chain, Sutherland–Hodgman-clip the target portal against
the running wedge stack, non-zero area ⇒ visible. Double precision throughout. **The algorithm
and citations are reusable; every identifier it names is dead** — `LeafPvsSection`, id 14,
`compute_pvs`, the compiler `visibility/portal_vis.rs`, and `postretro/src/portal_vis.rs` no
longer exist. Cite the math, not the seams. It tightened the *baked rendering* PVS, a use the
runtime narrowing frustum replaced; this substrate serves non-camera consumers instead.

## Generalizability paper-check (the doc's second-consumer gate)

The query returns, for a `CellId` pair, `CouplingTuple { perceivable: bool, distance:
Option<u32-fixed>, aperture: sentinel-in-v1 }` — Cell vocabulary only. Two consumers on paper:

**Network relevance (E15 Phase 4).** Per client, take the client's pawn Cell `c` via the
runtime locator. For each candidate entity, take its Cell `e`; replicate iff
`perceivable(c, e)`. Prioritize the send accumulator by `distance(c, e)` (path length, not
Euclidean). Relevance *radius*, *include-owner*, *hysteresis*, *grace period*, per-client
keying — all consumer-side, none in the query. Fits with CellId + policy-on-top.

**Audio PAS (E12).** Listener Cell `l`; a sound-source Cell `s`. Audible-at-all iff
`perceivable(l, s)` (the PVS) or its one-hop portal dilation (the PHS). Attenuate/lowpass by
`distance(l, s)` and, when it lands, `aperture(l, s)`. The dB curve, audible-range cutoff, and
obstruction model are consumer-side. Fits with the same query.

Both express their need as (map my domain object → its Cell) + query + (apply my own curve).
Neither needs a different substrate API. Gate passes on paper. Wire the smell-test audit
(no Player/Entity/Client/Sound/radius in the API) as an AC.

## Bake cost (measured) and guardrails

Measured by compiling the two largest dev fixtures (`--release`, `--no-cache --no-tui --verbose`).
Fixtures are NOT representative of real maps — there are none yet; `stress-warren` is a synthetic
pressure-probe. So these numbers are a *today-floor sanity check*, not a production projection.

| Fixture | Portals | Cells (BSP leaves) | Interior-empty (flood source set) |
|---|---|---|---|
| `stress-warren.map` (largest) | 2,561 | 3,339 | 956 |
| `campaign-test.map` | 807 | 434 | 190 |

Per-stage Build Summary (campaign-test, total 257.5s): **SH Bake 212.7s (~83%)**, ShadowmaskAtlas
19.6s, Lightmap 12.9s, TextureMips 5.5s, Delta/Direct SH ~3.3s, **Visibility (portal gen +
exterior flood + encode) 0.00s**, BVH 0.00s. Timing infra: `begin_stage`/`finish_stage` in
`pipeline.rs`, printed by `reporter.rs` "Build Summary". No PVS timing exists anywhere in git
history — the retired `LeafPvs` BFS was never benchmarked.

Complexity: portals are ~4-vertex; per portal-pair the separating-plane + Sutherland–Hodgman clip
is a small constant. Loose-regime flood is ~O(P²) pair-tests, aggressively pruned by wedge-empty
termination. Analytic estimate at P=2,561: single-threaded low-tens-of-seconds worst case, more
likely seconds, sub-second on campaign-test; parallel over 956 source cells drops wall-clock by
core count. Dijkstra distance is O(P log P), trivial. Verdict: **low risk at current scale** — the
largest fixture has an order of magnitude fewer portals than the shipped Quake maps where `vis`
famously cost minutes-to-hours (10k–50k+ portals), and SH-bake headroom (213s) dwarfs any
plausible PVS cost. The estimate is analytic, uncertain by ~an order of magnitude on adversarial
topology, and can't be validated against representative content until real maps exist.

Guardrail prior art — `context/plans/drafts/perf-anti-penumbra-pvs/index.md` (algorithm/citations
reusable; identifiers dead): a `--pvs-fast` BFS fallback for iteration; a wall-clock "3×-BFS
auto-degrade"; and it deliberately shipped **no recursion depth cap** (deferred "only if a
pathological map surfaces"). Parallelism: the SH/lightmap bakers run a rayon parallel
iterator (`into_par_iter()` in `sh_bake.rs`; `par_iter().enumerate()` in `lightmap_layer.rs`) +
`control.governor().enter()` per work-item — the per-source-cell flood reuses this directly, with an
order-preserving collect so per-cell rows land index-aligned to cell id (see index.md pin P1).

Decision (owner): keep the sightline PVS in v1 (affordable now), plus three guardrails, all
**determinism-preserving** (AC10 byte-identical). We adopt `--pvs-fast` and stage-timing logging,
add an explicit **recursion/chain-depth cap** (closes the draft's deferred gap), and **reject the
draft's wall-clock 3×-BFS auto-degrade** — a timing-triggered branch makes output machine-load-
dependent and breaks byte-identical determinism. The depth cap gives the same worst-case protection
deterministically: past the cap the flood conservatively includes (localized reachability
fallback), zero false negatives preserved.

## Owner-approved divergences from `context/research/cell-visibility-substrate.md`

The substrate doc is design intent; two of its build-guidance clauses are consciously
overridden by the owner for this build:

1. **"Trigger is a real, measured consumer need."** Overridden: every consumer in the roadmap
   (E15 relevance, E12 audio, E10 perception, E17-F doors, VFX) is a committed item, so the
   substrate is shared foundation, not speculative infra. Built ahead of the measured-need
   gate deliberately.
2. **"Build WITH the first real (gameplay) consumer, A→B."** Overridden: proven by property
   tests + an observability dump + the on-paper generalizability gate above, not a wired
   gameplay consumer. Rationale: foundation-first, all consumers table-stakes; the runtime
   struct is complete day one so no consumer signature churns; the observability dump is the
   thin first caller of the query; the paper-check stands in for real-usage API validation.
   The doc's *architectural* clause — consumer-agnostic, CellId-only, neutral crate — is
   **kept** in full.

## Runtime-state note

v1 is a static baked relation with no runtime mutation — the dynamic blocker-mask layer (a
door flipping a portal-state bit) is deferred to the door/destructible consumer. So there is
no runtime ordering/timer/event surface to enumerate; the only "two candidate values per pair"
merge is the symmetrization tie-break, captured as an invariant, not an ordering.

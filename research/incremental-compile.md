# Incremental Compilation for prl-build

> **Status:** Research — not a design decision or plan.
> **Audience:** Developer evaluating whether to invest in incremental compile support for `postretro-level-compiler`.

---

## 1. Pass Cost and Input Classification

Based on the pipeline in `main.rs`, every pass is linear in the compiler's output (no external dependencies, no RNG — both `sh_bake` and `lightmap_bake` are deterministic by construction).

| Pass | Rough cost | Primary inputs |
|------|-----------|----------------|
| Parse | Cheap (IO + brush parsing) | `.map` file |
| BSP partitioning | Medium (plane-split recursion) | Brush geometry |
| Visibility / portals | Cheap–medium | BSP tree |
| Geometry extraction | Cheap | BSP tree, faces |
| BVH build | Cheap (SAH, small N) | Geometry |
| Lightmap bake | **Expensive** (O(faces × texels × lights × rays)) | Geometry, BVH, static lights, `--lightmap-density` |
| SH volume bake | **Expensive** (O(probes × RAYS_PER_PROBE=256 × lights)) | Geometry, BVH, BSP tree, static + animated lights, `--probe-spacing` |
| Chunk light list bake | Medium (shadow rays per chunk × light) | Geometry, BVH, static lights |
| Animated light chunks | Cheap (spatial partition, no raytracing) | BVH leaves, face charts, animated lights |
| Animated light weight maps | **Expensive** (O(chunks × texels × animated lights)) — parallelized via rayon | BVH, geometry, chunks, animated lights, charts, placements |
| Pack | Cheap (IO) | All sections |

The three expensive passes — lightmap bake, SH volume bake, and animated weight maps — all do per-texel or per-probe raytracing through the shared BVH. These are the only passes worth caching.

---

## 2. Dependency Graph

```
.map file
    │
    ▼
Parse ──────────────────────────────────► lights (all)
    │
    ▼
BSP partitioning
    │
    ├──► Visibility / portals
    │         │
    │         └──► exterior_leaves
    │
    └──► Geometry extraction ──► face_index_ranges, geo_result
              │
              ▼
           BVH build ──► bvh, bvh_primitives, bvh_section
              │
              ├──► Lightmap bake (geometry + BVH + static-non-animated lights)
              │         │
              │         └──► charts, placements, atlas dims, lightmap UVs (written into geo_result)
              │
              ├──► SH volume bake (geometry + BVH + BSP + exterior + static + animated lights)
              │
              ├──► Chunk light list bake (geometry + BVH + static lights)
              │
              ├──► Animated light chunks (bvh_section mut + charts + animated lights)
              │         │
              │         └──► chunk_section (stamped into bvh_section)
              │
              └──► Animated light weight maps (BVH + geometry + chunks + charts + placements + animated lights)
```

Key observations:
- **Geometry-only group:** BSP, visibility, geometry extraction, BVH build. Change any brush → must rerun all of these and everything downstream.
- **Light-only group (given fixed geometry):** All three bake passes (lightmap, SH, weight maps) and chunk light list can be re-run without touching geometry/BSP. The lightmap bake also *writes lightmap UVs back into `geo_result`*, which means the geometry section of the PRL file changes when the lightmap bake runs — but the vertex positions and structural geometry do not.
- **Animated-light-only group:** The animated weight maps and chunk descriptors can be rerun if only animated light properties change, provided geometry and the lightmap atlas layout are stable.

There is one tight coupling to watch: `lightmap_bake` mutates `geo_result` by writing per-vertex `lightmap_uv` fields and by calling `split_shared_vertices`. If lightmap UVs change (e.g., due to a geometry change or density change), the atlas layout changes and the animated light weight maps must be rebuilt in full — they index directly into the same atlas rectangles.

---

## 3. A Content-Hash / Dirty-Flag Scheme

### What to hash per pass

| Cache key | Contents |
|-----------|----------|
| Geometry key | SHA-256 of raw `.map` file bytes + format flag + `--pvs` flag |
| Light key | SHA-256 of the serialized light list (all `MapLight` structs, including `is_dynamic`, `animation`, `bake_only`) + `--probe-spacing` + `--lightmap-density` |
| Format version | `CURRENT_VERSION` from `postretro_level_format::CURRENT_VERSION` (currently `1`) |
| Compiler version | Cargo package version string from `postretro-level-compiler` |

A combined cache entry is valid only when all four match. Separating the geometry key from the light key allows the light-only fast path.

### Cache storage

A sidecar file alongside the output, e.g. `campaign-test.prl.cache`, containing:

```
[header]
format_version: u16
compiler_version: string (length-prefixed)
geometry_key: [u8; 32]   # SHA-256 of .map + flags
light_key: [u8; 32]      # SHA-256 of light list + params

[sections]
lightmap: blob           # cached LightmapSection bytes
sh_volume: blob          # cached ShVolumeSection bytes
chunk_light_list: blob   # cached ChunkLightListSection bytes
anim_weight_maps: blob   # cached AnimatedLightWeightMapsSection bytes (optional)
anim_light_chunks: blob  # cached AnimatedLightChunksSection bytes (optional)
bvh_section: blob        # cached BvhSection bytes (needed for chunk_range_ fields)
```

Geometry sections (BSP, portals, geometry vertices/indices) are never cached — they recompute fast and serve as the foundation for everything else.

**Invalidation rules:**

1. `format_version` or `compiler_version` mismatch → invalidate everything.
2. `geometry_key` mismatch → invalidate everything (geometry changed means atlas layout, probe grid, and BVH all change).
3. `light_key` mismatch, `geometry_key` matches → invalidate lightmap, SH, chunk light list, anim chunks, and anim weight maps; reload the cached BVH section.
4. Only animated lights changed (no change to static lights, geometry unchanged) → could narrow further to anim weight maps and anim chunks only, but tracking this sub-distinction is extra complexity for modest gain.

### Fast path invocation

```
prl-build input.map -o output.prl --incremental
```

On a geometry-only change the full rebuild runs. On a lights-only change, the expensive bake passes are skipped and cached section bytes are spliced into the pack step.

---

## 4. Risks and Complications

**4.1 Atlas layout stability.** The lightmap bake writes per-vertex `lightmap_uv` back into `geo_result` via `assign_lightmap_uvs`. If the geometry hash matches but the atlas layout were to change for any other reason (e.g., a different Rust standard library sort order in `shelf_pack`), the cached weight maps would reference stale atlas positions and show seams. The shelf packer is deterministic given identical chart dimensions, but this invariant is implicit — there is no written contract. A regression test that asserts atlas layout is deterministic across two runs (independent of Rust version) would be necessary before trusting the cache.

**4.2 Format version mismatches.** `CURRENT_VERSION = 1` in `postretro_level_format::lib.rs` is a single u16 over the entire PRL format. If any section's binary layout changes without bumping the version, a stale cache produces a malformed PRL silently. This is the worst failure mode. Mitigation: hash the compiler binary itself (or embed a build hash), not just the format version constant.

**4.3 Animated light dependency on static geometry.** The animated light weight maps (`animated_light_weight_maps.rs`) consume `charts` and `placements` from the lightmap bake, which are geometry-derived. If geometry changes, animated weight maps must be fully rebuilt even if no animated light changed. This is correctly captured in the dependency graph above.

**4.4 BVH section mutation by the animated chunks builder.** `build_animated_light_chunks` mutates `bvh_section` in place, stamping `chunk_range_start` / `chunk_range_count` on every `BvhLeaf`. This means the cached `BvhSection` bytes already include the chunk range fields — they cannot be produced by the geometry-only path and then patched by the light-only path. The cache must store the fully-stamped `BvhSection`, and the light-only fast path must produce a new `bvh_section` from scratch (cheap — it's a flattening of the BVH tree, not raytracing) and re-stamp it.

**4.5 `split_shared_vertices` is destructive.** The lightmap bake calls `split_shared_vertices` on `geo_result`, which duplicates shared vertices. After a cached bake hit, the geometry section in the PRL contains the post-split vertex count. A rebuild that skips the lightmap bake (because lights changed, not geometry) would run `split_shared_vertices` again on a fresh `geo_result` — duplicating the same vertices again. The cache scheme must either always run the lightmap bake for vertex layout, or cache the post-split geometry alongside the lightmap section.

**4.6 Retried atlas density.** The main loop retries `bake_lightmap` up to three times at coarser texel densities on atlas overflow. The final `lightmap_density` (not the CLI `--lightmap-density`) is what the animated chunk builder receives. A cache hit must store and restore the effective density, not the CLI-requested density.

---

## 5. Minimum Useful Slice

**Goal:** Skip the SH bake and the animated weight map bake when only light properties (intensity, color, falloff, animation curves) change and no light is added or removed.

**Why this matters:** The SH bake (`sh_bake.rs`) fires 256 rays per probe, parallelized but still proportional to map volume. The animated weight map bake (`animated_light_weight_maps.rs`) fires rays per texel per chunk. Together these are the majority of build time on a well-lit map.

**What it requires:**

1. Hash the `.map` file. If it matches the last run, assume geometry is stable (BSP, geometry extraction, BVH).
2. Hash the light list (parsed `MapLight` structs). If it also matches, use the fully cached PRL.
3. If only the light hash changed, re-run from the lightmap bake onward, reusing the cached geometry/BVH sections. This requires that the geometry sections be written to the PRL first with stale lightmap UVs, then patched — or more simply, that the compiler always runs geometry then bake, and the cache substitutes the bake outputs.

**Estimated implementation effort:** Medium. The cache file format is straightforward. The tricky parts are:

- Correctly threading the cached sections through `pack_and_write_portals` / `pack_and_write_pvs` (both take all sections by reference — no structural change needed, just pass cached bytes).
- Handling the `split_shared_vertices` / post-split vertex layout issue (§4.5 above) — this likely requires caching the post-split geometry alongside the bake outputs.
- Writing enough regression tests to trust the cache is not silently stale.

**Minimum viable implementation without `split_shared_vertices` complication:** cache only the SH volume section (`ShVolumeSection`). When the geometry key matches and only static light properties changed (no animated lights, no geometry), substitute the cached SH section. The SH bake does not write into `geo_result`, so there is no vertex layout coupling. This is the least risky starting point — roughly 300–400 lines of new code (hashing, sidecar read/write, cache bypass in `main.rs`).

---

## 6. Can the BVH Limit Re-baking to Spatially Affected Regions?

A natural question after reading §5: instead of re-running the full bake when lights change, can we use the BVH to identify only the faces and probes within a changed room and re-bake just those? The answer is **no — not without significant restructuring**.

### Lightmap atlas has no spatial locality

The atlas is a flat 2D shelf-packed grid. Faces are placed at arbitrary 2D positions with no correlation to their 3D location. There is no texel → (face, local\_uv) reverse index, so there is no way to read back "which atlas region covers room X" and patch only those texels. Compounding this, `split_shared_vertices` (`lightmap_bake.rs:261–304`) duplicates shared vertices before UV assignment and then discards the mapping. Any geometry change requires re-running this pass, which invalidates all per-face UV positions — so even a spatially restricted re-bake would need to re-derive the atlas layout for affected faces, which cascades into the global shelf-pack.

Within a single pass, every texel already evaluates every static light with a falloff-range early-exit, but there is no pre-pass spatial culling that skips lights whose sphere doesn't reach the face. Adding that culling wouldn't help partial re-baking — it would help per-texel performance, which is a different problem.

### SH probe grid is global and unindexed

The probe grid is one uniform 3D grid over the entire world AABB — not per-room, not per-cell. Probes carry no compile-time index of which lights affect them; the BSP leaf for each probe position is re-derived at bake time via tree walk (`sh_bake.rs:219`). No probe → lights index is written into the PRL output, and the set of affected probes for a given light's falloff sphere has no structure to query cheaply.

Single-bounce indirect (BOUNCE_ALBEDO = 0.5, `sh_bake.rs:37`) does bound a light's influence to its falloff sphere, so the affected probe set is geometrically bounded — but without a spatial index over probes, finding that set costs a full grid scan.

### Animated weight maps are the closest — but only at compile time

Weight maps are the one pass with real spatial structure. `InfluenceRecord` (center + radius, `animated_light_chunks.rs`) is used during chunk building to cull faces via sphere–AABB overlap. This is exactly the kind of per-region structure that partial re-baking would need. The problem: `InfluenceRecord` is a compiler-only intermediate — it is not stored in the PRL output. The runtime cannot query "which lights affect region X" without rebuilding from scratch.

### BVH leaves record cell IDs but no reverse index exists

`BvhLeaf.cell_id` records which cell a leaf's geometry lives in, so in principle a "changed room" → affected BVH leaves mapping could be built. But no reverse cell → face-list index exists in either the compiler or the PRL format, so that mapping has to be computed from scratch each time. The BVH is used only for occlusion ray-casting during baking — it does not partition bake results.

### What would actually be required

To make BVH-guided partial re-baking work, you would need all of:

1. A face → cell reverse index (changed room → affected face list)
2. An atlas-texel → (face, local\_uv) reverse index
3. A probe → affected-light-indices index stored in PRL
4. Stable, cacheable vertex addressing (eliminate or memoize the `split_shared_vertices` discard pattern)
5. Sparse result representation for both the atlas and probe grid (today both are dense flat arrays)

None of these exist. The two-stage split described in §7 (Prior Art) remains the most tractable path. A lights-only re-bake avoids `split_shared_vertices` entirely and does not need the reverse atlas index — it re-runs the three expensive passes with a stable vertex layout. The structural couplings called out in §4 (animated chunks stamping into `BvhLeaf`, effective atlas density needing to be cached) are the remaining blockers for that path, and they are narrower engineering problems than building the spatial index infrastructure described above.

---

## 7. Prior Art

The existing research in `research/bo3-rbdoom3-compile-targets.md` is directly relevant. Its key finding: **both Black Ops 3 and rbdoom-3-bfg separate compilation into independent passes**, each of which can run standalone. BO3's "onlyents" mode recompiles entity data (including lights) without touching geometry. rbdoom-3-bfg's `bakeLightGrids` command is a separate invocation from `dmap` geometry compilation.

The classic Quake toolchain (`qbsp` / `light` / `vis`) also separates concerns: `qbsp` builds BSP geometry, `light` computes lightmaps, `vis` computes PVS. Each tool reads the previous tool's output file and writes its own. This is the simplest form of incremental compilation — separate executables with file-level dependencies. TrenchBroom's compile preset system exposes these as separate steps the user can choose to skip.

Postretro's current single-binary, single-pass design trades authoring simplicity for build-time agility. Moving to a two-stage model (`prl-build --geometry` → `campaign-test.geo.prl`, then `prl-build --bake` → `campaign-test.prl`) would be architecturally clean and aligned with the BO3/Quake art. It avoids the cache-invalidation complexity entirely: the geometry stage produces a stable intermediate artifact the bake stage reads; rebuilding lights rebuilds only the bake stage.

---

## 8. Go / No-Go Summary

**Go signals:**
- The three expensive passes (lightmap, SH, animated weight maps) are clearly separable from the geometry passes at the data level.
- Both passes are fully deterministic — identical inputs produce byte-identical outputs. No random state to worry about.
- A minimal win (cache only `ShVolumeSection`) has very limited blast radius and no vertex-layout coupling.

**No-go signals / things to resolve first:**
- The `split_shared_vertices` mutation in `lightmap_bake.rs` complicates any cache scheme that tries to reuse the geometry section; this should be understood and documented as a known coupling before building the cache.
- The `bvh_section` mutation in `animated_light_chunks.rs` means the geometry-only fast path cannot produce a complete BVH section without re-running the chunk builder.
- No format versioning below the `CURRENT_VERSION` level exists — any cache scheme is a bet that the format stays stable. For a pre-release codebase, that bet has short odds.

**Recommended starting point if go:** implement the minimal SH-only cache (skip `bake_sh_volume` when `.map` file hash and light list hash both match the sidecar). This is isolated, reversible, and yields the most consistent win on maps with many static lights and dense probe grids.

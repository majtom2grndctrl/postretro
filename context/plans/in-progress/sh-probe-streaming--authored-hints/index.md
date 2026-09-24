# SH Probe Streaming — Authored Hints

## Goal

Let mappers influence SH cluster boundaries and warm-up without replacing the
algorithm-default partition. Brush hints mark streaming seams, pin important
regions, and rank pressure-eligible prefetch under the existing residency budget.

## Scope

### In

- Three compiler-only TrenchBroom brush entities: `streaming_seam_volume`,
  `stream_resident_volume`, and `stream_priority_region`.
- Deterministic portal association, seam-constrained clustering, versioned
  id-49 hint metadata, shared semantic validation, and rebuilt id-50 chunks.
- Runtime seam warm-up, always-resident pins, priority-aware request and
  pressure ordering, bounded diagnostics, and headless regression tests.
- A manual doorway check when a runnable hinted PRL and GPU are available.

### Out

- Door movement, gameplay collision, portal visibility, co-op authority, or
  map identity rules beyond the existing PRL content hash.
- A hard guarantee that a cold door never reveals ambient-floor SH. Opening
  before an async install composes keeps the Slice 3 miss fallback.
- Streaming billboards, textures, lightmaps, geometry, or other 2D resources;
  new GPU bindings, shaders, or per-fragment residency lookups.
- Player-facing budget settings, new runtime scripting primitives, and `unsafe`.

## Shared context

Every task reads `context/lib/index.md`, `development_guide.md`,
`testing_guide.md`, `build_pipeline.md` (FGD, source-format boundary, PRL),
`rendering_pipeline.md` (SH and frame handoff), this spec's AC and Invariants,
and `research.md`. Read the parent epic and Slice 3 spec once. Source wins over
stale line anchors. `parse.rs` and `cluster_directory.rs` exceed the file-size
guideline; split relevant seams before extending either file.

## Decisions

### Authoring and geometry

All three entities own exactly one convex brush. Zero or multiple brushes fail
the bake with classname and entity location. They emit no world geometry,
collision, runtime `MapEntity`, or non-SH gameplay effect. Peel them from the
static brush set before BSP construction: they change neither BSP/portals,
geometry/BVH, collision, lightmaps, SDF, navmesh, nor any non-SH bake. Invalid,
non-finite, or zero-volume hulls are hard author errors, never warning-and-skip.
The Quake adapter converts geometry to engine meters; shared partition and runtime see only
canonical coordinates and portal IDs, never FGD classnames.

`streaming_seam_volume` marks every portal polygon with positive-area
intersection with its convex hull *and* brush interior on both sides of the
portal plane. Follow the kinematic portal-geometry precedent: clip at
`PORTAL_EPSILON = 0.01` m, certify at zero epsilon, and require intersection
area above `1e-12` m² plus strictly positive hull depth on both sides. Face,
edge, and point contact do not match; AABB is only a broad phase, with convex
brush planes deciding membership. Zero matches is an author error. Duplicate
matches from overlapping seam brushes collapse to one portal ID. Marked portal endpoints
must occupy different clusters. The canonical greedy partition keeps its
current seed key and limits, but cannot traverse a marked portal or admit a
cell that would put both endpoints of a marked portal in one cluster via an
alternate path. Thus a seam is a cut constraint, not a manually numbered
partition. Repeated cold bakes produce identical directory and SH chunks.

`stream_resident_volume` pins any cluster containing a runtime cell whose
world-space AABB overlaps the brush's resolved AABB by positive volume. Empty
match is an author error. Pinning a cluster also pins its baked owner closure
at runtime.
`stream_priority_region` uses `_stream_priority` as an integer in `0..=3`;
absent or blank is zero. It applies to the same resolved-AABB cell match.
Overlaps take the maximum priority per cluster. Zero priority emits no hint;
nonzero priority changes only pressure-eligible SeamWarm/Prefetch request,
ready-install, suppression, and eviction order, never visible work or owner
safety. Priority is the budget hint: it ranks who retains scarce optional capacity, not the byte
floor itself. No hints and no-op priority hints preserve the baseline
partition and normalized controller trace.

### Wire and runtime

Id 49 advances internal `u32` word 0 from version 1 to 2 and container `u16`
section version from 1 to 2. The existing 40-byte little-endian header keeps
the meanings of words 1–7 (offsets 4–28); words at byte offsets 32 and 36
become `seam_portal_count` and `cluster_hint_count`. Existing cluster,
resource, member, and range tables keep their layout and order. Append sorted,
unique seam portal IDs (`u32` each), then sorted, unique 16-byte cluster hint
records: `cluster_id: u32`, `flags: u32` (bit 0 = pinned), `priority: u32`
(`0..=3`), `reserved: u32` (zero). Counts of zero encode empty lists; no
sentinel is used. A hint record with neither pin nor nonzero priority is
invalid. Arithmetic and allocation checks precede reads. The loader rejects
v1 with a named version mismatch; the owner has accepted breaking pre-stable
PRLs. Id 50 keeps its current format, but compiles new chunks whenever seam
cuts change id-49 membership. No-hint input still uses v2 and must preserve
the old partition and normalized controller trace, not old id-49 bytes. The
trace compares per-tick class, target, request, suppression, and eviction
cluster IDs and order; version, generation, and content tag are excluded.

Shared semantic validation recomputes the canonical partition using the
persisted seam IDs. It rejects out-of-range, unsorted, duplicate, same-cell,
or same-cluster seams and invalid hint records with named errors. Compiler
readback validates the same rules. Id-49 bytes remain part of the manifest
content tag, so a hint edit cannot reuse a stale stream generation.

The loader retains portal-ID endpoints alongside cluster adjacency. When a
near-side cluster is visible, each marked portal promotes its far-side
cluster to `SeamWarm`, even if an opaque door blocks render traversal. This
does not change the renderer's visible-cell set. Owner closure propagates
`(class, priority)` to a fixed point, revisiting an owner when priority rises
within the same class. Effective priority is the maximum of the cluster's
authored priority and inherited same-class dependent priorities only for
SeamWarm/Prefetch; it is zero for Visible, Pinned, and Hysteresis. Thus an
urgent optional dependent cannot wait behind its owner. Pins are always targets
and cannot be pressure-suppressed or evicted. Only Prefetch and SeamWarm are
pressure-eligible, provided no current target depends on them. Hysteresis
remains protected until its timer expires.

Request and ready-install order is `(class rank, descending effective priority,
cluster ID)`, with class rank Visible, Pinned, SeamWarm, Prefetch, Hysteresis.
Pressure selects victims to suppress by `(Prefetch before SeamWarm, ascending effective priority,
oldest visible time, oldest target time, cluster ID)`; departed eviction after
hysteresis retains its existing `(oldest visible time, oldest target time,
cluster ID)` order. Once victims are selected, the planner may request
dependent-before-owner evictions in nonnumeric order. The renderer recomputes
a dependency-safe release sequence and returns confirmed IDs sorted; that
release order does not change which optional target survives. Thus all-zero/no-hint inputs retain
each old ordering.
A newly activated seam clears stale suppression for its warm target and
required owners even if the ordinary two-hop horizon has not changed.
Hysteresis and suppression remain time-based. Overshoot
from visible/pinned/owner-locked demand follows the existing once-per-onset
warning and controlled pool-growth policy. If GPU pool growth is impossible,
the existing named `ShResidencyDrainError::GpuCapacity` surfaces; a pin is not
silently evicted to make it fit.

All hint effects stop at the CPU planner. The renderer still admits only
generation-matched atomic batches before compose and samples new installs on
the next submitted frame. A seam opened cold shows the existing ambient-floor
placeholder rather than stale or uninitialized atlas data.

## Boundary inventory

| Concept | FGD KVP / classname | Authoring adapter output (engine space) | Finalized resolved data | PRL id 49 v2 | Runtime |
|---|---|---|---|---|---|
| Seam | `streaming_seam_volume` | convex hull/AABB + source location | sorted portal IDs/endpoints | `seam_portal_ids: u32[]` | `SeamWarm` target |
| Pin | `stream_resident_volume` | convex hull/AABB + source location | matched cells → cluster pin | hint `flags & 1` | non-evictable target plus owners |
| Budget priority | `stream_priority_region`, `_stream_priority` `0..=3` | convex hull/AABB + priority + source location | matched cells → max per cluster | hint `priority: u32` | optional request/install/retention rank |

## Acceptance criteria

- [ ] A no-hint bake retains the pre-slice canonical cell membership and id-50
      SH content against a tiny pre-slice golden section hash, and a controller
      fixture with all hint values zero produces
      the same normalized per-tick class/target/request/suppression/eviction
      cluster IDs and order as before, excluding version/generation/content tag.
- [ ] Each brush class is available in `postretro.fgd`; compile rejects zero/
      multi-brush entities, non-finite/fractional/out-of-range priority,
      invalid/non-finite/zero-volume hulls, seam-with-no-portal, and pin/priority
      regions with no cell overlap by
      named errors. Blank priority takes zero. An otherwise identical hinted
      map preserves Cells, Portals, Geometry/BVH, collision, lightmap, SDF,
      navmesh, and non-SH bake output.
- [ ] A seam brush spanning a multi-polygon door marks the matching portals,
      no adjacent near-miss or face/edge/point-contact portal, and forces every
      marked endpoint pair into
      different clusters. Two cold bakes produce byte-identical id-49/id-50
      data for the same input.
- [ ] A v2 directory round-trips compiler → format → loader. Malformed counts,
      ordering, flags, priorities, reserved words, portal IDs, cross-cluster
      semantics, or partition membership fail with named errors before runtime
      planning. A v1 internal word with v2 container version and a v2 internal
      word with v1 container version each fail by named version mismatch, not panic.
- [ ] A visibility/controller integration fixture or windowed run proves that
      when a marked door hides the far-side cell, visible near-side cells
      target far-side SH before the door opens; static capture alone is not
      proof. A cold opening uses the Slice 3 miss fallback until atomic install
      and compose complete; gameplay and portal visibility remain unchanged.
- [ ] A pinned cluster and its owner closure remain targeted across empty
      visibility and budget pressure, never evict; excess protected demand
      warns once per onset and drops the warning latch after recovery. A
      GPU-free controller fixture proves pin retention; a separate pure
      renderer-capacity fixture reports the named `GpuCapacity` error for
      impossible growth. Review confirms the live handoff never silently
      selects a pin as an eviction victim.
- [ ] Under pressure, higher priority SeamWarm/Prefetch clusters among
      simultaneously eligible, owner-safe peers request and ready-install
      before lower priority peers of the same class and survive
      them longer, while visible work wins
      regardless of authored priority. Zero-priority ordering matches the
      Slice 3 baseline and stationary suppression does not churn; seam
      activation clears stale suppression even with unchanged two-hop horizon.
- [ ] Focused compiler/format/loader/controller tests pass. The full format,
      Clippy, and test gates pass after integration. A short manual doorway
      check records adapter, fixture, mode, and whether seam pops were seen;
      two explicit `--no-cache` bakes verify id-49/id-50 determinism. Record
      `not-yet-evaluable` only when GPU proof is unavailable, not as a substitute
      for a missing fixture or run procedure. Frame-time and visual observations
      are tuning inputs, not feature-existence gates; malformed format data and
      behavioral regressions remain correctness gates.

## Invariants

| Invariant | Established by | Threatened at | Verified by |
|---|---|---|---|
| One deterministic partition in compiler and loader | Tasks 3–4 | Seam portal cuts, alternate graph path, id-50 regrouping | AC 1, 3, 4 |
| Hints cannot change gameplay/visibility | Tasks 2, 4–5 | Door and visible-cell plumbing | AC 2, 5 |
| Visible, pinned, and their owners are never pressure victims | Task 5 | Suppression, departed eviction, renderer owner pins | AC 6, 7 |
| A frame samples only composed generation-matched SH | Existing Slice 3; Task 5 preserves | Warm target added during frame, stale completion | AC 5, 8 |
| No-hint input keeps algorithm-default behavior | Tasks 2–5 | New wire defaults and ranking comparators | AC 1, 7 |

## Tasks

### Task 1 — Split large seams without behavior change

Extract the authoring-region handling around `parse.rs` into a focused
submodule, and the canonical partition/wire helpers around
`cluster_directory.rs` into focused submodules. Preserve public re-exports,
existing bytes, tests, and callers. Freeze a no-hint controller trace fixture
before later tasks change ranking: compare per-tick class/target/request/
suppression/eviction IDs and order, excluding version/generation/content tag.
Freeze the current compiler's id-50 section hash and cell-membership list on
a tiny fixed no-hint fixture before the wire/partition edits, so AC1 has a
real pre-slice baseline rather than a newly invented expectation.
This precedes edits in those files.

### Task 2 — Authoring parse

Add the three FGD entries and canonical compiler hint types. Parse their
brushes in `parse.rs`'s extracted region handler into `MapData`, with
an explicit pre-`has_brushes` check rejecting zero-brush hint entities;
invalid/non-finite/zero-volume hulls hard-fail rather than warning and
returning `None`. Keep the resolved engine-space hull/AABB, priority, and
source location in `MapData`; do not attempt to name portals before they
exist. Exclude all hint brushes from the static world set before BSP and all
non-SH outputs. Add parse/validation tests and a hinted-versus-unhinted map
comparison for Cells, Portals, Geometry/BVH, collision, lightmap, SDF,
navmesh, and every applicable non-SH section. No id-49 producer changes yet,
so this task compiles independently.

### Task 3 — Versioned directory and canonical cuts

Implement id-49 v2 encode/decode, checked size/layout, semantic validation,
and seam-constrained canonical partition in level-format. Internal `u32`
version word 0 and container `u16` version become 2. The 40-byte LE header
keeps words 1–7; offsets 32/36 hold seam/hint counts. Existing tables keep
their order and layout, followed by sorted unique seam portal `u32` IDs and
sorted unique 16-byte hints `(cluster_id u32, flags u32 [pin bit 0], priority
u32 0..=3, reserved u32 zero)`. Empty lists have zero counts and no sentinel;
reject bad order/range/flags/reserved/no-op records and v1/mixed versions by
named errors. A marked portal cuts traversal and forbids a cluster containing
both endpoints, even through another portal route; no seam retains the old
seed key, limits, and membership. Update every
compiler/loader/test constructor to pass empty seam/hint lists until Task 4
resolves authored geometry; this task remains independently buildable. Update
section table version and loader version checks. Keep old
cluster/resource/member/range layout and id-50 schema fixed. Add malformed
wire, alternate-route cut, round-trip, and no-hint partition tests.

### Task 4 — Compiler resolution and publication

Define `ResolvedStreamingHints { seam_portal_ids, pinned_cell_ids,
cell_priorities }` (proposed compiler-owned name, with `cell_priorities`
holding `(cell_id, priority)` pairs) and resolve Task 2's
engine-space hints against generated/packed portal polygons and packed runtime
cells in `build_finalized_cluster_metadata`. A seam clips a valid convex
portal polygon against the brush hull using the existing 0.01 m portal
tolerance as a broad check and zero-epsilon certification; intersection area
must exceed `1e-12` m² and the hull interior must have positive depth on
both portal-plane sides. Face/edge/point-only contact does not match.
Pins and priority map by positive-volume overlap between cell AABB and
resolved brush AABB; overlapping priority takes max in `0..=3`, and zero
emits no record. Pass the result from
`pipeline.rs` through `cluster_directory_bake.rs`. After Task 3's partition,
emit one merged, sorted `ClusterHintRecord` per affected cluster; never put
hint flags in `ClusterRecord.flags`, whose bit 0 already means indivisible
oversize. Build id-50 from that directory. `pack.rs`'s standalone/default
bake entry explicitly passes empty
hints. Compiler readback validates the same id-49 semantics as the loader.
Add geometric resolver tests and a compiled-doorway PRL round-trip proving
portal IDs and hint records survive compiler → format → loader. Two cold bakes
of the same hinted map must yield identical id-49/id-50 sections. Create the
minimal committed doorway `.map` fixture at
`content/dev/maps/sh-streaming-hinted-door.map` for this and Task 5.

### Task 5 — Planner hint lifecycle

Pass validated portal IDs/endpoints and cluster hints from
`ShStreamManifest`/`prl_loader.rs` to `PlannerTopology`. Extend
`ShResidencyController::update_targets`, request and ready-install ranking,
suppression, eviction, and overshoot handling with Pin and SeamWarm classes.
Propagate `(class, effective priority)` through owner closure to a fixed
point, revisiting an owner on same-class priority rise. Class rank is
Visible, Pinned, SeamWarm, Prefetch, Hysteresis. Only owner-safe Prefetch and
then SeamWarm may yield under pressure; Hysteresis stays until its timer
expires and pins/visible never yield. Effective priority is max of own and
same-class dependent authored priorities only for SeamWarm/Prefetch; it is
zero for protected classes. Request/ready order is `(class, descending
priority, cluster ID)`. Pressure victim selection order is `(Prefetch before SeamWarm,
ascending priority, oldest visible time, oldest target time, cluster ID)`;
expired departures keep the old time/time/ID comparator. Drain eviction
requests may be dependent-first; confirmed renderer outcomes remain sorted.
Keep `main.rs`'s
pre-compose drain and renderer data contract unchanged; the planner consumes
the existing visible-cell signal, while seam endpoints derive from the
loaded portal table. A seam entering `SeamWarm` clears stale suppression for
its target/owner closure even if two-hop membership is unchanged. Preserve
all zero-priority ties. Impossible GPU growth reports the named capacity
error, never an implicit pin eviction. Prove pin retention in a GPU-free
controller test and impossible growth in a separate pure renderer-capacity
test; review the handoff between them. Add controller/session tests for pins,
owner retention, pressure rank, no-hint trace, stale generation, and fallback.
Add one headless cross-crate test loading Task 4's compiled doorway fixture,
deriving closed-door near-side visibility through the real visibility path,
and asserting that the exact loader-resolved far endpoint becomes SeamWarm
without changing `VisibleCells`.

### Task 6 — Integration and evidence

Reuse Task 4's committed hinted doorway fixture. Allocate a fresh `mktemp -d`
output directory. Bake twice explicitly with `--no-cache`, writing `a.prl`
and `b.prl` there. Use a format-based test/helper that reads `ContainerMeta`
section offsets and sizes to compare exact id-49/id-50 bytes; no extraction
CLI exists. Record the commands and temporary path. Build the engine before
starting a GPU run; start a separate watchdog when the engine process starts,
allow at most 60 seconds of engine uptime, and always stop and reap it. Remove
only the generated PRLs and owned temporary directory after the check.
Run focused cross-crate gates, full preflight, and a bounded manual hinted
doorway check when GPU is available. Record no-hint comparison, hinted bytes,
memory/target diagnostics, and unavailable GPU evidence explicitly under
`measurements/sh-probe-streaming/`. Update durable compiler/renderer contracts
and FGD authoring docs. Review the integrated diff across
compiler→format→loader→planner→renderer and fix findings before landing.
Manual frame-time and visual observations guide subsequent tuning; they do not
gate the feature's existence. Deterministic wire bytes, loader validation, and
controller/renderer correctness regressions remain gates.

## Sequencing

**Phase 1 (sequential):** Task 1 — split-before-extend prerequisite.

**Phase 2 (sequential):** Task 2 — creates canonical authoring input.

**Phase 3 (sequential):** Task 3 — adds versioned format and cut algorithm,
using empty hints until Task 4.

**Phase 4 (sequential):** Task 4 — resolves Task 2 geometry into Task 3 wire
and publishes matching id-50 chunks.

**Phase 5 (sequential):** Task 5 — consumes validated v2 metadata.

**Phase 6 (sequential):** Task 6 — integration evidence and review after code.

## Seam interpretation

The parent epic's load-hide point is preferred early warm-up behind a
portal/door, not a hard visual/door gate. A cold opening still uses the
ambient-floor miss fallback. Delaying the door would change gameplay/co-op
authority and is outside this slice.

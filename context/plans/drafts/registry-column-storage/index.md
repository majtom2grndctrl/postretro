# registry-column-storage

Brief · **stub, awaiting a `/draft-session`** · reads: `context/lib/index.md` §4 · `context/lib/entity_model.md` §1, §2 · `context/lib/crate-graph.md` · read at 2095e74c6

> Stub. It records a problem and its questions only. It has no Decisions, Acceptance or Path, and it is not ready for `/validate-plan`.

## Problem
Raised during the E24 stress pre-flight (`perf-particle-sim-cost`, `perf-ai-tick-cost`). `index.md` §4 says internal storage is "dense per-kind component columns". The registry stores something else:
- `EntityRegistry` holds one column per `ComponentKind`, as `Vec<Option<ComponentValue>>` indexed by slot: `[Vec<Option<ComponentValue>>; ComponentKind::COUNT]`. `COUNT` is 22 today.
- `spawn` pushes a `None` cell into every column for each new slot, so every column is as long as the slot vector, whether or not the kind is present.
- Each cell is a full `Option<ComponentValue>`, sized by the largest unboxed variant. `PlayerMovement` and `Mesh` are boxed to cap that size. The reported size is 800 bytes (*unverified this session*: measured by `size_of` in the perf pre-flight; no build was run here).
- At 800 bytes that is about 22 × 800 B ≈ 17.6 KB per slot: about 200 MB at the stress fixture's ~11.2k slots, and about 1.15 GB at the `u16` slot ceiling (65,535). Parallel per-slot vectors (`previous_transforms`, `projectile_presentation_ages`, `tags`) add to it.
- The slot vector never shrinks. `clear_for_level_unload` despawns but keeps slots to preserve generation semantics, so a session keeps its high-water mark.
- Every column walk (`iter_with_kind`, `for_each_with_kind_mut`, `snapshot_transforms`) strides every slot and reads 800-byte-strided cells.

When done, per-kind storage costs memory in proportion to that kind's members, and a column walk touches only members' data, densely packed.

## Relationship to other work
- **Registry occupancy index** (last-landing follow-up to `perf-particle-sim-cost` and `perf-ai-tick-cost`; not yet drafted). It makes walks O(members) but leaves cell size and per-slot memory unchanged. This stub owns the storage shape. If both proceed, decide whether the index survives a dense layout or is subsumed by it.
- **`perf-particle-sim-cost`** removes particles from the registry, which shrinks this fixture's slot count about 11× but not the per-slot cost.
- `plans/done/perf-billboard-emitter`'s "columnar / batched registry mutation API" non-goal covered that plan's Slice 4 only. It does not foreclose this work.
- `entity_model.md` §9 and `index.md` §4 rule out a general ECS (archetype storage, query planner, scheduler). A dense per-kind layout inside the closed vocabulary stays within that.
- **Compile chokepoint.** `entities` has 11 dependents (`crate-graph.md`). Changing a public registry type recompiles all of them, and the mutation API (`set_component`, `get_component`, `iter_with_kind`) has callers across the workspace.

## Open questions
- Is the fix storage shape (typed per-kind columns, sparse sets, slot-indexed dense arrays with an index) or cell size (boxing more variants)? — owner — **blocks build**
- Must column walks keep slot-index order? Several callers rely on it for determinism. — owner — **blocks build**
- Does `ComponentValue` stay the dynamic transport (FFI, serde, observability) while storage becomes typed, or do both change? — owner — **blocks build**
- Should the slot vector's high-water mark be reclaimable across level loads without breaking stale-`EntityId` rejection? — owner — **blocks build**
- Verify `size_of::<Option<ComponentValue>>()` and record per-kind member counts on the stress fixture and a shipped map, to size the win. — **delegated**
- Is the occupancy index still worth a brief after this lands, or does this subsume it? — owner — **blocks build**

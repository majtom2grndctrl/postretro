# shadow-span-draws

Seed · not yet drafted. Run `/draft-session` before choosing a route. Split out of `visible-span-draws` on 2026-10-03.

## Observation
A `/review-brief` premise check of `visible-span-draws` found that shadow passes still draw whole buckets. A shadow fill pays one Metal driver draw per BVH leaf, about 49 ns each on the compatibility-floor Mac, whether the leaf is in the light's cone or not. The world-depth cache keeps static lights that fit in it (3 spot and 4 cube units, keyed on the exact projection matrix) from refilling. A moving shadow-ranked light, or one past capacity, still refills every frame. One cube light on stress-warren-hallway-inspection is about 6·L slots (L = 8,437), roughly 2.5 ms per frame. That is about 3× the camera cost `visible-span-draws` removes.

## Starting facts (unverified here; re-ground in the session)
- The shadow cone cull binds an all-ones cell bitmask, so it has no cell set to restrict draws by (`shadow_cull.rs`, cell-bitmask setup).
- Cache capacity and key: `dynamic_depth_cache.rs`.
- Shadow draws go through `ShadowCullPipeline::draw_slot_indirect` → `draw_indirect_buckets`. After `visible-span-draws`, that function takes a range list and executes a pure draw plan. Shadows pass their whole buckets as the list today, so restricting them is a change of input, not a new draw path.
- `indirect_contract_tests` pins six `draw_slot_indirect` call sites and the owner rules.

## Candidate shape (hypothesis)
Test cell bounds against each slot's cone or face frustum on the CPU to get a per-slot cell set, then draw that set's spans through the same range builder the camera uses.

## Questions for the session
- How often do moving or over-capacity shadow lights occur in real content? That sets the priority.
- How does this relate to `shadow-cone-cull-parallel-dispatch`, which targets the GPU cost of the same cull?
- Is a per-slot cell set a new pass or new persistent GPU state? If so, it takes the plan route, not a brief.

Depends on: `visible-span-draws` (the range-list draw function and the pure draw plan).

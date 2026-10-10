# static-specular-mask-ownership

Brief · resumable · reads: `context/lib/rendering_pipeline.md` §4 (World specular
shadowmask, Promoted static lights, Lightmap cell-block residency),
`context/lib/build_pipeline.md` §Compiler pipeline · §Build Cache · §PRL section IDs ·
read at 4e127a7b8

## Problem

The owner saw partial, hard-edged specular patches on floors and walls throughout
`stress-warren-hallway-inspection`. This is an observed defect.

Cause: world specular evaluates every static light in the fragment's 8 m chunk list,
and treats a light with no mask slot as fully visible. A slotless light's highlight
passes through walls and stops only at a chunk plane.
- The diagnosed instance is a light sealed inside a wall. The `buried-lights` build
  (merged) now excludes such lights.
- The mechanism follows from source for lights that are not buried, but has not been
  seen on one. A capture gates the build (Open questions).

Slots are scarce:
- Each light holds one channel map-wide, and competes for it everywhere it could reach
  without shadows.
- Static lights outside the entity-shadow selection never get a slot.
- SDF lights outside the fragment's top four render unshadowed.

When done, a static light contributes world specular only where its baked contribution
is non-zero. Every highlight it contributes is shadowed, and the compiler reports where
a light lost a highlight to capacity.

## Decisions

- **Ownership replaces global slots.** Each lightmapped face carries up to four lights
  owning its four mask channels.
  - World specular and the promoted-light union subtraction evaluate only those owners,
    each finding its channel by the light's position in the owner list.
  - A face's owners sit in ascending light-index order, so channel placement never
    depends on the order the bake meets them.
  - The map-wide per-light slot and its dropped sentinel retire.
- **One owner list per lightmapped face** (an id-17 face record, after compiler face
  cuts).
  - Per block is too coarse (`research.md` §Granularity).
  - Delivery adds no fragment binding, and leaves the PRL vertex format and its 36-byte
    stride unchanged.
- **Unowned means no world specular.** This supersedes the `rendering_pipeline.md` §4
  rule "absent, rejected, or dropped shadowmask data is fully lit" for specular. A
  missing highlight is a smaller error than light through a wall.
- **Eligibility: every static lightmap light,** decoupled from entity-shadow selection.
  - Eligible: non-dynamic, non-animated, not bake-only, `StaticLightMap`. Point, spot
    and directional lights all qualify.
  - Id 40 stays the promotion set only.
  - Including directional lights reverses the sun exclusion in
    `plans/done/specular-shadowmask-occlusion`. That exclusion existed because a sun
    cost a map-wide slot.
- **A directional highlight follows the light's direction,** as its bake does, rather
  than aiming at the entity origin.
- **Admission by shadowed contribution.** A light is a candidate on a face only where
  its quantized baked visibility is non-zero on a chart-interior texel. Padding never
  admits.
  - This supersedes the "coverage is unshadowed" contract in
    `plans/done/lighting-scale--shadowmask-cold-working-set`, which protected slot
    sharing that ownership removes.
  - Ownership is decided during the fused lightmap walk, except on cut faces, whose
    owners are fixed just before it. That reverses the `build_pipeline.md` ordering
    "channel assignment finishes before the walk".
- **Ranking when more than four candidates reach a face:**
  1. Candidates above a lit-texel floor rank ahead of those below it. The floor is
     measured from the candidate's own baked texels on that face.
  2. Within the above-floor set, id-40 lights rank ahead of specular-only lights. A lost
     entity shadow costs more than a lost highlight.
  3. Within each tier, the larger shadowed contribution summed over the face ranks
     higher.
  4. A tie goes to the lower light index.

  The outcome is independent of arrival order, worker count, window size and cache
  state.
- **A cut face has one owner set.** All sub-faces of one compiler-cut face share it, so
  a highlight never cuts along a cut line.
  - Its ranking comes from a sparse shadowed estimate traced before the walk, because
    sub-faces span bake layers. Sample spacing follows the floor, so an above-floor
    light rarely falls between samples.
  - The estimate's owners are final. A light it missed takes no channel, even a free
    one, and the drop report names it.
  - An estimated owner that bakes no lit texel on any sub-face is cleared after the
    walk.
  - For ranking, a cut face's floor comes from the estimate scaled to area. For the
    report, it comes from the parent's exact lit-texel count after the walk.
  - Faces that were not cut rank on exact per-face sums.
- **An owned channel reads occluded wherever its owner wrote nothing,** including
  padding and texels another owner held before.
- **Walk resource bounds stay.**
  - No per-(light, texel) visibility is retained, and no texel's visibility is traced
    twice, cold or warm.
  - The cut-face estimate traces only its sparse sample points.
- **World specular set = face owners ∪ the SDF top-four selection.**
  - World specular stops reading the chunk light list. The chunk list stays baked for
    billboards and SDF selection.
  - An SDF light outside the fragment's selection adds no specular.
- **Id 42 exists whenever any face admits an owner,** whatever id 40 holds. Clearing id
  40 or 41 at load never drops it.
- **The drop report is complete on every build.**
  - A warning names each above-floor light dropped and its face. A cut face is named
    once, as its parent with its sub-face range.
  - `--verbose` reports peak per-face demand.
  - Both are identical on cold and warm builds.
- **Capacity stays at four per face.**
  - Further face cuts and more mask groups are deferred; the drop report is their
    trigger.
  - Seams where an over-capacity face meets a neighbour that kept a light it dropped
    are accepted residue. Diffuse stays continuous there. This diverges from
    `plans/done/specular-continuity` at those faces only.
- **Non-goals:**
  - **Billboard specular shadowing.** Its loops have no mask binding by documented
    design (`rendering_pipeline.md` §4).
  - **Kinematic mover specular.** It comes only from promoted records shadowed by the
    pool shadow map, so it cannot leak through a wall.
  - **Excluding lights inside solid.** `buried-lights` owns that exclusion. This brief
    does not depend on it, because shadowed admission gives such a light no channel.
  - **Entity-shadow selection heuristics and chunk-list contents.** Both are unchanged.
  - **Animated-baked owners and animation-following highlights.** A follow-up brief owns
    both. Their visibility comes from the weight-map stage, and holding it through the
    walk is a memory risk. Until then they lose world specular. The owner table can
    already name them (`research.md` §Animated-baked lights).

## Acceptance

### Automated
No leak, both sides of ownership:
- [ ] A static light walled off from a receiver inside its range adds zero specular to
      that receiver. A second light that does reach the receiver keeps its highlight.
- [ ] A light that owns face R but not adjacent distinct face S lights R and adds
      nothing to S. Both fragments at the shared edge are asserted.
- [ ] The sub-faces of one compiler-cut face carry identical owner sets, including when
      they sit in different bake layers and when one sub-face alone would have ranked a
      different fourth light. Each owner's mask holds its own baked values in every
      sub-face. (P1)
- [ ] On a cut face, a light walled off from the whole face takes no channel even when
      it is in range. A light lighting part of it does.
- [ ] A cut face's owner that lights only one of its sub-faces reads zero across the
      others, including their padding. No sub-face keeps the unwritten fill value in an
      owned channel. (P16)
- [ ] On a cut face meeting a wall, a light behind that wall that reaches only the
      face's edge, and no interior texel, takes no channel. (P22)
- [ ] On a cut face, a light that lights more texels than the floor but misses every
      estimate sample takes no channel in any sub-face, and the drop warning names it.
      (P14)
- [ ] On a cut face, a light the estimate chose that bakes no lit texel on any sub-face
      holds no channel in the final owner table. (P15)
- [ ] On a cut face, a promoted light lighting fewer texels than the floor across the
      whole face never takes a channel from an above-floor specular-only light, even when
      one sub-face alone would put it above the floor. (P17)
- [ ] Within a face it owns, a light's specular still follows the baked mask: zero on
      an occluded texel, full on a lit one.
- [ ] An SDF light outside the fragment's selection adds zero specular. One inside it
      adds specular at its SDF visibility.
- [ ] A directional light's highlight lies along its baked direction. Moving its entity
      origin without changing its direction leaves the highlight unchanged.
- [ ] Missing data adds no static non-SDF specular, and SDF specular is unchanged. This
      holds for:
      - a block miss;
      - a missing id 42;
      - placeholder mode;
      - a mask pool the renderer refuses while lightmap blocks stay resident, both
        all-resident and streamed.

      The refusal logs a `[Renderer]` error and never panics. (P9)
- [ ] An owner occluded at a face edge reads zero there, including taps into chart
      padding. No owned channel keeps the unwritten fill value on a texel its owner does
      not reach. (P4)
- [ ] Each owner position reads its own channel in either mask group. Swapping two
      owners' table positions together with their mask planes leaves the image
      unchanged.
- [ ] Streamed and all-resident captures stay identical at the existing streaming poses
      with owner specular on. (P12)
- [ ] A static light outside id 40 owns channels and gets shadowed specular.

Union subtraction:
- [ ] On a fixture where every promoted light owns its face, subtraction matches the
      pre-change output.
- [ ] A promoted light that does not own a face subtracts nothing there.

Compiler:
- [ ] Five eligible lights lit across one face yield four owners, chosen by largest
      shadowed contribution, plus a warning that names the fifth. Four lights yield four
      owners and no warning.
- [ ] The drop warning names each face that lost a light above the floor, as well as
      the light. A cut face appears once, as its parent with its sub-face range.
      `--verbose` reports the most candidates seen on one face.
- [ ] A face's owners appear in ascending light-index order whatever order the bake met
      them, and every owner's mask values sit in that owner's channel. (P24)
- [ ] A warm build that reuses the cached id 42 repeats the cold build's drop warnings
      and its peak per-face demand under `--verbose`. (P2)
- [ ] When a later, higher-ranked light evicts a face's owner, none of the evicted
      light's values remain in that channel. Texels the newcomer does not reach read
      zero. (P4)
- [ ] With five above-floor candidates on a face, a promoted light with the smallest
      contribution keeps its channel over a brighter specular-only light. With no
      promoted light among them, the smallest contribution drops.
- [ ] A promoted candidate below the lit-area floor drops silently and never takes a
      channel from an above-floor specular-only light. (P5)
- [ ] A below-floor promoted light that arrives first and takes a free channel loses it
      once four above-floor specular-only lights arrive. (P19)
- [ ] With three above-floor candidates and two below-floor ones on a face, the last
      channel goes to the brighter below-floor light, even when the dimmer one is
      promoted.
- [ ] A directional light takes a channel on a face it lights, and none on a face it
      cannot reach, such as one under a roof.
- [ ] A fifth candidate below the lit-area floor is dropped without a warning. The same
      light above the floor is warned.
- [ ] Two candidates with equal contribution for the last channel resolve to the lower
      light index, whichever arrives first. The result is identical across worker-thread
      counts. (P7)
- [ ] Id 42 is byte-identical when the fill receives the same partitions in reversed
      light order, and across partition-window sizes. (P6)
- [ ] With a cut face whose sub-faces span two bake layers, id 42 and the drop report
      are identical across worker-thread counts and partition-window sizes, and between
      cold, warm and lightmap-reuse builds. Two lights tied on the estimate resolve to
      the lower light index. (P18)
- [ ] With dynamic, SDF and bake-only lights placed before a static light in the light
      list, each owner entry names the light whose visibility filled its channel. A
      bake-only light takes no channel. (P8)
- [ ] A texel with non-finite visibility neither admits a light nor adds to its
      contribution. (P13)
- [ ] A light whose only lit texels on a face are padding takes no channel there.
- [ ] A light occluded on every texel of a face takes no channel there. The same
      light with one lit texel there takes one.
- [ ] Id 42 is byte-identical across worker-thread counts, and between a cold build, a
      warm build, and a warm build that reuses the lightmap but rebuilds id 42.
- [ ] Editing a light outside id 40, or changing only id 40 membership, rebuilds id 42
      on a warm build. The result is byte-identical to a cold build, including when the
      lightmap comes from cache. (P3)
- [ ] A build that reuses the lightmap but rebuilds id 42 reads each eligible light's
      partition at most once per bake layer, holds one raw fill, and traces no
      visibility beyond the cut-face estimate.
- [ ] A warm build that reuses the cached id 42 traces no visibility, including the
      cut-face estimate. (P20)
- [ ] A level with no cut face traces no estimate sample. On a cut face, the estimate
      traces only for lights whose unshadowed term reaches it.
- [ ] Changing the cut-face estimate's density or sampling mode rebuilds id 42 on a warm
      build. (P23)
- [ ] Load rejects:
      - an owner index at or past the static spec-light count;
      - a duplicate owner within a face;
      - owners out of ascending order, or an owner after an empty channel;
      - a face count that disagrees with id 17;
      - a payload length that disagrees with the block arithmetic.

      A section with the retired tag asks for a re-bake.
- [ ] A level whose eligible lights include none in id 40 still emits id 42 and
      shadows their specular.
- [ ] A level whose non-empty id 40 is cleared at load, because its deltas are missing,
      keeps id 42. Formerly promoted owners keep shadowed specular and subtract nothing.
      (P10)
- [ ] A level whose eligible lights light no face interior emits no id 42.
- [ ] A level with more static lights than the owner index can hold fails the compile
      with a named error before any lightmap bake work starts, including the cut-face
      estimate. (P11, P21)
- [ ] The existing forward binding inventory test passes with its snapshot unedited.
- [ ] The world vertex stride stays 36 bytes and id 17's format version is unchanged.

### Manual
- [ ] `stress-warren-hallway-inspection` at pose (-17, 21, 86): no sheen. Toggling
      `light_term_mask` bit 0x40 shows no specular patch.
- [ ] The pose from the pre-build leak capture (Open questions) shows no leak after the
      change.
- [ ] An animated light's world highlight is gone, and nothing else at that pose
      changes. Its diffuse animation is unchanged.
- [ ] `campaign-test`: highlights in lit rooms read continuous across 8 m chunk planes.
      Visual read of what was lost against pre-change.
- [ ] `kinematic-platform` large floor and `movement-feel`: drop report read, and the
      visible highlight loss assessed.
- [ ] Measured and reported on the 1660 at a dense pose, before and after: forward-pass
      GPU ms, from a capture built with `capture,dev-tools`.
- [ ] Measured and reported on `stress-warren-hallway-inspection`, cold `--release`,
      before and after, per `testing_guide.md` §Resource bounds:
      - shadowmask and lightmap stage time, including the cut-face estimate;
      - peak compile RSS.

## Path

- **Assignment in the walk.** Partitions arrive layer-major in light order through
  `bake_fused_windowed` → `FusedShadowmaskPlan::consume_partition` → `ShadowmaskFill`,
  whose raw fill is resident for the walk.
  - Shape: sum a partition per face, then admit or evict per face, then write. An
    evicting newcomer zeroes the channel across the face before writing.
- **Cut-face estimate.**
  - `plan_face_cuts`/`apply_face_cuts` know each sub-face's parent before
    `rebuild_face_identity`. Carry that parent forward.
  - Trace a coarse grid over the parent's chart interior against the rebuilt BVH the
    walk uses, for each light whose analytic term reaches it. The pre-cut BVH is not
    kept. Rank once, and fix the owners before the walk writes any
    sub-face.
- **Sun lobe.** `pack_spec_lights` packs directional lights with a zero direction, and
  the world specular loop derives `L` from position. The bake's direction comes from
  `light_contribution_and_direction`.
- **Owner delivery.**
  - A forward-only second vertex buffer, filled at load from per-face data through face
    index ranges, feeds a flat varying.
  - Precedent: `stamp_animated_block_ids`.
  - The forward pipeline has attribute and varying room (`research.md` §Runtime
    budget).
  - A vertex-stage storage lookup is not an option: `vs_main` has no face key.
- **Id 42 presence keyed on id 40 today:** `prepare_fused_shadowmask` (`NoSelection`),
  the lightmap residency dry run (`ShadowmaskState`), and the load-time reconcile that
  drops id 42 on a mismatched selection.
- **Retiring seams:**
  - compiler: `build_analytic_overlap_graph_in_order` and
    `assign_channels_with_drops_controlled`;
  - runtime: `build_spec_light_shadowmask_channels`, the promoted metadata channel
    field, and the `cone_cos.z` slot.
- **Runtime seams:** in `forward.wgsl`, `shadowmask_visibility_for_spec_light`,
  `shadowmask_union_subtraction` and the world specular loop. The loop is inline in
  `fs_main`; extract it first. Its SDF half iterates the selection directly.
- **First slice:** a forward-harness fixture that renders owner-only specular from a
  hand-built per-face owner buffer, with a GPU required.
- **Split first:** `pipeline/lightmap_stage.rs` and `shadowmask_bake.rs`.
- **Rivals considered:** `research.md` §Rejected rivals.
- **Durable capture at promotion:**
  - `rendering_pipeline.md` §4;
  - `build_pipeline.md` stage ordering and id 42.

## Open questions

- Before build, a pre-change capture must show a leak from a light the `buried-lights`
  build does not explain: a dropped non-buried light, or an unselected `campaign-test`
  light. If none is found, the Problem reduces to coverage and the owner revisits scope.
  — owner: executor, reported to the project owner — **blocks build**
- Lit-texel floor value. The census cleared every stress-map overflow at 16 texels. —
  **delegated**: report the value and its drop report in the plan of record.
- Cut-face estimate sampling: hard ray or area samples, at a spacing that follows the
  floor. — **delegated**: report rays traced and stage time.

## Wire format

Id 42 changes tag from `SMB6` to `SMB7`. Everything not listed here mirrors `SMB6`:
little-endian, block records and the two BC5 group planes per id-22 block, offsets and
lengths as today.

| Element | Encoding |
|---|---|
| Per-selected-light slot table and its count | Removed |
| Face count | u32, placed where the slot-table count was. Must equal id 17's face count |
| Owner table | One entry per id-17 face, in face order. Each entry is 4 × u16 compact static spec-light indices: the `!is_dynamic` order that id 23 already uses. Entry position *s* is channel *s*: BC5 group *s*/2, channel *s*%2. Loaded whole at install; never streamed |
| Empty channel | `0xFFFF`. Owners are distinct and ascending within an entry; empty channels follow the last owner |
| Empty level | Face count 0, no table |
| Retired `SMB6` | Named so load asks for a re-bake |

## Boundary inventory

| Name | Rust | PRL | WGSL |
|---|---|---|---|
| Empty owner channel | `u16` const `0xFFFF` in `level-format` | `0xFFFF` | `0xFFFFu` after unpack |
| Owner index space | compact `!is_dynamic` spec index (`pack_spec_lights` order) | u16 | index into `spec_lights` |

# static-specular-mask-ownership

Brief · resumable · reads: `context/lib/rendering_pipeline.md` §4 (World specular
shadowmask, Promoted static lights, Lightmap cell-block residency),
`context/lib/build_pipeline.md` §Compiler pipeline · §Build Cache · §PRL section IDs ·
read at 4e127a7b8

## Problem

The owner saw partial, hard-edged specular patches on floors and walls throughout
`stress-warren-hallway-inspection`. This is an observed defect. Cause: world specular
evaluates every static light in the fragment's 8 m chunk list and treats a light with
no mask slot as fully visible. A slotless light's highlight therefore passes through
walls and stops only at a chunk plane. Slots are scarce:
- Each light holds one channel map-wide, and competes for it everywhere it could reach
  without shadows, occluded or not.
- Static lights outside the entity-shadow selection never get a slot.
- SDF lights outside the fragment's top four also render unshadowed.

Separately, an animated light's world highlight ignores its animation: it paints a
constant full-strength highlight however the light strobes, cycles or is switched off.

When done, a static light contributes world specular only where its baked contribution
is non-zero. Every highlight it contributes is shadowed and follows the light's current
brightness and color, and the compiler reports where a light lost a highlight to
capacity.

## Decisions

- **Ownership replaces global slots.** Each lightmapped face carries up to four lights
  that own its four mask channels. World specular evaluates only those lights there,
  and so does the promoted-light union subtraction. Each finds a light's channel by its
  position in the face's owner list. The map-wide per-light slot and its dropped
  sentinel retire.
- **Granularity: one owner list per lightmapped face,** meaning an id-17 face record
  after compiler face cuts.
  - Per block is too coarse. One 2048×1904 block in `kinematic-platform` meets 15
    lights that never share a texel; per face, at most 9 meet there (`research.md`
    §Granularity).
  - World vertices are unique per face, so the owner list rides a forward-only second
    vertex buffer, filled at load from per-face data, to a flat varying. The PRL vertex
    format, the 36-byte stride and every other pipeline stay untouched.
- **Unowned means no world specular.** This supersedes the `rendering_pipeline.md` §4
  rule "absent, rejected, or dropped shadowmask data is fully lit" for specular.
  - No static non-SDF world specular comes from a missing or rejected id 42, a block
    miss, placeholder mode, or a light outside the owner list.
  - A missing highlight is a smaller error than light through a wall.
  - Union subtraction already skips unslotted lights, so it keeps that behavior.
- **Eligibility: every static lightmap light,** decoupled from entity-shadow selection.
  - Eligible: non-dynamic, not bake-only, `StaticLightMap` shadow type. Point, spot and
    directional lights all qualify, animated or not.
  - Id 40 stays the promotion set only.
  - A promoted light still subtracts only where it owns a channel.
  - Directional lights are included. This reverses the sun exclusion in
    `plans/done/specular-shadowmask-occlusion`, which existed because a sun cost a
    map-wide slot. Under shadowed per-face admission a sun takes a channel only on faces
    it lights.
  - Animated-baked lights are included. Bake-only lights stay out, because they have
    no spec-light record for an owner index to name.
- **Admission by shadowed contribution.** A light is a candidate in a face only where
  its quantized baked visibility is non-zero on a texel it covers.
  - This supersedes the "coverage is unshadowed" contract in
    `plans/done/lighting-scale--shadowmask-cold-working-set`. That contract protected
    map-wide slot sharing, where an occluded light would read a co-slotted light's
    value. Ownership removes the hazard, because a non-owner never reads a channel.
  - Ownership is decided during the fused lightmap walk. That reverses the
    `build_pipeline.md` ordering "channel assignment finishes before the walk".
- **Animated owners use visibility already baked; no new trace.**
  - The animated weight-map stage (ids 24/25) already traces the same soft visibility
    on the same charts and texel grid as the walk.
  - That stage moves ahead of the fused walk; its inputs are final after atlas
    preparation. Animated candidates join each face's competition from its output.
  - The shadowmask memo key folds the weight-map inputs.
- **An owned channel starts at zero across its face.** Eviction and a new owner reset
  the channel to occluded before writing. The weight maps omit fully occluded texels,
  and a 255 residue would recreate the through-wall leak.
- **Same walk resource bounds.** The stage retains no per-(light, texel) visibility and
  runs no second visibility trace, cold or warm. The deleted record behind the 16 GiB
  OOM stays deleted. Undo cost is high once this ships, because retained visibility is
  what the cold-working-set brief removed.
- **World specular set = face owners ∪ the SDF top-four selection.**
  - World forward specular stops reading the chunk light list.
  - An SDF light outside the fragment's selection contributes no specular.
  - The chunk list stays baked for billboards and for SDF selection in the forward and
    SDF-shadow passes.
- **Over-capacity faces keep promoted lights first, then rank by shadowed
  contribution, deterministically and loudly.**
  - When more than four eligible lights reach a face, id-40 lights take channels ahead
    of specular-only lights. A lost entity shadow costs more than a lost highlight.
  - Within each group, the four kept are those with the largest shadowed contribution
    summed over the face's interior texels.
  - An animated light ranks by its animation's peak brightness, matching the lookahead
    maximum promotion already uses. It is never in id 40, so it ranks in the
    specular-only tier.
  - On the census, contribution ranking loses roughly a third of what intensity ranking
    loses, and no light loses every face.
  - Ties break by light index. The result is independent of worker count, cache state
    and window order.
  - A candidate below a small lit-area floor drops first and silently. Such specks are
    every overflow on the stress maps.
  - A warning without `--verbose` names each light dropped above the floor.
  - `--verbose` reports peak per-face demand.
- **Animated highlights follow their animation.** An animated owner's specular uses
  the light's current curve brightness and color, the same values its animated
  lightmap composes. This is evaluated for owners only, at most four per fragment.
  No binding is added: the curve index rides the `SpecLight` field the retired slot
  frees.
- **Id 42 exists whenever any face admits an owner,** whether or not id 40 selects a
  light. Clearing id 40 or 41 at load never drops id 42.
- **Admission reads interior texels only.** Chart padding and ring texels hold copied
  values and positions off the polygon, which inflate overlap.
- **Placement.** Ownership is bake-time policy in the compiler; the runtime is
  mechanism. The owner list reaches the fragment as a flat value through the vertex
  stage.
  - No fragment binding is added. Fragment storage is at 8 of 8 and sampled textures
    at 16 of 16.
  - Owner data rides a vertex buffer, not storage. A test pins the forward pipeline's
    per-stage storage and sampled-texture counts at today's values.
- **Capacity stays at four per face.** Both capacity levers are deferred:
  - cutting over-capacity faces, which reuses the existing product-grid face cuts;
  - more mask groups.

  The drop report is their trigger. After the floor, residual overflow is one face in
  `campaign-test` and two in `kinematic-platform`. `movement-feel`'s overflow is
  texel-deep, so cuts cannot cure it (`research.md` §Granularity, §Capacity lever).
- **Non-goals:**
  - **Billboard specular shadowing.** Its loops have no mask binding by documented
    design (`rendering_pipeline.md` §4, billboard lighting models). Unchanged.
  - **Kinematic mover specular.** It reads promoted records only.
  - **Lights inside solid.** The in-flight `buried-lights` direct build excludes them.
    This brief must not depend on it, because shadowed admission already gives them no
    channel.
  - **Entity-shadow selection heuristics and chunk-list contents.** Both are unchanged.

## Acceptance

### Automated
No leak, both sides of ownership:
- [ ] A static light walled off from a receiver inside its range adds zero specular to
      that receiver. A second light that does reach the receiver keeps its highlight.
- [ ] A light that owns face R but not adjacent face S lights R and adds nothing to S.
      Both fragments at the shared edge are asserted, including across a compiler face
      cut.
- [ ] Within a face it owns, a light's specular still follows the baked mask: zero on
      an occluded texel, full on a lit one.
- [ ] An SDF light outside the fragment's selection adds zero specular. One inside it
      adds specular at its SDF visibility.
- [ ] Missing data adds no static non-SDF specular and SDF specular is unchanged. This
      holds for a block miss, a missing id 42, a rejected id 42, and placeholder mode.
      Rejection logs a `[Renderer]` error and never panics.
- [ ] A static light outside id 40 owns channels and gets shadowed specular.

Union subtraction:
- [ ] On a fixture where every promoted light owns its face, subtraction matches the
      pre-change output.
- [ ] A promoted light that does not own a face subtracts nothing there.
- [ ] An animated owner never enters the union subtraction.

Animated owners:
- [ ] An animated owner's world highlight is shadowed: zero on a texel its weight map
      leaves occluded, including a texel that has no weight-map entry.
- [ ] Its highlight follows the curve. It is zero while a strobe is off, tracks a
      color cycle, and matches the animated lightmap's brightness at the same time.
- [ ] A light the animated lightmap treats as off adds no highlight. A static owner on
      the same face is unaffected.
- [ ] After an evicted owner's channel is reused, no texel of that face reads the old
      owner's value or 255.
- [ ] Editing only an animated light on a warm build re-bakes id 42.

Compiler:
- [ ] Five eligible lights lit across one face yield four owners, chosen by largest
      shadowed contribution, plus a warning that names the fifth. Four lights yield four
      owners and no warning.
- [ ] With five candidates on a face, a promoted light with the smallest contribution
      keeps its channel over a brighter specular-only light. With no promoted light
      among them, the smallest contribution drops.
- [ ] A directional light takes a channel on a face it lights, and none on a face it
      cannot reach, such as one under a roof.
- [ ] A fifth candidate below the lit-area floor is dropped without a warning. The same
      light above the floor is warned.
- [ ] Two candidates with equal contribution resolve by light index. The result is
      identical across worker-thread counts.
- [ ] A light whose only lit texels on a face are padding or off-polygon texels takes
      no channel there.
- [ ] A light occluded on every texel of a face takes no channel there. The same
      light with one lit texel there takes one.
- [ ] Id 42 is byte-identical across worker-thread counts, and between cold and warm
      builds.
- [ ] Load rejects:
  - an owner index at or past the static spec-light count;
  - a duplicate owner within a face;
  - a face count that disagrees with id 17;
  - a payload length that disagrees with the block arithmetic.

  A section with the retired tag asks for a re-bake.
- [ ] A level whose eligible lights include none in id 40 still emits id 42 and
      shadows their specular. Loading it with id 40 cleared keeps id 42.
- [ ] A level with more static lights than the owner index can hold fails the compile
      with a named error.
- [ ] Forward pipeline per-stage storage and sampled-texture counts are pinned by test.
      Fragment counts are unchanged.

### Manual
- [ ] `stress-warren-hallway-inspection` at pose (-17, 21, 86): no sheen. Toggling
      `light_term_mask` bit 0x40 shows no specular patch.
- [ ] A pre-change capture shows a leak from a light the `buried-lights` build does
      not explain: a dropped non-buried light, or an unselected `campaign-test` light.
      The same pose after the change shows no leak.
- [ ] `campaign-test`: highlights in lit rooms read continuous across 8 m chunk planes.
      Visual read of what was lost against pre-change.
- [ ] `kinematic-platform` large floor and `movement-feel`: drop report read, and the
      visible highlight loss assessed.
- [ ] Measured and reported on the 1660 at a dense pose, before and after: forward-pass
      GPU ms, from a capture built with `capture,dev-tools`.
- [ ] Measured and reported on `stress-warren-hallway-inspection`, cold `--release`,
      before and after, per `testing_guide.md` §Resource bounds:
      - shadowmask and lightmap stage time;
      - peak compile RSS.

## Path

- **Assignment in the walk.** Partitions arrive in global light order through
  `bake_fused_windowed` → `FusedShadowmaskPlan::consume_partition` →
  `ShadowmaskFill`, whose raw fill buffer is resident for the walk.
  - Chosen shape: ranked admission with eviction. A higher-ranked newcomer resets the
    evicted owner's channel within that face, then writes its own values. One pass,
    nothing retained.
  - Rival: colour after the walk from a warm-cache re-read. It fails cold builds and
    the no-second-trace decision.
- **Rejected rival: specular from the directional lightmap** (dominant direction ×
  irradiance).
  - It cannot leak and has no capacity limit.
  - It holds one direction per texel, from a low-resolution, nearest-sampled direction
    atlas. Overlapping lights blur into one highlight.
  - The leftover overflow after ownership, mostly `movement-feel`, does not justify
    that quality loss everywhere.
- **Animated candidates.**
  - Move the AnimatedLightChunks and AnimatedWeightMaps stages ahead of
    `bake_fused_prepared` in `run_after_parsing` (`pipeline.rs`). Keep id 25 in its
    identity layout during the walk; culling and compact repack stay after it.
  - Recover visibility as weight ÷ `contribution_to_weight(light_contribution_and_direction(...))`,
    from the entries `bake_one_chunk` emits.
  - Charge the resident id 25 in the stage's predicted peak.
- **Id 42 presence keyed on id 40 today:** `prepare_fused_shadowmask` (`NoSelection`),
  the lightmap residency dry run (`ShadowmaskState`), and the load-time drop on a
  mismatched channel table.
- **Animated specular:**
  - `pack_spec_lights` packs constant `color × intensity` once at install.
  - The curve helpers already exist in `curve_eval.wgsl`, and the group-3
    `anim_descriptors`/`anim_samples` bindings are fragment-visible.
  - The loader supplies the map from spec index to animated descriptor index.
- **Retiring seams:** `build_analytic_overlap_graph_in_order`,
  `assign_channels_with_drops_controlled` and the analytic-graph half of
  `prepare_fused_shadowmask`. On the runtime side: `build_spec_light_shadowmask_channels`
  and the promoted metadata channel field (`render/shadowmask.rs`), and the `cone_cos.z`
  slot in `pack_spec_lights`.
- **Runtime seams:**
  - In `forward.wgsl`: `shadowmask_visibility_for_spec_light`,
    `shadowmask_union_subtraction` and the world specular loop. The SDF half of the
    loop iterates the selection's indices directly.
  - Owner vertex buffer: the forward "Textured Pipeline" in
    `renderer_init_pipelines.rs` uses 6 of 16 vertex attributes on 1 of 8 buffers, with
    9 of 16 varyings.
  - The per-face fill walks face index ranges at load. Precedent for a per-face
    per-vertex value: `stamp_animated_block_ids`.
  - A vertex-stage storage lookup is not an option: `vs_main` has no face key, and
    index builtins are banned (`indirect_contract_compiled_pipeline_sources_ban_index_builtins`).
- **First slice:** a forward-harness fixture that renders owner-only specular from a
  hand-built per-face owner buffer. It falsifies the vertex-buffer and flat-varying
  plumbing before the bake moves.
- **Split first:** `pipeline/lightmap_stage.rs` and `shadowmask_bake.rs` are past
  ~800 lines; `forward.wgsl` too, if its include composition allows.
- **Durable capture at promotion:**
  - `rendering_pipeline.md` §4: World specular shadowmask, Promoted static lights.
  - `build_pipeline.md`: stage ordering and id 42.

## Open questions

- Lit-area floor value: a minimum lit-texel count, a share of the face's
  contribution, or both. The census cleared every stress-map overflow at 16 texels or
  1%. — **delegated**: report the value and the drop report it yields in the plan of
  record.

## Wire format

Id 42 changes tag from `SMB6` to `SMB7`. Everything not listed here mirrors `SMB6`:
little-endian, block records and the two BC5 group planes per id-22 block, offsets and
lengths as today.

| Element | Encoding |
|---|---|
| Per-selected-light slot table and its count | Removed |
| Face count | u32, placed where the slot-table count was. Must equal id 17's face count |
| Owner table | One entry per id-17 face, in face order. Each entry is 4 × u16 compact static spec-light indices: the `!is_dynamic` order that id 23 already uses. Entry position *s* is channel *s*: BC5 group *s*/2, channel *s*%2. Loaded whole at install; never streamed |
| Empty channel | `0xFFFF`. Owners are distinct within an entry; empty channels may sit at any position |
| Empty level | Face count 0, no table |
| Retired `SMB6` | Named so load asks for a re-bake |

## Boundary inventory

| Name | Rust | PRL | WGSL |
|---|---|---|---|
| Empty owner channel | `u16` const `0xFFFF` in `level-format` | `0xFFFF` | `0xFFFFu` after unpack |
| Owner index space | compact `!is_dynamic` spec index (`pack_spec_lights` order) | u16 | index into `spec_lights` |

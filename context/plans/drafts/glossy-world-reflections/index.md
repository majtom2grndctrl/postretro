# glossy-world-reflections

Brief · resumable · reads: `context/lib/rendering_pipeline.md` §6, §7.3, §7.3a (Reflection, Reflection quality tiers), §10, §13 · `context/lib/resource_management.md` §4.7, §5 · `context/lib/build_pipeline.md` §Custom FGD, §Compiler pipeline step 14, §PRL section IDs · `context/lib/testing_guide.md` §Resource bounds · read at e7438238 · builds after `specular-texel-sharpness`, `reflection-probe-fluid-pass-performance`, and counsel on Tencent WO2026113651A1 claim 1

## Problem
Owner-requested capability, for a genre-staple look: polished floors and walls that reflect their room, as in Control. Today only up-facing fluid faces bind a reflection probe: placement, the binding format and the shader all assume water. The first consumer is wet marble and bronze accents in the owner's game, but the feature serves any glossy static surface, not marble alone. When done, every static world face whose material paints sharpness above a floor reflects its room from compile-time probes. Each texel's reflection is blurred by the exponent that shapes its highlight and coloured as its highlight is. It follows the author's reflection style and the player's reflection quality. Water reflects through the same general planar-reflector path and looks as it does today.

## Decisions
- **§13 lifted for opaque world faces (owner).** Image-based specular on static world faces, blurred by the per-texel Blinn-Phong exponent. Still no energy-conserving BRDF and no roughness or metallic input (`rendering_pipeline.md` §13, `index.md` §2 Physical bake, stylized runtime). Amends §13's "outside fluid surfaces" exception and `resource_management.md` §3's "Environment-mapped reflections" row at promotion.
- **Faces in scope (owner).** Floors and walls; any planar static world face orientation the generalized rules support.
- **Probe-only on world faces (owner).** No SSR or planar layer there. Both stay water's (§7.3a); SSR on walls would be a second screen-space exception (`index.md` §1.1).
- **One planar-reflector path; water moves onto it (owner).** Water's look is pinned by parity, not re-tuned.
- **Generalized probes (owner).** The capture point lifts along the face normal. Box fit and live-lighting shares work over the face's hemisphere. A probe that binds any world face captures every static brush, with no capture exclusion (owner): exclusion leaves small brushes to a screen-space layer, and world faces have none (`resource_management.md` §5.2). Water-only probes keep today's exclusion. Lobes become six world axes: a format change.
  - One probe per coplanar glossy face group, not room-centre probes: those leave capture-shadow holes behind pillars, which the IP rules forbid filling.
  - `reflection_volume` may claim glossy world faces as well as water.
  - The binding format addresses world (id-17) faces too: a PRL change. Layer placement: placement and capture stay compile-time (`resource_management.md` §5, IP fences below).
- **Opt-in by sharpness (owner).** A material reflects only if its `_p` sharpness has texels above an engine-owned sharpness floor. This is decided per material at compile time and weighted per texel at runtime. Prefix-only materials (no `_p`) never reflect. PBR-converted materials (owner uses Material Maker) all carry `_p`; the floor keeps rough ones free.
- **Blur (owner).** Compile-time prefiltered colour mips, selected per texel by a lobe matched to the highlight's exponent, so a texel's reflection blurs exactly as wide as its highlight. The distance tile stays one level. Texels at the exponent cap (`specular-texel-sharpness`: smoothness, 1–8192) read the unblurred level, so mirrors exist.
- **Strength and colour (owner).** Strength is Schlick Fresnel on water's reflectance scale (`resource_management.md` §5): a fixed head-on base of 0.04 for untinted texels, raised toward 1 by the texel's tint, so a metal approaches a full tinted mirror as metallic does in Unity and Godot. `_s` keeps shaping highlights only, so Material Maker's maps, which carry no specular-strength map, fill every input. A texel's reflection is coloured exactly as its highlight is, under the tint rule `specular-texel-sharpness` owns: with `_t`, a mix from white to the albedo's hue normalized to its largest channel, by t; without `_t`, the raw albedo, as today's highlight. Diffuse is unchanged either way. This brief inherits the rule and does not define it.
- **Overlay pass, not forward integration (owner).** After opaque world geometry, redraw glossy material buckets with depth `Equal` and the forward pass's clip-position computation, `@invariant`. Blend over HDR scene colour with the fluid Fresnel composite (§7.3a Weight and composite, with no SSR or planar layer, so c = 0). Forward has no free binding (§10), and this leaves the always-run forward shader's cost and registers untouched (§7.3 Surface Depth cost note). The compiler groups glossy buckets so pipeline switches stay few.
- **IP fences kept (owner).** Hard per-face binding by position, never by lookup direction. Capture shadow withheld on world faces, never filled. Tiles never rewritten; live lighting stays a post-sample ratio. No precomputed intersection table; no history (`plans/done/reflection-probes/ip-check.md` §4, §7.3a).
- **Probe cap stays at 256 (owner).** Raising it is a separate brief beside the roadmap's "reflection-probe streaming and seams", sized from measured glossy content. Probes still gain a memory-ledger row, so their cost shows against the 2 GB floor (`index.md` §1.1). Past the cap, derived probes drop smallest bound area first, as `apply_probe_cap` does today, but the build warning names each dropped probe's faces instead of only counting them.
- **Governed by existing switches (owner).** `render.reflections.style`: pixel uses the texel-centre reconstruction from `specular-texel-sharpness`; smooth evaluates per fragment. `reflection_quality`: off draws nothing; low and above draw the probe term on world faces. No new author or player surface.
- **Sequencing (owner).** Build follows `specular-texel-sharpness` (sharpness, tint, texel centre) and `reflection-probe-fluid-pass-performance`, whose once-per-probe live lighting glossy coverage needs. Counsel must also clear Tencent WO claim 1 (`context/research/gi-rtx-lite/index.md`). Drafting now is fine; build is gated.
- **Non-goals.**
  - SSR or planar on world faces (above).
  - Tinted metal through a metallic input or diffuse suppression: the tint rule covers metals without it.
  - Glossy mover faces: the mover pass has its own layout and a documented register cliff (§7.3). Movers also never enter captures.
  - Skinned meshes: they never bind the world material bundle.
  - Probe blending across boxes: `reflection-probe-seams` owns it.
  - Mover-aware captures: draft `mover-aware-reflection-captures.md` owns them.
  - Filling capture shadow (IP).
  - A PBR importer in `postretro-tool`.
  - A per-material reflectivity override or opt-out in `materials.toml` (owner, deferred). Strength already uses water's reflectance meaning, so a later override is purely additive.
  - The roughness→`_p` conversion formula: the sharpness brief owns its doc row.

## Acceptance
### Automated
**Water parity**
- [ ] On every water map in `content/dev`, placement output is byte-equal to the build before: boxes, capture points and bindings. So are mip-0 colour tiles and distance tiles. Shares match on the five original lobes, with −Y zero.
- [ ] Water renders within 1/255 of the build before at the reflection-probes parity views, every tier and both styles. Water samples colour mip 0 only.
**Opt-in**
- [ ] A material without `_p`, or with `_p` but no texel above the floor, gets no probe and no binding. No overlay draw reaches its faces, and its render is byte-identical to the build before under both styles.
- [ ] On a bound face, a texel at or below the floor draws no reflection term; a texel above it does.
**Placement and capture**
- [ ] Derived probes on a floor, a wall and a down-facing ceiling each sit on their face's front side, lifted along its normal. Coplanar glossy faces of one group share a probe; faces on different planes never share a derived probe.
- [ ] A `reflection_volume` claims glossy world faces inside it. A box over neither water nor glossy faces drops with a warning.
- [ ] An unobstructed glossy floor and wall reflect over their whole area: no fragment lies in capture shadow cast by the reflector's own surface.
- [ ] Behind a pillar on a glossy floor, as seen from the capture point, fragments draw no probe term. A source gate shows no fill from SH, lightmap, screen space or another probe, as the water gate does.
- [ ] Lobe weights for any unit direction sum to 1. For y ≥ 0 they equal today's five weights exactly, with −Y zero.
**Shading**
- [ ] The exponent→mip function is one pinned function shared by the compiler prefilter and the shader, with a GPU-free reference. Blur grows monotonically as the exponent falls.
- [ ] At every colour mip, each tile border copies its wrapped interior.
- [ ] A texel's reflection colour follows its highlight's tint: with `_t` by the tint rule, without `_t` by raw albedo. Diffuse is byte-identical with and without the overlay.
- [ ] Head-on, an untinted texel reflects at 0.04 and a tint-1 texel at 1; at grazing both approach 1. The weight scales by the capture-shadow weight. Changing `_s` changes the highlight and leaves the reflection byte-identical.
- [ ] Under pixel style, fragments inside one magnified texel get identical reflection colour, mip and weight, and adjacent texels differ. Under smooth style they vary within the texel.
**Gates and passes**
- [ ] At reflection quality off, no overlay pass is recorded, and the frame matches the build before. At low and above, glossy faces draw the probe term, and no SSR or planar layer reaches a world face.
- [ ] Any lighting mask with a bit of the all-terms set off makes the overlay draw no term, as for water.
- [ ] The forward shader text, its layout and its budget pins are unchanged.
- [ ] Every forward-shaded pixel of a bound glossy face receives exactly one overlay fragment, and no other pixel receives one.
- [ ] The no-history gate's binding inventory covers the overlay shader.
- [ ] A stored light forced off its reference changes a glossy face's reflection on that frame.
**Format and cap**
- [ ] Id 54 version 3 round-trips through write and read. A version-2 section is refused with the named version error. Bindings to world faces are checked against id 17 at load.
- [ ] A fixture one probe past the cap drops exactly one derived probe, the one with the smallest bound area, and the build warning names it. A fixture at the cap drops none.
- [ ] A probe binding both water and a glossy world face captures a brush small enough that exclusion would drop it for water alone.
- [ ] Probe resident bytes appear in the memory ledger and return to zero at unload.
**Docs**
- [ ] `docs/level_design.md` and the FGD help text document glossy reflection: opt-in through `_p` above the floor, colour by `_t`, and `reflection_volume` over world faces.
### Manual
- [ ] Dev map: a glossy floor and a glossy wall, using a dev material with a `_p` gradient and a `_t` tint. Each reflects its room, blur follows sharpness, colour matches the highlights, and the term is withheld behind a pillar.
- [ ] Under pixel style, a slow camera move shows reflection blocks holding per texel, with no seam at the octahedral fold at coarse mips.
- [ ] e24 and the probes map: water looks unchanged at near, mid and far views.
- [ ] 5300M, glossy-heavy coverage at 4K and Auto: overlay cost measured and reported, by pass, against the ceiling the plan sets.
- [ ] 5300M: forward pass cost unchanged against the build before, on a map without glossy faces and at reflection quality off.
- [ ] Probe-stage compile time on the stress fixture: measured and reported.

## Path
- **Compiler seams.** `placement.rs` `up_facing_faces` becomes the reflector-face census: id-52 up-facing faces plus glossy id-17 faces. Glossiness comes from the `.prm` `_p` channel, which `ReflectionProbeInputs` can already reach (`research.md` §Stage order).
  - Generalize `plane_heights` / `plane_of` to planes (normal and offset), and `derive_probes`' grouping with them.
  - Generalize `CAPTURE_LIFT_M` along the normal, `fit_rays` to the face's hemisphere, and `PointClassifier::settle` to in-plane axes.
  - Make `share.rs`'s `direction.y < 0` test the face's hemisphere, and the exclusion rules' reflector plane and normal.
  - Bound reflector faces must be transparent to their own probe's primary rays, as water is absent from `SceneTracer` (`research.md` §The reflector is in its own capture).
- **Runtime seams.**
  - Per-vertex world probe index: a forward-free side stream like the specular owner stream (§6), built like `fill_probe_indices`.
  - Overlay draws: an `IndirectDrawPlan` over glossy bucket ranges.
  - Reuse `fluid_reflection_ray.wgsl`'s search, fade and composite, with `fluid_reflection_above_horizon` generalized to the face plane.
  - Colour arrays need `mip_level_count` past 1. BC6H stops the chain at 4².
- **Live lighting** reads `reflection-probe-fluid-pass-performance`'s per-probe sums. Its dispatch condition extends to frames with overlay draws.
- **Strongest rival:** forward integration. It needs a binding forward lacks and taxes the always-run shader (`research.md` §Rivals).
- **First slice.** One dev glossy floor: a derived floor probe with the self-transparent capture, and the overlay drawing the mip-0 term. It falsifies the self-occlusion and `Equal`-landing assumptions before the format and mip work.
- **Split first.** `compute_cull.rs` (1400 lines), `renderer_render_frame.rs` (1424) and `pipeline.rs` (2600) are past the size guideline. Split before extending, behaviour-preserving, each in its own commit.

## Open questions
- Surface Depth materials: does the overlay rerun the march, matching highlights at a march per glossy pixel, or reflect about the unmarched plane and normal map, so carved glossy texels mismatch? — owner — **blocks build**
- Counsel on Tencent WO2026113651A1 claim 1 for world faces. — owner, with counsel — **blocks build**
- Sharpness floor value (estimate n ≈ 6–16). — **delegated**
- Capture point and warning for an authored box binding faces on several planes. Generalize `_capture_offset` to in-plane. — **delegated**
- Whether the planar mirror view records the overlay. — **delegated**
- A mover face coplanar with a glossy world face takes the world's reflection under the overlay's `Equal`. — **delegated**: suppress it or record it as an accepted limit.
- Overlay cost ceiling on the 5300M. — **delegated**: the executor measures the first slice and proposes a ceiling at the resumable plan stop.

## Boundary inventory
No new cross-boundary name. Existing names change meaning only:

| Name | Where | Change |
|---|---|---|
| `reflection_volume` | FGD classname | may claim glossy world faces |
| `render.reflections.style` | mod render profile | also governs world-face reflections |
| `reflection_quality` | player option | off / low+ gate world-face reflections |
| `{name}_p.png`, `{name}_t.png` | sibling PNGs | inherited from `specular-texel-sharpness` |

## Wire format
ReflectionProbes (id 54), version 3. It mirrors version 2's layout (`level-format` `reflection_probes.rs` doc comment); a version-2 section is refused. Little-endian, unsigned counts in the header as today.
- **Lobes.** Lobe count 6, order +Y, +X, −X, +Z, −Z, −Y. −Y is appended, so water's five lobes keep their indices.
- **Colour mips.** The header gains a u32 colour mip count (≥ 1). Each probe's colour tile stores mips 0..count in sequence, each with a 1-texel border copied from its wrapped interior. The distance tile stays one level.
- **Bindings.** Two lists, each ascending strictly, with a u32 count each in the header:
  - fluid: `u32 volume, u32 face, u32 probe` (id-52 numbering), as today;
  - world: `u32 face, u32 probe` (id-17 numbering).
  - An empty list is count 0. A face binds at most once.
- **Probe placement.** The record's `layer` field follows the cap mechanism the executor picks; `plan.md` pins it.

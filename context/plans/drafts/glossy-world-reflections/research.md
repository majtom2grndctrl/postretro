# glossy-world-reflections — research

Derivation behind the brief. Source read at `e7438238` and re-checked at `a1d14351` (main plus `reflection-probe-fluid-pass-performance`): every census claim below holds. The placement, share, exclusion, capture and level-format files are unchanged between the two. `ReflectionProbeInputs` also gained `emissive_textures`, `emissive_volumes` and `area_glow`.

## Source census

**Placement is water-only end to end.**
- `placement.rs`: `up_facing_faces` walks id-52 `surface_faces` and keeps `FaceGeometry::is_up_facing` (`UP_FACING_MIN_COS`, 1°). `place_probes` runs authored, then derived for unclaimed faces, then `apply_probe_cap`. `CAPTURE_LIFT_M` (2 mm) and `plane_heights` / `plane_of` cluster by centroid `y`.
- `authored.rs`: `place_authored` drops a box binding no face through `warn_binds_no_face` ("binds no up-facing fluid face"). `authored_capture_point` takes the lowest plane's horizontal centroid.
- `derived.rs`: `derive_probes` groups unclaimed faces per fluid entity and plane height, then by `box_match_components`; origin is centroid + `DVec3::Y * CAPTURE_LIFT_M`; `group_probe` uses `capture_point_over`.
- `box_fit.rs`: `fit_rays` covers the upper hemisphere only; `fit_box` reads them.
- `capture_point.rs`: `PointClassifier::settle` steps on `DVec3::X` / `DVec3::Z` only.
- `share.rs`: skips samples with `direction.y < 0.0`.
- `exclusion.rs`, `exclusion_underside.rs`: per-probe rules phrased against "bound water"; the underside rule keeps downward faces over the water.
- `capture.rs` `capture_row`: full-sphere octahedral rows; shares keep the upper hemisphere.
- `hit.rs` `SceneTracer`: "Movers and environment volumes are not in it". Water is never in its own capture.

**Format.** `level-format/src/reflection_probes.rs`:
- `REFLECTION_PROBES_VERSION` 2; `REFLECTION_PROBE_TILE_DIMENSION` 128, border included; `REFLECTION_PROBE_TILE_BORDER` 1; `REFLECTION_PROBE_LOBE_COUNT` 5 (up, +X, −X, +Z, −Z); `MAX_REFLECTION_PROBES` = `PORTABLE_MAX_TEXTURE_ARRAY_LAYERS` (256).
- `ReflectionProbeBinding { volume, face, probe }`, documented as "An up-facing fluid face, addressed in EnvironmentVolumes (id 52) numbering". `ReflectionProbeRecord` is reflector-agnostic already.
- `reflection_probe_lobe_weights` splits the downward part `max(−y,0)²` evenly over the four horizontal lobes.

**Runtime.**
- `fluid_reflection_probes.rs` `array_descriptor`: `mip_level_count: 1`.
- render-cpu `fluid_surface/reflection.rs` `fill_probe_indices` builds the fluid per-vertex probe index at install.
- `fluid_reflection_ray.wgsl` `fluid_reflection_above_horizon` floors the ray to `FLUID_REFLECTION_RAY_MIN_Y` above the horizon.
- `compute_cull.rs` `IndirectDrawPlan::visit` issues `IndirectDrawCommand::BindMaterial` once per bucket change; `draw_indirect_buckets` executes it.
- `@invariant` clip position is shared by `depth_prepass.wgsl`, `forward.wgsl` and `kinematic_brush.wgsl`; that is what lets a second entry land on `Equal`.
- `pipeline_layout.rs` `FORWARD_SAMPLED_TEXTURE_BUDGET` = 16, at 16/16 (`surface_depth.wgsl` header). Forward uses groups 0–6 (`rendering_pipeline.md` §10).
- Precedent for a per-vertex side stream: world specular owners, forward-only, vertex slot 1, stamped from id 42 per id-17 face (`rendering_pipeline.md` §6).
- No memory-ledger row for probes: `sh_allocation.rs` `ShAllocationKind` names none.

**Stage order (handoff "not verified", now grounded).** `pipeline.rs` runs `texture_stage::bake_world_materials` and `bake_remaining_textures` before `reflection_probes::bake_or_load_reflection_probes`. `ReflectionProbeInputs` already carries `texture_keys` and `prm_root`, so placement can read each material's baked `_p` channel before capture. `specular_slot.rs` `bake_specular_slot` widens the slot to `Rgba8Unorm` when `_p` or `_t` exists: B holds smoothness, 0 (`SMOOTHNESS_ABSENT`) without `_p`; A holds tint, 255 (`TINT_ABSENT`) without `_t`. Without either the slot stays R8 or RG8.

## Today's specular combine and the tint rule

Forward `fs_main` returns `base_color.rgb * total_light` with specular inside `total_light`; the mover pass does the same (`specular-texel-sharpness/research.md` §Tint). So a highlight without `_t` is tinted by raw albedo. The reflection takes the same colour as the highlight, `_t` or not. The tint rule and its storage belong to `specular-texel-sharpness`, whose Decisions store tint in the specular slot's A.

## The reflector is in its own capture

Water is absent from `SceneTracer`, so a capture ray through the plane reads the pool beneath. A glossy floor is a static face, so it is in the BVH.
- The capture point sits 2 mm off the face. A ray toward any fragment of that face meets the face itself.
- The distance texel near the face's horizon averages 1/L1 over hits a few centimetres away (a 2.8° texel band at 2 mm lift spans hits from ~4 cm outward). The mean of inverse distance is dominated by the near hits.
- So h at the fragment reads ≥ κ almost everywhere: the whole face would sit in capture shadow.

Generalizing "from water" therefore needs the bound reflector faces transparent to their own probe's primary rays, as water is, or an equivalent. The brief carries it as an Acceptance row and a Path note. Shadow rays and the box fit still see the face.

## Six lobes and water parity

Today: weights `y², x²+q, x̄²+q, z²+q, z̄²+q` with q = max(−y,0)²/4. Six world axes: each lobe max(d·axis,0)², with −Y its own lobe; the six clamped squares sum to 1.
- For y ≥ 0, q = 0 and the −Y weight is 0. The first five weights are identical to today's.
- Water reads only rays floored above the horizon, so its blended ratio is unchanged.
- Appending −Y last keeps the first five lobe indices. Water's compiled shares match on those five, with −Y zero, because `share.rs` still skips below-hemisphere samples for an up-facing face.

Parity can therefore be byte-level for placement and tiles and exact for shares, not only visual.

## Blur: exponent to mip

Probe tile: 128 texels including border. On the octahedral axes, pole to antipode is 64 texels for 180°, so about 2.8° per texel. The solid-angle mean is about 1.6°.

Blinn-Phong lobe, half-vector half-angle at half maximum: θh = acos(0.5^(1/n)).

| n | θh | Reflected-direction HWHM (≈ 2θh) | Mip-0 texels across FWHM |
|---|---|---|---|
| 256 | 4.2° | 8.4° | ≈ 6 |
| 64 | 8.4° | 16.8° | ≈ 12 |
| 6 | 27° | 54° | most of a hemisphere |

Rotating the lookup direction by φ rotates the half-vector by φ/2, so the environment lobe is about twice the half-vector lobe. The sharpness brief's estimate ("a 256 lobe of about 5° is about two texels") reads the half-vector angle. Under a matched lobe, the sharpest texel (256) blurs over about six mip-0 texels, two to three mips down, and never reads as a mirror.

So a matched lobe at exponent 256 never reads as a mirror. `specular-texel-sharpness` stores smoothness with an 8192 cap, so highlights can be very tight. Reflections stop short of that: the owner ruled no glossy world face is a mirror (2026-11-22), so the selection clamps at a minimum-blur mip and texels near the cap read it. The ruling serves the look (a pixel world that comes alive, not a photoreal one) and the floor layer's cost: a blurred screen-space term needs less target resolution and march detail.

Floor estimate: the roughness conversion in `specular-texel-sharpness/research.md` §Tint gives r = 0.7 → n ≈ 6. At that width the term is close to the SH ambient it would double-count. A floor near n ≈ 6–16 keeps rough PBR conversions out of the overlay. Unmeasured.

## Memory and the cap

Per probe today: BC6H colour (128/4)² × 16 B = 16 KiB, plus R16F distance 128² × 2 B = 32 KiB, so 48 KiB. A full colour mip chain adds about a third of the colour: 128→64→32→16→8→4 adds 4 + 1 + 0.25 + … ≈ 5.3 KiB. That is about 53 KiB per probe. Guard texels per mip change this slightly. BC6H needs 4×4 blocks, so the chain stops at 4².

| Probes | Resident (≈ 53 KiB each) |
|---|---|
| 256 (today's cap) | ≈ 13.3 MiB |
| 1024 | ≈ 53 MiB |
| 4096 | ≈ 212 MiB |

Constraints on raising the cap:
- **Layers.** The renderer requires and relies on 256 array layers (`rendering_pipeline.md` §10). Past 256 probes, tiles must pack several per layer (a 2048² layer holds 256 tiles of 128²), or bind more arrays.
- **Share table.** One uniform laid out at the cap: 61 456 B at 256 (`resource_management.md` §5.3), near the 64 KiB uniform-binding floor. Past the cap it must move to a texture or a storage buffer. The fluid fragment stage is at 8/8 storage, so for water it rides a texture or a chunked uniform. The overlay owns its layout and has more freedom.
- **Six lobes** grow each share entry from 5 to 6 lobe rows.
- **Ledger.** Probes have no memory-ledger row, so a raised cap is invisible to the 2 GB accounting today.

## Probe count on glossy-heavy maps

Not measured. One probe per coplanar glossy face group means a box room with a glossy floor and four glossy walls takes five probes. A Control-scale hall with glossy floor, pillars and wall panels can take dozens per room. `campaign-test`-scale maps with glossy trim throughout could pass 256. The owner sets the stress fixture's scale.

## Rivals

- **Forward integration.** Read the probe term inside `fs_main` for glossy buckets. Forward is at 16/16 sampled textures and 8/8 fragment storage. The always-run shader would also pay registers and layout cost for a branch most fragments skip, as Surface Depth showed on AMD Metal (`rendering_pipeline.md` §7.3). Rejected by the owner.
- **Per-material forward pipeline variant** (`context/research/gi-rtx-lite/index.md` measurement item 9). It keeps the always-run shader clean but still needs a binding slot forward lacks. The overlay's own layout sidesteps it.
- **Room-centre probes.** Fewer probes, but one elevated point per room leaves capture-shadow holes on faces it cannot see (behind pillars, alcoves), and the IP rules forbid filling them. Face-lifted probes also shadow behind pillars, but each face group gets the vantage closest to its own plane. Rejected by the owner.
- **SSR or planar on world faces.** Planar stays water's. SSR is signed off for up-facing glossy faces only (owner, 2026-11-22; §Monument); walls stay probe-only, as a second screen-space use there would need its own sign-off (`index.md` §2 Pre-RTX).
- **Composite (1 − W)·diffuse** (the first lock, reopened 2026-11-21 by `/validate-plan`). Diffuse includes Σ dynamic direct (`rendering_pipeline.md` §7.3 output), and dynamic lights draw no specular (§4), so W removed their light with nothing in its place. Fresnel drives W toward 1 at grazing: about 0.44 untinted for a floor 10 m ahead at eye height, and near 1 at any angle for a tint-1 metal. Replaced by forward + W·(R − D_static), which also spares the overlay forward's specular and emissive inputs.
- **Animated glow in reflections** (glow delta tiles, selective glow SSR): moved with their IP checks to `drafts/dynamic-reflection-layer/`. v1 holds animated content at reference.

## IP basis

The fences are in the brief's IP Decision. Their sources: `plans/done/reflection-probes/ip-check.md` §4, `rendering_pipeline.md` §7.3a (Live lighting IP rules, Screen-space layer IP rules), `plans/done/reflection-probe-fluid-pass-performance/ip-check.md` §Lever A and §Shape B.

Basis for the 2026-11-22 claim-1 ruling, from `reflection-probes/ip-check.md` §1, §3, §5:
- Shipped water already carries the reading: elements 1a, 1b, 1d and 1e appear present; 1c ("texture image") is left to counsel, since water samples albedo per fragment in forward and builds no screen-space unshaded image.
- The same check reads claim 1 as covering Bjorke 2004 and Lagarde & Zanuttini 2012.
- The application is pending. US national phase is due about 2027-05-28, EP about 2027-06-28; claims may narrow before grant.
- The overlay redraws each face and samples its material per fragment, as water does, with no G-buffer. It adds no claim-1 element and keeps every dependent-claim fence. The description names metal and glass among its reflectors; that adds no element.

`gi-rtx-lite/index.md` and the roadmap's Lighting extensions row gated glossy world surfaces on counsel; the ruling lifts that gate at promotion. Compile-time exponent-driven prefiltering has no recorded patent in the project's IP notes; it was not searched. The floor layer's blur is charted in `ip-check.md` here.

## Live lighting

The sums landed with `reflection-probe-fluid-pass-performance` (`rendering_pipeline.md` §7.3a Per-probe sums). As built:
- `FluidProbeResources::new` creates the sums buffer, `FLUID_PROBE_SUMS_BYTES` = `MAX_REFLECTION_PROBES` × `REFLECTION_PROBE_LOBE_COUNT` × 16 = 20 480 B, usage storage and uniform. Six lobes make it 24 576 B, still a multiple of 256 and under 64 KiB.
- `InstalledFluidSurfaces` owns it, so a level with no fluid face has none. The fluid fragment reads it as a uniform at group 4 binding 4, beside the colour array, share table and distance array; it borrows group 3's sampler.
- `FluidProbeSumsPass` (`fluid_reflection_sums.wgsl`) dispatches from `record_fluid_surface_passes`, inside the fluid pass's own draw gate, when `FluidSurfacePass::has_probe_term_draws`. That is after world forward, movers, skinned meshes and ghosts; the overlay records before the Ghost Pass, so the dispatch must move ahead of it.
- WGSL hard-codes five lobes in `fluid_reflection_lobe_weights`, `fluid_reflection_light_share`, the sums shader's unrolled writes, `fluid_reflection_ratio_from_sums`' unrolled reads, and the `array<vec4<f32>, 1280>` sums declarations. Tests pin the literals to the Rust constant.
- The per-fragment `fluid_reflection_ratio` stays reachable from no entry: the sums' oracle, and the fallback for a moving reflector. Glossy world faces are static, so they read the sums.

## Monument

The owner's motivating case: a freestanding monument on a glossy floor, as in Control, which appears to switch probes at wall corners so only one reflection shows at a time. A freestanding monument has no occluder to hide a switch.

**Probe-only leaves a wedge.** The capture point sits 2 mm above the floor. A floor point behind the monument, seen from it, lies along a segment under 2 mm high that meets the monument's base. So every floor fragment in the angle the monument covers from the capture point, out to the walls, is in capture shadow: a dull stripe across the floor, withheld and never filled.

**Splitting groups cannot help.** Each probe's capture point lies on the same plane, so each sees its own wedge. Placing boundaries around the monument puts them in open floor, which the seams ruling rules out.

**Exclusion plus a floor layer** is water's answer for pillars (`resource_management.md` §5.2). With the monument out of the capture, the probe holds the room behind it, and the screen-space layer draws the monument at the tiers that run it. At Low it is absent and nothing is withheld. Threshold t = p(W)·W with W = √(bound area), p = 0.85 to a 10 m knee, then a power law through 0.33 at 50 m (`ExclusionConstants::size_threshold`):

| W | p(W) | t |
|---|---|---|
| 10 m | 0.85 | 8.5 m |
| 20 m | ≈ 0.57 | ≈ 11.3 m |
| 30 m | ≈ 0.45 | ≈ 13.4 m |

So a monument whose brushes are each shorter in plan than t is excluded with no override. A brush with a downward face over the floor stays, as water keeps undersides, and leaves its wedge; `never` excludes it.

**Cost evidence.** Water's screen-space pass on the 5300M at High (target fraction 1, 90 steps, stride rows ÷ 180), at the e24 near view, which looks straight down with water filling the screen: 2.6 ms at Auto, 6.7 ms at 1440p, 14.7 ms at 4K, against forward's 5.6, 14.2 and 31.5 ms (`measurements/fluid-base-pass-cost/results-remaining-cost.md`). Scaled by coverage, a floor on 40% of the screen would cost about 1.1, 2.7 and 5.9 ms: an extrapolation from a view unlike a floor's usual grazing one. A glossy hall's floor is on screen most frames, unlike water. The blur lets the floor layer run a reduced target and a coarser march; its values are set at the plan stop from the owner's 5300M runs.

**Movers and characters.** The layer can only show what is drawn when it runs. The owner chose to include movers and characters, so the overlay moves from before the mover pass to after the skinned-mesh pass, before the Ghost Pass. Character and mover pixels in front of a floor fail the overlay's `Equal`; a mover face exactly coplanar with the floor passes it, so it needs a guard (pin P15).

## Overlay hazards

- **Coplanar mover.** A mover face exactly coplanar with a world face passes the colour pass's `Equal` and draws over the world (`rendering_pipeline.md` §7.3). The overlay records after the mover pass, so its `Equal` redraw of the glossy world face would blend the world face's reflection over the mover there. It needs a guard (P15).
- **Glossy faces in captures.** Capture shading has no specular, so a glossy face seen in another probe's capture shows its diffuse only. No recursion. Accepted.
- **Planar mirror.** The mirror view reuses main pipelines for world draws. Whether it also records the overlay is open.

## Pins

Orderings the Acceptance rows cite.

| ID | Scenario | Ordering | Expected outcome |
|---|---|---|---|
| P1 | A level with glossy world faces and no fluid volume | geometry install, then a fluid install with no surface face | Probe resources upload, the overlay draws, the ledger counts them; nothing drops before unload |
| P2 | A glossy face flush with a coplanar face whose material does not reflect (a bronze inlay in a stone floor, or a matte strip crossing a glossy floor) | capture traces from 2 mm off the shared plane | No fragment of the glossy face lies in capture shadow cast by any face on its plane, bound or not |
| P3 | One authored box binds a glossy floor and a glossy wall, or a pool and a glossy wall | the box makes one probe per facing; each probe captures with only its own faces transparent | The floor reflects the wall and the wall reflects the floor; the pool reflects the wall, and the wall reflects the pool's bed where the water lies: captures hold no fluid surface (`SceneTracer`) |
| P4 | An authored box binds faces on several planes, including a pool above a lower glossy floor | one probe per facing; each probe's capture point chosen from its own faces, then the offset | Each probe's capture point is on the front side of every face it binds, or a face it cannot serve binds nothing and the warning names it |
| P5 | A stored light forced off its reference on frame N, with only a floor, a wall or a ceiling drawn; N is the first frame after install, or the first overlay frame after frames with none, or a frame with no water draw | descriptor flush, per-probe sums, overlay, all on N | The reflection changes on N for every orientation; no overlay fragment reads unwritten or stale sums |
| P6 | A texel whose exponent selects between colour mips m and m+1, near the octahedral fold, down to the 4² mip | per-mip interiors differ (126, 62, … 2 texels), so one UV cannot address two mips | Each mip read lands on the interior texel the compiler wrote for that direction |
| P7 | `reflection_quality` off → low → off live; a level built at off and switched to low on its commit frame | tier change applies live; a level load's commit applies no tier | Off frames record no overlay; the low frame draws the probe term; nothing is built or reinstalled |
| P8 | Unload level A, install level B with different glossy faces and probes | B's install replaces A's side stream, draw plan and probes | B's first frame draws only B's faces from B's probes; the ledger holds only B's probes |
| P9 | A committed reflection-style switch while a level is installed, either direction | style commit, then the next frame | That frame's overlay is in the new style |
| P10 | Very high, selected plane, glossy faces in the mirror's reach | planar group records world draws with main pipelines | Mirror glossy faces draw no overlay, or one tested against mirror depth from the mirror camera |
| P11 | Surface Depth quality toggled live over a glossy Surface Depth material | toggle applies, then the next frame | The overlay's sampled texel follows the same quality as forward's on that frame |
| P12 | A ghost, water, a billboard or fog drawn over a bound glossy face | world forward, movers, skinned meshes, overlay, then the Ghost Pass and every later colour pass | The overlay's destination is forward's output alone; nothing drawn after it is dimmed by the reflection weight |
| P13 | A character under a promoted static light casts its pool shadow on a bound glossy floor, head-on and at grazing, untinted and tint-1 | forward subtracts the shadowmask union from static direct, then the overlay adds W·(R − D_static) | D_static carries the same union subtraction at the same promotion weight, so the shadow fades by (1 − W) with the rest of static diffuse; no channel of the overlay's output is below 0 |
| P14 | On frame N an animated baked light pulses, a promoted light's w crossfades, an SH cluster becomes resident, and the lighting mask toggles one bit | mask snapshot and frame scales, compose, forward, then the overlay, all on N | The overlay's D_static reads N's values for every input forward read on N; with any all-terms bit off on N, the overlay draws no term on N |
| P15 | A mover face coplanar with a bound glossy world face, and a mover or character in front of one | world forward, the mover pass, the skinned-mesh pass, then the overlay | The mover's and character's pixels show their own pass's output alone; no overlay term lands on them, the coplanar face included |
| P16 | A freestanding monument on a glossy floor, each brush under the exclusion threshold; a character walks past it; tier switched live low → medium → low | capture with the monument excluded; then per frame the floor layer's march, then the overlay | No floor fragment is withheld behind the monument at any tier; at medium both monument and character reflect on that frame; at low neither does |
| P17 | A glossy level at low or above; the camera turns from a view with N glossy faces to one with 0, then to one with 1 | cull, then the overlay gate and the sums' dispatch condition | A frame with no overlay draw records no overlay pass and, with no reflecting fluid draw, dispatches no sums; the 1-face frame records one pass and dispatches the sums once, ahead of the overlay |
| P18 | A water map one probe under the cap gains glossy faces needing two probes | authored, derived, then the cap | Every water probe survives; the smaller of the two world-face-only probes drops, and the warning names its faces |
| P19 | A map at the cap whose derived water probes fill it, plus an authored `reflection_volume` over glossy walls making world-face-only probes | authored and derived probes placed, then the cap | Every water probe survives; the authored world-face-only probes drop, smallest bound area first, and the warning names the box and its faces |



## Not verified

- Overlay and floor-layer cost on the 5300M at glossy-heavy coverage (the owner runs it).
- Probe counts on glossy-heavy maps (above).
- The exponent-to-blur estimates (above).
- Source and Black Ops gloss-driven envmap precedent: from memory, not re-checked.
- Compile-time cost of one probe per glossy face group at stress scale.

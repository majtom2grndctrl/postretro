# glossy-world-reflections — research

Derivation behind the brief. Source read at `e7438238`. The handoff's verified facts are re-cited here by symbol; each was re-read this session unless marked.

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
- `fluid_reflection_ray.wgsl` `fluid_reflection_above_horizon` floors the ray above y = 0.
- `compute_cull.rs` `IndirectDrawPlan::visit` issues `IndirectDrawCommand::BindMaterial` once per bucket change; `draw_indirect_buckets` executes it.
- `@invariant` clip position is shared by `depth_prepass.wgsl`, `forward.wgsl` and `kinematic_brush.wgsl`; that is what lets a second entry land on `Equal`.
- `pipeline_layout.rs` `FORWARD_SAMPLED_TEXTURE_BUDGET` = 16, at 16/16 (`surface_depth.wgsl` header). Forward uses groups 0–6 (`rendering_pipeline.md` §10).
- Precedent for a per-vertex side stream: world specular owners, forward-only, vertex slot 1, stamped from id 42 per id-17 face (`rendering_pipeline.md` §6).
- No memory-ledger row for probes: `sh_allocation.rs` `ShAllocationKind` names none.

**Stage order (handoff "not verified", now grounded).** `pipeline.rs` runs `texture_stage::bake_world_materials` and `bake_remaining_textures` before `reflection_probes::bake_or_load_reflection_probes`. `ReflectionProbeInputs` already carries `texture_keys` and `prm_root`, so placement can read each material's baked `_p` channel before capture.

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

So a matched lobe at exponent 256 never reads as a mirror. `specular-texel-sharpness` therefore stores smoothness with an 8192 cap: a matched lobe stays matched everywhere, and texels at the cap read the unblurred level.

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
- **SSR or planar on world faces.** No-go by owner decision; SSR on world faces would be a second screen-space exception needing its own sign-off (`index.md` §1.1).
- **Glow delta tiles** (rival for animated glow; §Glow delta tiles).
- **Selective glow SSR** (rival for animated glow; §Glow SSR).

## Thin emitters

Why a thin, bright, animated emitter (a neon tube, a strip light, area-light glass) is the hard case for probe reflections. Discussion 2026-11-21.

- **Probe resolution.** A 128² octahedral tile spans about 1.6° per texel. A 5 cm tube seen from 3 m subtends about 1°, so it reflects as a dimmed streak under a texel wide, which coarser mips spread further. Its height is fine: 2 m at 3 m is about 37°.
- **Share ratio.** Area-light glass counts in its light's share, but a share covers a whole lobe. Switching the tube off removes only its fraction of that lobe. With other lights in the same quadrant, a ghost streak remains, at 40% of the glow when the tube supplies 60% of the lobe, and at default `_glow` the streak can still bloom. The area-lights brief's reflection rows go fully dark only in a fixture lit by that light alone.
- **Plain glow.** An `emissive_volume` tube's reflection holds its no-script state, so a flickering sign over a wet floor reflects steadily (§Volume glow share).
- **Considered and set aside: a per-texel glow id.** Store which source each texel shows and scale it at sample time. It is exact at mip 0, but it cannot mix sources under the blurred mips, and a sub-texel tube needs coverage weights even at mip 0. §Glow delta tiles is the exact form of the same idea.
- **The candidates** are §Glow delta tiles (exact everywhere) and §Glow SSR (exact on screen, reference glow elsewhere). Both are rivals in the brief's Path.

## Glow delta tiles

A rival to the deferred limit (§Volume glow share) for glow that changes at runtime: area-light glass, `emissive_volume` signs, neon tubes. It follows the delta-SH form: a base plus each animated source's baked contribution × its live value. IP reading: `ip-delta-tiles.md`.

- **Shape.**
  - The compiler bakes, per probe, one sparse delta tile per animated glow source: what that source's glow adds to the probe's view at unit strength. A source is an area light's glass or an `emissive_volume`'s faces. Tiles are stored as 8×8 blocks of the 128² colour tile, with prefiltered mips.
  - At lookup, the overlay samples the base tile and each delta block present at that texel, and adds current_s × delta_s. Here current_s is the presented value: the volume table entry, or the area light's compose descriptor, both of which the overlay already binds.
  - Cast light stays on the share ratio. Its deltas would be dense, at about 21 KiB per light per probe, or about 22 MB at 4 lights × 256 probes [E]. Light spreads softly enough that the ratio hides its error.
- **Why it beats the alternatives.**
  - Prefiltering is linear, so blurring each delta and summing equals blurring the sum: the result is exact at every colour mip. A per-texel glow id cannot mix sources under blur.
  - It is exact off screen, where a screen-space layer has nothing.
  - It is per texel, where the aggregate share dims a whole lobe.
  - A switched-off tube goes fully dark in its reflection, and a recolour into a channel the reference lacks shows.
- **Base variant.** Prefer a base that holds glow at reference, with deltas adding (current − reference) × delta. It keeps today's tiles, water parity and the reference-state contract, and differs from the alternative (no animated glow in the base) only in how claim wording maps onto it (`ip-delta-tiles.md` §2, §7).
- **Cost** [E].
  - Bytes: a 2 m tube touches a handful of mip-0 blocks per probe, a few KB per source per probe with mips, plus a per-probe block index.
  - Per glossy fragment: one block-index read and one sample per glow source present, times the mips the exponent selects. Cap it at about 4 glow sources per probe; glow past the cap holds its reference, as today.
  - Compile: one more accumulation per glow hit in the capture trace, which `ShareSums::add` already visits.
  - Id 54: rides this brief's version 3.
- **IP.**
  - A compose pass that writes base + Σ deltas into a live tile on a source change is out. It updates stored probe lighting on a light change, which is what Tencent WO2026113651A1 claims 10–14 and 19 describe (`plans/done/reflection-probes/ip-check.md` §4).
  - Summing at sample time rewrites nothing and appears to stay outside WO claims 10–14 and 19, with claim 10 the closest element.
  - The nearest risk is Tencent US12293451B2 claim 1(c): a fixed per-probe, per-source delta × a runtime coefficient reads like its "radiance transfer parameter". This is the same open question as the shipped animated SH deltas (id 27). The shipped water share ratio has the same shape at lobe resolution and has no claim-level chart yet.
  - Fences: write tiles once at compile, keep deltas glow-only, and store them at base resolution (`ip-delta-tiles.md` §7).
- **Open.**
  - Per-fragment cost on the 5300M near a large glossy floor with several glow sources.
  - The sparse-block index format.
  - Whether the overlay's binding budget holds the delta array beside the base.

## IP fences carried over

From `plans/done/reflection-probes/ip-check.md` §4 and `rendering_pipeline.md` §7.3a:
- Placement and capture at compile time; the camera never places or moves a probe.
- Tiles never rewritten or re-shaded at runtime; live lighting stays a post-sample ratio.
- Capture shadow withheld, never filled (SH, lightmap, screen space or another probe).
- Hard per-face binding by position; no lookup-direction value picks the probe.
- No precomputed intersection table; no history.

Tencent WO2026113651A1 claim 1 reads on most parallax-corrected probe reflection on a textured surface. `gi-rtx-lite/index.md` gates glossy world surfaces on counsel. Compile-time exponent-driven prefiltering has no recorded patent in the project's IP notes; it was not searched this session.

## Overlay hazards

- **Coplanar mover.** A mover face exactly coplanar with a world face passes the colour pass's `Equal` and draws over the world (`rendering_pipeline.md` §7.3). An overlay `Equal` redraw of the glossy world face would then blend the world face's reflection over the mover there.
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
| P12 | A ghost, water, a billboard or fog drawn over a bound glossy face | world forward, overlay, then the Ghost Pass and every later colour pass | The overlay's destination is forward's output alone; nothing drawn after it is dimmed by the reflection weight |

P11 holds only if the Surface Depth open question answers that the overlay reruns the march. No Acceptance row cites it until that answer lands.

## Not verified

- Overlay cost on the 5300M at glossy-heavy coverage.
- Probe counts on glossy-heavy maps (above).
- The exponent-to-blur estimates (above).
- Source and Black Ops gloss-driven envmap precedent: from memory, not re-checked.
- Compile-time cost of one probe per glossy face group at stress scale.

## Volume glow share

Estimate, 2026-11-21, against the code at `e9495f27`. [M] read from code; [E] estimated.

- **Today** [M]: id 54 stores 4 lights × 5 lobes per probe (`REFLECTION_PROBE_LIGHTS_PER_PROBE`, `REFLECTION_PROBE_LOBE_COUNT`). The GPU `FluidProbeTable` is 240 B per probe, 61,456 B at 256 probes, under a 64 KiB `const_assert` (`render-cpu` `fluid_surface/reflection.rs`). A fifth light slot does not fit. `fluid_reflection_ratio` evaluates the shares per fragment.
- **Cheapest design** [E]: one aggregated glow term per probe and lobe, Σ_v share_v × (current_v / ref_v − 1). This is exact because the ratio is linear. current_v is the post-clamp table entry; ref_v is max × colour. It joins the per-probe sums of `reflection-probe-fluid-pass-performance` (unlanded, blocked on counsel) at zero per-fragment cost. Before that lands it fits the table's spare words, but the table then stops being install-only.
- **Cost** [E]: CPU is about 15 MADs per (probe, volume) pair, so a few µs typically and about 1 ms at 256 × 256, and nothing on a static frame. Id 54 gains about 64 B per volume per probe and needs a version bump shared with this brief's v3. Compile cost is negligible: `ShareSums::add` gains one lobe accumulation.
- **Why it is deferred** [E]: a share spreads over a whole lobe. A sign is small and sharp, so pulsing one dims or tints a quadrant of the reflection while the sign's own image stays fixed. Ref-0 channels cannot show a recolour, and tiny refs amplify unorm10 share error.

## Glow SSR

A second rival for glow that changes at runtime, beside §Glow delta tiles. A screen-space march fixes only the glow; the probe keeps everything else. IP reading: `ip-glow-ssr.md`.

- **Shape.**
  - *Glow pass.* After forward, an `IndirectDrawPlan` redraws only the selected faces, with depth `Equal`, into a small screen target. Selected faces are animated `light_area` glass and `emissive_volume` faces whose presented value is off reference. Each pixel writes current − reference glow, never full glow. The forward shader is untouched, as the brief's forward-unchanged row requires.
  - *Probe.* Unchanged: the base tile holds glow at reference, and cast light stays on the share ratio.
  - *March.* In the overlay, a glossy fragment marches the frame's depth along its reflected ray under the `reflection_quality` step budget, reusing `fluid_reflection_ray.wgsl`'s search. On a hit it samples the glow-change target at a mip chosen from the texel's exponent and the hit distance, and adds the sample to the probe term.
  - *Fallback.* A miss, a hit off screen or behind an occluder, or a fragment in capture shadow adds nothing, so the reflection shows reference glow. The change fades to 0 at screen edges.
  - *Gate.* The CPU skips both passes on any frame where no selected source is in view and off reference.
- **Against §Glow delta tiles.**
  - Delta tiles are exact off screen; this is exact only on screen, where neon over a wet floor usually is.
  - It bakes nothing: no id-54 bytes and no compile cost.
  - It appears to avoid '451 element (c), the stored transfer × coefficient form, with medium-high confidence: the march finds the glow at runtime from live screen data (`ip-glow-ssr.md` §4).
  - The two can stack: SSR on screen, delta tiles off screen.
- **IP fences** (`ip-glow-ssr.md` §7, §8). The nearest risk is Tencent WO2026113651A1 claims 10 and 19, which this approach appears to avoid on the same footing as the shipped share ratio, and it sits further from claim 15 than fluid SSR does. Keep:
  - The change term inside W, so capture shadow withholds it; never weight it by (1 − s) or march only where s < 1.
  - Colour from the glow target, never the frame buffer.
  - One depth buffer: selected faces write colour only.
  - The edge fades, and the ray never bends toward the screen interior (Nintendo US12694606B2, granted).
  - One direction-blind ray per fragment, with no history, no probe lookup seeded from the hit, and nothing baked per source.
  - State this layer's own capture-shadow rule in the brief, since §7.3a's "SSR never reads s" was written for water's layer beside the probe.
- **Policy.** This lifts the owner's probe-only ruling for world faces (`index.md`, Decisions) and adds a screen-space march on world faces, which needs owner sign-off, a gate and a fixed step budget (`context/lib/index.md` §2, Pre-RTX). The gate here is glossy fragments only, and only on frames whose glow pass drew. The selection is the glow pass's draw list; projectile or billboard glow could join it later.
- **Cost** [E].
  - Target: R11G11B10 or RGBA16F, about 33 MB at 4K full resolution, a quarter of that at half.
  - GPU: the march on every visible glossy fragment on frames the gate opens. A permanently animated sign in view keeps it open. Measure on the 5300M against the overlay's ceiling, with half-resolution and step-budget variants.
  - CPU: the gate test, and building the glow pass's draw list from the emissive table and the animated light set.
- **Open.**
  - Owner sign-off on the screen-space exception.
  - The glow target's resolution and format.
  - Whether the change term can be negative (a source dimmed below reference) in a blended HDR target, or needs a signed format.
  - The march's cost when the gate is permanently open.
  - The IP reading.

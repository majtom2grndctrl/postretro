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
- No memory-ledger row for probes: `sh_residency.rs` `ShAllocationKind` names none.

**Stage order (handoff "not verified", now grounded).** `pipeline.rs` runs `texture_stage::bake_world_materials` and `bake_remaining_textures` before `reflection_probes::bake_or_load_reflection_probes`. `ReflectionProbeInputs` already carries `texture_keys` and `prm_root`, so placement can read each material's baked `_p` channel before capture.

## Today's specular combine and the tint rule

Forward `fs_main` returns `base_color.rgb * total_light` with specular inside `total_light`; the mover pass does the same (`specular-texel-sharpness/research.md` §Tint). So a highlight without `_t` is tinted by raw albedo. The reflection takes the same colour as the highlight, `_t` or not. The tint rule and its storage belong to `specular-texel-sharpness`; at this read that brief's Decisions still say "A unused". Its research §Tint and the orchestrator's correction carry the rule.

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

## Not verified

- Overlay cost on the 5300M at glossy-heavy coverage.
- Probe counts on glossy-heavy maps (above).
- The exponent-to-blur estimates (above).
- Source and Black Ops gloss-driven envmap precedent: from memory, not re-checked.
- Compile-time cost of one probe per glossy face group at stress scale.

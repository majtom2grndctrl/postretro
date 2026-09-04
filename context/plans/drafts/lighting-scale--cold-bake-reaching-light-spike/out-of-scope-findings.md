# Out-of-scope findings — cold-bake reaching-light spike

prl-build opportunities surfaced while measuring the cold-bake reaching-light cull.
Outside that spike's scope; recorded here so the evidence survives. Reaching-light
distributions and wall-clock numbers cited below live in `findings.md` (same folder).

Provenance: **[confirmed]** read in source or reproduced this session ·
**[hypothesis]** inferred, not yet measured.

Ranked by expected value.

---

## 1. The SH bounce pays the full soft-visibility sample count, no adaptive escalation

**Mechanism [confirmed] in source. Win [hypothesis].**

`sample_radiance_rgb` (`sh_bake.rs`) casts every shadow ray at
`DEFAULT_AREA_SAMPLE_COUNT` (32), unconditionally. Its own comment: "SH bounce is
low-frequency, so it has no author-facing sample-count knob — it always uses the
default full-sample target." The cold lightmap already escalates adaptively — a spread
`SOFT_PROBE_SAMPLES` probe set, full count only in penumbras (`lightmap_bake.rs`). The
low-frequency SH bounce does not.

The SH bake is the dominant stage (~83% of compile; `findings.md`). After the shipped
range cull each hit point still casts ~6.8 in-range rays (mean, `stress-warren-lit`),
each 32 samples. A lower fixed count, or lightmap-style adaptive escalation, multiplies
directly against that — stacked on the cull, on the biggest stage.

Unmeasured: whether a low-frequency bounce holds visual quality at fewer samples. Same
shape as this spike — drop 32 → N, judge SH quality within tolerance.

## 2. The shadowmask atlas bake exceeds 16 GB at ship config

**[confirmed] — reproduced twice this session.**

Two cold bakes of `stress-warren-lit` at ship config were SIGKILL-ed (exit 137) during
the **Shadowmask atlas bake** stage, ~405 s in, after SH and lightmap completed. The
container has 16 GB.

**Repro:** `prl-build content/dev/maps/stress-warren-lit.map -o out.prl
--sh-probe-spacing 10.0 --lightmap-density 0.25 --no-cache` — SIGKILL at shadowmask.
`--lightmap-density 1.0` completes.

A 157-light map at 0.25 m/texel cannot finish a ship bake on a 16 GB box. Root
allocation unprofiled: `bake_shadowmask_atlas` builds a per-selected-light
`Vec<LightmapLayer>` over the full atlas, so peak likely scales lights × atlas area.
Robustness, not perf — profile peak memory before it blocks a real map.

## 3. The cold lightmap iterates every static light per texel

**[confirmed] in source and measured.**

`bake_face_chart` runs `for light in static_lights` per texel (`lightmap_bake.rs`). The
early-out gates the *shadow ray* (why the lightmap saw no cull win in `findings.md`),
but the O(texels × N) contribution loop is unculled: ~10 M texels × 157 lights, ~96% of
them far lights returning zero (mean in-range ~6.5/157). A per-leaf reaching-light list
would make it O(texels × reaching) — the lightmap's analogue of the SH probe affinity
cull. Lower priority: the stage is small (~13–37 s), so the ceiling is modest.

## 4. The shipped SH cull is range-only; cone and back-face rays still cast

**[confirmed] in source.**

The shipped early-out matches `falloff() == 0` exactly — distance vs. `falloff_range`
(`sh_bake.rs`). A spot light in range but aimed away, or a back-facing light,
contributes zero yet still casts a shadow ray. The lightmap already skips these:
`light_texel_contribution_and_visibility` returns before the ray when
`contribution.length_squared() <= 1e-12`, covering cone + back-face + range. Extending
the SH early-out to that fuller contribution test is provably zero-contribution and
byte-identical — an additive follow-up on the seam this spike opened. Marginal on top of
range; cheap.

---

## Already tracked (not new)

The 1.0 m default `--sh-probe-spacing` explodes on any large world — `stress-warren`'s
README insists on a coarse override or it bakes "millions of probes." This was the base-density
line's bake-time concern; the forward-predictor path that would have cut probe counts was measured
and closed — see `context/research/base-density-forward-predictor.md`.

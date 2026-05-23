# Research Notes

External references used while drafting:

- [Dynamic Diffuse Global Illumination with Ray-Traced Irradiance Fields](https://jcgt.org/published/0008/02/01/) — DDGI paper; establishes visibility-aware moment-based irradiance probe interpolation.
- [Real-Time Global Illumination using Precomputed Light Field Probes](https://research.nvidia.com/publication/2017-02_real-time-global-illumination-using-precomputed-light-field-probes) — background on probe visibility data and variance-shadow-map style visibility for static and dynamic objects.
- [Flax realtime GI documentation](https://docs.flaxengine.com/manual/graphics/lighting/gi/realtime.html) — implementation reference noting DDGI's low-resolution probe depth buffer and Chebyshev visibility weight.

Repo-local anchors:

- `context/plans/done/M9--probe-weight-correctness/` — shared manual 8-corner SH blend and validity/backface weighting.
- `context/plans/done/M9--probe-depth-visibility-atlas/` — baked `E[d]` / `E[d²]` depth moments in `ShVolume`.
- `context/lib/rendering_pipeline.md` — group-3 SH volume contract and shader module composition pattern.

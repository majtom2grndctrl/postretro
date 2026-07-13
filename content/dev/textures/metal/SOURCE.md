# Metal texture sources

`metal_brushed_012.png` is the `Color` map from [ambientCG Metal 012](https://ambientcg.com/view?id=Metal012), downloaded from the 1K PNG bundle on 2026-07-12. ambientCG releases the asset under [CC0 1.0](https://creativecommons.org/publicdomain/zero/1.0/).

`metal_brushed_012_s.png` and `metal_brushed_012_n.png` are Postretro authoring derivatives of that diffuse map, generated on 2026-07-12 with `tools/gen_specular.py` and `tools/gen_normal.py`. They are linear PNGs required by the world-material `_s` and `_n` slots; the original ambientCG NormalGL map is not redistributed here.

This is a reusable dev material collection, not an E17-specific asset. Map-facing name: `metal/metal_brushed_012`.

`metal_rough_046a.png` is the `Color` map from [ambientCG Metal 046 A](https://ambientcg.com/view?id=Metal046A), downloaded from the 1K PNG bundle on 2026-07-12. ambientCG releases the asset under [CC0 1.0](https://creativecommons.org/publicdomain/zero/1.0/).

`metal_rough_046a_n.png` is that bundle's authored `NormalGL` map, converted to Postretro's linear RGBA `_n` format. `metal_rough_046a_s.png` is an offline legacy-specular proxy derived from the bundle's `Roughness` and `Metalness` maps as `metalness × (1 − roughness)^5`. The fifth power intentionally suppresses highlights on rough regions: Postretro's `_s` map changes intensity only, while the source roughness map would normally also broaden a PBR highlight. The original Roughness and Metalness maps are not redistributed here.

This is a reusable dev material collection, not an E17-specific asset. Map-facing name: `metal/metal_rough_046a`.

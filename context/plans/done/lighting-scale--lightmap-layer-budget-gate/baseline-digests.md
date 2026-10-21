# Pre-change cold lighting section digests

Captured at `7e7614d36ea2741cbcacb28d917b2f88b29885be`, before production changes.
Command: `cargo test -p postretro-level-compiler --bin prl-build layer_budget_ -- --ignored --nocapture`.
Result: two focused tests passed, 3.38 s bake time after dependency compilation.

The six maps use default 0.04 m/texel, four soft-shadow samples, compressed irradiance, and a fresh empty stage cache. Each is baked once cold and then rebuilt warm in the same loop; id 22, id 42, and the id-22 cache entry are checked together. Maps with no static blocks return an empty id 22 and intentionally write no section memo. The four-layer cut uses the existing 24 m floor, 0.25 m/texel, and 64-texel pool edge. Synthetic ownership pins use prepared in-memory partitions.

The exact values below are frozen assertions in the regression harnesses. The id-42 digest includes the owner table. `report_digest` in the four-layer cut hashes the memo report prefix; synthetic pins hash the printed report representation.

```text
PIN_BASELINE one-layer id42=5d4b0a3657c6dbbd327ff4b0d83d5b1a7daf73e496167ecbc3192ace7389f856 owners=Some([[0, 65535, 65535, 65535]]) report=OwnerReport { peak_face_demand: 1, drops: [] } report_digest=5870c73433323b29c315e4b0abc0542c5d472273ea30e0f0f5addae00e3b3800
PIN_BASELINE three-layers id42=2f425ea1e66b10a17e5daddedccec005ed06098c9a80655b9fcc3deefdb7f007 owners=Some([[0, 65535, 65535, 65535], [0, 65535, 65535, 65535], [0, 65535, 65535, 65535]]) report=OwnerReport { peak_face_demand: 1, drops: [] } report_digest=5870c73433323b29c315e4b0abc0542c5d472273ea30e0f0f5addae00e3b3800
PIN_BASELINE dark-middle-layer id42=c706c93112c32431d6b806171a5a202398999a4753e02d2cb04cfe6dac1479ee owners=Some([[0, 65535, 65535, 65535], [65535, 65535, 65535, 65535], [0, 65535, 65535, 65535]]) report=OwnerReport { peak_face_demand: 1, drops: [] } report_digest=5870c73433323b29c315e4b0abc0542c5d472273ea30e0f0f5addae00e3b3800
PIN_BASELINE cut-same-layer id42=702f29d09be12b267380de12199b4384bc4a18eec31c69244f63c5342b0af0ec owners=Some([[1, 3, 65535, 65535], [1, 3, 65535, 65535], [3, 65535, 65535, 65535]]) report=OwnerReport { peak_face_demand: 3, drops: [OwnerDrop { site: CutFace { parent: 9, first_face: 0, last_face: 1 }, spec_index: 4, lit_texels: 48 }] } report_digest=0f272f137ea21210d2692c8680b9eb0db62508e8e9573a3e41a339c730ded9cb
PIN_BASELINE cut-first-to-last id42=aaa684851e03d6d07fff775393bd73366a3876ec07884b75bde196ae2e66b59b owners=Some([[1, 3, 65535, 65535], [1, 3, 65535, 65535], [3, 65535, 65535, 65535]]) report=OwnerReport { peak_face_demand: 3, drops: [OwnerDrop { site: CutFace { parent: 9, first_face: 0, last_face: 1 }, spec_index: 4, lit_texels: 48 }] } report_digest=0f272f137ea21210d2692c8680b9eb0db62508e8e9573a3e41a339c730ded9cb
PIN_BASELINE cut-owners-baked-nothing id42=af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262 owners=None report=OwnerReport { peak_face_demand: 0, drops: [] } report_digest=b341720f2ad435043532219c2bb700e304301d7cdabf20839afb3db9fd2a3600
BASELINE gate-heavily-lit layers=1 id22=85ed0cad0b9c3f2fbbf106fc6e138d8811879f3efd7a8b70bbcf2fe0423e7988 id42=b6084630583ea0edb0e3f3d505aee4071ecd06886f911a217272a1998284540e
BASELINE soft_shadow_test layers=1 id22=aea7bcf1140d6b7197cf41237e670a6820144e2e38e2b0d88975a79ba7893818 id42=6402ba73f82a1e86271987e4152f9c1dd403c84e78080770b39361d3d68e4b90
BASELINE test_animated_weight_maps_cap layers=1 id22=fe87dcb8396cfeee8452cce791c76eb38bec190c3741746950c7fb629c8a114d id42=af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262
BASELINE test_animated_weight_maps_mixed layers=1 id22=40961fd54f94bfb0eb04a9c407b756b33604f2cc92478f1b4cac082587ff2a18 id42=862f163f768233721ee44d87ccea4676f69a37ad5159e5d70484b5122982bfcd
BASELINE test_animated_weight_maps_occluded layers=1 id22=fe87dcb8396cfeee8452cce791c76eb38bec190c3741746950c7fb629c8a114d id42=af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262
BASELINE test_animated_weight_maps_single layers=1 id22=fe87dcb8396cfeee8452cce791c76eb38bec190c3741746950c7fb629c8a114d id42=af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262
BASELINE four-layer-cut layers=4 id22=a6faca302b8c39846d56eddc3924eadbc5fc082b55fbddb976ddd25d9c510b12 id42=f1a1d6e5d9e186c654a130d50ef52ce66e3d391a99022ee3f1f3957a4535d0d3 owners=[[0, 1, 2, 3], [0, 1, 2, 3], [0, 1, 2, 3], [0, 1, 2, 3]] report=[5, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 3, 0, 0, 0, 4, 0, 0, 0, 200, 0, 0, 0] report_digest=43f79abd54f2ea42e36dc0f3d36ef68fe865d0829ea9cd4c392d6871bc3e6f05
```

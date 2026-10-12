# bc7-color-textures

Brief · compact · reads: `context/lib/build_pipeline.md` §Baked texture mips, §Build Cache (Determinism invariant) · `context/lib/resource_management.md` §8.3 · `context/lib/rendering_pipeline.md` §7.3 · read at 784af6536

## Problem
Developer-raised, anticipated need toward the 2 GB VRAM target (`index.md` §1.1). The `.prm` color slots (diffuse and emissive) still bake as 32 bpp `Rgba8UnormSrgb`, while normals and HDR atlases are block-compressed. Material textures load whole for the level's lifetime, because streaming is a recorded non-goal (`resource_management.md`). Shrinking each texture is the only lever on their VRAM. The cost sits in hi-res color textures and grows with every unique texture a mod ships. When this is done, every baked color slot that BC7 represents faithfully ships as 8 bpp `Bc7RgbaUnormSrgb`, byte-identical on every platform. Any other slot still loads as uncompressed, and the bake reports which slots stayed uncompressed and why.

## Decisions
- **New format, color slots only.** `.prm` gains a BC7 tag mapping to `Bc7RgbaUnormSrgb`. Diffuse and emissive accept it, and every other slot rejects it, mirroring BC5's normal-only rule (`plans/done/prm-bc5-normals`).
- **Reverses a recorded non-goal.** `prm-bc5-normals` and `plans/roadmap-q1-archive.md` §Epic 8 kept BC7 off diffuse, assuming it would damage pixel art. A per-texture fidelity gate replaces that blanket assumption. BC5's "one filter, one pipeline" ends: a slot's stored format now depends on its content.
- **Every color producer in one pass.** World diffuse and emissive, model diffuse, and baked sprite and flipbook layers share one color chain and switch together. Sprite alpha weights the additive blend, so the encoder carries alpha.
- **Sprite path parity.** The baked-sprite plan admits BC7 diffuse and the RGBA8 normal fallback; today it accepts only RGBA8 diffuse and BC5 normals. A sprite collection checks its cache before building any chain.
- **In-tree, pure-Rust, deterministic encoder.** No FFI, no `unsafe`, no C or ISPC toolchain in the SDK bundle. Output is byte-identical across platforms, so BC7 stays inside the Determinism invariant, with no carve-out like BC6H's.
- **One format-decision chokepoint per slot.** One named function picks each slot's stored format and records why. Every producer calls it, and a future authored override would resolve behind it. Its rules, in order:
  - **Size rule, color and normal.** Block-encode only when the base width and height are multiples of 4, which wgpu requires. A failing color slot stays `Rgba8UnormSrgb`. A failing normal slot stays `Rgba8Unorm`, including below 4×4, where today the slot drops to a placeholder. This cures a latent defect: an unaligned normal map bakes BC5 that wgpu rejects at texture creation.
  - **Fidelity gate, color only.** Decode the BC7 output and judge the texture by its worst block, not its mean. If any block exceeds the threshold, the slot stays `Rgba8UnormSrgb`. A flipbook is judged as one slot. The threshold is one engine constant, calibrated once and pinned by a test.
- **No authored override, no quality knob.** The gate protects unreviewed content at zero authoring cost. An authored setting is a modder-facing one-way door with no consumer yet.
- **The decision is visible.** The bake byte summary reports bytes per stored format and lists each uncompressed color slot with its reason (size or fidelity) and byte cost, identically on warm and cold builds.
- **`STAGE_VERSION` bumps.** The bundle hash covers source bytes, not the transform (`build_pipeline.md` §Baked texture mips), so without a bump stale RGBA8 sidecars stay valid cache hits. No encoder epoch goes into the key.
- **Bake cost is a budget.** Bytes do not depend on worker count, and a warm build runs no BC7 encode on any producer.
- **Non-goals:**
  - Reaching 2 GB: this brief contributes to the target. Material streaming is a separate brief.
  - Specular and surface-map slots: already 1–2 bpp, the same reasoning that kept BC5 off them.
  - UI and splash images: not `.prm`.
  - A new adapter gate: `TEXTURE_COMPRESSION_BC` is already required and covers BC7. Only its error text changes.

## Acceptance
### Automated
- [ ] Round trip: BC7 encode→decode stays within a stated error bound on smooth gradients, hard-edged 2–4-colour pixel art and alpha ramps. Flat blocks (RGB and alpha) round-trip exactly.
- [ ] Determinism: a golden-bytes test pins BC7 output for fixed inputs, and baking at 1 worker and at N workers yields identical `.prm` bytes.
- [ ] Format gate, both sides: diffuse and emissive accept the BC7 tag; specular and normal reject it on read and on write.
- [ ] Size rule, both sides, color and normal: base 8×8, 12×12 and 64×32 block-encode. Base 6×6, 10×10, 13×16 and 2×2 stay uncompressed (`Rgba8UnormSrgb` / `Rgba8Unorm`) and neither error nor degrade to a placeholder.
- [ ] Fidelity gate, both sides:
  - A block-aligned photo-style gradient is stored BC7.
  - A block-aligned fixture with one block of 16 pseudo-random, unrelated colours stays `Rgba8UnormSrgb`, reason "fidelity".
  - A texture clean except for one such block also stays uncompressed: the worst block decides.
- [ ] The calibrated threshold constant is pinned by a test.
- [ ] Byte summary: a mixed fixture set reports bytes per stored format and lists each uncompressed color slot with its reason and byte cost.
- [ ] Payload: a block-aligned color slot's payload equals the per-level block count × 16 down to the block floor. Campaign-test's color-slot total is ≤ 30% of the RGBA8 baseline in the byte summary. That half is a cold-bake measurement, ignored by default.
- [ ] A 12×12 color slot bakes 2 levels with a payload of 9 + 4 blocks at 16 bytes each and reads back cleanly. A 36×20 slot does the same with edge-padded lower levels. A uniform fixture stays BC7 whatever its padding. (P2)
- [ ] Flipbook with mixed layers: one layer over the threshold keeps every layer uncompressed, listed once with reason "fidelity"; all layers passing stores every layer BC7. (P1)
- [ ] One sidecar with BC7 diffuse and uncompressed emissive, and the reverse, round-trips. Upload planning gives each slot its own format and level count, and the shared LOD clamp follows the deepest slot. (P3)
- [ ] A model texture baked through the tool and through the compiler yields byte-identical sidecars. A stale-version sidecar at a model's address is rebaked, not kept. (P4)
- [ ] A warm, fully cached build's byte summary equals the cold build's: same bytes per format, same uncompressed list and reasons, and a texture reached under two names is listed once. (P5)
- [ ] A bundle with no color slot runs no BC7 encode. (P6)
- [ ] Cache: a cache baked before the version bump is rejected and rebaked. A second build is a full cache hit that runs no chain build or encode for world, model or sprite producers.
- [ ] Baked-sprite plan: a BC7 sprite sidecar plans as a baked array with its own layer and level counts. So does a sprite with an unaligned normal stored `Rgba8Unorm`. A sidecar that also carries emissive still declines to the PNG fallback.
- [ ] GPU-free upload planning accepts a BC7 color slot and an unaligned RGBA8 fallback slot, each with its own format.

### Manual
- [ ] Gate calibration: A/B screenshot pairs in the PR, gate applied vs uncompressed, close up so one texel covers many pixels. They cover 32–64 px pixel art, the 128–256² weapon textures in first-person view, a skinned-model texture (nearest magnification) and a 1024² photo set. Visible banding, hard-edge colour shift or fringing on anything stored BC7 means the threshold must tighten. The PR records the photo-set share stored BC7 under each metric candidate tried. The plan fails if every candidate leaves most of the photo set uncompressed at a pixel-art-safe threshold.
- [ ] Additive sprites against a dark backdrop show no coverage halo or block edge, BC7 vs uncompressed.
- [ ] Emissive under bloom shows no visible change from uncompressed.
- [ ] Cold texture-stage bake time on campaign-test and the full dev content set, before and after. The PR records machine class, worker count and cache mode. Warm-build texture-stage time does not rise.
- [ ] A 6×6 normal map loads and renders (it fails texture creation on `main`).

## Path
- Seams:
  - format: `PrmFormat`, `format_allowed_for_slot`, `expected_payload_bytes`, `bc5_level_count`, `mip_level_bytes`
  - upload: `prm_format_to_wgpu`, `upload_bytes_per_pixel`, which already has block row math for D2 and D2Array
  - chains: `build_diffuse_chain_impl`, `build_normal_bc5_chain_impl`; a linear `Rgba8Unorm` normal chain is new
  - sprites: `bake_sprite_collection` (cache check after chain build today), `plan_baked_sprite_array`
  - adapter error text: `request_renderer_device`
- Encoder: beside `bc5.rs` / `bc6h.rs`. Modes 6, 5 and 1 with a partition search and least-squares endpoint refinement. Integer error metric, or f32 in a fixed operation order. Parallelize encode per texture and record serially; pin which error surfaces first.
- Gate metric candidates and the bias argument behind them are in `research.md`. If too little of the photo set passes, levers in order: a better metric, then an exhaustive re-search of failing blocks, then more modes.
- Rivals not taken: `intel_tex_2` and other FFI encoders; an authored per-texture setting; BC1 for opaque diffuse; a minimum-size cutoff. `research.md` gives the reasons.
- First slice: encoder, decoder and metric with round-trip and golden tests. Then the close-up A/B on one 64 px pixel-art texture, one 256² weapon texture and one 1024² texture, before any wiring.
- `texture_mips.rs` is far past ~800 lines. Split it first, preserving behavior, in its own commit.
- At landing, `context/lib` gets: BC7 on color slots, the chokepoint and its rules (`build_pipeline.md` §Baked texture mips), and BC7 named as non-exempt in the Determinism invariant.

## Open questions
- Gate metric, threshold value and round-trip error bounds — **delegated**: set from the first slice and reported in the plan of record.
- Cold-bake time ceiling — **delegated**: measure and report; flag anything over 2× the RGBA8 cold baseline for the owner.

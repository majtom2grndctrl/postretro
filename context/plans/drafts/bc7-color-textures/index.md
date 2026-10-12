# bc7-color-textures

Brief · compact · reads: `context/lib/build_pipeline.md` §Baked texture mips, §Build Cache (Determinism invariant) · `context/lib/resource_management.md` §8.3 · `context/lib/rendering_pipeline.md` §7.3 · read at 784af6536

## Problem
Developer-raised, anticipated need, toward a 2 GB VRAM target. The `.prm` color slots (diffuse and emissive) still bake as 32 bpp `Rgba8UnormSrgb`, while normals (BC5) and HDR atlases (BC6H) are block-compressed. Material textures load whole for the level's lifetime, because material streaming is a recorded non-goal (`resource_management.md`). Shrinking each texture is therefore the only lever on their VRAM. Dev content's baked materials already total about 279 MB, mostly large photo-style color textures (1024² sets), and the cost grows with every unique texture a mod ships. The win goes to mods and content that ship hi-res textures. The engine's own low-res pixel art saves little, and the gate below exists to keep it exact. When this is done, every baked color slot that BC7 represents faithfully ships as 8 bpp `Bc7RgbaUnormSrgb`. Its bytes are identical on every platform. A texture BC7 cannot represent, or cannot represent faithfully, still loads as uncompressed, and the bake reports which textures stayed uncompressed and why. The same size rule cures a latent BC5 defect: a normal map whose base size is not a multiple of 4 bakes BC5 at its logical size, which wgpu rejects at texture creation.

## Decisions
- **New format, color slots only.** `.prm` gains a BC7 format tag mapping to `Bc7RgbaUnormSrgb`. Diffuse and emissive accept it, and every other slot rejects it, which mirrors BC5's normal-only rule (`plans/done/prm-bc5-normals`). Hardware sRGB decode means no shader change, as `plans/done/emissive-surfaces-bloom` §Wire format anticipated. Unknown-tag readers already reject cleanly.
- **Reverses a recorded non-goal.** `prm-bc5-normals` and `plans/roadmap-q1-archive.md` §Epic 8 ruled BC7 on diffuse out of scope because of an assumed conflict with hard-edged pixel art. This brief replaces that blanket assumption with a per-texture fidelity gate (below). It also ends BC5's "one filter, one pipeline": a slot's stored format now depends on its content. Content now includes 1024² photo-style sets, where nearly all of the win is.
- **Every color producer in one pass.** World diffuse and emissive, model diffuse, and baked sprite and flipbook layers all build through one shared color-chain function, so all of them switch at once. The baked-sprite plan's diffuse-format gate widens to admit BC7, otherwise BC7 sprites would silently fall back to PNG. Sprite alpha weights the additive blend, so the encoder must carry alpha, not just RGB.
- **In-tree, pure-Rust, deterministic encoder.** It lives beside `bc5.rs` and `bc6h.rs` in the level compiler, with no FFI, no `unsafe`, and no C or ISPC toolchain in the SDK bundle. Output is byte-identical across platforms, so the Determinism invariant holds with no new exemption: BC7 does not join BC6H's lossy carve-out. Rival FFI encoders give up the SDK build or determinism (see `research.md`).
- **One format-decision chokepoint per slot.** A single named function picks each slot's stored format and records why. Every producer calls it, and a future authored override would resolve behind it additively. It applies two automatic, content-derived rules in order:
  - **Size rule, shared with BC5.** A slot is block-encoded only if its base width and height are both multiples of 4 (wgpu rejects other sizes; smaller than 4×4 falls under the same rule). A failing color slot stays `Rgba8UnormSrgb`. A failing normal slot stays `Rgba8Unorm`, which remains readable, and the shader reconstructs z from RG either way.
  - **Fidelity gate, color slots only.** The baker decodes its own BC7 output and measures error per block, judging the texture by its worst block, not its mean, because pixel art fails locally. Blocks over the threshold first get an exhaustive re-search. If any block is still over, the slot stays `Rgba8UnormSrgb`. Normal slots get no gate, matching BC5 today. The threshold is one engine constant, calibrated once from the A/B fixtures and pinned by a test. Determinism matters here beyond the bytes: a machine-dependent encoder would make the BC7-or-RGBA8 choice itself vary between machines.
- **No authored override and no quality knob in this brief.** The gate protects content nobody has reviewed, at zero authoring cost. An authored per-texture setting is a modder-facing one-way door with no consumer yet (the warrant matches `prm-bc5-normals`' no-override decision). The chokepoint is the seam a later one plugs into.
- **The decision is visible.** The bake byte summary reports bytes per stored format, and lists the textures that stayed uncompressed with their reason (size or fidelity) and their byte cost. That is the meter for the 2 GB push.
- **`STAGE_VERSION` bumps.** The bundle hash covers source bytes, not the bake transform (`build_pipeline.md` §Baked texture mips). Without the bump, stale RGBA8 sidecars stay valid cache hits and the win never appears. Later encoder changes that alter bytes bump it again. No encoder epoch goes into the key.
- **Bake cost is a budget.** Color encode runs per texture in parallel, and per-texture output stays order-independent, so bytes do not depend on worker count. A warm build does no BC7 work.
- **Ships in one pass.** Every in-scope slot lands together, with no emissive-first pilot: they share one chain, so staging would only add cost. The manual A/B calibrates the gate rather than vetoing the plan. The plan fails only if the calibrated gate leaves the photo-style sets uncompressed, which would remove the win.
- **Non-goals:**
  - Reaching 2 GB: this brief contributes to the target but does not own it. Material streaming is a separate brief.
  - Specular and surface-map slots: they are 1–2 bpp already, and the BC5 reasoning applies.
  - HDR atlases.
  - UI and splash images: these are not `.prm`.
  - Runtime or GPU-side encoding.
  - A new adapter gate: `TEXTURE_COMPRESSION_BC` is already required and covers BC7. Its error text gets updated.

## Acceptance
### Automated
- [ ] Color-slot round trip: BC7 encode→decode stays within a stated error bound on smooth gradients, hard-edged 2–4-colour pixel art, and alpha ramps. Flat blocks (RGB and alpha) round-trip exactly.
- [ ] Determinism: a golden-bytes test pins BC7 output for fixed inputs, and the same inputs baked at 1 worker and at N workers produce identical `.prm` bytes.
- [ ] Format gate, both sides: diffuse and emissive accept the BC7 tag; specular and normal reject it with a slot-format error on read and on write.
- [ ] Size rule, both sides, for color and normal slots: base 8×8, 12×12 and 64×32 encode block-compressed. Base 6×6, 10×10, 13×16 and 2×2 stay uncompressed (`Rgba8UnormSrgb` / `Rgba8Unorm`), and neither error nor degrade to a placeholder.
- [ ] Fidelity gate, both sides:
  - A block-aligned photo-style gradient fixture is stored BC7.
  - A block-aligned fixture with one block of 16 pseudo-random, unrelated colours stays `Rgba8UnormSrgb`, with reason "fidelity".
  - A texture that is clean everywhere except one such block also stays uncompressed: the worst block decides, not the mean.
- [ ] Re-search: a fixture block over the threshold on the first search but under it after the exhaustive re-search is stored BC7.
- [ ] The calibrated threshold constant is pinned by a test, so changing it fails a test.
- [ ] Byte summary: a mixed fixture set reports bytes per stored format and lists each uncompressed color slot with its reason (size or fidelity) and byte cost.
- [ ] Payload size: for a block-aligned color slot, payload bytes equal the per-level block count × 16 down to the block floor, and the bake byte summary shows campaign-test's color-slot total at ≤ 30% of the RGBA8 baseline. The campaign-test half is a cold-bake measurement and is ignored by default.
- [ ] A 12×12 color slot bakes 2 levels with a payload of 9 + 4 blocks at 16 bytes each, and reads back cleanly. A 36×20 slot does the same with edge-padded lower levels. A uniform fixture stays BC7 whatever its padding: padding texels never decide the gate. (P2)
- [ ] Flipbook with mixed layers: a sprite collection with one layer over the fidelity threshold is stored uncompressed for every layer, and the byte summary lists it once with reason "fidelity". A collection whose layers all pass is stored BC7 for every layer. (P1)
- [ ] One sidecar with a BC7 diffuse and an uncompressed emissive, and the reverse, round-trips through write and read. Upload planning gives each slot its own format and level count, and the shared LOD clamp follows the deepest slot. (P3)
- [ ] A model texture baked through the tool and through the compiler yields byte-identical sidecars. A stale-version sidecar at a model's address is rebaked, not kept. (P4)
- [ ] The byte summary from a warm, fully cached build equals the cold build's: same bytes per format, same uncompressed list with the same reasons, and a texture reached under two names is listed once. (P5)
- [ ] A bundle with no color slot runs no BC7 encode. (P6)
- [ ] Cache: after the version bump, a cache baked before it is rejected and rebaked, and a second build is a full cache hit that runs no encode.
- [ ] Baked-sprite plan: a BC7 sprite sidecar plans as a baked array with the layer count and level count it carries, not a PNG fallback. A sidecar that also carries emissive still declines to the PNG fallback.
- [ ] GPU-free upload planning accepts a BC7 color slot and a non-multiple-of-4 RGBA8 fallback slot, and plans each with its own format.

### Manual
- [ ] Gate calibration: A/B screenshot pairs in the PR, as shipped (gate applied) vs uncompressed, taken close up so single texels cover many pixels. Cover the 32–64 px pixel-art sets, the 128–256² weapon textures in first-person view (nearest magnification), and a 1024² photo set. On anything the gate stored as BC7, visible block banding, colour shifts on hard edges, or fringing fails, and the fix is a tighter threshold. The PR records the photo-set share stored BC7 under each metric candidate tried. If every candidate leaves most of the photo set uncompressed at a pixel-art-safe threshold, the plan has failed.
- [ ] Sprite and flipbook alpha: additive sprites against a dark backdrop show no visible coverage halo or block edge, BC7 vs uncompressed.
- [ ] Emissive under bloom shows no visible change from uncompressed.
- [ ] Cold texture-stage bake time on campaign-test and the full dev content set, before and after, with machine class, worker count and cache mode recorded in the PR. Warm-build texture-stage time is unchanged.
- [ ] A 6×6 normal map loads and renders after the fix (it fails texture creation on `main`).

## Path
- Seams:
  - format: `PrmFormat`, `format_allowed_for_slot`, `expected_payload_bytes`, `bc5_level_count`, `mip_level_bytes`
  - upload: `prm_format_to_wgpu`, `upload_bytes_per_pixel`, which already has block row math for D2 and D2Array
  - chains: `build_diffuse_chain_impl`, `build_normal_bc5_chain_impl`
  - sprites: `plan_baked_sprite_array`
  - adapter error text: `request_renderer_device`
- Encoder shape: modes 6 (RGBA, one subset), 5 (rotated alpha) and 1 (two subsets with a partition search), plus least-squares endpoint refinement. Add more modes only if the A/B demands them. Integer error metric, or f32 in a fixed operation order. The strongest rival is `intel_tex_2`, rejected under Decisions.
- Gate metric: worst-block integer score in sRGB-encoded space, with alpha weighted. An absolute metric is biased against the win. A 1024² texture has hundreds of times more blocks than a small one, so more chances for one bad block. Its noisy blocks (grass, bark, concrete) score badly while noise hides their error, and the same error in a 3-colour pixel-art block is plainly visible. Candidates:
  - absolute max per-channel error, or block SSE
  - error normalised by the block's own contrast or variance
  - a deterministic distinct-colour class per block: a strict bound for low-palette blocks, a looser one for noisy blocks
  The first slice reports the photo-set pass share under each candidate at a pixel-art-safe threshold before picking.
- The strongest policy rival is an authored per-texture compression setting, as in Unreal or Godot. Deferred under Decisions: it needs a consumer first, and the chokepoint keeps it additive.
- BC1 for opaque world diffuse would halve BC7's footprint again (world passes ignore alpha). It isn't chosen: its two 565 endpoints and 4 colours per block fail far more blocks, so the gate would reject much more. Sprites need alpha, so BC7 would have to ship as well. BC1 could later become a second format behind the same chokepoint, tried first on opaque slots.
- Complementary, not rival: a runtime top-mip drop as a low-VRAM player option (it costs crispness, so a separate brief).
- First slice: encoder, decode, and gate metric with round-trip and golden tests. Then the close-up A/B on one 64 px pixel-art texture, one 256² weapon texture and one 1024² texture, before any wiring. This calibrates the threshold and shows whether the photo set clears it.
- `texture_mips.rs` is past 3,400 lines. Split it first, preserving behavior, in its own commit, and put the BC7 chain in a sibling module.
- At landing, `context/lib` gets: BC7 on color slots, the format-decision chokepoint and its two rules (`build_pipeline.md` §Baked texture mips), and the determinism invariant naming BC7 as non-exempt.

## Open questions
- Gate metric and threshold value, plus the round-trip error bound per fixture class — **delegated**: the executor sets them from the first slice and reports them in the plan of record.
- Cold-bake time ceiling — **delegated**: measure and report. The owner judges the number at review, and the executor flags anything over 2× the RGBA8 baseline cold time.

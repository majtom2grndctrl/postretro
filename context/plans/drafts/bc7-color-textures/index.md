# BC7 on Color Textures (diffuse + emissive)

> **Status: stub.** Sketch-level scoping, not yet task-decomposed. Gated on an
> aesthetic A/B against real art (see Verification) — do not promote until that gate
> is planned and the dependency below has landed.

## Goal

Encode the sRGB **color** slots (diffuse and emissive) of `.prm` textures as **BC7**
(`Bc7RgbaUnormSrgb`) instead of `Rgba8UnormSrgb`, cutting color-texture VRAM and disk
to ~25% (BC7 = 8 bpp vs Rgba8's 32 bpp). This completes the block-compression story
already shipped for normals (BC5, `prm-bc5-normals`) and HDR irradiance (BC6H) — the
two color slots are the last uncompressed high-footprint assets. The payoff scales
with the project's full-potential workload: large maps and a modder ecosystem shipping
many unique textures, where color textures dominate VRAM.

## Why this is a separate epic (and depends on emissive, not the reverse)

BC7 is a **horizontal encoding layer** under every sRGB color slot — orthogonal to
what any slot *means*. It is deliberately **not** a prerequisite of emissive+bloom:

- Emissive+bloom ships a product pillar (neon glow); gating it behind an
  aesthetic-veto, A/B-dependent encoding decision would put the wrong risk on the
  critical path.
- The format seam is **additive and cheap** — BC5-normals proved the pattern (a new
  `format_tag`, backward-compatible, content-hash-addressed so caches regenerate).
  Emissive shipping as `format_tag = 0` and later gaining a BC7 tag is the same
  additive move, not a rewrite.
- **Depends on `emissive-surfaces-bloom`** so the color-slot vocabulary is settled
  (diffuse + emissive both exist) and BC7 targets the **full color-slot set in one
  pass** — rather than covering diffuse alone and needing an emissive retrofit. The
  emissive spec's Wire-format section already pins the enabling constraint: the
  emissive slot's format flows through the shared `PrmFormat → wgpu::TextureFormat`
  match, with no `slot == emissive ⇒ Rgba8UnormSrgb` assumption to unwind here.

## Scope

### In scope
- New `PrmFormat` arm + `format_tag` (BC5 is tag 3 → BC7 is tag 4), mapping to
  `wgpu::TextureFormat::Bc7RgbaUnormSrgb`, permitted on the **diffuse and emissive
  (color) slots** — the mirror of the BC5 "normal-slot-only" restriction.
- A BC7 encoder in `postretro-level-compiler`, emitting a BC7 mip chain for the color
  slots with the same block-size-floor tail-out the BC5 chain uses (levels stop where
  both dims are ≥ 4 px).
- Runtime GPU upload path: detect the BC7 tag on a color slot, request
  `Bc7RgbaUnormSrgb`, upload block payloads per level. Sampler unchanged (BC7 decodes
  to the same sRGB color the shader already samples — **no shader change**).
- The reserved-adapter feature `TEXTURE_COMPRESSION_BC` is **already a hard engine
  requirement** (`prm-bc5-normals`), so BC7 adds **no new platform gate**.

### Out of scope
- **Specular (R8Unorm).** Single-channel, already 1 byte/texel; BC4 gain is marginal
  (same reasoning that kept BC5 off specular in `prm-bc5-normals`).
- **Normals / HDR atlases.** Already BC5 / BC6H.
- **A per-texture or per-material "disable BC7" override.** One filter, one pipeline
  (matches the BC5 decision). Revisit only if specific assets prove they need it.
- **Model textures** unless/until the model path adopts the `.prm` color slots — model
  albedo compression is its own decision.
- **PBR channels.** Non-goal, unchanged.

## Dependency & sequencing

- **Hard dependency:** `emissive-surfaces-bloom` promoted and landed (color-slot set
  settled; the shared-`format_tag` plumbing in place).
- **Soft dependency:** whichever of {emissive, this} runs first carries the
  `texture_mips.rs` split hygiene; if emissive's Task 5 already split it, BC7's baker
  arm lands on the split file.

## Tasks (sketch — decompose at draft time)

1. **`.prm` format tag + validation.** Add the BC7 `PrmFormat` arm; permit it on the
   color slots (diffuse, emissive), reject it on specular/normal with a clear error
   (mirror of the BC5 slot restriction). `expected_payload_bytes` BC7 branch (16-byte
   blocks, like BC5/BC6H). Do **not** bump `STAGE_VERSION` if avoidable — additive,
   content-hash-addressed (confirm against the reader's version-lockstep rule).
2. **BC7 encoder.** The heavy piece — see Risks. Either an in-tree encoder (as `bc5.rs`
   / `bc6h.rs` are) or a vetted dependency. Must be **deterministic** for the build
   cache (see Risks). Encode the color slot's linear-filtered → sRGB-re-encoded chain.
3. **Baker wiring.** Route the diffuse and emissive color chains through the BC7
   encoder behind the format decision; extend bundle-hash / filename-key /
   cache-validation to the new tag.
4. **Runtime upload.** Detect the BC7 tag on a color slot, request `Bc7RgbaUnormSrgb`,
   upload block payloads. No shader change.
5. **A/B verification + docs.** The aesthetic gate (below); update
   `resource_management.md` / `rendering_pipeline.md` compression notes; flip the
   `prm-bc5-normals` "BC7 on diffuse — out of scope" line to point here.

## Verification — the aesthetic A/B gate

This is the load-bearing gate, reused from `prm-bc5-normals`' A/B-screenshot method:

- **Automated (GPU-free):** BC7 round-trip encode/decode error bounds; `.prm`
  color-slot payloads ≤ ~25% of the Rgba8 baseline on the campaign-test scene;
  slot-restriction rejection tests (BC7-on-specular/normal errors).
- **Manual GPU gate (the veto):** A/B screenshots of **real low-res, full-palette art
  at display density**, BC7 vs uncompressed, on the intended sampler (nearest vs
  bilinear). The aesthetic pillar holds veto — ship BC7 only if it does not visibly
  soften the intended look.
  - **Quality note (from scoping):** low-res + full-palette leans favorable — full
    color means smooth gradients (BC7's strength), not the hard palette edges that
    fringe. The real risk is **magnification**: low-res-shown-large blows up any
    per-block error on screen. So the A/B must be at true display scale, not a
    texture-space diff.
  - **Emissive is the low-risk pilot:** the bloom pass low-passes the emissive
    contribution, softening any BC7 block error. If landing BC7 incrementally, prove
    it on the emissive slot first, then diffuse.

## Risks / open questions

- **BC7 encoder cost & determinism.** BC7 is far heavier to encode than BC5 (8 modes,
  partition search). A quality encoder (e.g. an ISPC-texcomp-class algorithm) is a
  large in-tree effort; a dependency must be **deterministic across platforms** or it
  breaks the build cache — note `build_pipeline.md`'s existing "determinism / BC6H
  exemption" precedent, which may need a BC7 analogue. Resolve encoder choice before
  task decomposition.
- **Bake-time budget.** BC7 encode is slow; it lands on the level-compiler critical
  path. May need the same incremental/cached treatment the lightmap bake got.
- **Does the final art actually stay low-res + full-palette?** The whole aesthetic
  gate assumes it. If art direction shifts (higher-res detail, or toward palettized
  pixel art), re-run the A/B — the answer can move.
- **Increment vs. big-bang:** emissive-slot-only BC7 first (bloom-hidden, low risk)
  vs. diffuse+emissive together. Decide at draft time.

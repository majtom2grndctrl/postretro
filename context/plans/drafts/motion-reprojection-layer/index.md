# motion-reprojection-layer

Brief · session decides · no epic yet · reads: `context/lib/rendering_pipeline.md` (fog composite, §7.8, §12) · read at 5be7ca580

> **Seed.** Raised by the owner when E23 U1's GPU frame limiter was withdrawn (2026-09-28). Nothing below is decided. Draft only when a first consumer earns it; run `/draft-session` first.

## Problem

An anticipated need. The renderer keeps no motion information between frames: no previous-frame camera matrix and no per-pixel velocity. Volumetric fog has "no temporal history, reprojection, or resolve pass" (`rendering_pipeline.md`). So any temporal technique would have to build its own reprojection: temporal accumulation (fewer fog samples per frame, a direct laptop performance win), motion blur, TAA or temporal upscaling, or a motion-aware flash limiter. When this is done, one renderer-owned facility answers "where was this pixel last frame", and each consumer is thin.

## Known shape

- Camera motion: the previous frame's view-projection plus the scene depth buffer (already sampled by the fog pass). No extra render target.
- Object motion (optional, second step): a velocity target that geometry passes write from each object's previous transform. About 8 MB at 1080p (RG16F).

## Candidate consumers

- Temporal fog accumulation. The likely first consumer, because it pays for itself in GPU time.
- Motion blur. An optional look, off under reduce motion.
- TAA or temporal upscaling. May conflict with the crisp-pixel look.
- A motion-aware flash limiter. The U1 motion test in git is its gate. See `drafts/E23--photosensitivity-source-floor`.

## Questions for the session

- Which consumer comes first, and does it need object motion or only camera motion?
- Is history kept at full or reduced resolution for each consumer, and who owns the ping-pong targets?

//! Output-preserving SH coarsenability analysis pass (spike, measurement only).
//!
//! Governing intent: `context/research/archived-plans/lighting-scale--adaptive-base-probe-density`
//! (design intent only — its surface-distance classifier and seam proxy are
//! ABANDONED). This module classifies coarsenability from **composed receiver
//! error**, measures seams as **actual shared-face reconstruction differences**,
//! attributes compaction / exact-zero delta dropping / density coarsening
//! **separately**, and reports savings both with and without a protection
//! stand-in — never touching a single emitted `.prl` byte.
//!
//! See `context/lib/experimental_spikes.md`: a spike cuts scope and hardening,
//! not rigor. This pass runs entirely CPU-side in the compiler, per 4×4×4 brick
//! incrementally, and never materializes the whole-map dense composed atlas.

// Implementation lands incrementally; see the module body below.

//! Surface Depth (texel-space parallax): CPU reference for the shader DDA and
//! the per-material uniform packing it reads.
//!
//! See: context/lib/rendering_pipeline.md §7.3 (Surface Depth).
//!
//! The height field baked into the specular slot's G channel is a SIGNED
//! height around the polygon plane: authored mid-gray (128) is the plane,
//! darker sinks below it, lighter rises above it, by up to the material's
//! depth in each direction. The bake stores `G = 255 − h` unchanged; the
//! re-centering lives entirely here and in the shader ([`height`]). The field
//! is piecewise-constant per texel — a grid of boxes whose lattice is the same
//! texel grid `sample_post_retro` snaps albedo to. Marching it is therefore an
//! exact 2D DDA (Amanatides-Woo) over that grid, not a fixed-step raymarch:
//! within one texel the solid's top is a single height, so the only events are
//! "cross a side wall" and "meet the top" ([`march`]).
//!
//! The march walks only the material's relief band, from its peak raise down
//! to its trough. Each uploaded mip's band is measured at load, and the uniform
//! packs the band of the mip the march reads. A map that never rises above
//! mid-gray pays nothing for raise, and an all-mid-gray map marches nothing.
//!
//! Everything in this module is GPU-free, and it is the authority for the
//! march itself, the packed material layout, and every tuning constant that
//! the WGSL snippet `shaders/surface_depth.wgsl` mirrors — the renderer's rule
//! is that data logic stays testable without a GPU. Constants defined here are
//! pinned against the shader text by `render/tests/surface_depth_tests.rs`.
//!
//! One rule is GPU-only and this module does NOT mirror it: the texel→meters
//! conversion in `surface_depth.wgsl` (`texels_per_m`, `texel_rate`, and the
//! `depth_scale_m` derivation gated on `SURFACE_DEPTH_TEXEL_MODE`) reads a
//! per-fragment UV Jacobian that only exists mid-shader, so
//! [`march_surface_depth`] takes the already-converted `depth_scale_meters` as
//! a parameter rather than deriving it. That is an accepted gap, not an
//! oversight: Surface Depth is purely graphical — collision still walks the
//! true brush plane — so a wrong conversion is a visible error on screen,
//! never a corrupted game-logic value, and there is no save data or netcode
//! downstream of it to silently corrupt.

mod frame;
mod height;
mod march;
mod shading;
mod uniform;

pub use frame::*;
pub use height::*;
pub use march::*;
pub use shading::*;
pub use uniform::*;

/// Re-exported so the shader-parity test and the uniform resolve read the depth
/// unit and its ceiling from one place. Both live in `render-data`, beside the
/// authoring tables they select; see [`SURFACE_DEPTH_TEXEL_MODE`] for which unit
/// ships and why. The ceilings bound the height in EACH direction.
pub use postretro_render_data::material::{
    SURFACE_DEPTH_MAX_METERS, SURFACE_DEPTH_MAX_TEXELS, SURFACE_DEPTH_TEXEL_MODE,
    surface_depth_is_texel_relative, surface_depth_max_authored,
};

/// Degenerate-geometry floor. Below it the UV→world Jacobian is unusable and
/// the fragment stays flat.
pub const SURFACE_DEPTH_EPS: f32 = 1.0e-9;

/// `x` strictly greater than `lo`. A NaN answers `false`, which is the point:
/// every degenerate test in this module is written as `!above(..)` so a NaN
/// falls out to the flat path instead of slipping through a `<=` comparison and
/// poisoning the march. (Also keeps `clippy::neg_cmp_op_on_partial_ord` happy,
/// which would otherwise flag the `!(x > lo)` this replaces.)
fn above(x: f32, lo: f32) -> bool {
    x > lo
}

/// `x` strictly inside the open range `(lo, hi)`. NaN and infinity both answer
/// `false`.
fn within(x: f32, lo: f32, hi: f32) -> bool {
    x > lo && x < hi
}

#[cfg(test)]
mod step_harness;
#[cfg(test)]
mod tests;

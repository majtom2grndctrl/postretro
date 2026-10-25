// Portal polygon clipping and frustum narrowing shared by the walk and its oracle.
// See: context/lib/rendering_pipeline.md §2

mod clip;
#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) use clip::clip_polygon_to_frustum;
pub use clip::narrow_frustum;
pub(crate) use clip::{CLIP_EPSILON, clip_polygon_to_planes};

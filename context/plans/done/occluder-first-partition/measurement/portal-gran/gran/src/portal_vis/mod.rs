// Portal polygon clipping for the walk; frustum narrowing for its test oracle.
// See: context/lib/rendering_pipeline.md §2

mod clip;

pub(crate) use clip::{CLIP_EPSILON, clip_polygon_to_planes};
pub(crate) use clip::{clip_polygon_to_frustum, narrow_frustum};

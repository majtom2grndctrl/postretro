// Static scene-spec capture: parse tool-facing JSON and drive one
// renderer-owned offscreen scene/capture frame. See: context/lib/rendering_pipeline.md §7.8

mod driver;
mod scene;

pub(crate) use driver::run_capture;

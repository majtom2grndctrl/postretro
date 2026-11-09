// Static scene-spec capture: parse tool-facing JSON and drive one
// renderer-owned offscreen scene/capture frame. See: context/lib/rendering_pipeline.md §7.8

mod cpu_windows;
mod driver;
mod forced_movers;
mod ghosts;
mod lightmap;
#[cfg(test)]
mod mover_frame_tests;
mod planar;
mod prepared;
mod receivers;
mod report;
mod scene;
mod setup;

pub(crate) use driver::run_capture;

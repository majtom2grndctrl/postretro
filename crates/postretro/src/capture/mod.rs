// Static scene-spec capture: parse the tool-facing JSON and drive one
// renderer-owned offscreen world frame. See: context/plans/in-progress/E20--frame-capture

mod driver;
mod scene;

pub(crate) use driver::run_capture;

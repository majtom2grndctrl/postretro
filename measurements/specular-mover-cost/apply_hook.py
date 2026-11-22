# Inserts an env-gated Surface Depth Off hook into capture setup. MEASUREMENT ONLY, never commit.
import sys, pathlib
p = pathlib.Path(sys.argv[1]) / "crates/postretro/src/capture/setup.rs"
s = p.read_text()
anchor = "pub(super) fn apply_scene_reflection(renderer: &mut Renderer, scene: &CaptureScene) {\n"
assert s.count(anchor) == 1, "anchor"
hook = ('    if std::env::var_os("POSTRETRO_CAPTURE_SURFACE_DEPTH_OFF").is_some() {\n'
        '        renderer.set_surface_depth_quality(postretro_render_cpu::surface_depth::SurfaceDepthQuality::Off);\n'
        '        log::warn!("[Capture] MEASUREMENT HOOK: Surface Depth Off");\n'
        '    }\n')
p.write_text(s.replace(anchor, anchor + hook))
print("hooked", p)

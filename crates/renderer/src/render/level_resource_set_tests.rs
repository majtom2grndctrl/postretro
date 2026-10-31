// Per-load renderer resource set: release drops the set and creates nothing,
// and a reinstall draws like the first install.
// See: context/plans/in-progress/level-load-pipeline/index.md (Per-load resources)

use super::renderer_textures::tests::{chain, install_one_triangle_with_sh, write_prm};
use super::*;
use crate::render::fluid_baked_frame_test_fixtures::{direct_section, sh_section};
use crate::render::gpu_test_harness::offscreen_renderer_or_skip;
use crate::render::{ClearColor, ShSampleRegionSets};
use glam::{Mat4, Vec3};
use postretro_level_format::prm::PrmFormat;
use postretro_level_loader::ShDrainBatch;
use postretro_visibility::{CameraCullVisibility, VisibilityPath, VisibleCells};

const CLEAR: ClearColor = ClearColor {
    r: 0.05,
    g: 0.1,
    b: 0.2,
    a: 1.0,
};

/// Textures from one baked sidecar, then the one-triangle level with an SH
/// volume, so world textures, geometry and the SH family all bind real data.
fn install_fixture(renderer: &mut Renderer, root: &Path) {
    let key = [0x42u8; 32];
    write_prm(
        root,
        key,
        [
            Some(chain(PrmFormat::Rgba8UnormSrgb, 4, 4, 0x5A)),
            None,
            None,
            None,
        ],
    );
    let names = vec!["baked".to_string()];
    let keys = TextureCacheKeysSection { keys: vec![key] };
    let materials = vec![Material::Default];
    renderer.install_textures(&names, &keys, root, &materials);
    let sh = sh_section([-1.0; 3], [1.0; 3], [2, 2, 2], |_| Vec3::splat(0.25));
    let direct = direct_section(&sh, Vec3::splat(0.5));
    install_one_triangle_with_sh(renderer, &materials, Some(&sh), Some(&direct));
}

fn capture(renderer: &mut Renderer, render_world: bool) -> Vec<u8> {
    let cells = VisibleCells::Culled(vec![0]);
    renderer
        .capture_frame_indirect(
            CameraCullVisibility {
                cells: &cells,
                path: VisibilityPath::PrlPortal { walk_reach: 1 },
            },
            &[],
            &[],
            &[],
            ShSampleRegionSets {
                visible_cells: &cells,
                fog_cells: &[],
                movers: &[],
            },
            Some(0),
            Mat4::IDENTITY,
            Vec3::new(0.0, 0.0, 1.0),
            &[],
            &[],
            1.0,
            CLEAR,
            render_world,
            ShDrainBatch::default(),
        )
        .expect("capture drains")
        .frame
        .expect("capture records")
}

fn world_less(renderer: &mut Renderer, font: &mut postretro_ui::text::FontSystem) -> Vec<u8> {
    renderer
        .capture_world_less_frame(font, CLEAR, 1.0)
        .expect("world-less capture records")
}

fn hash(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

#[test]
fn release_creates_no_gpu_object_and_drops_the_level_set() {
    let test = "release_creates_no_gpu_object_and_drops_the_level_set";
    let Some(mut renderer) = offscreen_renderer_or_skip(test, 16, 16) else {
        return;
    };
    let mut font = postretro_ui::text::build_font_system();
    let dir = tempfile::tempdir().unwrap();
    install_fixture(&mut renderer, dir.path());
    assert!(
        renderer.has_level_resources(),
        "the install publishes a set"
    );
    capture(&mut renderer, true);

    let before = gpu_creation_counts();
    renderer.release_level_resources();
    let released = gpu_creation_counts().since(before);
    assert_eq!(
        released.total(),
        0,
        "release creates no GPU object: {released:?}"
    );
    assert!(!renderer.has_level_resources(), "release drops the set");
    assert!(
        renderer.full().pending_textures.is_none(),
        "release keeps no staged materials"
    );

    let scope = renderer
        .device
        .push_error_scope(wgpu::ErrorFilter::Validation);
    let world = capture(&mut renderer, true);
    let empty = capture(&mut renderer, false);
    let ui = world_less(&mut renderer, &mut font);
    let error = pollster::block_on(scope.pop());
    assert!(
        error.is_none(),
        "a world-less frame after release raised a validation error: {error:?}"
    );
    assert_eq!(
        world, empty,
        "a world frame with no set draws as world-less"
    );
    assert_eq!(
        hash(&world),
        hash(&ui),
        "and as the windowed world-less frame"
    );
}

#[test]
fn install_after_release_draws_like_first_install() {
    let test = "install_after_release_draws_like_first_install";
    let Some(mut renderer) = offscreen_renderer_or_skip(test, 16, 16) else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    install_fixture(&mut renderer, dir.path());
    let first = capture(&mut renderer, true);
    renderer.release_level_resources();
    let released = capture(&mut renderer, true);
    assert_ne!(first, released, "the fixture draws over the clear");

    install_fixture(&mut renderer, dir.path());
    let second = capture(&mut renderer, true);
    assert_eq!(
        hash(&second),
        hash(&first),
        "the reinstalled level draws as the first install did"
    );
    eprintln!(
        "[LevelSetProof] install {:016x}, released {:016x}, reinstall {:016x}",
        hash(&first),
        hash(&released),
        hash(&second)
    );
}

/// With no set, every frame kind draws what a never-installed renderer's
/// world-less frame draws, before an install and after a release.
#[test]
fn world_less_frames_match_a_fresh_renderer_before_install_and_after_release() {
    let test = "world_less_frames_match_a_fresh_renderer_before_install_and_after_release";
    let Some(mut renderer) = offscreen_renderer_or_skip(test, 16, 16) else {
        return;
    };
    let mut font = postretro_ui::text::build_font_system();
    let dir = tempfile::tempdir().unwrap();
    let fresh = world_less(&mut renderer, &mut font);
    assert_eq!(capture(&mut renderer, true), fresh, "fresh world frame");
    assert_eq!(capture(&mut renderer, false), fresh, "fresh empty frame");
    install_fixture(&mut renderer, dir.path());
    renderer.release_level_resources();
    assert_eq!(capture(&mut renderer, true), fresh, "released world frame");
    assert_eq!(capture(&mut renderer, false), fresh, "released empty frame");
    assert_eq!(
        world_less(&mut renderer, &mut font),
        fresh,
        "released UI frame"
    );
}

/// A Surface Depth change with no set and no staged materials stages
/// nothing: there is no material to rewrite.
#[test]
fn surface_depth_change_with_no_level_set_stages_no_material_write() {
    use postretro_render_cpu::surface_depth::SurfaceDepthQuality;
    let test = "surface_depth_change_with_no_level_set_stages_no_material_write";
    let Some(mut renderer) = offscreen_renderer_or_skip(test, 16, 16) else {
        return;
    };
    let quality = |renderer: &Renderer| match renderer.surface_depth_quality() {
        SurfaceDepthQuality::Off => SurfaceDepthQuality::On,
        _ => SurfaceDepthQuality::Off,
    };
    let before = renderer.queue.counts();
    let next = quality(&renderer);
    renderer.set_surface_depth_quality(next);
    assert_eq!(renderer.queue.counts().writes, before.writes);
}

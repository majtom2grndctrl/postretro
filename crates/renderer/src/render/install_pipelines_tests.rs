// Install-time pipelines: built once per renderer off the main thread, reused by every install, retired with the upload thread.
// See: context/lib/testing_guide.md §3 (GPU tests) · context/plans/in-progress/level-load-pipeline/index.md

use super::*;
use crate::render::fluid_baked_frame_test_fixtures::{direct_section, sh_section};
use crate::render::gpu_test_harness::{
    offscreen_renderer_holding_pipeline_build_or_skip, offscreen_renderer_or_skip,
};
use crate::render::renderer_textures::tests::install_one_triangle_with_sh;
use crate::render::upload_thread::tests::wait_for;
use crate::render::{Material, Renderer, SPLASH_CLEAR_COLOR};
use glam::Vec3;
use std::time::{Duration, Instant};

/// How long a test lets a held build's queue run before it checks that a
/// job behind the build has not: generous for a job that would otherwise
/// finish in microseconds.
const QUEUE_SETTLE: Duration = Duration::from_millis(200);

fn uniform_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("install-pipelines test uniform layout"),
        entries: &crate::render::pipeline_layout::uniform_bind_group_layout_entries(),
    })
}

/// One triangle with a BVH leaf (BVH and candidate cull), an SH volume (SH
/// compose binds data) and baked direct SH (direct compose, Case 1).
fn install_lit_triangle(renderer: &mut Renderer) {
    let sh = sh_section([-1.0; 3], [1.0; 3], [2, 2, 2], |_| Vec3::splat(0.25));
    let direct = direct_section(&sh, Vec3::splat(0.5));
    install_one_triangle_with_sh(renderer, &[Material::Default], Some(&sh), Some(&direct));
}

/// Every family the fixture enables binds the slot's pipeline, not a copy.
fn assert_installed_families_bind(renderer: &Renderer, set: &InstallPipelines) {
    let full = renderer.full();
    let level = full
        .level
        .as_ref()
        .expect("a level-gated renderer path requires an active level set");
    assert_eq!(
        level.sh_compose.bound_pipeline(),
        Some(set.sh_compose.pipeline()),
        "SH compose binds the renderer's pipeline"
    );
    assert_eq!(
        level.direct_sh_compose.promotion_pipeline(),
        Some(set.direct_sh_compose.promotion_pipeline()),
        "direct SH compose binds the renderer's promotion pipeline"
    );
    assert_eq!(
        level.compute_cull.as_ref().map(|cull| cull.pipeline()),
        Some(set.compute_cull.pipeline()),
        "the BVH cull binds the renderer's pipeline"
    );
    assert_eq!(
        level.candidate_cull.as_ref().map(|cull| cull.pipeline()),
        Some(set.candidate_cull.pipeline()),
        "the candidate cull binds the renderer's pipeline"
    );
}

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !done() {
        assert!(Instant::now() < deadline, "{what} within 30 s");
        std::thread::sleep(Duration::from_millis(5));
    }
}

// Families covered: SH compose, direct SH compose (Case 1), BVH cull and
// candidate cull bind data here; the animated-lightmap, billboard-scatter
// and animated-direct families take their no-data forms, and the streamed
// families are covered by `a_streamed_install_builds_no_pipeline`.
#[test]
fn install_pipelines_build_once_per_renderer() {
    let test = "install_pipelines_build_once_per_renderer";
    let Some(mut renderer) = offscreen_renderer_or_skip(test, 16, 16) else {
        return;
    };
    let set = renderer.wait_install_pipelines_for_test();

    for install in ["first", "second"] {
        let before = gpu_creation_counts();
        install_lit_triangle(&mut renderer);
        let created = gpu_creation_counts().since(before);
        assert_eq!(
            created.pipelines, 0,
            "the {install} install builds no pipeline: {created:?}"
        );
        assert!(
            created.buffers > 0 && created.bind_groups > 0,
            "the count source reads the {install} install's level half: {created:?}"
        );
        assert_installed_families_bind(&renderer, &set);
    }

    let before = gpu_creation_counts();
    renderer.release_level_resources();
    let released = gpu_creation_counts().since(before);
    assert_eq!(
        released.pipelines, 0,
        "release builds no pipeline: {released:?}"
    );
    assert!(
        !renderer.has_level_resources(),
        "release drops the level set, the cull with it"
    );
    assert!(
        Arc::ptr_eq(&set, &renderer.wait_install_pipelines_for_test()),
        "the renderer keeps one set"
    );
}

#[test]
fn a_streamed_install_builds_no_pipeline() {
    let test = "a_streamed_install_builds_no_pipeline";
    let Some(mut renderer) = offscreen_renderer_or_skip(test, 16, 16) else {
        return;
    };
    renderer.wait_install_pipelines_for_test();
    let (_temp, path) = crate::render::sh_streaming::write_nine_cluster_prl();
    for install in ["first", "second"] {
        let mut world = postretro_level_loader::load_prl(path.to_str().unwrap()).unwrap();
        assert!(world.sh_stream_manifest().is_some(), "the fixture streams");
        let payloads = world.take_gpu_lighting_payloads();
        let geometry = crate::render::level_world_to_geometry(&world, &[]);
        let before = gpu_creation_counts();
        renderer.install_level_geometry(&geometry, payloads);
        let created = gpu_creation_counts().since(before);
        assert!(
            renderer.level().sh_streaming.is_some(),
            "the {install} install streams"
        );
        assert_eq!(
            created.pipelines, 0,
            "the {install} streamed install builds no pipeline: {created:?}"
        );
    }
}

#[test]
fn full_init_builds_no_install_pipeline_on_the_main_thread() {
    let test = "full_init_builds_no_install_pipeline_on_the_main_thread";
    let family_before = install_family_pipelines_built();
    let counts_before = gpu_creation_counts();
    let Some((renderer, release)) = offscreen_renderer_holding_pipeline_build_or_skip(test, 16, 16)
    else {
        return;
    };
    let full_init = gpu_creation_counts().since(counts_before);
    assert!(
        full_init.pipelines > 0,
        "full init still builds its renderer-lifetime pipelines here: {full_init:?}"
    );
    assert_eq!(
        install_family_pipelines_built() - family_before,
        0,
        "full init built no install-family pipeline on the main thread"
    );
    let slot = Arc::clone(&renderer.full().install_pipelines);
    assert!(
        slot.try_get().is_none(),
        "the held build has not published, so full init did not build the set"
    );

    release.release();
    renderer.wait_install_pipelines_for_test();
    let by_job = slot
        .pipelines_created_by_job()
        .expect("the published set reports its job's count");

    // The same set built here is the reference: every pipeline it creates is
    // an install-family pipeline, and the job built exactly that many.
    let layout = uniform_layout(&renderer.device);
    let reference_family_before = install_family_pipelines_built();
    let reference_before = gpu_creation_counts();
    let _reference = InstallPipelines::build(&renderer.device, &layout);
    let reference = gpu_creation_counts().since(reference_before).pipelines;
    assert_eq!(
        install_family_pipelines_built() - reference_family_before,
        reference,
        "every pipeline in the set is an install-family pipeline"
    );
    assert!(reference > 0);
    assert_eq!(
        by_job, reference,
        "the upload thread built the whole set once"
    );
}

#[test]
fn upload_job_queued_behind_held_pipeline_build_waits_off_main() {
    let test = "upload_job_queued_behind_held_pipeline_build_waits_off_main";
    let Some((mut renderer, release)) =
        offscreen_renderer_holding_pipeline_build_or_skip(test, 16, 16)
    else {
        return;
    };
    let slot = Arc::clone(&renderer.full().install_pipelines);
    let job_slot = Arc::clone(&slot);
    let mut pending = renderer
        .full()
        .upload_thread
        .submit(move |_: &UploadContext| Ok(job_slot.try_get().is_some()));

    std::thread::sleep(QUEUE_SETTLE);
    assert!(
        pending.try_take().is_none(),
        "a job queued behind the held build does not run"
    );
    // The main thread keeps rendering meanwhile.
    let mut font_system = postretro_ui::text::build_font_system();
    let pixels = renderer
        .capture_world_less_frame(&mut font_system, SPLASH_CLEAR_COLOR, 0.0)
        .expect("a world-less frame renders while the build is held");
    assert!(!pixels.is_empty());
    assert!(pending.try_take().is_none(), "still held after the frame");
    assert!(slot.try_get().is_none());

    release.release();
    assert!(
        wait_for(pending).expect("the queued job reports"),
        "the job behind the build finds the set published, with no main-thread wait"
    );
}

#[test]
fn frontend_boot_renders_while_pipeline_build_runs() {
    let test = "frontend_boot_renders_while_pipeline_build_runs";
    let Some((mut renderer, release)) =
        offscreen_renderer_holding_pipeline_build_or_skip(test, 16, 16)
    else {
        return;
    };
    let mut font_system = postretro_ui::text::build_font_system();
    for frame in 0..3 {
        renderer
            .capture_world_less_frame(&mut font_system, SPLASH_CLEAR_COLOR, f64::from(frame))
            .expect("a frontend frame renders while the build is held");
    }
    assert!(
        renderer.full().install_pipelines.try_get().is_none(),
        "the frames ran without the set"
    );
    release.release();
    renderer.wait_install_pipelines_for_test();
}

#[test]
fn retired_upload_thread_joins_only_when_finished() {
    let test = "retired_upload_thread_joins_only_when_finished";
    let Some((mut renderer, release)) =
        offscreen_renderer_holding_pipeline_build_or_skip(test, 16, 16)
    else {
        return;
    };
    let old_slot = Arc::clone(&renderer.full().install_pipelines);
    let mut retirement = renderer
        .take_upload_retirement()
        .expect("a full renderer hands off its upload thread");
    assert!(
        renderer.take_upload_retirement().is_none(),
        "the handle is handed off once"
    );
    drop(renderer);

    std::thread::sleep(QUEUE_SETTLE);
    assert!(
        !retirement.try_finish(),
        "the thread is parked in the held build, so it is not joined"
    );

    release.release();
    wait_until("the retired thread finishes after release", || {
        retirement.try_finish()
    });
    assert!(
        retirement.try_finish(),
        "a finished retirement stays finished"
    );

    let Some(resumed) = offscreen_renderer_or_skip(test, 16, 16) else {
        return;
    };
    let resumed_set = resumed.wait_install_pipelines_for_test();
    assert!(
        !Arc::ptr_eq(&old_slot, &resumed.full().install_pipelines),
        "the new renderer has its own slot"
    );
    if let Some(old_set) = old_slot.try_get() {
        assert!(!Arc::ptr_eq(&old_set, &resumed_set));
        assert_ne!(
            old_set.sh_compose.pipeline(),
            resumed_set.sh_compose.pipeline(),
            "no pipeline from the old renderer reaches the new one"
        );
    }
    assert!(
        resumed
            .full()
            .install_pipelines
            .pipelines_created_by_job()
            .is_some_and(|count| count > 0),
        "the new renderer built its own set"
    );
}

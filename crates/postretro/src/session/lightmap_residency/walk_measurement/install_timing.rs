//! Measurement harness: CPU level-load and settled-entry preload timing over a real PRL.
//! See: context/lib/experimental_spikes.md · context/lib/rendering_pipeline.md §4

use std::time::Instant;

use glam::Vec3;

use super::pool_mirror::{PoolMirror, RetireClock};
use super::report::mib;
use super::{LightmapLevelView, LightmapStreamingSession, PRL_ENV};
use crate::streaming::cluster_hints::decode_level_hints;

/// CPU half of the install-timing check: level load time under the process's
/// `POSTRETRO_LIGHTMAP_STREAMING` mode, then (streaming only) a synchronous
/// preload at `POSTRETRO_LIGHTMAP_WALK_SPAWN` = `x,y,z`, approximating a
/// settled entry: positional reads plus the pool model's plan. GPU upload is
/// not timed here; headless captures carry it. Run once per mode:
///
/// ```text
/// POSTRETRO_LIGHTMAP_STREAMING=all-resident \
/// POSTRETRO_LIGHTMAP_WALK_PRL=$PWD/content/dev/maps/campaign-test.prl \
/// POSTRETRO_LIGHTMAP_WALK_SPAWN=-65.84,2.6,-45.92 \
///   cargo test -p postretro --bin postretro lightmap_install_timing_from_prl \
///   -- --ignored --nocapture
/// ```
#[test]
#[ignore = "measurement helper; set POSTRETRO_LIGHTMAP_WALK_PRL"]
fn lightmap_install_timing_from_prl() {
    const REPEATS: usize = 3;
    let path = std::env::var(PRL_ENV).unwrap_or_else(|_| panic!("{PRL_ENV} must name a PRL"));
    let spawn: Vec<f32> = std::env::var("POSTRETRO_LIGHTMAP_WALK_SPAWN")
        .expect("POSTRETRO_LIGHTMAP_WALK_SPAWN = x,y,z")
        .split(',')
        .map(|v| v.trim().parse().expect("spawn coordinate"))
        .collect();
    let spawn_eye = Vec3::new(spawn[0], spawn[1], spawn[2]);
    println!("PRL: {path}");
    for repeat in 0..REPEATS {
        let started = Instant::now();
        let mut world = postretro_level_loader::load_prl(&path).expect("load PRL");
        let load = started.elapsed();
        if LightmapLevelView::of(&world).is_none() {
            let payloads: u64 = world
                .take_gpu_lighting_payloads()
                .blocks
                .iter()
                .map(|block| {
                    (block.irradiance.len()
                        + block.direction.len()
                        + block
                            .shadowmask
                            .as_ref()
                            .map_or(0, |[a, b]| a.len() + b.len())) as u64
                })
                .sum();
            println!(
                "repeat {repeat}: all-resident load {:.1} ms; whole lightmap payloads held for \
                 upload {:.1} MiB",
                load.as_secs_f64() * 1000.0,
                mib(payloads),
            );
            continue;
        }
        let view = LightmapLevelView::of(&world).expect("streams");
        let hints = decode_level_hints(world.cluster_directory()).expect("id-49 hints");
        let mut session =
            LightmapStreamingSession::new(view, hints.as_deref()).expect("lightmap session");
        let mut mirror = PoolMirror::new(
            view.manifest,
            session.controller.levers().pool_cap_layers(),
            RetireClock::Drains,
        );
        let camera_cell = world.locate_cell(spawn_eye) as u32;
        let preload_started = Instant::now();
        session.update_camera_set(view.residency_set, camera_cell);
        let summary = session
            .preload(&[], |batch| Ok(mirror.drain(batch)))
            .expect("settled-entry preload");
        let preload = preload_started.elapsed();
        println!(
            "repeat {repeat}: stream load {:.1} ms; settled-entry preload (cell \
             {camera_cell}) {} pairs, {:.1} MiB, {:.1} ms; pool first generation {} layers",
            load.as_secs_f64() * 1000.0,
            summary.reads.pairs,
            mib(summary.reads.bytes),
            preload.as_secs_f64() * 1000.0,
            mirror.model().layers(),
        );
    }
}

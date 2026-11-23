//! The level's reflection probe records: which sections build them, that a
//! level without fluid holds them, what the memory ledger meters, and that a
//! level change and an unload leave only the installed level's probes.
//! See: context/lib/resource_management.md §5.3 · context/lib/rendering_pipeline.md §7.3a

use postretro_level_format::environment_volumes::EnvironmentVolumeRecord;
use postretro_level_format::lightmap::IRRADIANCE_FORMAT_RGBA16F;
use postretro_level_format::reflection_probes::{
    ReflectionProbeWorldBinding, reflection_probe_colour_chain_bytes,
};
use postretro_render_cpu::fluid_surface::reflection::{
    FLUID_PROBE_SUMS_BYTES, FLUID_PROBE_TABLE_BYTES,
};

use super::*;
use crate::render::Renderer;
use crate::render::fluid_reflection_frame_test_fixtures::{
    FRAME, POOL_X, PROBE_TILE, base_view, mirror, pool,
};
use crate::render::fluid_reflection_sums_frame_test_fixtures::{
    SumsLevel, dispatches, frame_at, probe, probe_section, take,
};
use crate::render::fluid_surface_frame_test::look_draw;
use crate::render::gpu_test_harness::offscreen_renderer_or_skip;
use crate::render::level_resource_set::PreviousLevelSet;
use crate::render::reflection_probe_residency::ReflectionProbeResidencyReport;

/// `probe_count` probes, each a two-mip `Rgba16Float` chain, bound to the
/// fluid faces `fluid` `(volume, face, probe)` and the id-17 faces `world`
/// `(face, probe)`.
fn section(
    probe_count: u32,
    fluid: &[(u32, u32, u32)],
    world: &[(u32, u32)],
) -> ReflectionProbesSection {
    let mut section = probe_section(
        (0..probe_count).map(|layer| probe(layer, &[])).collect(),
        fluid,
    );
    section.colour_mip_count = 2;
    let chain = reflection_probe_colour_chain_bytes(IRRADIANCE_FORMAT_RGBA16F, PROBE_TILE, 2)
        .expect("an 8² tile carries two mips");
    section.tiles = vec![0; chain * probe_count as usize];
    section.world_bindings = world
        .iter()
        .map(|&(face, probe)| ReflectionProbeWorldBinding { face, probe })
        .collect();
    section
}

/// A world-less level with no animated lights, its fluid `volumes` and
/// `probes`.
fn level(volumes: Vec<EnvironmentVolumeRecord>, probes: ReflectionProbesSection) -> SumsLevel {
    SumsLevel {
        roster: Vec::new(),
        volumes,
        probes,
    }
}

/// The bytes the ledger must meter for `section`: the colour chain as
/// stored (every mip), one distance tile per probe, the table and the sums
/// at the cap.
fn expected_rows(section: &ReflectionProbesSection) -> [(&'static str, u64); 4] {
    [
        (REFLECTION_PROBE_COLOUR, section.tiles.len() as u64),
        (REFLECTION_PROBE_DISTANCE, section.distances.len() as u64),
        (REFLECTION_PROBE_SHARE_TABLE, FLUID_PROBE_TABLE_BYTES as u64),
        (REFLECTION_PROBE_SUMS, FLUID_PROBE_SUMS_BYTES as u64),
    ]
}

fn assert_meters(report: &ReflectionProbeResidencyReport, section: &ReflectionProbesSection) {
    let rows = expected_rows(section);
    for (name, bytes) in rows {
        assert_eq!(report.bytes(name), Some(bytes), "{name}");
    }
    assert_eq!(report.allocations.len(), 4);
    assert_eq!(
        report.total_bytes,
        rows.iter().map(|(_, bytes)| bytes).sum::<u64>()
    );
}

fn report(renderer: &Renderer) -> ReflectionProbeResidencyReport {
    renderer
        .reflection_probe_residency_report()
        .expect("a full renderer reports")
}

#[test]
fn a_level_probe_section_must_be_installable_and_bind_a_face() {
    let world_only = section(2, &[], &[(5, 1)]);
    let fluid_only = section(2, &[(0, 0, 0)], &[]);
    let unbound = section(2, &[], &[]);
    assert!(level_probe_section(None).is_none());
    assert!(level_probe_section(Some(&world_only)).is_some());
    assert!(level_probe_section(Some(&fluid_only)).is_some());
    assert!(
        level_probe_section(Some(&unbound)).is_none(),
        "a section binding no face allocates nothing"
    );
    let mut torn = world_only.clone();
    torn.tiles.pop();
    assert!(
        level_probe_section(Some(&torn)).is_none(),
        "an uninstallable section allocates nothing"
    );
}

// Pin P1, rows 64 and 65: a level whose id 54 binds world faces and no fluid
// face, with no fluid volume, installs its probes and sums on both build
// paths, meters every colour mip, and dispatches no sums while nothing draws
// a probe term (no overlay exists yet).
#[test]
fn a_level_with_world_bound_probes_and_no_fluid_installs_and_meters_them() {
    let test = "a_level_with_world_bound_probes_and_no_fluid_installs_and_meters_them";
    let Some(mut renderer) = offscreen_renderer_or_skip(test, FRAME, FRAME) else {
        return;
    };
    let probes = section(3, &[], &[(0, 0), (1, 2), (4, 1)]);
    let world_only = level(Vec::new(), probes.clone());
    world_only.install(&mut renderer);
    assert!(renderer.fluid_surfaces().is_none(), "no fluid installs");
    let installed = renderer
        .reflection_probes()
        .expect("the level's probes install without fluid");
    assert_eq!(installed.array_size().depth_or_array_layers, 3);
    assert_eq!(
        installed.sums_dispatch().map(|(_, count)| count),
        Some(3),
        "the sums pass would evaluate every installed probe"
    );
    let oracle = report(&renderer);
    assert_meters(&oracle, &probes);
    assert_eq!(
        oracle.bytes(REFLECTION_PROBE_COLOUR),
        Some(3 * (8 * 8 + 4 * 4) * 8),
        "both colour mips of every probe"
    );

    let before = dispatches(&renderer);
    frame_at(&mut renderer, base_view(), 1.0);
    assert_eq!(
        dispatches(&renderer),
        before,
        "no probe-term draw, no dispatch"
    );

    // Off the main thread: the set holds the probes before it publishes,
    // and the publish swaps them in with the set.
    let id = world_only.build_off_main(&mut renderer);
    let set = renderer.level_set(id).expect("the built set is resident");
    assert!(set.fluid_surfaces.is_none());
    assert_eq!(
        set.reflection_probes
            .as_ref()
            .map(ReflectionProbeResources::residency_report),
        Some(oracle.clone()),
        "the set build meters what the oracle does"
    );
    renderer.publish_level_set(id, PreviousLevelSet::Retire);
    assert_eq!(report(&renderer), oracle);
    eprintln!("[GlossyTask4] world-only level: {oracle:?}");
}

// Row 65's second half and pin P1's fluid side: a level with neither a
// probe-binding id 54 nor fluid allocates no probe resource; a fluid level
// without probes binds the fluid install's dummies, which are not probes.
#[test]
fn a_level_without_bound_probes_allocates_no_probe_resource() {
    let test = "a_level_without_bound_probes_allocates_no_probe_resource";
    let Some(mut renderer) = offscreen_renderer_or_skip(test, FRAME, FRAME) else {
        return;
    };
    assert_eq!(report(&renderer), ReflectionProbeResidencyReport::EMPTY);
    for (case, volumes, probes) in [
        ("neither", Vec::new(), None),
        (
            "a section binding no face",
            Vec::new(),
            Some(section(2, &[], &[])),
        ),
        ("fluid without probes", vec![pool(0, 0.0, &POOL_X)], None),
    ] {
        renderer.install_empty_level_for_test();
        renderer.install_fluid_surfaces(&volumes, probes.as_ref());
        assert!(renderer.reflection_probes().is_none(), "{case}");
        assert_eq!(
            report(&renderer),
            ReflectionProbeResidencyReport::EMPTY,
            "{case}"
        );
        let fluid = renderer.fluid_test();
        let expected_layers = (!volumes.is_empty()).then_some(1);
        assert_eq!(fluid.probe_array_layers(), expected_layers, "{case}");
    }
}

// Pin P8, resource half, and row 64: an unload returns the probe rows to
// zero, and a level change, on either build path, leaves only the new
// level's probes bound and metered.
#[test]
fn a_level_change_replaces_the_probes_and_an_unload_returns_them_to_zero() {
    let test = "a_level_change_replaces_the_probes_and_an_unload_returns_them_to_zero";
    let Some(mut renderer) = offscreen_renderer_or_skip(test, FRAME, FRAME) else {
        return;
    };
    let a_probes = section(3, &[(0, 0, 0)], &[(2, 1)]);
    let a = level(vec![pool(0, 0.0, &POOL_X)], a_probes.clone());
    let b_probes = section(1, &[], &[(0, 0)]);
    let b = level(Vec::new(), b_probes.clone());

    a.install(&mut renderer);
    assert_meters(&report(&renderer), &a_probes);
    renderer.set_fluid_surface_draws(&[look_draw(0, mirror())]);
    let before = dispatches(&renderer);
    frame_at(&mut renderer, base_view(), 1.0);
    assert_eq!(dispatches(&renderer), before + 1, "A's reflecting water");

    renderer.release_level_resources();
    assert_eq!(
        report(&renderer),
        ReflectionProbeResidencyReport::EMPTY,
        "unload"
    );
    assert!(renderer.reflection_probes().is_none());

    b.install(&mut renderer);
    let b_report = report(&renderer);
    assert_meters(&b_report, &b_probes);
    assert_eq!(
        renderer
            .reflection_probes()
            .map(|probes| probes.array_size().depth_or_array_layers),
        Some(1),
        "B's one probe, none of A's three"
    );

    // A level change with no unload between: the oracle retires B's set.
    a.install(&mut renderer);
    assert_meters(&report(&renderer), &a_probes);

    // Off the main thread: B builds beside A, which stays metered until the
    // publish swaps B's set in.
    let mut pending = b.begin_off_main(&mut renderer);
    let id = take(&mut renderer, &mut pending);
    assert_meters(&report(&renderer), &a_probes);
    renderer.publish_level_set(id, PreviousLevelSet::Retire);
    assert_eq!(report(&renderer), b_report);
    renderer.release_level_resources();
    assert_eq!(report(&renderer), ReflectionProbeResidencyReport::EMPTY);
}

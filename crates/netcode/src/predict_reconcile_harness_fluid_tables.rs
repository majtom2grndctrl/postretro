// Co-op fluid parity through the loopback harness: host and client hold different
// fluid declarations, and the client resolves level fluids only against the host's
// table, which reaches it through the tuning-payload install.
// See: context/lib/networking.md §What gates, and what replicates instead

#![cfg(test)]

use glam::Vec3;
use postretro_collision::{
    BodyProbe, EnvironmentView, EnvironmentVolume, ResolvedEnvironment, resolve_environment,
};
use postretro_foundation::{FluidDescriptor, FluidRegistry};

use super::predict_reconcile_harness_test_fixtures::{
    LoopbackHarness, NamedEnvironmentVolume, gravity_box_volume, idle_command, light_link,
};

const HOST_WATER_VISCOSITY: f32 = 2.0;
const HOST_SLUDGE_VISCOSITY: f32 = 6.5;
const HOST_LAVA_VISCOSITY: f32 = 9.0;
const HOST_WATER_TINT: [f32; 4] = [0.1, 0.3, 0.4, 0.35];
const CLIENT_WATER_TINT: [f32; 4] = [0.8, 0.1, 0.6, 0.9];

/// Pool centres, well away from the pawn's start so its walk never matters.
const WATER_POOL: Vec3 = Vec3::new(12.0, 2.0, 0.0);
const SLUDGE_POOL: Vec3 = Vec3::new(22.0, 2.0, 0.0);
const LAVA_POOL: Vec3 = Vec3::new(32.0, 2.0, 0.0);
const DRY_GROUND: Vec3 = Vec3::new(-12.0, 2.0, 0.0);

fn fluid(name: &str, viscosity: f32, view_tint: [f32; 4]) -> FluidDescriptor {
    FluidDescriptor {
        name: name.to_string(),
        viscosity,
        view_tint,
        muffle: 0.8,
        surface_opacity: 0.6,
        enter_sound: None,
        exit_sound: None,
        fog_distance: None,
        fog_color: None,
        wave_period: None,
        wave_amplitude: 8.0,
        wave_length: [64.0, 80.0],
        surface_reflectivity: 1.0,
    }
}

/// A committed manifest's fluids, in the given declaration order.
fn manifest(fluids: Vec<FluidDescriptor>) -> FluidRegistry {
    FluidRegistry::from_declarations("harness", fluids.into_iter().map(Ok))
}

fn host_water() -> FluidDescriptor {
    fluid("water", HOST_WATER_VISCOSITY, HOST_WATER_TINT)
}

fn host_sludge() -> FluidDescriptor {
    fluid("sludge", HOST_SLUDGE_VISCOSITY, [0.3, 0.3, 0.1, 0.8])
}

fn lava() -> FluidDescriptor {
    fluid("lava", HOST_LAVA_VISCOSITY, [1.0, 0.2, 0.0, 0.8])
}

/// A 4 m pool centred on `centre` naming `fluid`, optionally setting gravity.
fn pool(centre: Vec3, fluid: &str, gravity: Option<f32>) -> NamedEnvironmentVolume {
    let volume = gravity_box_volume(centre - Vec3::splat(2.0), centre + Vec3::splat(2.0), 0.0);
    NamedEnvironmentVolume {
        volume: EnvironmentVolume { gravity, ..volume },
        fluid: Some(fluid.to_string()),
    }
}

/// The level every test loads: a water, a sludge and a lava pool.
fn pools() -> Vec<NamedEnvironmentVolume> {
    vec![
        pool(WATER_POOL, "water", None),
        pool(SLUDGE_POOL, "sludge", None),
        pool(LAVA_POOL, "lava", None),
    ]
}

fn probe_at(position: Vec3) -> BodyProbe {
    BodyProbe {
        position,
        half_height: 0.5,
        radius: 0.4,
        eye_height: 0.6,
    }
}

fn resolve_host(h: &LoopbackHarness, position: Vec3) -> ResolvedEnvironment {
    resolve_environment(h.host_environment_view(), &probe_at(position))
}

fn resolve_client(h: &LoopbackHarness, position: Vec3) -> ResolvedEnvironment {
    resolve_environment(
        h.client_environment_view()
            .expect("the host's tuning has installed"),
        &probe_at(position),
    )
}

/// Arm the client: the participation send installs the host's tuning on the way.
fn joined(
    records: Vec<NamedEnvironmentVolume>,
    host_fluids: FluidRegistry,
    client_fluids: FluidRegistry,
) -> LoopbackHarness {
    let mut h =
        LoopbackHarness::with_fluid_tables(light_link(), records, host_fluids, client_fluids);
    h.step_until_armed(&idle_command());
    assert_eq!(h.client_tuning_installs, 1, "one participation send");
    h
}

/// The client's presentation for a fluid it resolved: the installed host table
/// names it, the client's own manifest describes it.
fn client_presentation(h: &LoopbackHarness, fluid: u16) -> Option<&FluidDescriptor> {
    h.client_tuning
        .as_deref()
        .expect("the host's tuning has installed")
        .environment
        .fluids
        .local_presentation(fluid, &h.client_fluids)
}

// Host and client hold different fluid tables. The client's
// volumes resolve dry until the host's table installs through the tuning
// payload, then resolve every fluid at the host's index with the host's
// viscosity; the client's own declarations never enter resolution.
#[test]
fn client_resolves_fluids_only_through_the_host_table_installed_by_tuning() {
    let client_fluids = manifest(vec![
        fluid("sludge", 0.5, [0.2; 4]),
        lava(),
        fluid("water", 7.0, CLIENT_WATER_TINT),
    ]);
    let mut h = LoopbackHarness::with_fluid_tables(
        light_link(),
        pools(),
        manifest(vec![host_water(), host_sludge()]),
        client_fluids,
    );
    assert!(
        h.client_environment_view().is_none(),
        "no prediction view before the host's tuning"
    );
    for position in [WATER_POOL, SLUDGE_POOL, LAVA_POOL] {
        let before = resolve_environment(
            EnvironmentView::new(0.0, &h.client_environment_volumes, &[]),
            &probe_at(position),
        );
        assert_eq!(
            before.fluid, None,
            "every fluid is dry before the host's table"
        );
    }

    h.step_until_armed(&idle_command());
    assert_eq!(h.client_tuning_installs, 1);
    let installed = &h
        .client_tuning
        .as_deref()
        .expect("installed through the tuning path")
        .environment
        .fluids;
    assert_eq!(
        installed.names(),
        ["water", "sludge"],
        "the host's table, in its order"
    );
    assert_eq!(
        installed.viscosities(),
        [HOST_WATER_VISCOSITY, HOST_SLUDGE_VISCOSITY]
    );
    assert_eq!(h.client_volume_fluid_names, ["water", "sludge"]);

    let water = resolve_client(&h, WATER_POOL);
    assert_eq!(
        (water.fluid, water.viscosity),
        (Some(0), HOST_WATER_VISCOSITY)
    );
    let sludge = resolve_client(&h, SLUDGE_POOL);
    assert_eq!(
        (sludge.fluid, sludge.viscosity),
        (Some(1), HOST_SLUDGE_VISCOSITY)
    );
    let lava = resolve_client(&h, LAVA_POOL);
    assert_eq!(
        (lava.fluid, lava.viscosity),
        (None, 0.0),
        "the client's own lava declaration resolves nothing"
    );
}

// A client whose manifest gives `water` a different tint predicts with the
// host's viscosity and shows its own tint; a client with no `water` shows no
// tint and still resolves the host's water for movement.
#[test]
fn client_predicts_host_viscosity_and_presents_its_own_fluid_declaration() {
    let host_fluids = || manifest(vec![host_water(), host_sludge()]);

    let retinted = joined(
        pools(),
        host_fluids(),
        manifest(vec![fluid("water", 7.0, CLIENT_WATER_TINT), host_sludge()]),
    );
    let water = resolve_client(&retinted, WATER_POOL);
    assert_eq!(water.fluid, Some(0));
    assert_eq!(
        water.viscosity, HOST_WATER_VISCOSITY,
        "movement takes the host's viscosity, never the client's 7.0"
    );
    let shown = client_presentation(&retinted, 0).expect("the client declares water");
    assert_eq!(shown.view_tint, CLIENT_WATER_TINT, "the client's own tint");

    let without_water = joined(pools(), host_fluids(), manifest(vec![host_sludge()]));
    let water = resolve_client(&without_water, WATER_POOL);
    assert_eq!(
        (water.fluid, water.viscosity),
        (Some(0), HOST_WATER_VISCOSITY),
        "the host's water still resolves for movement"
    );
    assert!(water.immersion > 0.99);
    assert_eq!(
        client_presentation(&without_water, 0),
        None,
        "no local water: no tint, muffle, surface or sounds"
    );
    assert_eq!(
        client_presentation(&without_water, 1).map(|fluid| fluid.name.as_str()),
        Some("sludge")
    );
}

// A host manifest reload that adds a fluid resends the table;
// the client installs it and re-resolves its volumes, so a volume naming the new
// fluid is no longer dry on the client either.
#[test]
fn host_manifest_reload_that_adds_a_fluid_makes_its_volume_no_longer_dry_on_the_client() {
    let mut h = joined(
        pools(),
        manifest(vec![host_water()]),
        manifest(vec![host_water(), lava()]),
    );
    assert_eq!(resolve_host(&h, LAVA_POOL).fluid, None);
    assert_eq!(resolve_client(&h, LAVA_POOL).fluid, None);

    h.host_reload_fluids(manifest(vec![host_water(), lava()]));
    for _ in 0..8 {
        h.step(&idle_command());
    }

    assert_eq!(
        h.client_tuning_installs, 2,
        "exactly one resend for the reload"
    );
    let host = resolve_host(&h, LAVA_POOL);
    let client = resolve_client(&h, LAVA_POOL);
    assert_eq!((host.fluid, host.viscosity), (Some(1), HOST_LAVA_VISCOSITY));
    assert_eq!(
        client, host,
        "the client resolves the new fluid as the host does"
    );
    assert_eq!(
        resolve_client(&h, WATER_POOL).fluid,
        Some(0),
        "the existing fluid keeps its index"
    );
}

// A client whose manifest declares different `water` numbers, lacks
// `water`, or declares its fluids in another order resolves every point exactly
// as the host does: same fluid index, same viscosity, same immersion. The
// zero-corrections half is in the swim harness.
#[test]
fn client_with_differing_fluid_declarations_resolves_every_point_like_the_host() {
    let client_manifests = [
        (
            "different water numbers",
            manifest(vec![fluid("water", 11.0, CLIENT_WATER_TINT), host_sludge()]),
        ),
        ("no water", manifest(vec![host_sludge()])),
        (
            "reversed order",
            manifest(vec![host_sludge(), host_water()]),
        ),
        ("no fluids at all", FluidRegistry::default()),
    ];
    // Submerged, half in at the surface, and dry.
    let points = [
        WATER_POOL,
        WATER_POOL + Vec3::new(0.0, 2.0, 0.0),
        SLUDGE_POOL,
        SLUDGE_POOL + Vec3::new(0.0, 2.0, 0.0),
        DRY_GROUND,
    ];
    for (case, client_fluids) in client_manifests {
        let h = joined(
            pools(),
            manifest(vec![host_water(), host_sludge()]),
            client_fluids,
        );
        for point in points {
            assert_eq!(
                resolve_client(&h, point),
                resolve_host(&h, point),
                "{case}: client and host resolve {point:?} identically"
            );
        }
        let water = resolve_client(&h, WATER_POOL);
        assert_eq!(
            (water.fluid, water.viscosity),
            (Some(0), HOST_WATER_VISCOSITY),
            "{case}: the host's water"
        );
        let surface = resolve_client(&h, WATER_POOL + Vec3::new(0.0, 2.0, 0.0));
        assert!(
            surface.immersion > 0.0 && surface.immersion < 1.0,
            "{case}: the surface point is partly immersed"
        );
    }
}

// A volume naming a fluid the host lacks is dry on the client too, even
// when the client's own manifest declares it; its gravity still applies.
#[test]
fn volume_naming_a_fluid_the_host_lacks_is_dry_on_the_client_even_when_declared_locally() {
    let records = vec![
        pool(WATER_POOL, "water", None),
        pool(SLUDGE_POOL, "sludge", Some(-3.0)),
    ];
    let h = joined(
        records,
        manifest(vec![host_water()]),
        manifest(vec![host_water(), host_sludge()]),
    );

    let host = resolve_host(&h, SLUDGE_POOL);
    let client = resolve_client(&h, SLUDGE_POOL);
    assert_eq!((host.fluid, host.viscosity), (None, 0.0), "dry on the host");
    assert_eq!(
        (client.fluid, client.viscosity, client.immersion),
        (None, 0.0, 0.0),
        "dry on the client despite its own sludge declaration"
    );
    assert_eq!(
        client.gravity, -3.0,
        "the dry volume's gravity still applies"
    );
    assert_eq!(client, host);
    assert_eq!(resolve_client(&h, WATER_POOL).fluid, Some(0));
}

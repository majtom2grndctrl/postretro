// K3 capture proofs: promotion pops no more at a forced terminus than the
// same light's ungated arms. With the mover forced to each terminus and the
// gated light's promotion pinned at w = 0 and w = 1, the mean summed-RGB
// change over a kinematic receiver's lit texels is no larger than the same
// light's change on the same receiver with the mover forced open and id 57
// stripped, plus `LIT_ENDPOINT_DRIFT`. The open terminus also proves the
// promoted arm's casters follow the forced pose: casters left at the closed
// rest pose would shadow the runtime arm while the baked arm reads open, a
// pop far past the bar.
// See: context/lib/rendering_pipeline.md §4 (Mover state lighting,
// Promoted static lights, Promoted animated lights) · §7.8
//
// Means are summed RGB (0–765) of the post-tonemap sRGB capture. Captures
// compare on one adapter; no golden images.

use std::path::Path;

use image::RgbaImage;
use postretro_level_format::SectionId;
use postretro_level_format::mover_probe_channel::{MoverLightNamespace, MoverProbeChannelSection};

mod capture_support;
// Shared by two targets, each using part of it.
#[allow(dead_code)]
mod mover_capture_support;

use capture_support::compile_dev_map;
use mover_capture_support::{
    Pose, Run, Scene, ScreenBox, read_map, read_sections, rgb_sum, sibling_map, with_entity,
    without_channel,
};

const RESOLUTION: [u32; 2] = [960, 540];
/// Allowed summed-RGB drift between promotion endpoints, as
/// `capture_probe_depth_tiles.rs`'s constant of the same name; another test
/// target cannot share it.
const LIT_ENDPOINT_DRIFT: f64 = 12.0;

const CLOSET_DOOR: &str = "closet_door";
const SETPIECE_WALL: &str = "setpiece_wall";
/// Closet station 3: the closet floor at the door.
const CLOSET_STATION_3: Pose = Pose {
    map: [320.0, 610.0, 96.0],
    yaw: 270.0,
    pitch: -38.0,
    fov: 100.0,
};
/// The set-piece's arena spot. It reaches the back room through the
/// opening, and the wall gates it at back-room probes.
const ARENA_SPOT: &str = "368 560 288";
/// A slab on the set-piece's back-room floor, in the arena spot's beam.
const SETPIECE_SLAB: ([f32; 3], [f32; 3]) = ([432.0, 1240.0, 0.0], [496.0, 1304.0, 16.0]);
/// Looking down at the set-piece slab from the back room.
const SETPIECE_SLAB_VIEW: Pose = Pose {
    map: [464.0, 1440.0, 112.0],
    yaw: 270.0,
    pitch: -30.0,
    fov: 100.0,
};

/// A `kinematic_mover` slab named `name`, box `min`–`max` in map units,
/// resting at its first waypoint and never started: a kinematic receiver.
fn slab_mover(name: &str, min: [f32; 3], max: [f32; 3]) -> String {
    let ([x0, y0, z0], [x1, y1, z1]) = (min, max);
    let texture = "metal/metal_rough_046a 0 0 0 1 1";
    let planes = [
        [[x0, y1, z0], [x0, y0, z1], [x0, y0, z0]],
        [[x0, y0, z1], [x1, y0, z0], [x0, y0, z0]],
        [[x0, y0, z0], [x1, y1, z0], [x0, y1, z0]],
        [[x0, y1, z1], [x1, y0, z1], [x0, y0, z1]],
        [[x1, y1, z1], [x0, y1, z1], [x0, y1, z0]],
        [[x1, y0, z0], [x1, y1, z1], [x1, y1, z0]],
    ]
    .map(|points| {
        let [a, b, c] = points.map(|[x, y, z]| format!("( {x} {y} {z} )"));
        format!("{a} {b} {c} {texture}")
    })
    .join("\n");
    let centre = [(x0 + x1) / 2.0, (y0 + y1) / 2.0, (z0 + z1) / 2.0];
    format!(
        "{{\n\"classname\" \"kinematic_mover\"\n\"name\" \"{name}\"\n\"path\" \"{name}_rest\"\n\
         \"speed\" \"1\"\n{{\n{planes}\n}}\n}}\n\
         {{\n\"classname\" \"kinematic_waypoint\"\n\"name\" \"{name}_rest\"\n\"next\" \"{name}_up\"\n\
         \"origin\" \"{} {} {}\"\n}}\n\
         {{\n\"classname\" \"kinematic_waypoint\"\n\"name\" \"{name}_up\"\n\
         \"origin\" \"{} {} {}\"\n}}",
        centre[0],
        centre[1],
        centre[2],
        centre[0],
        centre[1],
        centre[2] + 32.0
    )
}

/// `map` with `keys` added to the light at `origin`.
fn with_light_keys(map: &str, origin: &str, keys: &[(&str, &str)]) -> String {
    let marker = format!("\"origin\" \"{origin}\"");
    let at = map
        .find(&marker)
        .unwrap_or_else(|| panic!("no entity at origin {origin}"))
        + marker.len();
    let added: String = keys
        .iter()
        .map(|(key, value)| format!("\n\"{key}\" \"{value}\""))
        .collect();
    format!("{}{added}{}", &map[..at], &map[at..])
}

/// One K3 case: a map with a kinematic receiver and a tagged gated light,
/// its mover, the camera on the receiver, and the receiver's box.
struct PopCase {
    name: &'static str,
    source: String,
    mover: &'static str,
    tag: &'static str,
    /// An id-57 namespace the compiled map must gate a light in, when the
    /// tagged light is the map's only light of that namespace.
    gated_namespace: Option<MoverLightNamespace>,
    pose: Pose,
    receiver: ([f32; 3], [f32; 3]),
}

/// Mean summed RGB over `pixels` of `image`.
fn rgb_mean(image: &RgbaImage, pixels: &[(u32, u32)]) -> f64 {
    pixels
        .iter()
        .map(|&(x, y)| rgb_sum(image.get_pixel(x, y).0))
        .sum::<f64>()
        / pixels.len() as f64
}

/// The K3 bar on one case. Returns the misses.
fn pop_misses(run: &Run, case: PopCase) -> Vec<String> {
    let source = sibling_map(&run.workspace, &format!("k3-{}", case.name), &case.source);
    let stem = source
        .file_stem()
        .and_then(|stem| stem.to_str())
        .expect("generated map stem")
        .to_owned();
    let map = compile_dev_map(&run.workspace, &stem);
    if let Some(namespace) = case.gated_namespace {
        let channel = channel_of(&map);
        assert!(
            channel
                .direct_records
                .iter()
                .any(|record| record.namespace == namespace),
            "{}: id 57 gates a {namespace:?} light",
            case.name
        );
    }
    let stripped = without_channel(
        &run.workspace,
        &map,
        &format!("k3-{}-no-id57", case.name),
        false,
    );
    let shot = |prl: &Path, t: f32, w: f32, label: &str| {
        run.capture(
            prl,
            &format!("t11-k3-{}-{label}-t{t}-w{w}", case.name),
            case.pose,
            &Scene {
                resolution: RESOLUTION,
                force_movers: vec![(case.mover, t)],
                force_promotion: vec![(case.tag, w)],
                ..Scene::default()
            },
        )
    };
    let reference = [
        shot(&stripped, 1.0, 0.0, "stripped"),
        shot(&stripped, 1.0, 1.0, "stripped"),
    ];
    let region = ScreenBox::of(case.receiver.0, case.receiver.1, case.pose, RESOLUTION);
    let in_box: Vec<(u32, u32)> = reference[0]
        .enumerate_pixels()
        .filter(|(x, y, _)| region.contains(*x, *y))
        .map(|(x, y, _)| (x, y))
        .collect();
    assert!(
        !in_box.is_empty(),
        "{}: the receiver must be in view",
        case.name
    );
    // Lit texels: lit by either arm in the ungated reference.
    let lit: Vec<(u32, u32)> = in_box
        .iter()
        .copied()
        .filter(|&(x, y)| {
            reference
                .iter()
                .any(|image| rgb_sum(image.get_pixel(x, y).0) >= 30.0)
        })
        .collect();
    let pixels = if lit.len() >= 32 { &lit } else { &in_box };
    let pop =
        |pair: &[RgbaImage; 2]| (rgb_mean(&pair[1], pixels) - rgb_mean(&pair[0], pixels)).abs();
    let reference_pop = pop(&reference);
    let mut misses = Vec::new();
    for t in [0.0, 1.0] {
        let gated = [shot(&map, t, 0.0, "channel"), shot(&map, t, 1.0, "channel")];
        let gated_pop = pop(&gated);
        eprintln!(
            "K3 {} t {t}: pop {gated_pop:.3} (w=0 {:.3}, w=1 {:.3}) against ungated {reference_pop:.3} \
             (w=0 {:.3}, w=1 {:.3}) + {LIT_ENDPOINT_DRIFT}, over {} {} px",
            case.name,
            rgb_mean(&gated[0], pixels),
            rgb_mean(&gated[1], pixels),
            rgb_mean(&reference[0], pixels),
            rgb_mean(&reference[1], pixels),
            pixels.len(),
            if lit.len() >= 32 {
                "lit"
            } else {
                "receiver (none lit)"
            },
        );
        if gated_pop > reference_pop + LIT_ENDPOINT_DRIFT {
            misses.push(format!(
                "{} t {t}: pop {gated_pop} against {reference_pop} + {LIT_ENDPOINT_DRIFT}",
                case.name
            ));
        }
    }
    misses
}

/// The compiled map's id 57.
fn channel_of(prl: &Path) -> MoverProbeChannelSection {
    let section = read_sections(prl)
        .into_iter()
        .find(|section| section.section_id == SectionId::MoverProbeChannel as u32)
        .expect("the map ships id 57");
    MoverProbeChannelSection::from_bytes(&section.data).expect("id 57 decodes")
}

/// The set-piece with the arena spot carrying `keys` and the K3 slab.
fn setpiece_case(
    run: &Run,
    name: &'static str,
    tag: &'static str,
    keys: &[(&str, &str)],
) -> PopCase {
    PopCase {
        name,
        source: with_entity(
            &with_light_keys(
                &read_map(&run.workspace, "mover-setpiece"),
                ARENA_SPOT,
                keys,
            ),
            &slab_mover("k3_slab", SETPIECE_SLAB.0, SETPIECE_SLAB.1),
        ),
        mover: SETPIECE_WALL,
        tag,
        gated_namespace: None,
        pose: SETPIECE_SLAB_VIEW,
        receiver: SETPIECE_SLAB,
    }
}

// K3, static promotion. The set-piece's point lights are too dim for the
// entity-shadow selection; its spots are not, so the arena spot carries the
// pin. The closet gates no direct light (its id 57 has no direct block), so
// its case holds the bar with nothing gated: the kinematic-receiver slab
// behind the door and the centre bright-room light it promotes
// (mover-closet.README.md §Reference variants).
#[test]
#[ignore = "requires a GPU adapter and a local prl-build bake; run with `cargo test -p postretro --features capture --test capture_mover_promotion_pop -- --ignored`"]
fn k3_promotion_pops_no_more_at_a_forced_terminus_than_the_ungated_arms() {
    let run = Run::new();
    let closet_slab = ([296.0, 505.0, 0.0], [344.0, 521.0, 8.0]);
    let closet = PopCase {
        name: "closet",
        source: with_entity(
            &with_light_keys(
                &read_map(&run.workspace, "mover-closet"),
                "320 392 288",
                &[("_tags", "centre_light")],
            ),
            &slab_mover("k3_slab", closet_slab.0, closet_slab.1),
        ),
        mover: CLOSET_DOOR,
        tag: "centre_light",
        gated_namespace: None,
        pose: CLOSET_STATION_3,
        receiver: closet_slab,
    };
    let setpiece = setpiece_case(&run, "setpiece", "k3_light", &[("_tags", "k3_light")]);
    let misses: Vec<String> = [setpiece, closet]
        .into_iter()
        .flat_map(|case| pop_misses(&run, case))
        .collect();
    assert!(
        misses.is_empty(),
        "K3 misses its bar:\n{}",
        misses.join("\n")
    );
}

// K3, animated promotion. The set-piece's arena spot made animated-baked
// (`_animated 1`), so id 45 bakes it and id 57 gates it in namespace 1; its
// animated promotion is pinned at w = 0 and 1 on the same slab.
#[test]
#[ignore = "requires a GPU adapter and a local prl-build bake; run with `cargo test -p postretro --features capture --test capture_mover_promotion_pop -- --ignored`"]
fn k3_animated_promotion_pops_no_more_at_a_forced_terminus_than_the_ungated_arms() {
    let run = Run::new();
    let case = PopCase {
        gated_namespace: Some(MoverLightNamespace::AnimatedBaked),
        ..setpiece_case(
            &run,
            "setpiece-animated",
            "k3_animated_light",
            &[("_tags", "k3_animated_light"), ("_animated", "1")],
        )
    };
    let misses = pop_misses(&run, case);
    assert!(
        misses.is_empty(),
        "K3 (animated) misses its bar:\n{}",
        misses.join("\n")
    );
}

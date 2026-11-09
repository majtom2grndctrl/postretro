//! Mover probe channel, bounce and depth halves, against reference builds where the mover is a
//! static brush at its pose (bake rows B3, B4, B7) and against an animated light added to the
//! bright room (B11, bake side). Every SH base here is stored at uniform L0 (`_sh_coarsen "0"`),
//! so id 34 reads back per probe; the references and today's id 34 build uncompressed, so they
//! read the gathered tile itself, and the channel builds BC6H, as production does.
//! See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel)

mod mover_bounce_support;
mod prl_build_support;

use std::sync::OnceLock;

use mover_bounce_support::{
    Built, Cache, Volume, bounce_states_in, build, channel, depth_tiles, dev_map, display_levels,
    f16_close, max_abs, round_trip_error, staticize, with_worldspawn_key,
};
use postretro_level_format::SectionId;
use postretro_level_format::kinematic_geometry::KinematicGeometrySection;
use postretro_level_format::probe_depth_tiles::PROBE_DEPTH_TILE_TEXELS;

const UNCOMPRESSED: &[&str] = &["--uncompressed-irradiance"];

fn uniform(map: &str) -> String {
    with_worldspawn_key(map, "_sh_coarsen", "0")
}

fn mover_id(built: &Built, name: &str) -> u32 {
    let movers = KinematicGeometrySection::from_bytes(
        &mover_bounce_support::section(&built.prl, SectionId::KinematicGeometry).expect("id 43"),
    )
    .expect("id 43 decodes");
    movers
        .movers
        .iter()
        .find(|mover| mover.name == name)
        .map(|mover| mover.mover_id)
        .unwrap_or_else(|| panic!("no mover named {name}"))
}

/// The closet builds: the channel (BC6H), today's id 34 and the static-door
/// reference (both uncompressed).
struct Closet {
    channel: Built,
    today: Built,
    reference: Built,
}

fn closet() -> &'static Closet {
    static BUILT: OnceLock<Closet> = OnceLock::new();
    BUILT.get_or_init(|| {
        let map = uniform(&dev_map("mover-closet.map"));
        Closet {
            channel: build(&map, "mover-closet", Cache::Cold, &[]),
            today: build(&map, "mover-closet", Cache::Cold, UNCOMPRESSED),
            reference: build(
                &uniform(&dev_map("mover-closet-static-door.map")),
                "mover-closet-static-door",
                Cache::Cold,
                UNCOMPRESSED,
            ),
        }
    })
}

/// A probe past the door's closet face, inside the closet's walls.
fn in_closet(quake: [f64; 3]) -> bool {
    (160.0..480.0).contains(&quake[0]) && quake[1] > 504.0 && quake[1] < 760.0 && quake[2] < 128.0
}

// Bake row B3: with the door closed, the composed base at every closet probe
// valid in both bakes (S^b₀ where the probe stores it, today's id 34 where it
// does not) equals the static-door reference's id 34 within that tile's BC6H
// round-trip error. With the door open, every stored S^b₁ equals today's id
// 34 within the same bar; an unstored one composes id 34 itself (owner,
// 2026-11-09).
#[test]
fn closed_door_closet_bounce_matches_the_static_door_reference() {
    let closet = closet();
    let door = mover_id(&closet.channel, "closet_door");
    let channel = channel(&closet.channel.prl).expect("the closed door changes the closet");
    let today = Volume::read(&closet.today.prl);
    let reference = Volume::read(&closet.reference.prl);
    assert_eq!(
        today.section.grid_dimensions, reference.section.grid_dimensions,
        "both builds lay one probe grid"
    );
    let states = bounce_states_in(&channel, &today, door);
    let (mut compared, mut stored, mut worst) = (0, 0, (0.0f32, 0.0f32));
    for probe in 0..today.probe_count() {
        if !today.is_valid(probe)
            || !reference.is_valid(probe)
            || !in_closet(today.quake_position(probe))
        {
            continue;
        }
        let reference_tile = reference.tile(probe);
        let composed = match states.get(&probe) {
            Some([Some(closed), _]) => {
                stored += 1;
                closed.clone()
            }
            Some([None, _]) | None => today.tile(probe),
        };
        let (difference, bar) = (
            max_abs(&composed, &reference_tile),
            round_trip_error(&reference_tile),
        );
        if difference - bar > worst.0 - worst.1 {
            worst = (difference, bar);
        }
        assert!(
            difference <= bar,
            "closet probe {probe} at {:?}: |composed − reference| {difference} past the \
             reference tile's round-trip error {bar}",
            today.quake_position(probe)
        );
        compared += 1;
    }
    eprintln!(
        "B3: {compared} closet probes valid in both, {stored} storing S^b; worst |composed − \
         reference| {} against its bar {}",
        worst.0, worst.1
    );
    assert!(compared > 100, "the closet holds its probes: {compared}");
    assert!(
        stored > 100,
        "the closed door stores the closet's bounce: {stored}"
    );

    let mut open_stored = 0;
    for (probe, [_, open]) in &states {
        let Some(open) = open else {
            continue;
        };
        open_stored += 1;
        let today_tile = today.tile(*probe);
        let (difference, bar) = (max_abs(open, &today_tile), round_trip_error(&today_tile));
        assert!(
            difference <= bar,
            "probe {probe}: |S^b₁ − today| {difference} past its round-trip error {bar}"
        );
    }
    eprintln!(
        "B3: {} affected probes, {open_stored} storing S^b₁ (the rest compose id 34 open)",
        states.len()
    );
}

// Bake row B4: every probe the static-door reference tiles either carries a
// closed-pose tile equal to the reference's id-59 tile within f16 rounding
// (its gather rays meet the closed door), or keeps today's id-59 tile, equal
// to the reference's (they never meet it). Every depth record is the door's,
// at a valid probe.
#[test]
fn closed_door_depth_tiles_match_the_static_door_references_tiles() {
    let closet = closet();
    let door = mover_id(&closet.channel, "closet_door");
    let channel = channel(&closet.channel.prl).expect("the closed door changes the closet");
    let reference = depth_tiles(&closet.reference.prl);
    let today = depth_tiles(&closet.channel.prl);
    let mut met = 0;
    for (index, &probe) in reference.probe_indices.iter().enumerate() {
        let expected = reference.tile(index);
        let record = channel
            .depth_record_tiles()
            .find(|(record, _)| record.mover_id == door && record.probe_index == probe);
        let actual = match record {
            Some((_, [Some(closed), _])) => {
                met += 1;
                closed
            }
            Some((_, [None, _])) | None => {
                let tile = today.tile_of_probe(probe).unwrap_or_else(|| {
                    panic!("reference-tiled probe {probe} carries neither mover depth nor id 59")
                });
                today.tile(usize::from(tile))
            }
        };
        assert_eq!(actual.len(), PROBE_DEPTH_TILE_TEXELS);
        for (texel, (a, b)) in actual.iter().zip(expected).enumerate() {
            assert!(
                f16_close(a[0], b[0]) && f16_close(a[1], b[1]),
                "probe {probe} texel {texel}: {a:?} against the reference's {b:?} \
                 (closed-pose: {})",
                record.is_some()
            );
        }
    }
    eprintln!(
        "B4: {} reference tiles, {met} carried as closed-pose depth, {} id-57 depth records",
        reference.tile_count(),
        channel.depth_records.len()
    );
    assert!(met > 0, "the closed door's probes carry closed-pose depth");
    assert!(
        channel
            .depth_records
            .iter()
            .all(|record| record.mover_id == door)
    );
}

// Bake row B4, last clause, and B17's no-change map: a mover no probe ray can
// reach emits no section, so no depth block.
#[test]
fn a_mover_no_probe_ray_meets_emits_no_depth_block() {
    let far_slab = "{\n\"classname\" \"kinematic_mover\"\n\"name\" \"far_slab\"\n\"path\" \
                    \"far_a\"\n\"speed\" \"1\"\n{\n( 8000 8000 0 ) ( 8000 8001 0 ) ( 8000 8000 1 ) \
                    metal/metal_rough_046a 0 0 0 1 1\n( 8064 8000 0 ) ( 8064 8000 1 ) ( 8064 8001 \
                    0 ) metal/metal_rough_046a 0 0 0 1 1\n( 8000 8000 0 ) ( 8000 8000 1 ) ( 8001 \
                    8000 0 ) metal/metal_rough_046a 0 0 0 1 1\n( 8000 8064 0 ) ( 8001 8064 0 ) ( \
                    8000 8064 1 ) metal/metal_rough_046a 0 0 0 1 1\n( 8000 8000 0 ) ( 8001 8000 0 ) \
                    ( 8000 8001 0 ) metal/metal_rough_046a 0 0 0 1 1\n( 8000 8000 64 ) ( 8000 8001 \
                    64 ) ( 8001 8000 64 ) metal/metal_rough_046a 0 0 0 1 1\n}\n}\n{\n\"classname\" \
                    \"kinematic_waypoint\"\n\"name\" \"far_a\"\n\"next\" \"far_b\"\n\"origin\" \
                    \"8032 8032 32\"\n}\n{\n\"classname\" \"kinematic_waypoint\"\n\"name\" \
                    \"far_b\"\n\"origin\" \"8032 8032 160\"\n}\n";
    let map = format!("{}\n{far_slab}", dev_map("mover-closet-no-door.map"));
    let built = build(&map, "mover-closet-far-mover", Cache::Cold, &[]);
    assert!(channel(&built.prl).is_none(), "{}", built.logs);
}

/// The setpiece builds: the channel (BC6H), today's id 34 and the reference
/// with the room static at its raised pose (both uncompressed).
struct Setpiece {
    channel: Built,
    today: Built,
    reference: Built,
}

fn setpiece() -> &'static Setpiece {
    static BUILT: OnceLock<Setpiece> = OnceLock::new();
    BUILT.get_or_init(|| {
        let map = uniform(&dev_map("mover-setpiece.map"));
        let raised = staticize(&map, "setpiece_room", "setpiece_room_raised");
        Setpiece {
            channel: build(&map, "mover-setpiece", Cache::Cold, &[]),
            today: build(&map, "mover-setpiece", Cache::Cold, UNCOMPRESSED),
            reference: build(
                &raised,
                "mover-setpiece-room-raised",
                Cache::Cold,
                UNCOMPRESSED,
            ),
        }
    })
}

/// The raised room's interior, Quake units: inside its walls, floor and
/// roof, the authored lowered brushes lifted 176 units.
fn in_raised_room(quake: [f64; 3]) -> bool {
    const MIN: [f64; 3] = [848.0, 176.0, 0.0];
    const MAX: [f64; 3] = [1136.0, 464.0, 160.0];
    (0..3).all(|axis| MIN[axis] < quake[axis] && quake[axis] < MAX[axis])
}

// Bake row B7 (restated, owner, Stop 2): with `setpiece_room` raised, at every
// interior probe valid in both bakes, the composed base (S^b₁ where the probe
// stores an entry, today's id 34 where it does not) is nearer the
// static-room reference's id 34 than today's id 34 is, and within 3 display
// levels of it.
#[test]
fn raised_room_bounce_is_nearer_the_static_room_reference_than_today() {
    let setpiece = setpiece();
    let room = mover_id(&setpiece.channel, "setpiece_room");
    let channel = channel(&setpiece.channel.prl).expect("the setpiece's movers change probes");
    let today = Volume::read(&setpiece.today.prl);
    let reference = Volume::read(&setpiece.reference.prl);
    assert_eq!(
        today.section.grid_dimensions, reference.section.grid_dimensions,
        "both builds lay one probe grid"
    );
    let states = bounce_states_in(&channel, &today, room);
    let mut levels = Vec::new();
    let (mut compared, mut stored, mut worst_ratio) = (0, 0, 0.0f32);
    for probe in 0..today.probe_count() {
        if !today.is_valid(probe)
            || !reference.is_valid(probe)
            || !in_raised_room(today.quake_position(probe))
        {
            continue;
        }
        let (today_tile, reference_tile) = (today.tile(probe), reference.tile(probe));
        let composed = match states.get(&probe) {
            Some([_, Some(raised)]) => {
                stored += 1;
                raised.clone()
            }
            Some([_, None]) | None => today_tile.clone(),
        };
        let (composed_gap, today_gap) = (
            max_abs(&composed, &reference_tile),
            max_abs(&today_tile, &reference_tile),
        );
        let level = display_levels(&composed, &reference_tile);
        assert!(
            composed_gap < today_gap,
            "interior probe {probe} at {:?}: composed {composed_gap} is no nearer the \
             reference than today's {today_gap}",
            today.quake_position(probe)
        );
        assert!(
            level <= 3.0,
            "interior probe {probe} at {:?}: composed reads {level} display levels from the \
             reference",
            today.quake_position(probe)
        );
        worst_ratio = worst_ratio.max(composed_gap / today_gap);
        levels.push(level);
        compared += 1;
    }
    levels.sort_by(f32::total_cmp);
    eprintln!(
        "B7: {compared} interior probes valid in both, {stored} storing S^b; composed against \
         reference: median {} max {} display levels; |composed − ref| / |today − ref| max \
         {worst_ratio}",
        levels.get(levels.len() / 2).copied().unwrap_or(f32::NAN),
        levels.last().copied().unwrap_or(f32::NAN),
    );
    assert!(compared > 100, "the room holds its probes: {compared}");
}

// Bake row B11, bake side: an animated light added to the closet's bright
// room leaves every bounce-state and depth byte unchanged (it shapes only
// the direct block). It is appended last, so no static light's seed shifts.
#[test]
fn an_animated_light_leaves_the_bounce_and_depth_blocks_unchanged() {
    let closet = closet();
    let animated = "{\n\"classname\" \"light\"\n\"origin\" \"320 224 200\"\n\"light\" \
                    \"200\"\n\"_falloff_range\" \"600\"\n\"style\" \"1\"\n}\n";
    let map = format!("{}\n{animated}", uniform(&dev_map("mover-closet.map")));
    let with_animated = build(&map, "mover-closet", Cache::Cold, &[]);
    let (before, after) = (
        channel(&closet.channel.prl).expect("id 57"),
        channel(&with_animated.prl).expect("id 57"),
    );
    assert_eq!(after.bounce_entries, before.bounce_entries);
    assert_eq!(after.bounce_tiles, before.bounce_tiles);
    assert_eq!(after.depth_records, before.depth_records);
    assert_eq!(after.depth_tiles, before.depth_tiles);
    assert!(!before.bounce_tiles.is_empty());
}

// Bake row B14: a budget one byte under the SH base gate's mover-priced peak
// refuses the build with the gate's named error before any posed bounce ray:
// no mover's bounce half logs a bake.
#[test]
fn a_budget_under_the_mover_priced_peak_refuses_before_any_bounce_ray() {
    let map = dev_map("mover-closet.map");
    let admitted = build(&map, "mover-closet", Cache::Cold, &[]);
    let peak: u64 = admitted
        .logs
        .lines()
        .find_map(|line| line.split_once("SH base working-set gate: estimated peak "))
        .and_then(|(_, tail)| tail.split_once(" bytes"))
        .map(|(peak, _)| peak.parse().expect("a byte count"))
        .unwrap_or_else(|| panic!("the gate logs its peak\n{}", admitted.logs));
    assert!(
        admitted
            .logs
            .contains("[MoverLight] probe channel bounce: mover"),
        "{}",
        admitted.logs
    );
    let budget = (peak - 1).to_string();
    let logs = mover_bounce_support::try_build(
        &map,
        "mover-closet",
        Cache::Cold,
        &["--working-set-max-size", &budget],
    )
    .err()
    .expect("the mover channel's peak exceeds the budget");
    assert!(
        logs.contains("SH base working-set gate refused before patch shading")
            && logs.contains("mover bounce"),
        "{logs}"
    );
    assert!(
        !logs.contains("[MoverLight] probe channel bounce: mover"),
        "no posed bounce ray before the refusal\n{logs}"
    );
}

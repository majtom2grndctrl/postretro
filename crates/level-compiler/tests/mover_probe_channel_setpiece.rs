//! Mover probe channel on `mover-setpiece` (bake row B2): which lights `setpiece_wall` closed
//! and `setpiece_room` raised gate, at which probes, read from the packed id 57, which the
//! runtime loader accepts.
//! See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel)

mod prl_build_support;

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use postretro_level_format::alpha_lights::AlphaLightsSection;
use postretro_level_format::kinematic_geometry::KinematicGeometrySection;
use postretro_level_format::mover_probe_channel::{MoverLightNamespace, MoverProbeChannelSection};
use postretro_level_format::sh_reconstruct::local_xyz;
use postretro_level_format::sh_volume::OctahedralShVolumeSection;
use postretro_level_format::{SectionId, read_container, read_section_data};
use prl_build_support::{REACH_VERIFY_ENV, prl_build};

/// Quake map units to engine meters.
const UNIT: f64 = 0.0254;

/// The arena lights whose falloff reaches past the wall: three points and
/// two spots aimed at it (`mover_light_gate_setpiece.rs`).
const ARENA_LIGHTS: [[f64; 3]; 5] = [
    [256.0, 256.0, 256.0],
    [640.0, 320.0, 192.0],
    [992.0, 320.0, 288.0],
    [368.0, 560.0, 288.0],
    [60.0, 900.0, 250.0],
];
/// Two points, and the spot aimed at the plinth just behind the wall.
const BACK_ROOM_LIGHTS: [[f64; 3]; 3] = [
    [240.0, 1376.0, 256.0],
    [1000.0, 1376.0, 256.0],
    [150.0, 1420.0, 290.0],
];
/// The arena spots that never reach the wall.
const WALL_SHY_SPOTS: [[f64; 3]; 2] = [[560.0, 760.0, 305.0], [560.0, 60.0, 300.0]];
/// The point above the shaft the room rises into.
const SHAFT_LIGHT: [f64; 3] = [992.0, 320.0, 288.0];

/// The arena ends at Quake y 1024 and the back room runs from y 1088.
const ARENA_EDGE_Y: f64 = 1024.0;
const BACK_ROOM_EDGE_Y: f64 = 1088.0;
/// The raised room's interior, Quake units: inside its walls, floor and
/// roof, the authored lowered brushes lifted 176 units.
const ROOM_INTERIOR_MIN: [f64; 3] = [848.0, 176.0, 0.0];
const ROOM_INTERIOR_MAX: [f64; 3] = [1136.0, 464.0, 160.0];

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|crates| crates.parent())
        .expect("workspace root")
        .to_path_buf()
}

/// Quake coordinates of an engine point: engine x is Quake −y, engine y is
/// Quake z, engine z is Quake −x.
fn quake(engine: [f64; 3]) -> [f64; 3] {
    [-engine[2] / UNIT, -engine[0] / UNIT, engine[1] / UNIT]
}

struct Setpiece {
    _dir: tempfile::TempDir,
    prl: PathBuf,
}

/// The one cold build every test here reads.
fn setpiece() -> &'static Setpiece {
    static BUILT: OnceLock<Setpiece> = OnceLock::new();
    BUILT.get_or_init(|| {
        let workspace = workspace_root();
        let dir = tempfile::tempdir().expect("create output directory");
        let prl = dir.path().join("mover-setpiece.prl");
        let output = prl_build()
            .env(REACH_VERIFY_ENV, "0")
            .arg(workspace.join("content/dev/maps/mover-setpiece.map"))
            .arg("-o")
            .arg(&prl)
            .args(["--no-cache", "--no-tui", "--jobs", "6"])
            .arg("--baked-root")
            .arg(dir.path().join("baked"))
            .current_dir(&workspace)
            .output()
            .expect("spawn prl-build");
        assert!(
            output.status.success(),
            "prl-build failed\n{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        Setpiece { _dir: dir, prl }
    })
}

fn section(prl: &Path, id: SectionId) -> Option<Vec<u8>> {
    let bytes = std::fs::read(prl).expect("read the packed PRL");
    let mut cursor = Cursor::new(bytes.as_slice());
    let meta = read_container(&mut cursor).expect("read the container");
    read_section_data(&mut cursor, &meta, id as u32).expect("read the section")
}

/// One stored visibility pair at one probe, in Quake coordinates.
#[derive(Debug, Clone, Copy)]
struct Gate {
    mover: u32,
    light: [f64; 3],
    probe: [f64; 3],
    q: [u8; 2],
}

/// Every (mover, valid probe) of each term record's cell in id 57's direct
/// block, with the mover and the record's static light's authored origin.
fn gates() -> (Vec<Gate>, Vec<(String, u32)>) {
    let prl = &setpiece().prl;
    let channel = MoverProbeChannelSection::from_bytes(
        &section(prl, SectionId::MoverProbeChannel).expect("setpiece's movers gate lights"),
    )
    .expect("id 57 decodes");
    let base = OctahedralShVolumeSection::from_bytes(
        &section(prl, SectionId::OctahedralShVolume).expect("id 34"),
    )
    .expect("id 34 decodes");
    let lights =
        AlphaLightsSection::from_bytes(&section(prl, SectionId::AlphaLights).expect("id 18"))
            .expect("id 18 decodes");
    let movers = KinematicGeometrySection::from_bytes(
        &section(prl, SectionId::KinematicGeometry).expect("id 43"),
    )
    .expect("id 43 decodes");

    let [ax, ay, _] = channel.affinity_dims.map(|dim| dim as usize);
    let mut gates = Vec::new();
    for view in channel.direct_record_views() {
        assert_eq!(view.record.namespace, MoverLightNamespace::StaticLight);
        let light = quake(lights.lights[view.record.light_index as usize].origin);
        let cell = view.cell as usize;
        let (cx, cy, cz) = (cell % ax, cell / ax % ay, cell / (ax * ay));
        for mover in view.movers() {
            // Every valid probe of the cell, as the dense layout stored it: a
            // probe outside the record's mask keeps q = 1 at both termini.
            let mut slot = 0;
            for local in 0..64 {
                if view.valid_probe_mask & (1 << local) == 0 {
                    continue;
                }
                let q = if view.record.probe_mask & (1 << local) != 0 {
                    slot += 1;
                    mover.q(slot - 1)
                } else {
                    [255, 255]
                };
                let (lx, ly, lz) = local_xyz(local);
                let grid = [cx * 4 + lx, cy * 4 + ly, cz * 4 + lz];
                let engine: [f64; 3] = std::array::from_fn(|axis| {
                    f64::from(base.grid_origin[axis])
                        + grid[axis] as f64 * f64::from(base.cell_size[axis])
                });
                gates.push(Gate {
                    mover: mover.mover_id,
                    light,
                    probe: quake(engine),
                    q,
                });
            }
        }
    }
    let names = movers
        .movers
        .iter()
        .map(|mover| (mover.name.clone(), mover.mover_id))
        .collect();
    (gates, names)
}

fn mover_id(names: &[(String, u32)], name: &str) -> u32 {
    names
        .iter()
        .find(|(candidate, _)| candidate == name)
        .map(|&(_, id)| id)
        .unwrap_or_else(|| panic!("no mover named {name}"))
}

fn same_light(a: [f64; 3], b: [f64; 3]) -> bool {
    (0..3).all(|axis| (a[axis] - b[axis]).abs() < 0.01)
}

// The packed channel loads: the loader's checks against ids 34, 18 and 43
// pass, and the movers take their author ramps.
#[test]
fn the_setpiece_channel_loads() {
    let world = postretro_level_loader::load_prl(setpiece().prl.to_str().expect("UTF-8 path"))
        .expect("the runtime loads the packed channel");
    let channel = world.mover_probe_channel().expect("id 57 is present");
    assert!(!channel.direct_records.is_empty());
    assert_eq!(channel.ramps.len(), 2, "both movers gate a light");
}

// Bake row B2: `setpiece_wall` closed stores q < 1 at back-room probes for
// every arena light that reaches the wall and at arena probes for every
// back-room light, and stores no entry for the two spots that never reach
// it; `setpiece_room` raised stores q < 1 at its interior probes for the
// shaft light.
#[test]
fn setpiece_wall_and_room_gate_the_lights_that_reach_them() {
    let (gates, names) = gates();
    let wall = mover_id(&names, "setpiece_wall");
    let room = mover_id(&names, "setpiece_room");
    let closed = |light: [f64; 3], probe_side: fn(f64) -> bool| {
        gates.iter().any(|gate| {
            gate.mover == wall
                && same_light(gate.light, light)
                && probe_side(gate.probe[1])
                && gate.q[0] < 255
        })
    };
    for light in ARENA_LIGHTS {
        assert!(
            closed(light, |y| y > BACK_ROOM_EDGE_Y),
            "arena light {light:?} gated at back-room probes"
        );
    }
    for light in BACK_ROOM_LIGHTS {
        assert!(
            closed(light, |y| y < ARENA_EDGE_Y),
            "back-room light {light:?} gated at arena probes"
        );
    }
    for light in WALL_SHY_SPOTS {
        assert!(
            !gates
                .iter()
                .any(|gate| gate.mover == wall && same_light(gate.light, light)),
            "spot {light:?} never reaches the wall"
        );
    }

    let inside = |probe: [f64; 3]| {
        (0..3).all(|axis| {
            ROOM_INTERIOR_MIN[axis] < probe[axis] && probe[axis] < ROOM_INTERIOR_MAX[axis]
        })
    };
    let interior: Vec<&Gate> = gates
        .iter()
        .filter(|gate| {
            gate.mover == room && same_light(gate.light, SHAFT_LIGHT) && inside(gate.probe)
        })
        .collect();
    assert!(
        !interior.is_empty(),
        "the raised room stores the shaft light inside it"
    );
    let open: Vec<&&Gate> = interior.iter().filter(|gate| gate.q[1] == 255).collect();
    assert!(
        open.is_empty(),
        "every interior probe the room stores reads the shaft light gated raised: {open:?}"
    );
}

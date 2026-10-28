// Harness for the runtime-cell rows: fixture maps authored as `.map` text in
// Quake units, their runtime cells built in process by the production
// functions, the pre-change build beside them, and full compiles loaded
// through the runtime loader.
// See: context/plans/in-progress/occluder-first-partition/index.md §Acceptance

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use glam::{DVec3, Mat4, Vec3};
use postretro_level_format::cell_locator::CellLocatorSection;
use postretro_level_format::cells::CellsSection;
use postretro_level_format::portals::PortalsSection;
use postretro_level_format::texture_names::TextureNamesSection;
use postretro_level_loader::LevelWorld;
use postretro_visibility::{VisibilityPath, VisibleCells};

use super::{OccluderPartition, RuntimeCells, partition_with_occluders};
use crate::map_data::MapData;
use crate::map_format::MapFormat;
use crate::partition::{self, BspTree, find_leaf_for_point};
use crate::portals::{self, Portal};
use crate::{pack, visibility};

/// Texture on every fixture brush; any name compiles, a missing file bakes
/// a placeholder.
const TEXTURE: &str = "debug_wall_grey";

/// Slab thickness of a fixture hall, Quake units.
const HALL_SLAB_Q: f64 = 16.0;

/// Engine-space metres of a Quake-unit point.
pub(super) fn engine(q: [f64; 3]) -> DVec3 {
    crate::parse::quake_to_engine(DVec3::from_array(q)) * MapFormat::IdTech2.units_to_meters()
}

/// A fixture map: worldspawn brushes, then entities, in Quake units.
#[derive(Clone, Default)]
pub(super) struct MapText {
    world_brushes: Vec<String>,
    entities: Vec<String>,
}

impl MapText {
    pub(super) fn new() -> Self {
        Self::default()
    }

    /// A worldspawn axis-aligned box brush.
    pub(super) fn solid(&mut self, min: [f64; 3], max: [f64; 3]) -> &mut Self {
        self.world_brushes.push(box_brush(min, max));
        self
    }

    /// A sealed hall of 16-unit slabs overlapping at the corners, interior
    /// `[0, size]`.
    pub(super) fn hall(&mut self, size: [f64; 3]) -> &mut Self {
        let t = HALL_SLAB_Q;
        let [x, y, z] = size;
        self.solid([-t, -t, -t], [x + t, y + t, 0.0])
            .solid([-t, -t, z], [x + t, y + t, z + t])
            .solid([-t, -t, 0.0], [0.0, y + t, z])
            .solid([x, -t, 0.0], [x + t, y + t, z])
            .solid([0.0, -t, 0.0], [x, 0.0, z])
            .solid([0.0, y, 0.0], [x, y + t, z])
    }

    /// A static point light.
    pub(super) fn light(&mut self, origin: [f64; 3], range: f64) -> &mut Self {
        self.point_entity(
            "light",
            &[
                ("origin", &quake_triple(origin)),
                ("light", "300"),
                ("_falloff_range", &format!("{range}")),
            ],
        )
    }

    pub(super) fn point_entity(&mut self, classname: &str, keys: &[(&str, &str)]) -> &mut Self {
        self.entities.push(entity(classname, keys, &[]));
        self
    }

    pub(super) fn brush_entity(
        &mut self,
        classname: &str,
        keys: &[(&str, &str)],
        boxes: &[([f64; 3], [f64; 3])],
    ) -> &mut Self {
        let brushes: Vec<String> = boxes.iter().map(|&(a, b)| box_brush(a, b)).collect();
        self.entities.push(entity(classname, keys, &brushes));
        self
    }

    /// A `kinematic_mover` of axis-aligned boxes, authored closed where it
    /// stands, with a second waypoint `rise` units above to slide open to.
    pub(super) fn mover(
        &mut self,
        name: &str,
        boxes: &[([f64; 3], [f64; 3])],
        rise: f64,
    ) -> &mut Self {
        let (mut min, mut max) = (DVec3::splat(f64::INFINITY), DVec3::splat(f64::NEG_INFINITY));
        for &(a, b) in boxes {
            min = min.min(DVec3::from_array(a));
            max = max.max(DVec3::from_array(b));
        }
        let closed = ((min + max) * 0.5).to_array();
        let open = (DVec3::from_array(closed) + DVec3::Z * rise).to_array();
        let (closed_name, open_name) = (format!("{name}_closed"), format!("{name}_open"));
        self.brush_entity(
            "kinematic_mover",
            &[
                ("name", name),
                ("path", &closed_name),
                ("speed", "4"),
                ("move_mode", "once"),
                ("start_on_spawn", "0"),
            ],
            boxes,
        )
        .point_entity(
            "kinematic_waypoint",
            &[
                ("name", &closed_name),
                ("next", &open_name),
                ("origin", &quake_triple(closed)),
            ],
        )
        .point_entity(
            "kinematic_waypoint",
            &[("name", &open_name), ("origin", &quake_triple(open))],
        )
    }

    pub(super) fn text(&self) -> String {
        let mut entities = vec![entity("worldspawn", &[], &self.world_brushes)];
        entities.extend(self.entities.iter().cloned());
        entities.join("\n") + "\n"
    }

    /// Parse the map as `prl-build` does, through unit scale.
    pub(super) fn parse(&self) -> MapData {
        let dir = tempfile::tempdir().expect("temporary map directory");
        let path = write_map(dir.path(), self);
        crate::parse::parse_map_file(&path, MapFormat::IdTech2).expect("fixture map parses")
    }
}

/// `"x y z"`, as map keys spell a point.
pub(super) fn quake_triple([x, y, z]: [f64; 3]) -> String {
    format!("{x} {y} {z}")
}

fn entity(classname: &str, keys: &[(&str, &str)], brushes: &[String]) -> String {
    let mut text = format!("{{\n\"classname\" \"{classname}\"\n");
    for (key, value) in keys {
        text.push_str(&format!("\"{key}\" \"{value}\"\n"));
    }
    for brush in brushes {
        text.push_str(brush);
        text.push('\n');
    }
    text.push('}');
    text
}

fn box_brush(min: [f64; 3], max: [f64; 3]) -> String {
    let ([x0, y0, z0], [x1, y1, z1]) = (min, max);
    format!(
        "{{\n\
         ( {x0} 0 0 ) ( {x0} 1 0 ) ( {x0} 0 1 ) {TEXTURE} 0 0 0 1 1\n\
         ( {x1} 0 0 ) ( {x1} 0 1 ) ( {x1} 1 0 ) {TEXTURE} 0 0 0 1 1\n\
         ( 0 {y0} 0 ) ( 0 {y0} 1 ) ( 1 {y0} 0 ) {TEXTURE} 0 0 0 1 1\n\
         ( 0 {y1} 0 ) ( 1 {y1} 0 ) ( 0 {y1} 1 ) {TEXTURE} 0 0 0 1 1\n\
         ( 0 0 {z0} ) ( 1 0 {z0} ) ( 0 1 {z0} ) {TEXTURE} 0 0 0 1 1\n\
         ( 0 0 {z1} ) ( 0 1 {z1} ) ( 1 0 {z1} ) {TEXTURE} 0 0 0 1 1\n\
         }}"
    )
}

/// `<dir>/maps/fixture.map`, so the content root resolves inside `dir`.
fn write_map(dir: &Path, map: &MapText) -> PathBuf {
    let maps = dir.join("maps");
    std::fs::create_dir_all(&maps).expect("create fixture maps directory");
    let path = maps.join("fixture.map");
    std::fs::write(&path, map.text()).expect("write fixture map");
    path
}

// ---- in process ----

/// A fixture's runtime cells, built in process by the production stages:
/// the occluder partition, the full tree's portals and exterior flood, and
/// the cell view with its portals.
pub(super) struct Cells {
    pub(super) map_data: MapData,
    pub(super) partition: OccluderPartition,
    pub(super) full_exterior: HashSet<usize>,
    pub(super) runtime: RuntimeCells,
}

impl Cells {
    pub(super) fn of(map: &MapText) -> Self {
        Self::of_map_data(map.parse())
    }

    pub(super) fn of_map_data(map_data: MapData) -> Self {
        let partition =
            partition_with_occluders(&map_data.brush_volumes, &map_data.kinematic_movers)
                .expect("occluder partition builds");
        let full_portals = portals::generate_portals(&partition.result.tree);
        let full_exterior = visibility::find_exterior_leaves(&partition.result.tree, &full_portals);
        let runtime = RuntimeCells::build(
            &partition.result.tree,
            &partition.tier,
            &full_exterior,
            full_portals.len(),
        );
        Self {
            map_data,
            partition,
            full_exterior,
            runtime,
        }
    }

    pub(super) fn full_tree(&self) -> &BspTree {
        &self.partition.result.tree
    }

    pub(super) fn cell_tree(&self) -> &BspTree {
        &self.runtime.cells.tree
    }

    /// Cell of a Quake-unit point.
    pub(super) fn cell_at(&self, q: [f64; 3]) -> usize {
        find_leaf_for_point(self.cell_tree(), engine(q))
    }

    /// Full-tree leaf of a Quake-unit point.
    pub(super) fn leaf_at(&self, q: [f64; 3]) -> usize {
        find_leaf_for_point(self.full_tree(), engine(q))
    }

    pub(super) fn is_interior_cell(&self, cell: usize) -> bool {
        !self.cell_tree().leaves[cell].is_solid
            && !self.runtime.cells.exterior_cells.contains(&cell)
    }

    /// The full-tree leaves grouped into `cell`, ascending.
    pub(super) fn members(&self, cell: usize) -> Vec<usize> {
        (0..self.runtime.cells.leaf_cell.len())
            .filter(|&leaf| self.runtime.cells.leaf_cell[leaf] == cell)
            .collect()
    }

    /// Per mover, the runtime portal ids its closed pose seals, as the
    /// KinematicGeometry encoder records them.
    pub(super) fn sealed_portals(&self) -> Vec<Vec<u32>> {
        let mut texture_names = TextureNamesSection { names: Vec::new() };
        crate::kinematic_geometry::encode_kinematic_geometry_section(
            &self.map_data.kinematic_movers,
            &self.map_data.kinematic_waypoints,
            &[],
            &self.runtime.portals,
            &mut texture_names,
        )
        .map(|section| {
            section
                .movers
                .into_iter()
                .map(|mover| mover.sealed_portal_ids)
                .collect()
        })
        .unwrap_or_default()
    }

    /// Cells, Portals and CellLocator as the pack stage encodes them.
    pub(super) fn sections(&self) -> (CellsSection, PortalsSection, CellLocatorSection) {
        encode_sections(
            self.cell_tree(),
            &self.runtime.portals,
            &self.runtime.cells.exterior_cells,
        )
    }

    /// The visibility world the runtime loader builds from these sections.
    pub(super) fn visibility_world(&self) -> LevelWorld {
        let (cells, portals, locator) = self.sections();
        LevelWorld::visibility_only_from_sections(&cells, &portals, &locator)
            .expect("loader accepts the runtime cells")
    }
}

/// The pre-change build: today's one-tier tree, each leaf a cell, and its
/// full portal set. The untiered builder equals main's
/// (`unclassified_build_keeps_todays_tree`).
pub(super) struct Untiered {
    pub(super) tree: BspTree,
    pub(super) portals: Vec<Portal>,
    pub(super) exterior: HashSet<usize>,
}

impl Untiered {
    pub(super) fn of(map_data: &MapData) -> Self {
        let result = partition::partition(&map_data.brush_volumes).expect("untiered partition");
        let portals = portals::generate_portals(&result.tree);
        let exterior = visibility::find_exterior_leaves(&result.tree, &portals);
        Self {
            tree: result.tree,
            portals,
            exterior,
        }
    }

    pub(super) fn cell_at(&self, q: [f64; 3]) -> usize {
        find_leaf_for_point(&self.tree, engine(q))
    }

    pub(super) fn visibility_world(&self) -> LevelWorld {
        let (cells, portals, locator) = encode_sections(&self.tree, &self.portals, &self.exterior);
        LevelWorld::visibility_only_from_sections(&cells, &portals, &locator)
            .expect("loader accepts today's cells")
    }
}

fn encode_sections(
    tree: &BspTree,
    portals: &[Portal],
    exterior: &HashSet<usize>,
) -> (CellsSection, PortalsSection, CellLocatorSection) {
    let vis = visibility::encode_vis(tree, exterior);
    let portals = pack::encode_portals(portals).expect("portals encode");
    let cells = pack::encode_cells(&vis.leaves_section, &portals, exterior).expect("cells encode");
    let locator = pack::encode_cell_locator(tree).expect("locator encodes");
    (cells, portals, locator)
}

// ---- full compile ----

/// A fixture compiled by the production pipeline and loaded by the runtime
/// loader. The temporary directory holding the PRL lives as long as this.
pub(super) struct Compiled {
    _dir: tempfile::TempDir,
    pub(super) prl: PathBuf,
    pub(super) world: LevelWorld,
}

/// Compile `map` with `prl-build`'s pipeline (`--no-cache`, 4 m probes, plus
/// `extra_args`) and load it. Every loaded portal must carry orientation
/// evidence (guard C2).
pub(super) fn compile(map: &MapText, extra_args: &[&str]) -> Compiled {
    let dir = tempfile::tempdir().expect("temporary build directory");
    let input = write_map(dir.path(), map);
    let prl = dir.path().join("fixture.prl");
    let mut args = vec![
        input.to_str().expect("fixture path is UTF-8").to_owned(),
        "--no-cache".to_owned(),
        "--sh-probe-spacing".to_owned(),
        "4".to_owned(),
        "-o".to_owned(),
        prl.to_str().expect("fixture path is UTF-8").to_owned(),
    ];
    args.extend(extra_args.iter().map(|arg| (*arg).to_owned()));
    let args = crate::parse_args_from(args.into_iter()).expect("fixture arguments parse");
    let started = Instant::now();
    let reporter: Arc<dyn crate::reporter::Reporter> = Arc::new(
        crate::reporter::PlainReporter::new(started, crate::logger::LogSink::default()),
    );
    crate::pipeline::run(
        &args,
        None,
        started,
        reporter,
        Arc::new(crate::governor::Governor::new(1, false)),
    )
    .expect("fixture compiles");
    let world = load(&prl).expect("runtime loader accepts the compiled fixture");
    let unoriented = world
        .portal_planes()
        .iter()
        .filter(|plane| !plane.oriented)
        .count();
    assert_eq!(unoriented, 0, "every portal carries orientation evidence");
    Compiled {
        _dir: dir,
        prl,
        world,
    }
}

pub(super) fn load(prl: &Path) -> Result<LevelWorld, postretro_level_loader::PrlLoadError> {
    postretro_level_loader::load_prl(prl.to_str().expect("PRL path is UTF-8"))
}

// ---- the walk ----

/// The walk's visible cells and path from a Quake-unit eye looking along a
/// Quake-unit direction, 90° square view.
pub(super) fn walk(
    world: &LevelWorld,
    eye_q: [f64; 3],
    dir_q: [f64; 3],
) -> (Vec<u32>, VisibilityPath) {
    walk_blocked(world, eye_q, dir_q, &[])
}

/// `walk` with the portals whose ids `blocked` lists closed, as a closed
/// mover blocks them at runtime.
pub(super) fn walk_blocked(
    world: &LevelWorld,
    eye_q: [f64; 3],
    dir_q: [f64; 3],
    blocked: &[u32],
) -> (Vec<u32>, VisibilityPath) {
    let mut blocked_portals = vec![false; world.portals.len()];
    for &portal in blocked {
        blocked_portals[portal as usize] = true;
    }
    let eye = engine(eye_q).as_vec3();
    let dir = (engine(dir_q) - engine([0.0; 3])).as_vec3().normalize();
    let view = glam::camera::rh::view::look_at_mat4(eye, eye + dir, Vec3::Y);
    let projection: Mat4 =
        glam::camera::rh::proj::directx::perspective(std::f32::consts::FRAC_PI_2, 1.0, 0.01, 200.0);
    let (result, _) = postretro_visibility::determine_visible_cells(
        eye,
        projection * view,
        world,
        &blocked_portals,
        false,
        &mut postretro_visibility::PortalWalkScratch::new(),
        postretro_visibility::TimingGate::OFF,
    );
    let cells = match result.visible_cells {
        VisibleCells::Culled(ids) => ids,
        VisibleCells::DrawAll => (0..world.cells.len() as u32).collect(),
    };
    (cells, result.stats.path)
}

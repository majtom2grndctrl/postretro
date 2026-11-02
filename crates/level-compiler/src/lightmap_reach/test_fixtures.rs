// Reach-cull fixtures: two sealed rooms split by a wall, with an optional
// opening in it, compiled through partition, portals, geometry and BVH.
// See: context/lib/testing_guide.md §4

use std::collections::HashSet;

use glam::DVec3;

use crate::bake_control::BakeControl;
use crate::light_namespaces::StaticBakedLights;
use crate::lightmap_bake::{self, Chart, PreparedAtlas};
use crate::map_data::{FalloffModel, LightType, MapLight, ShadowType};
use crate::partition::{BspTree, find_leaf_for_point};
use crate::reflection_probes::placement_test_fixtures::{
    PlacementFixture, box_brush, engine, engine_box, sealed_room,
};

use super::{LightmapReachIndex, ReachLight, closed_leaves};

/// Room A spans x 0..256 map units, the wall 256..272, room B 272..528;
/// both span y 0..256 and z 0..128.
pub(crate) const WALL_X: [f64; 2] = [256.0, 272.0];
const ROOMS_MAX: [f64; 3] = [528.0, 256.0, 128.0];

/// The wall between the rooms.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Wall {
    Solid,
    /// A solid wall built of `(along y, along z)` box brushes, so each of its
    /// faces is split into that many faces.
    Grid(u32, u32),
    /// A rectangular opening through the wall, centred on the wall's middle
    /// unless placed: `(y0, y1, z0, z1)` in map units.
    Opening([f64; 4]),
}

impl Wall {
    /// A square vent `side` map units wide at the wall's centre.
    pub(crate) fn vent(side: f64) -> Self {
        Self::Opening([
            128.0 - side / 2.0,
            128.0 + side / 2.0,
            64.0 - side / 2.0,
            64.0 + side / 2.0,
        ])
    }

    /// A slit `height` map units tall and 200 wide at the wall's centre.
    pub(crate) fn slit(height: f64) -> Self {
        Self::Opening([28.0, 228.0, 64.0 - height / 2.0, 64.0 + height / 2.0])
    }

    /// A doorway 64 wide and 96 tall, from the floor.
    pub(crate) fn doorway() -> Self {
        Self::Opening([96.0, 160.0, 0.0, 96.0])
    }
}

/// Where the two rooms and their wall lie, in map units: room A spans x
/// `0..wall_x[0]`, the wall `wall_x[0]..wall_x[1]`, room B `wall_x[1]..max.x`;
/// both span y `0..max.y` and z `0..max.z`, sealed by 16 u slabs.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Rooms {
    pub(crate) wall_x: [f64; 2],
    pub(crate) max: [f64; 3],
}

impl Rooms {
    pub(crate) const STANDARD: Self = Self {
        wall_x: WALL_X,
        max: ROOMS_MAX,
    };

    /// The two rooms' world brushes.
    pub(crate) fn world(self, wall: Wall) -> Vec<String> {
        self.world_at(wall, [0.0; 3])
    }

    /// The two rooms' world brushes, moved `offset` map units.
    pub(crate) fn world_at(self, wall: Wall, offset: [f64; 3]) -> Vec<String> {
        let at = |q: [f64; 3]| [q[0] + offset[0], q[1] + offset[1], q[2] + offset[2]];
        let brush = |min: [f64; 3], max: [f64; 3]| box_brush(at(min), at(max));
        let max = self.max;
        let mut world = sealed_room(at([0.0, 0.0, 0.0]), at(max));
        let [x0, x1] = self.wall_x;
        let (y_max, z_max) = (max[1], max[2]);
        match wall {
            Wall::Solid => world.push(brush([x0, 0.0, 0.0], [x1, y_max, z_max])),
            Wall::Grid(along_y, along_z) => {
                let (dy, dz) = (y_max / f64::from(along_y), z_max / f64::from(along_z));
                for j in 0..along_y {
                    for k in 0..along_z {
                        let (y, z) = (f64::from(j) * dy, f64::from(k) * dz);
                        world.push(brush([x0, y, z], [x1, y + dy, z + dz]));
                    }
                }
            }
            Wall::Opening([y0, y1, z0, z1]) => {
                if z0 > 0.0 {
                    world.push(brush([x0, 0.0, 0.0], [x1, y_max, z0]));
                }
                world.push(brush([x0, 0.0, z1], [x1, y_max, z_max]));
                world.push(brush([x0, 0.0, z0], [x1, y0, z1]));
                world.push(brush([x0, y1, z0], [x1, y_max, z1]));
            }
        }
        world
    }

    /// Room B's interior, in engine space.
    pub(crate) fn room_b(self) -> (DVec3, DVec3) {
        engine_box([self.wall_x[1], 0.0, 0.0], self.max)
    }
}

/// The standard rooms' world brushes.
pub(crate) fn two_rooms(wall: Wall) -> Vec<String> {
    Rooms::STANDARD.world(wall)
}

/// The standard room B's interior, in engine space.
pub(crate) fn room_b() -> (DVec3, DVec3) {
    Rooms::STANDARD.room_b()
}

/// The centre of room A, where the fixtures' light sits by default.
pub(crate) fn room_a_centre() -> DVec3 {
    engine([128.0, 128.0, 64.0])
}

pub(crate) fn room_b_centre() -> DVec3 {
    engine([400.0, 128.0, 64.0])
}

/// A soft static point light at `origin`, `range` metres.
pub(crate) fn point_light(origin: DVec3, range: f32) -> MapLight {
    MapLight {
        origin,
        carrier: String::new(),
        light_type: LightType::Point,
        intensity: 2.0,
        color: [1.0, 0.9, 0.8],
        falloff_model: FalloffModel::Linear,
        falloff_range: range,
        light_size: 0.05,
        angular_diameter: 0.0,
        cone_angle_inner: None,
        cone_angle_outer: None,
        cone_direction: None,
        animation: None,
        bake_only: false,
        is_dynamic: false,
        casts_entity_shadows: false,
        is_animated: false,
        tags: Vec::new(),
        shadow_type: ShadowType::StaticLightMap,
    }
}

/// A compiled reach fixture: the placement fixture's products, which carry
/// the full tree and its exterior flood.
pub(crate) struct ReachFixture {
    pub(crate) compiled: PlacementFixture,
}

impl ReachFixture {
    pub(crate) fn two_rooms(wall: Wall) -> Self {
        Self::compile(&two_rooms(wall), &[])
    }

    /// Worldspawn with `world` brushes, then `entities` as map entities.
    pub(crate) fn compile(world: &[String], entities: &[String]) -> Self {
        Self {
            compiled: PlacementFixture::compile(world, entities),
        }
    }

    pub(crate) fn tree(&self) -> &BspTree {
        &self.compiled.tree
    }

    pub(crate) fn exterior(&self) -> &HashSet<usize> {
        &self.compiled.exterior
    }

    pub(crate) fn leaf_at(&self, point: DVec3) -> usize {
        find_leaf_for_point(self.tree(), point)
    }

    /// The atlas a bake of `lights` at `density` prepares, on a copy of the
    /// geometry.
    pub(crate) fn atlas(&self, lights: &[MapLight], density: f32) -> PreparedAtlas {
        let mut geometry = self.compiled.geometry.clone();
        let static_lights = StaticBakedLights::from_lights(lights);
        lightmap_bake::prepare_atlas(&mut geometry, &static_lights, density, &[])
            .expect("reach fixture atlas")
    }

    /// The index over `lights`, none flagged embedded.
    pub(crate) fn index<'t>(
        &'t self,
        lights: &[MapLight],
        charts: &[Chart],
        penumbra_floor: bool,
    ) -> LightmapReachIndex<'t> {
        let reach_lights: Vec<ReachLight<'_>> = lights
            .iter()
            .map(|light| ReachLight {
                light,
                embedded_emitter: false,
            })
            .collect();
        LightmapReachIndex::build(
            self.tree(),
            closed_leaves(self.tree(), self.exterior()),
            &reach_lights,
            charts,
            penumbra_floor,
            &BakeControl::unrestricted(),
        )
    }
}

/// Whether every interior texel centre of `chart` lies inside `bounds`.
pub(crate) fn chart_inside(chart: &Chart, (min, max): (DVec3, DVec3)) -> bool {
    let (width, height) = crate::chart_raster::chart_interior_dims(chart);
    [
        (0, 0),
        (width - 1, 0),
        (0, height - 1),
        (width - 1, height - 1),
    ]
    .into_iter()
    .map(|(tx, ty)| crate::chart_raster::chart_texel_world_position(chart, tx, ty).as_dvec3())
    .all(|p| p.cmpge(min - 1.0e-3).all() && p.cmple(max + 1.0e-3).all())
}

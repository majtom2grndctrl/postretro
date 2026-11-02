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

/// The two rooms' world brushes.
pub(crate) fn two_rooms(wall: Wall) -> Vec<String> {
    let mut world = sealed_room([0.0, 0.0, 0.0], ROOMS_MAX);
    let [x0, x1] = WALL_X;
    match wall {
        Wall::Solid => world.push(box_brush([x0, 0.0, 0.0], [x1, 256.0, 128.0])),
        Wall::Opening([y0, y1, z0, z1]) => {
            if z0 > 0.0 {
                world.push(box_brush([x0, 0.0, 0.0], [x1, 256.0, z0]));
            }
            world.push(box_brush([x0, 0.0, z1], [x1, 256.0, 128.0]));
            world.push(box_brush([x0, 0.0, z0], [x1, y0, z1]));
            world.push(box_brush([x0, y1, z0], [x1, 256.0, z1]));
        }
    }
    world
}

/// Room B's interior, in engine space.
pub(crate) fn room_b() -> (DVec3, DVec3) {
    engine_box([WALL_X[1], 0.0, 0.0], ROOMS_MAX)
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
        Self {
            compiled: PlacementFixture::compile(&two_rooms(wall), &[]),
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

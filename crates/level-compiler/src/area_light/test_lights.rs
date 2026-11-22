// Area lights built straight from polygons, for bake tests that need no map: panels, strips and
// a light wrapping a pillar, with their reach set or derived.
// See: context/lib/testing_guide.md §4

use glam::DVec3;

use super::{AreaEmitter, EmitterPolygon, derived_reach};
use crate::map_data::{FalloffModel, LightType, MapLight, ShadowType};

/// An axis-aligned rectangle in the plane `y`, x in `[x0, x1]`, z in
/// `[z0, z1]`, emitting along `facing` (±Y).
pub(crate) fn horizontal_rect(x: [f64; 2], z: [f64; 2], y: f64, facing: DVec3) -> EmitterPolygon {
    EmitterPolygon::new(
        vec![
            DVec3::new(x[0], y, z[0]),
            DVec3::new(x[1], y, z[0]),
            DVec3::new(x[1], y, z[1]),
            DVec3::new(x[0], y, z[1]),
        ],
        facing,
    )
}

/// A ceiling panel at height `y`, emitting down.
pub(crate) fn ceiling_panel(x: [f64; 2], z: [f64; 2], y: f64) -> EmitterPolygon {
    horizontal_rect(x, z, y, DVec3::NEG_Y)
}

/// An axis-aligned rectangle in the plane `x`, y in `[y0, y1]`, z in
/// `[z0, z1]`, emitting along `facing` (±X).
pub(crate) fn wall_rect_x(x: f64, y: [f64; 2], z: [f64; 2], facing: DVec3) -> EmitterPolygon {
    EmitterPolygon::new(
        vec![
            DVec3::new(x, y[0], z[0]),
            DVec3::new(x, y[1], z[0]),
            DVec3::new(x, y[1], z[1]),
            DVec3::new(x, y[0], z[1]),
        ],
        facing,
    )
}

/// An axis-aligned rectangle in the plane `z`, x in `[x0, x1]`, y in
/// `[y0, y1]`, emitting along `facing` (±Z).
pub(crate) fn wall_rect_z(z: f64, x: [f64; 2], y: [f64; 2], facing: DVec3) -> EmitterPolygon {
    EmitterPolygon::new(
        vec![
            DVec3::new(x[0], y[0], z),
            DVec3::new(x[1], y[0], z),
            DVec3::new(x[1], y[1], z),
            DVec3::new(x[0], y[1], z),
        ],
        facing,
    )
}

/// The four sides of a square pillar `half` metres from its axis through
/// `(cx, cz)`, each emitting outward over `y` in `[y0, y1]`: index 0 faces
/// +Z (north), 1 −Z, 2 +X, 3 −X. The centroid lies on the axis, inside the
/// pillar.
pub(crate) fn pillar_sides(cx: f64, cz: f64, half: f64, y: [f64; 2]) -> Vec<EmitterPolygon> {
    let (x, z) = ([cx - half, cx + half], [cz - half, cz + half]);
    vec![
        wall_rect_z(cz + half, x, y, DVec3::Z),
        wall_rect_z(cz - half, x, y, DVec3::NEG_Z),
        wall_rect_x(cx + half, y, z, DVec3::X),
        wall_rect_x(cx - half, y, z, DVec3::NEG_X),
    ]
}

/// A static area light over `polygons` at radiance `intensity`, white, its
/// reach `reach` or, when `None`, derived from its flux.
pub(crate) fn area_light(
    polygons: Vec<EmitterPolygon>,
    intensity: f32,
    reach: Option<f32>,
) -> MapLight {
    let emitter = AreaEmitter::new(polygons).expect("the polygons have area");
    let reach =
        reach.unwrap_or_else(|| derived_reach(f64::from(intensity), emitter.total_area) as f32);
    MapLight {
        origin: emitter.centroid,
        carrier: String::new(),
        light_type: LightType::Area,
        intensity,
        color: [1.0, 1.0, 1.0],
        falloff_model: FalloffModel::InverseSquared,
        falloff_range: reach,
        light_size: 0.0,
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
        area: Some(emitter),
    }
}

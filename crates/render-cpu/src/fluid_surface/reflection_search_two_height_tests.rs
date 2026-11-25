// Tests: one probe binding glossy floors at two heights (an authored box over
// a platform and the floor beside it, capturing 2 mm over the platform). A
// lower floor's fragments test their capture shadow from their true
// position, so an object the capture point sees standing between them and
// the capture point withholds the probe term there, as one on the platform
// would.
// See: context/lib/rendering_pipeline.md §7.3a (Reflection, Glossy world faces)

use glam::Vec3;

use super::super::fixtures::{Scene, SceneBox, TILE, capture_tile};
use crate::fluid_surface::reflection_hemisphere::FLUID_REFLECTION_UP;
use crate::fluid_surface::reflection_search::FluidReflectionSearchStatus;
use crate::fluid_surface::reflection_search_about::reflection_search_about;

/// The platform's top: the frontmost bound floor, which the capture point
/// sits 2 mm over.
const PLATFORM_TOP: f32 = 0.5;

/// A 6 × 3 × 6 m room whose west half is a platform 0.5 m high, its east
/// half the floor 0.5 m below, both glossy and bound to one probe captured
/// 2 mm over the platform's centre. A crate 0.3 m high stands on the lower
/// floor east of the platform, below the capture point's height, and a shelf
/// spans the capture point's height over the lower floor's south-east
/// corner, clear of it beneath. Both floor
/// planes are transparent to the capture, as a probe's bound planes are;
/// the platform's sides face away from the capture point, so the trace
/// skips them, and the analytic scene leaves the platform out.
fn platform_room() -> &'static Scene {
    static SCENE: std::sync::OnceLock<Scene> = std::sync::OnceLock::new();
    SCENE.get_or_init(|| {
        let slab = |min: [f32; 3], max: [f32; 3], name| SceneBox {
            min: Vec3::from(min),
            max: Vec3::from(max),
            name,
        };
        let brushes = vec![
            slab([-3.5, 3.0, -3.5], [3.5, 3.5, 3.5], "ceiling"),
            slab([-3.5, -0.5, -3.5], [-3.0, 3.5, 3.5], "wall"),
            slab([3.0, -0.5, -3.5], [3.5, 3.5, 3.5], "wall"),
            slab([-3.5, -0.5, -3.5], [3.5, 3.5, -3.0], "wall"),
            slab([-3.5, -0.5, 3.0], [3.5, 3.5, 3.5], "wall"),
            slab([1.0, 0.0, -0.2], [1.4, 0.3, 0.2], "crate"),
            slab([1.9, 0.45, -2.95], [2.95, 0.6, -1.9], "shelf"),
        ];
        let mut scene = Scene {
            brushes,
            capture: Vec3::new(-1.5, PLATFORM_TOP + 0.002, 0.0),
            box_min: Vec3::new(-3.0, 0.0, -3.0),
            box_max: Vec3::new(3.0, 3.0, 3.0),
            tile: Vec::new(),
        };
        scene.tile = capture_tile(|d| scene.inverse_l1(d));
        scene
    })
}

/// The probe term's search from `origin` along a reflected ray climbing
/// away from the platform.
fn search_from(origin: Vec3) -> (FluidReflectionSearchStatus, f32) {
    let scene = platform_room();
    let ray = Vec3::new(0.4, 0.8, 0.3).normalize();
    let search = reflection_search_about(
        origin,
        ray,
        scene.box_min,
        scene.box_max,
        scene.capture,
        FLUID_REFLECTION_UP,
        TILE,
        |uv| scene.distance(uv),
    );
    (search.status, search.weight)
}

// The lower floor behind the crate, as the capture point sees it, is in the
// capture shadow: the line from the capture point to the fragment passes
// through the crate, so the capture holds nothing the fragment reflects
// there and the probe term is withheld. Read at the capture point's height
// instead, the fragment would look past the crate's top, over the floor,
// and draw.
#[test]
fn a_lower_floor_behind_an_object_the_capture_point_sees_is_in_its_capture_shadow() {
    let scene = platform_room();
    for z in [-0.05, 0.0, 0.05] {
        let fragment = Vec3::new(2.0, 0.0, z);
        let towards = (fragment - scene.capture).normalize();
        assert_eq!(
            scene.trace(scene.capture, towards).map(|(_, name)| name),
            Some("crate"),
            "the capture point sees the crate toward {fragment}"
        );
        let (status, weight) = search_from(fragment);
        assert_eq!(
            (status, weight),
            (FluidReflectionSearchStatus::CaptureShadow, 0.0),
            "{fragment}: the crate shadows the lower floor"
        );
    }
}

// The lower floor the capture point sees, in front of the crate or beside
// its shadow, keeps its full term, as does the platform it was captured
// over: nothing stands between them and the capture point.
#[test]
fn a_lower_floor_the_capture_point_sees_keeps_its_full_term() {
    for fragment in [
        Vec3::new(0.5, 0.0, 0.0),
        Vec3::new(2.0, 0.0, 1.5),
        Vec3::new(2.0, 0.0, -1.5),
        Vec3::new(-2.5, PLATFORM_TOP, 1.0),
        Vec3::new(-0.5, PLATFORM_TOP, -2.0),
    ] {
        let (status, weight) = search_from(fragment);
        assert_ne!(
            status,
            FluidReflectionSearchStatus::CaptureShadow,
            "{fragment}"
        );
        assert_eq!(weight, 1.0, "{fragment}");
    }
}

// Under the shelf the lower floor is in the capture point's sight, below the
// shelf, but the search, which reads every tap over the capture point's
// plane, starts there behind the shelf and cannot run. The term is withheld
// as it was before the lower floor tested its true position: a lower floor
// only ever loses term to its own test.
#[test]
fn a_lower_floor_whose_plane_reading_is_behind_a_surface_stays_withheld() {
    let scene = platform_room();
    let fragment = Vec3::new(2.7, 0.0, -2.7);
    let towards = fragment - scene.capture;
    assert!(
        scene
            .trace(scene.capture, towards.normalize())
            .is_none_or(|(t, _)| t > towards.length()),
        "the capture point sees the floor under the shelf"
    );
    let over = Vec3::new(towards.x, 0.0, towards.z);
    assert_eq!(
        scene
            .trace(scene.capture, over.normalize())
            .map(|(_, name)| name),
        Some("shelf"),
        "over the capture point's plane, the shelf stands in front"
    );
    assert_eq!(
        search_from(fragment),
        (FluidReflectionSearchStatus::CaptureShadow, 0.0)
    );
}

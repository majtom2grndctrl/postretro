// Buried static lights: baked-tier lights whose origin lies inside solid world
// geometry. The compiler drops such a light from every light namespace and
// warns with the light's source label.
// See: context/lib/build_pipeline.md §Compiler pipeline

use glam::DVec3;

use crate::map_data::{LightType, MapLight};
use crate::partition::{BspChild, BspTree};

/// Surface tolerance for the origin test. An origin on, or within this
/// distance of, a face of the solid region counts as on the empty side and is
/// not buried; only an origin strictly inside solid is.
pub(crate) const BURIED_LIGHT_SURFACE_TOLERANCE_METERS: f64 = 1.0e-3;

/// True when every BSP leaf the sphere can touch is solid.
///
/// The descent visits both children of any node whose plane passes within
/// `radius` of `center`, so the visited leaves cover the sphere. Solid leaves
/// are wholly inside a brush (`build_pipeline.md` §Compiler pipeline, BSP
/// construction), so an all-solid visit proves the sphere is inside solid. An
/// empty tree has no solid space. A non-finite center gives NaN plane
/// distances, which descend both sides, so the visit reaches an empty leaf and
/// reports not solid.
pub(crate) fn sphere_wholly_in_solid(tree: &BspTree, center: DVec3, radius: f64) -> bool {
    if tree.leaves.is_empty() {
        return false;
    }
    if tree.nodes.is_empty() {
        return tree.leaves[0].is_solid;
    }
    let mut stack = vec![BspChild::Node(0)];
    while let Some(child) = stack.pop() {
        match child {
            BspChild::Leaf(index) => {
                if !tree.leaves.get(index).is_some_and(|leaf| leaf.is_solid) {
                    return false;
                }
            }
            BspChild::Node(index) => {
                let node = &tree.nodes[index];
                let distance = center.dot(node.plane_normal) - node.plane_distance;
                // A NaN distance (non-finite center) descends both sides.
                let unknown = distance.is_nan();
                if unknown || distance >= -radius {
                    stack.push(node.front.clone());
                }
                if unknown || distance <= radius {
                    stack.push(node.back.clone());
                }
            }
        }
    }
    true
}

/// Which `MapData::lights` entries are buried, indexed by source light.
///
/// A light is buried when its origin lies strictly inside solid, whatever its
/// `light_size`. The chunk lists admit a light from its origin, so an origin in
/// solid would add specular through that solid even when the emitter sphere
/// pokes into air. Only baked-tier Point and Spot lights are classified. A
/// dynamic light can move, and a directional light's origin plays no part in
/// its lighting.
#[derive(Debug, Clone, Default)]
pub struct BuriedLights {
    buried: Vec<bool>,
}

impl BuriedLights {
    /// Classify every light against the compiler BSP: buried when the ball of
    /// radius [`BURIED_LIGHT_SURFACE_TOLERANCE_METERS`] around its origin is
    /// wholly solid, so a light flush on a face is kept.
    pub fn classify(tree: &BspTree, lights: &[MapLight]) -> Self {
        let buried = lights
            .iter()
            .map(|light| {
                !light.is_dynamic
                    && matches!(light.light_type, LightType::Point | LightType::Spot)
                    && sphere_wholly_in_solid(
                        tree,
                        light.origin,
                        BURIED_LIGHT_SURFACE_TOLERANCE_METERS,
                    )
            })
            .collect();
        Self { buried }
    }

    /// No light is buried. Out-of-range indices also read as not buried.
    pub fn none() -> Self {
        Self::default()
    }

    /// An explicit buried set, indexed by source light, for tests whose
    /// subject is a consumer of the set rather than the classification.
    #[cfg(test)]
    pub(crate) fn from_flags(buried: Vec<bool>) -> Self {
        Self { buried }
    }

    pub fn is_buried(&self, source_index: usize) -> bool {
        self.buried.get(source_index).copied().unwrap_or(false)
    }

    pub fn count(&self) -> usize {
        self.buried.iter().filter(|&&buried| buried).count()
    }

    /// Warn once per buried light, naming it by the source-format adapter's
    /// label, then once with the total. Silent when nothing is buried.
    pub fn warn(&self, lights: &[MapLight], source_labels: &[String]) {
        let count = self.count();
        if count == 0 {
            return;
        }
        for (index, light) in lights.iter().enumerate() {
            if !self.is_buried(index) {
                continue;
            }
            let label = source_labels
                .get(index)
                .map(String::as_str)
                .unwrap_or("light with no source label");
            log::warn!(
                "[Compiler] Light inside solid geometry: {label} (engine ({:.3}, {:.3}, {:.3}) m). \
                 Excluded from every light bake and runtime light set; move it into open \
                 space.",
                light.origin.x,
                light.origin.y,
                light.origin.z,
            );
        }
        log::warn!(
            "[Compiler] {count} static light(s) inside solid geometry were excluded; they can \
             light nothing."
        );
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicU64, Ordering};

    use postretro_test_log_capture::LogCapture;

    use super::*;
    use crate::chunk_light_list_bake::{self, ChunkLightListInputs};
    use crate::entity_shadow_select::{EntityShadowSelectionInputs, select_entity_shadow_lights};
    use crate::geometry::extract_geometry;
    use crate::light_namespaces::{AlphaLightsNs, AnimatedBakedLights, StaticBakedLights};
    use crate::map_data::{EntityShadowParams, MapData};
    use crate::{bvh_build, pack, parse, partition, portals, visibility};

    const TEXTURE: &str = "test/wall";

    fn box_brush(min: [i32; 3], max: [i32; 3]) -> String {
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

    fn light_entity(origin: [f64; 3], extra: &str) -> String {
        format!(
            "{{\n\"classname\" \"light\"\n\"origin\" \"{} {} {}\"\n\"light\" \"300\"\n\
             \"_falloff_range\" \"512\"\n{extra}}}",
            origin[0], origin[1], origin[2]
        )
    }

    /// A sealed 512 u room (walls 32 u thick) with a 64 u pillar standing in
    /// the middle from floor to ceiling: x/y in [-32, 32], z in [0, 256].
    fn pillar_room(lights: &[String]) -> MapData {
        let brushes = [
            box_brush([-288, -288, -32], [288, 288, 0]),
            box_brush([-288, -288, 256], [288, 288, 288]),
            box_brush([-288, -288, 0], [-256, 288, 256]),
            box_brush([256, -288, 0], [288, 288, 256]),
            box_brush([-256, -288, 0], [256, -256, 256]),
            box_brush([-256, 256, 0], [256, 288, 256]),
            box_brush([-32, -32, 0], [32, 32, 256]),
        ];
        let worldspawn = format!(
            "{{\n\"classname\" \"worldspawn\"\n{}\n}}",
            brushes.join("\n")
        );
        let text = std::iter::once(worldspawn)
            .chain(lights.iter().cloned())
            .collect::<Vec<_>>()
            .join("\n");
        static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "postretro-buried-lights-{}-{}.map",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(&path, format!("{text}\n")).expect("write fixture map");
        let parsed = parse::parse_map_file(&path, crate::map_format::MapFormat::IdTech2);
        let _ = std::fs::remove_file(&path);
        parsed.expect("fixture map parses")
    }

    fn classify(map: &MapData) -> (BspTree, BuriedLights) {
        let result = partition::partition(&map.brush_volumes).expect("partition");
        let buried = BuriedLights::classify(&result.tree, &map.lights);
        (result.tree, buried)
    }

    /// Point-light origins, in map units, around the pillar's +x face (x = 32).
    const INSIDE_PILLAR: [f64; 3] = [0.0, 0.0, 128.0];
    const FLUSH_ON_FACE: [f64; 3] = [32.0, 0.0, 128.0];
    const JUST_OUTSIDE_FACE: [f64; 3] = [33.0, 0.0, 128.0];

    #[test]
    fn light_inside_a_brush_is_buried_and_neighbours_of_its_face_are_not() {
        let map = pillar_room(&[
            light_entity(INSIDE_PILLAR, "\"_light_size\" \"0\"\n"),
            light_entity(FLUSH_ON_FACE, "\"_light_size\" \"0\"\n"),
            light_entity(JUST_OUTSIDE_FACE, "\"_light_size\" \"0\"\n"),
        ]);
        let (_, buried) = classify(&map);

        assert!(
            buried.is_buried(0),
            "origin inside the pillar must be buried"
        );
        assert!(
            !buried.is_buried(1),
            "an origin flush on the pillar face reaches the room"
        );
        assert!(
            !buried.is_buried(2),
            "an origin just outside the pillar face is open space"
        );
        assert_eq!(buried.count(), 1);
    }

    #[test]
    fn shallow_origin_in_solid_is_buried_whatever_its_light_size() {
        // Origin 4 u (~0.1 m) inside the pillar's +x face. The default emitter
        // sphere pokes into the room, but the chunk lists admit the light from
        // its origin, so it is buried either way.
        let shallow = [28.0, 0.0, 128.0];
        let map = pillar_room(&[
            light_entity(shallow, "\"_light_size\" \"0\"\n"),
            light_entity(shallow, ""),
        ]);
        assert!(
            f64::from(map.lights[1].light_size) > 4.0 * 0.0254,
            "the default emitter must reach past the face for this test to mean anything"
        );
        let (_, buried) = classify(&map);

        assert!(buried.is_buried(0), "hard emitter, origin in solid");
        assert!(buried.is_buried(1), "default emitter, origin in solid");
    }

    #[test]
    fn non_finite_origin_is_not_buried() {
        let map = pillar_room(&[light_entity(INSIDE_PILLAR, "\"_light_size\" \"0\"\n")]);
        let result = partition::partition(&map.brush_volumes).expect("partition");
        for bad in [f64::NAN, f64::INFINITY] {
            let mut lights = map.lights.clone();
            lights[0].origin = DVec3::new(bad, 0.0, 0.0);
            assert!(
                !BuriedLights::classify(&result.tree, &lights).is_buried(0),
                "origin x = {bad} must not read as buried"
            );
        }
    }

    #[test]
    fn dynamic_and_directional_lights_are_never_classified() {
        let mut map = pillar_room(&[light_entity(INSIDE_PILLAR, "\"_light_size\" \"0\"\n")]);
        let result = partition::partition(&map.brush_volumes).expect("partition");
        map.lights[0].is_dynamic = true;
        assert!(!BuriedLights::classify(&result.tree, &map.lights).is_buried(0));
        map.lights[0].is_dynamic = false;
        map.lights[0].light_type = LightType::Directional;
        assert!(!BuriedLights::classify(&result.tree, &map.lights).is_buried(0));
    }

    #[test]
    fn buried_light_warning_names_the_light() {
        let map = pillar_room(&[
            light_entity(JUST_OUTSIDE_FACE, "\"_light_size\" \"0\"\n"),
            light_entity(INSIDE_PILLAR, "\"_light_size\" \"0\"\n"),
        ]);
        let (_, buried) = classify(&map);

        let capture = LogCapture::start();
        buried.warn(&map.lights, &map.light_source_labels);
        let logs = capture.records();

        let named = logs
            .iter()
            .filter(|record| record.level == log::Level::Warn)
            .filter(|record| record.message.contains("Light inside solid geometry"))
            .map(|record| record.message.clone())
            .collect::<Vec<_>>();
        assert_eq!(named.len(), 1, "only the buried light is named: {named:?}");
        assert!(
            named[0].contains("entity 2 'light'") && named[0].contains("origin \"0 0 128\""),
            "warning names the entity index, classname and authored origin: {}",
            named[0]
        );
        capture.assert_logged(log::Level::Warn, "1 static light(s) inside solid");
    }

    /// End to end through the consumers that let a buried light leak: shadow
    /// mask selection, the chunk light lists, and the AlphaLights records the
    /// runtime builds `spec_lights` from. The light just outside the same face
    /// is the control and must survive every one.
    #[test]
    fn buried_light_is_excluded_from_mask_selection_chunk_lists_and_spec_lights() {
        let map = pillar_room(&[
            light_entity(INSIDE_PILLAR, "\"_light_size\" \"0\"\n"),
            light_entity(JUST_OUTSIDE_FACE, "\"_light_size\" \"0\"\n"),
        ]);
        let result = partition::partition(&map.brush_volumes).expect("partition");
        let buried = BuriedLights::classify(&result.tree, &map.lights);
        assert!(buried.is_buried(0) && !buried.is_buried(1));

        let generated_portals = portals::generate_portals(&result.tree);
        let exterior_leaves = visibility::find_exterior_leaves(&result.tree, &generated_portals);
        let geometry = extract_geometry(&result.faces, &result.tree, &exterior_leaves);
        let (bvh, primitives, _) = bvh_build::build_bvh(&geometry).expect("bvh");

        let static_lights = StaticBakedLights::from_lights_excluding(&map.lights, &buried);
        let animated_lights = AnimatedBakedLights::from_lights_excluding(&map.lights, &buried);
        let alpha_lights = AlphaLightsNs::from_lights_excluding(&map.lights, &buried);
        assert!(animated_lights.is_empty());
        let static_sources = static_lights
            .entries()
            .iter()
            .map(|entry| entry.source_index)
            .collect::<Vec<_>>();
        assert_eq!(static_sources, [1], "lightmap/SH static set drops it");

        // Mask selection: EntityShadowLights is the set that competes for
        // shadowmask channels. Indices are AlphaLights slots.
        let selection = select_entity_shadow_lights(&EntityShadowSelectionInputs {
            bvh: &bvh,
            primitives: &primitives,
            geometry: &geometry,
            static_lights: &static_lights,
            alpha_lights: &alpha_lights,
            params: EntityShadowParams::default(),
        });
        assert_eq!(
            selection.light_indices,
            [0],
            "only the open-space light (AlphaLights slot 0) is selected"
        );

        // spec_lights: the runtime packs one record per non-dynamic AlphaLights
        // record, so the section holding one record proves the buried light
        // has no spec_lights slot.
        let alpha_section = pack::encode_alpha_lights(&alpha_lights, &result.tree);
        assert_eq!(alpha_section.lights.len(), 1);
        let origin = alpha_section.lights[0].origin;
        let expected = map.lights[1].origin;
        assert!(
            (DVec3::new(origin[0], origin[1], origin[2]) - expected).length() < 1.0e-6,
            "the surviving record is the open-space light"
        );

        // Chunk lists index the compacted spec_lights space: slot 0 only.
        let section = chunk_light_list_bake::bake_chunk_light_list(
            &ChunkLightListInputs {
                bvh: &bvh,
                primitives: &primitives,
                geometry: &geometry,
                lights: &alpha_lights,
                tree: &result.tree,
                portals: &generated_portals,
                exterior_leaves: &exterior_leaves,
            },
            chunk_light_list_bake::DEFAULT_CELL_SIZE_METERS,
            chunk_light_list_bake::DEFAULT_PER_CHUNK_LIGHT_CAP,
        )
        .expect("chunk light list bakes");
        assert!(
            !section.light_indices.is_empty(),
            "the control light is listed"
        );
        assert!(
            section.light_indices.iter().all(|&slot| slot == 0),
            "no chunk lists a slot beyond the one surviving static light: {:?}",
            section.light_indices
        );

        // Control: without the exclusion the buried light does reach the
        // chunk lists and mask selection. This pins that the assertions above
        // measure the exclusion, not an accident of the fixture.
        let unfiltered_static = StaticBakedLights::from_lights(&map.lights);
        let unfiltered_alpha = AlphaLightsNs::from_lights(&map.lights);
        let unfiltered_chunks = chunk_light_list_bake::bake_chunk_light_list(
            &ChunkLightListInputs {
                bvh: &bvh,
                primitives: &primitives,
                geometry: &geometry,
                lights: &unfiltered_alpha,
                tree: &result.tree,
                portals: &generated_portals,
                exterior_leaves: &exterior_leaves,
            },
            chunk_light_list_bake::DEFAULT_CELL_SIZE_METERS,
            chunk_light_list_bake::DEFAULT_PER_CHUNK_LIGHT_CAP,
        )
        .expect("chunk light list bakes");
        let listed = unfiltered_chunks
            .light_indices
            .iter()
            .copied()
            .collect::<HashSet<_>>();
        assert!(
            listed.contains(&0),
            "unfiltered, the buried light (slot 0) leaks into chunk lists"
        );
        let unfiltered_selection = select_entity_shadow_lights(&EntityShadowSelectionInputs {
            bvh: &bvh,
            primitives: &primitives,
            geometry: &geometry,
            static_lights: &unfiltered_static,
            alpha_lights: &unfiltered_alpha,
            params: EntityShadowParams::default(),
        });
        assert!(
            unfiltered_selection.light_indices.contains(&0),
            "unfiltered, the buried light competes for a mask channel"
        );
    }

    /// A shipped content map with no lights buried in its brushwork. Its
    /// lights sit beside brushes, where a bounding-box test would flag them;
    /// the BSP solid test must flag none.
    #[test]
    fn kinematic_platform_has_no_buried_lights() {
        let path = crate::fixture_pipeline::fixture_path("kinematic-platform");
        let map = parse::parse_map_file(&path, crate::map_format::MapFormat::IdTech2)
            .expect("kinematic-platform parses");
        assert!(
            map.lights.iter().any(|light| !light.is_dynamic),
            "fixture must carry static lights for this check to mean anything"
        );
        let (_, buried) = classify(&map);
        assert_eq!(buried.count(), 0);
    }
}

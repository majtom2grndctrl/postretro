// Canonical empty material/geometry states at the actual SH bake boundary.
// The CLI adapter cannot produce an empty name table with a finite-brush BSP:
// names intern before exterior culling; brushless input fails CellVisibility.

use super::{ShBakeCtx, ShConfig, bake_sh_volume};
use crate::bvh_build::build_bvh;
use crate::coloured_bounce_test_fixtures::{Appearance, room_map, write_textures};
use crate::delta_sh_bake::{DeltaBakeInputs, bake_delta_sh_volumes};
use crate::geometry::GeometryResult;
use crate::light_namespaces::{AnimatedBakedLights, StaticBakedLights};
use crate::map_data::MapLight;
use crate::map_format::MapFormat;
use crate::partition::BspTree;
use crate::portals::Portal;
use crate::surface_materials::stats::BounceMaterials;
use crate::texture_mips::{StageTextureBytes, bake_materials_into};
use postretro_level_format::geometry::GeometrySection;
use postretro_level_format::texture_names::TextureNamesSection;
use std::collections::HashSet;

struct Room {
    dir: tempfile::TempDir,
    tree: BspTree,
    portals: Vec<Portal>,
    exterior: HashSet<usize>,
    lights: Vec<MapLight>,
}

impl Room {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        write_textures(dir.path());
        let map_path = dir.path().join("room.map");
        std::fs::write(&map_path, room_map(Appearance::RedWall, true, true)).unwrap();
        let map = crate::parse::parse_map_file(&map_path, MapFormat::IdTech2).unwrap();
        let partition = crate::partition::partition(&map.brush_volumes).unwrap();
        let portals = crate::portals::generate_portals(&partition.tree);
        let exterior = crate::visibility::find_exterior_leaves(&partition.tree, &portals);
        assert!(!partition.tree.leaves.is_empty(), "keep a real valid BSP");
        Self {
            dir,
            tree: partition.tree,
            portals,
            exterior,
            lights: map.lights,
        }
    }

    fn empty_geometry(&self, names: Vec<String>) -> GeometryResult {
        GeometryResult {
            geometry: GeometrySection {
                vertices: Vec::new(),
                indices: Vec::new(),
                faces: Vec::new(),
            },
            texture_names: TextureNamesSection { names },
            face_index_ranges: Vec::new(),
        }
    }

    fn assert_legacy_bake_equal(&self, geometry: &GeometryResult, materials: &BounceMaterials) {
        let (bvh, primitives, _) = build_bvh(geometry).unwrap();
        assert!(bvh.nodes.is_empty() && primitives.is_empty());
        let static_lights = StaticBakedLights::from_lights(&self.lights);
        let animated_lights = AnimatedBakedLights::from_lights(&self.lights);
        // The empty result must be due to geometry, rather than a vacuous
        // no-light fixture. Both lighting namespaces come from the real map.
        assert!(!static_lights.is_empty() && !animated_lights.is_empty());
        let legacy = ShBakeCtx {
            bounce_materials: None,
            bvh: &bvh,
            primitives: &primitives,
            geometry,
            tree: &self.tree,
            exterior_leaves: &self.exterior,
            static_lights: &static_lights,
            animated_lights: &animated_lights,
            total_light_count: self.lights.len(),
        };
        let config = ShConfig { probe_spacing: 1.0 };
        let before = bake_sh_volume(&legacy, &config);
        let after = bake_sh_volume(
            &ShBakeCtx {
                bounce_materials: Some(materials),
                ..legacy
            },
            &config,
        );
        assert_eq!(
            before.to_bytes(),
            after.to_bytes(),
            "id34 equals the legacy None-material path"
        );
        assert_eq!(after.grid_dimensions, [0; 3]);
        assert!(after.probes.is_empty());
        assert!(after.compact_atlas.is_empty());
        assert_eq!(after.layer_count, 0);
        // This exact zero-grid result is what pipeline::finish_stage uses to
        // report base SH skipped. No reporter or CLI success is fabricated.
        assert_eq!(after.total_probes(), 0);
        let delta = DeltaBakeInputs {
            bounce_materials: None,
            bvh: &bvh,
            primitives: &primitives,
            geometry,
            tree: &self.tree,
            exterior_leaves: &self.exterior,
            portals: &self.portals,
            animated_lights: &animated_lights,
        };
        let before = bake_delta_sh_volumes(&delta, &config);
        let after = bake_delta_sh_volumes(
            &DeltaBakeInputs {
                bounce_materials: Some(materials),
                ..delta
            },
            &config,
        );
        assert_eq!(
            before.as_ref().map(|section| section.to_bytes()),
            after.as_ref().map(|section| section.to_bytes()),
            "id27 outcome equals legacy None-material path"
        );
        assert!(
            before.is_none() && after.is_none(),
            "degenerate grid emits no indirect delta section"
        );
    }
}

// No rays/lighting work runs: these exercise only the canonical empty-grid exits.
#[test]
fn empty_name_material_stage_preserves_legacy_empty_base_and_delta_bakes() {
    let room = Room::new();
    let geometry = room.empty_geometry(Vec::new());
    for saturation in [0.0, 0.5, 1.0] {
        let sidecars = room
            .dir
            .path()
            .join(format!("empty-materials-{saturation}"));
        let mut report = StageTextureBytes::new();
        let (keys, stats) = bake_materials_into(
            &geometry.texture_names.names,
            &room.dir.path().join("textures"),
            &sidecars,
            &mut report,
            Some(saturation),
        )
        .unwrap();
        assert!(keys.is_empty() && stats.is_empty());
        assert!(report.summary().is_empty());
        assert!(
            !sidecars.exists(),
            "empty stats create neither sidecars nor another stats file"
        );
        let materials = BounceMaterials::new(stats, saturation).unwrap();
        assert!(materials.resolved_albedo(0).is_none());
        assert!(materials.resolved_albedo(u32::MAX).is_none());
        room.assert_legacy_bake_equal(&geometry, &materials);
    }
}

#[test]
fn actual_material_names_without_static_faces_preserve_empty_bakes_and_empty_face_fold() {
    let room = Room::new();
    // These represent names retained for movers while no static face uses a
    // material. The sibling CLI gate proves real mover-name production.
    let names = vec!["bounce/red".to_owned(), "bounce/white".to_owned()];
    let geometry = room.empty_geometry(names.clone());
    let sidecars = room.dir.path().join("unused-materials");
    let build = |saturation| {
        let mut report = StageTextureBytes::new();
        let (keys, stats) = bake_materials_into(
            &names,
            &room.dir.path().join("textures"),
            &sidecars,
            &mut report,
            Some(saturation),
        )
        .unwrap();
        assert_eq!(keys.len(), names.len());
        assert_eq!(stats.len(), names.len());
        assert!(stats.iter().all(|record| record.snapped_mean().is_some()));
        assert!(keys.values().all(|key| *key != [0; 32]));
        // Identical sources after the edit legitimately share one sidecar charge.
        assert!(report.summary().material_count() > 0);
        BounceMaterials::new(stats, saturation).unwrap()
    };
    let fold = |materials: &BounceMaterials| {
        let mut hasher = blake3::Hasher::new();
        materials.fold_key(&mut hasher, &geometry);
        hasher.finalize()
    };
    let red = build(1.0);
    room.assert_legacy_bake_equal(&geometry, &red);
    let before = fold(&red);
    // Actual PNG edit and production material build, never a hand-built means table.
    std::fs::copy(
        room.dir.path().join("textures/bounce/white.png"),
        room.dir.path().join("textures/bounce/red.png"),
    )
    .unwrap();
    let white = build(1.0);
    assert_ne!(
        red.resolved_albedo(0).unwrap().to_array().map(f32::to_bits),
        white
            .resolved_albedo(0)
            .unwrap()
            .to_array()
            .map(f32::to_bits)
    );
    assert_eq!(
        before,
        fold(&white),
        "no static face means no material colour enters its key"
    );
    assert_ne!(
        before,
        fold(&build(0.5)),
        "empty static-face fold still includes saturation"
    );
    room.assert_legacy_bake_equal(&geometry, &white);
}

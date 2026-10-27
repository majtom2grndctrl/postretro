// Early world/mover materials and the late fluid/model/sprite tail.
// See: context/lib/build_pipeline.md §Compiler pipeline, §Baked texture mips

use super::*;
use crate::surface_materials::stats::{BOUNCE_SATURATION, BounceMaterials, validate_saturation};
use std::path::Path;

pub(super) struct MaterialStageResult {
    pub name_to_key: HashMap<String, [u8; 32]>,
    pub texture_bytes: texture_mips::StageTextureBytes,
    pub bounce_materials: BounceMaterials,
    world_name_count: usize,
}

pub(super) fn bake_world_materials(
    names: &[String],
    texture_root: &Path,
    prm_root: &Path,
) -> anyhow::Result<MaterialStageResult> {
    bake_world_materials_at(names, texture_root, prm_root, BOUNCE_SATURATION)
}

fn bake_world_materials_at(
    names: &[String],
    texture_root: &Path,
    prm_root: &Path,
    saturation: f32,
) -> anyhow::Result<MaterialStageResult> {
    // Validate even an empty material stage before walking the texture root.
    validate_saturation(saturation)?;
    let mut texture_bytes = texture_mips::StageTextureBytes::new();
    let (name_to_key, stats) =
        texture_mips::bake_materials_into(names, texture_root, prm_root, &mut texture_bytes, true)?;
    // SH hits index the table by texture index, so one record per name is the contract.
    anyhow::ensure!(
        stats.len() == names.len(),
        "material stage produced {} stats for {} texture names",
        stats.len(),
        names.len()
    );
    Ok(MaterialStageResult {
        name_to_key,
        texture_bytes,
        bounce_materials: BounceMaterials::new(stats, saturation)?,
        world_name_count: names.len(),
    })
}

pub(super) fn bake_remaining_textures(
    input: &Path,
    texture_names: &[String],
    map_entities: &[map_data::MapEntityRecord],
    texture_root: &Path,
    prm_root: &Path,
    materials: &mut MaterialStageResult,
) -> anyhow::Result<()> {
    // Fluid names append after every world/mover name, so existing indices and
    // first-name byte-report ownership survive the split.
    debug_assert!(
        texture_names.len() >= materials.world_name_count,
        "texture names may only grow between the material stage and the fluid tail"
    );
    let fluid_names = &texture_names[materials.world_name_count..];
    if !fluid_names.is_empty() {
        let (keys, _) = texture_mips::bake_materials_into(
            fluid_names,
            texture_root,
            prm_root,
            &mut materials.texture_bytes,
            false,
        )?;
        materials.name_to_key.extend(keys);
    }
    let content_root = resolve_content_root(input);
    bake_model_textures(
        map_entities,
        &content_root,
        prm_root,
        &mut materials.texture_bytes,
    );
    bake_sprite_textures(
        map_entities,
        texture_root,
        prm_root,
        &mut materials.texture_bytes,
    );
    // Flush once, after world, fluid, model and sprite bundles are all charged:
    // the report describes the level's whole texture cost, deduplicated.
    texture_mips::log_texture_byte_summary(materials.texture_bytes.summary());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coloured_bounce_test_fixtures::{
        shared_name_room_map, write_texture, write_textures,
    };
    use crate::texture_mips::probe_test_metrics as metrics;
    use postretro_level_format::environment_volumes::EnvironmentVolumesSection;
    use postretro_level_format::kinematic_geometry::KinematicGeometrySection;
    use postretro_level_format::texture_names::TextureNamesSection;
    use postretro_level_format::{SectionId, read_container, read_section_data};
    use postretro_test_log_capture::LogCapture;
    use std::collections::BTreeSet;

    #[test]
    fn material_stage_rejects_bad_saturation_before_baking_any_texture() {
        let root = tempfile::tempdir().unwrap();
        let textures = root.path().join("textures");
        std::fs::create_dir_all(textures.join("panels")).unwrap();
        image::RgbaImage::from_pixel(2, 2, image::Rgba([200, 40, 20, 255]))
            .save(textures.join("panels/red.png"))
            .unwrap();
        let prm_root = root.path().join("materials");
        for names in [Vec::new(), vec!["panels/red".to_owned()]] {
            for saturation in [-0.1, 1.1, f32::NAN, f32::INFINITY] {
                let error = bake_world_materials_at(&names, &textures, &prm_root, saturation)
                    .err()
                    .expect("invalid saturation must fail the material stage");
                assert!(error.to_string().contains("saturation"));
                assert!(
                    !prm_root.exists(),
                    "no sidecar is written before validation"
                );
            }
        }
        bake_world_materials_at(&["panels/red".to_owned()], &textures, &prm_root, 1.0).unwrap();
        assert!(
            prm_root.exists(),
            "the same names bake at a valid saturation"
        );
    }

    #[test]
    fn material_stage_gives_a_texture_the_same_mean_whatever_its_neighbours() {
        let root = tempfile::tempdir().unwrap();
        let textures = root.path().join("textures");
        let paint = |name: &str, pixel: fn(u32) -> [u8; 4]| {
            let path = textures.join(format!("{name}.png"));
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            image::RgbaImage::from_fn(4, 4, |x, _| image::Rgba(pixel(x)))
                .save(path)
                .unwrap();
        };
        paint("panels/shared", |x| {
            if x < 2 {
                [200, 40, 20, 255]
            } else {
                [30, 90, 210, 255]
            }
        });
        paint("panels/red", |_| [255, 0, 0, 255]);
        paint("panels/green", |_| [0, 255, 0, 255]);
        paint("panels/neon_blue", |_| [0, 0, 255, 255]);
        paint("panels/neon_blue_e", |x| {
            if x == 0 { [255; 4] } else { [0, 0, 0, 255] }
        });
        // Two maps sharing one baked root: the first bakes the shared texture
        // cold, the second reuses its sidecar amid different neighbours.
        let prm_root = root.path().join("materials");
        let first_names = ["panels/shared", "panels/red"].map(str::to_owned);
        let second_names = [
            "panels/green",
            "panels/neon_blue",
            "panels/missing",
            "panels/shared",
        ]
        .map(str::to_owned);
        let first = bake_world_materials_at(&first_names, &textures, &prm_root, 1.0).unwrap();
        let second = bake_world_materials_at(&second_names, &textures, &prm_root, 1.0).unwrap();
        let albedo = |stage: &MaterialStageResult, index: u32| {
            stage
                .bounce_materials
                .resolved_albedo(index)
                .map(|albedo| albedo.to_array().map(f32::to_bits))
        };
        let shared = albedo(&first, 0).expect("shared texture resolves");
        assert_eq!(albedo(&second, 3), Some(shared), "same mean in both maps");
        for (stage, index) in [(&first, 1), (&second, 0), (&second, 1)] {
            assert_ne!(albedo(stage, index), Some(shared), "neighbours differ");
        }
        assert_eq!(albedo(&second, 2), None);
    }

    // A name shared by a world face and a fluid face, and a mover-only name,
    // each bake once in the material stage; the fluid tail holds only the
    // fluid-only name; the byte report charges every bundle once.
    #[test]
    fn pipeline_bakes_shared_and_mover_names_once_and_tails_only_fluid_only_names() {
        let root = tempfile::tempdir().unwrap();
        write_textures(root.path());
        write_texture(root.path(), "lift", [40, 160, 40]);
        std::fs::create_dir(root.path().join("maps")).unwrap();
        let map = root.path().join("maps/shared.map");
        std::fs::write(&map, shared_name_room_map()).unwrap();
        let output = root.path().join("shared.prl");
        let path = |path: &Path| path.to_string_lossy().into_owned();
        let args = crate::parse_args_from(
            [
                path(&map),
                "-o".to_owned(),
                path(&output),
                "--baked-root".to_owned(),
                path(&root.path().join("baked")),
                "--no-cache".to_owned(),
                "--jobs".to_owned(),
                "1".to_owned(),
                "--lightmap-density".to_owned(),
                "0.25".to_owned(),
            ]
            .into_iter(),
        )
        .unwrap();
        let logs = LogCapture::start();
        let started = std::time::Instant::now();
        let reporter: std::sync::Arc<dyn crate::reporter::Reporter> = std::sync::Arc::new(
            crate::reporter::PlainReporter::new(started, crate::logger::LogSink::default()),
        );
        let (result, observed) = metrics::observe(|| {
            crate::pipeline::run(
                &args,
                None,
                started,
                reporter,
                std::sync::Arc::new(crate::governor::Governor::new(1, false)),
            )
        });
        result.expect("fixture compiles");

        let mut reader = std::io::Cursor::new(std::fs::read(&output).unwrap());
        let metadata = read_container(&mut reader).unwrap();
        let mut section = |id: SectionId| {
            read_section_data(&mut reader, &metadata, id as u32)
                .unwrap()
                .unwrap_or_else(|| panic!("{id:?} section"))
        };
        let names = TextureNamesSection::from_bytes(&section(SectionId::TextureNames))
            .unwrap()
            .names;
        let fluid =
            EnvironmentVolumesSection::from_bytes(&section(SectionId::EnvironmentVolumes)).unwrap();
        let movers =
            KinematicGeometrySection::from_bytes(&section(SectionId::KinematicGeometry)).unwrap();
        let index = |name: &str| {
            let found: Vec<_> = names
                .iter()
                .enumerate()
                .filter(|(_, interned)| *interned == name)
                .map(|(position, _)| position)
                .collect();
            assert_eq!(found.len(), 1, "{name} interned once in {names:?}");
            found[0]
        };
        // The fixture exercises every split case.
        let named = |indices: BTreeSet<u32>| -> BTreeSet<&str> {
            indices
                .into_iter()
                .map(|i| names[i as usize].as_str())
                .collect()
        };
        let fluid_names = named(
            fluid
                .volumes
                .iter()
                .flat_map(|volume| &volume.surface_faces)
                .map(|face| face.texture_index)
                .collect(),
        );
        assert_eq!(
            fluid_names,
            BTreeSet::from(["bounce/water", "bounce/white"])
        );
        let mover_names = named(
            movers
                .movers
                .iter()
                .flat_map(|mover| &mover.face_meta)
                .map(|face| face.texture_index)
                .collect(),
        );
        assert_eq!(mover_names, BTreeSet::from(["bounce/lift"]));
        // Fluid-only names append after every world and mover name.
        assert_eq!(index("bounce/water"), names.len() - 1);
        assert!(index("bounce/white") < index("bounce/lift"));

        // Every interned name passes through the bake loop exactly once. The
        // material stage derives stats for each of its names; the tail does not.
        assert_eq!(observed.texture_count, names.len());
        assert_eq!(
            observed.texture_count - observed.stats_calls,
            1,
            "the fluid tail bakes only the fluid-only name"
        );
        assert_eq!(observed.decode_attempts.len(), 4, "{observed:?}");
        assert!(observed.decode_attempts.values().all(|&count| count == 1));
        for stem in ["red", "white", "water", "lift"] {
            assert!(
                observed
                    .decode_attempts
                    .keys()
                    .any(|path| path.ends_with(format!("bounce/{stem}.png"))),
                "{stem} decoded"
            );
        }

        let report: Vec<String> = logs
            .records()
            .into_iter()
            .filter_map(|record| {
                record
                    .message
                    .strip_prefix("[prl-build] ")
                    .map(str::to_owned)
            })
            .collect();
        let totals: Vec<_> = report
            .iter()
            .filter(|line| line.starts_with("texture bytes:"))
            .collect();
        assert_eq!(totals.len(), 1, "one byte report: {report:?}");
        assert!(
            totals[0].ends_with("across 4 material bundle(s)"),
            "{}",
            totals[0]
        );
        let charged: Vec<_> = report
            .iter()
            .filter_map(|line| line.strip_prefix("  largest: "))
            .map(|line| line.split_whitespace().next().unwrap())
            .collect();
        let mut sorted = charged.clone();
        sorted.sort_unstable();
        assert_eq!(
            sorted,
            ["bounce/lift", "bounce/red", "bounce/water", "bounce/white"],
            "each bundle charged once, under its world, mover or fluid name"
        );
    }
}

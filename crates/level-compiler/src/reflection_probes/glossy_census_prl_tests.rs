// The glossy probe census over already-compiled PRLs and their baked materials, no rebake.
// See: context/lib/resource_management.md §5.4 · testing_guide.md §3

use std::collections::HashMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use postretro_level_format::bvh::BvhSection;
use postretro_level_format::geometry::GeometrySection;
use postretro_level_format::reflection_probes::{MAX_REFLECTION_PROBES, ReflectionProbesSection};
use postretro_level_format::texture_cache_keys::TextureCacheKeysSection;
use postretro_level_format::texture_names::TextureNamesSection;
use postretro_level_format::{SectionId, read_container, read_section_data};

use crate::bake_control::BakeControl;
use crate::geometry::{FaceIndexRange, GeometryResult};

use super::glossy_census::{GlossyCensus, ReflectorGroups, group_reflectors, take_census};
use super::glossy_materials::GlossyMaterials;
use super::placement::UP_FACING_MIN_COS;

/// PRL paths, separated as `PATH` is (`:` on Unix).
const PRLS_VAR: &str = "POSTRETRO_GLOSSY_CENSUS_PRLS";
/// The baked material root holding `<key>.prm`, as `prl-build` writes it
/// (`<baked>/materials`).
const PRM_ROOT_VAR: &str = "POSTRETRO_GLOSSY_CENSUS_PRM_ROOT";

/// One PRL as compiled: its world, keys and shipped water probes.
struct CompiledLevel {
    geometry: GeometryResult,
    texture_keys: HashMap<String, [u8; 32]>,
    /// Id 54's probe count, or why it could not be read.
    water_probes: Result<usize, String>,
}

fn read_level(path: &Path) -> Result<CompiledLevel, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read: {e}"))?;
    let mut cursor = Cursor::new(bytes);
    let meta = read_container(&mut cursor).map_err(|e| format!("not a PRL: {e}"))?;
    let mut section = |id: SectionId| -> Result<Option<Vec<u8>>, String> {
        read_section_data(&mut cursor, &meta, id as u32).map_err(|e| format!("{id:?}: {e}"))
    };
    let required = |bytes: Option<Vec<u8>>, id: SectionId| bytes.ok_or(format!("no {id:?}"));
    let geometry = GeometrySection::from_bytes(&required(
        section(SectionId::Geometry)?,
        SectionId::Geometry,
    )?)
    .map_err(|e| format!("Geometry: {e}"))?;
    let names = TextureNamesSection::from_bytes(&required(
        section(SectionId::TextureNames)?,
        SectionId::TextureNames,
    )?)
    .map_err(|e| format!("TextureNames: {e}"))?;
    let keys = TextureCacheKeysSection::from_bytes(&required(
        section(SectionId::TextureCacheKeys)?,
        SectionId::TextureCacheKeys,
    )?)
    .map_err(|e| format!("TextureCacheKeys: {e}"))?;
    let bvh = BvhSection::from_bytes(&required(section(SectionId::Bvh)?, SectionId::Bvh)?)
        .map_err(|e| format!("Bvh: {e}"))?;
    let water_probes = match section(SectionId::ReflectionProbes)? {
        None => Ok(0),
        Some(bytes) => ReflectionProbesSection::from_bytes(&bytes)
            .map(|section| section.probes.len())
            .map_err(|e| format!("id 54 unreadable: {e}")),
    };
    if keys.keys.len() != names.names.len() {
        return Err(format!(
            "{} texture key(s) for {} name(s)",
            keys.keys.len(),
            names.names.len()
        ));
    }
    let face_index_ranges = face_index_ranges(&bvh, geometry.faces.len(), geometry.indices.len())?;
    let texture_keys = names.names.iter().cloned().zip(keys.keys).collect();
    Ok(CompiledLevel {
        geometry: GeometryResult {
            geometry,
            texture_names: names,
            face_index_ranges,
        },
        texture_keys,
        water_probes,
    })
}

/// Id 17 keeps no face ranges; the compiler emits each face's indices as one
/// contiguous range in face order, each owning one BVH leaf, so the leaves
/// sorted by offset are the faces' ranges (as the loader recovers them).
fn face_index_ranges(
    bvh: &BvhSection,
    faces: usize,
    indices: usize,
) -> Result<Vec<FaceIndexRange>, String> {
    if bvh.leaves.len() != faces {
        return Err(format!("{} BVH leaves for {faces} faces", bvh.leaves.len()));
    }
    let mut ranges: Vec<FaceIndexRange> = bvh
        .leaves
        .iter()
        .map(|leaf| FaceIndexRange {
            index_offset: leaf.index_offset,
            index_count: leaf.index_count,
        })
        .collect();
    ranges.sort_unstable_by_key(|range| range.index_offset);
    let mut expected = 0u32;
    for range in &ranges {
        if range.index_offset != expected || range.index_count == 0 {
            return Err(format!("face ranges do not tile the indices at {expected}"));
        }
        expected += range.index_count;
    }
    if expected as usize != indices {
        return Err(format!("face ranges cover {expected} of {indices} indices"));
    }
    Ok(ranges)
}

/// Each group by facing (its first face's normal) and by bound area, for
/// sizing a cap policy: how many probes floors, ceilings and walls ask for,
/// and how many serve small trims.
fn report_breakdown(grouped: &ReflectorGroups) {
    const AREA_EDGES_M2: [f64; 3] = [0.25, 1.0, 4.0];
    let mut facing = [0usize; 3];
    let mut by_area = [0usize; 4];
    for group in &grouped.groups {
        let normal = grouped.faces[group[0]].geometry.normal;
        let side = if normal.y >= UP_FACING_MIN_COS {
            0
        } else if normal.y <= -UP_FACING_MIN_COS {
            1
        } else {
            2
        };
        facing[side] += 1;
        let area: f64 = group.iter().map(|&i| grouped.faces[i].geometry.area).sum();
        by_area[AREA_EDGES_M2.iter().filter(|&&edge| area >= edge).count()] += 1;
    }
    println!(
        "  groups facing up {}, down {}, other {}\n  groups by bound area: < 0.25 m² {}, \
         0.25–1 m² {}, 1–4 m² {}, ≥ 4 m² {}",
        facing[0], facing[1], facing[2], by_area[0], by_area[1], by_area[2], by_area[3],
    );
}

fn report_row(path: &Path, loader: &str, census: &GlossyCensus, water: &Result<usize, String>) {
    let (water_text, total) = match water {
        Ok(water) => (water.to_string(), Some(water + census.groups)),
        Err(error) => (error.clone(), None),
    };
    let verdict = match total {
        Some(total) if total <= MAX_REFLECTION_PROBES => {
            format!("{total} ≤ {MAX_REFLECTION_PROBES}")
        }
        Some(total) => format!("{total} > {MAX_REFLECTION_PROBES} (past the cap)"),
        None => "unknown".to_string(),
    };
    println!(
        "{}\n  loader: {loader}\n  glossy materials {} of {} used ({} unreadable)\n  glossy faces {} \
         ({} with no polygon or no area), planes {}, edge-connected groups {}\n  water probes (id 54) {water_text}\n  \
         estimated total {verdict}",
        path.display(),
        census.glossy_materials,
        census.used_materials,
        census.unreadable_materials,
        census.faces,
        census.unrecovered_faces,
        census.planes,
        census.groups,
    );
}

/// Run with `--ignored --nocapture`, both variables set. Fails when either is
/// absent, or when any PRL cannot be read: a census over nothing proves
/// nothing. A PRL the current loader refuses is reported, then censused from
/// its raw sections.
#[test]
#[ignore = "reads compiled PRLs and baked materials named by POSTRETRO_GLOSSY_CENSUS_PRLS and _PRM_ROOT"]
fn glossy_probe_census_of_compiled_levels_reports_estimates_against_the_cap() {
    let prls: Vec<PathBuf> = std::env::split_paths(
        &std::env::var_os(PRLS_VAR).unwrap_or_else(|| panic!("{PRLS_VAR} must name the PRLs")),
    )
    .filter(|path| !path.as_os_str().is_empty())
    .collect();
    assert!(!prls.is_empty(), "{PRLS_VAR} names no PRL");
    let prm_root = PathBuf::from(
        std::env::var_os(PRM_ROOT_VAR)
            .unwrap_or_else(|| panic!("{PRM_ROOT_VAR} must name the baked material root")),
    );
    assert!(
        prm_root.is_dir(),
        "{} is not a directory",
        prm_root.display()
    );
    let control = BakeControl::unrestricted();

    let mut failures = Vec::new();
    for path in &prls {
        let loader = match postretro_level_loader::load_prl(&path.to_string_lossy()) {
            Ok(_) => "accepts".to_string(),
            Err(error) => format!("REFUSES ({error})"),
        };
        let level = match read_level(path) {
            Ok(level) => level,
            Err(error) => {
                println!(
                    "{}\n  loader: {loader}\n  UNREADABLE: {error}",
                    path.display()
                );
                failures.push(format!("{}: {error}", path.display()));
                continue;
            }
        };
        let materials = GlossyMaterials::read(
            &level.geometry,
            &level.texture_keys,
            &prm_root,
            control.governor(),
        );
        let census = take_census(&level.geometry, &materials);
        report_row(path, &loader, &census, &level.water_probes);
        report_breakdown(&group_reflectors(&level.geometry, &materials));
        let glossy: Vec<&str> = level
            .geometry
            .texture_names
            .names
            .iter()
            .enumerate()
            .filter(|&(index, _)| materials.reflects(index as u32))
            .map(|(_, name)| name.as_str())
            .collect();
        println!("  glossy: {glossy:?}");
        if census.unreadable_materials > 0 {
            failures.push(format!(
                "{}: {} material sidecar(s) unreadable under {}",
                path.display(),
                census.unreadable_materials,
                prm_root.display()
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "census incomplete:\n{}",
        failures.join("\n")
    );
}

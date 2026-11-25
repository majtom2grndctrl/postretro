//! Water parity A/B for glossy world reflections: on every water map in `content/dev`,
//! each probe that binds no world face places and captures exactly as the build before the
//! glossy-world-reflections branch did.
//!
//! Inputs are two directories of fresh `--no-cache` bakes, one `<map>.prl` and one `<map>.log`
//! (the build's `-v` log, filtered or whole) per water map:
//! - `POSTRETRO_WATER_PARITY_BEFORE_DIR`: the pre-branch compiler's (`prl-build` at `8dda8a34a`;
//!   id 54 version 2, five lobes);
//! - `POSTRETRO_WATER_PARITY_AFTER_DIR`: this compiler's, built with `--glossy-reflections`.
//!
//! A water map is one whose source holds a `fluid_volume`. A missing PRL or log fails the test,
//! so a green run always means every water map was compared. Probes are matched by the fluid
//! faces they bind (bindings up to probe renumbering); for each match the source entity, box,
//! capture point, top-K light roster and references, mip-0 colour tile and distance tile are
//! byte-equal, the shares are equal on the +Y, ±X and ±Z lobes with −Y zero, and the build log's
//! excluded-brush count is equal (the exclusion classes; the excluded set itself is keyed into the
//! capture, whose tiles are compared). Every pre-branch probe is matched exactly once.
//!
//! Opt-in and `#[ignore]`: the bakes are minutes of work and need the pre-branch binary.
//!
//! ```text
//! POSTRETRO_WATER_PARITY_BEFORE_DIR=<dir> POSTRETRO_WATER_PARITY_AFTER_DIR=<dir> \
//!   cargo test -p postretro-level-compiler --test reflection_probe_water_parity_ab -- --ignored
//! ```
//!
//! See: context/lib/build_pipeline.md §Compiler pipeline step 14 · context/lib/resource_management.md §5

use std::collections::BTreeMap;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use postretro_level_format::reflection_probes::{
    REFLECTION_PROBE_LIGHTS_PER_PROBE, ReflectionProbesSection,
    reflection_probe_colour_chain_bytes, reflection_probe_distance_tile_bytes,
    reflection_probe_tile_bytes,
};
use postretro_level_format::{SectionId, read_container, read_section_data};

const BEFORE_ENV: &str = "POSTRETRO_WATER_PARITY_BEFORE_DIR";
const AFTER_ENV: &str = "POSTRETRO_WATER_PARITY_AFTER_DIR";
/// Lobes in the pre-branch section: +Y, ±X, ±Z. The current section appends −Y.
const BEFORE_LOBES: usize = 5;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|crates| crates.parent())
        .expect("workspace root")
        .to_path_buf()
}

fn bake_dir(var: &str) -> PathBuf {
    let dir = std::env::var_os(var)
        .map(PathBuf::from)
        .unwrap_or_else(|| panic!("set {var}; this file's module doc gives the inputs"));
    assert!(dir.is_dir(), "{var} = {} is not a directory", dir.display());
    dir
}

/// Every `content/dev` map whose source holds a fluid volume, by stem.
fn water_maps() -> Vec<String> {
    let maps = workspace_root().join("content/dev/maps");
    let mut out: Vec<String> = std::fs::read_dir(&maps)
        .expect("content/dev/maps")
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            (path.extension()? == "map").then_some(path)
        })
        .filter(|path| {
            std::fs::read_to_string(path)
                .expect("map source")
                .contains("\"classname\" \"fluid_volume\"")
        })
        .map(|path| path.file_stem().unwrap().to_string_lossy().into_owned())
        .collect();
    out.sort();
    out
}

fn id54(prl: &Path) -> Option<Vec<u8>> {
    let mut reader =
        BufReader::new(File::open(prl).unwrap_or_else(|e| panic!("{}: {e}", prl.display())));
    let meta = read_container(&mut reader).expect("container");
    read_section_data(&mut reader, &meta, SectionId::ReflectionProbes as u32).expect("id 54")
}

/// One probe, renumbering-free: everything the parity compares.
#[derive(Debug, PartialEq)]
struct ProbeView {
    source_entity: u32,
    box_min: [u32; 3],
    box_max: [u32; 3],
    capture_point: [u32; 3],
    /// (roster index, reference bits, the five water lobes' bits) per stored light slot.
    lights: Vec<(u32, [u32; 3], Vec<[u32; 3]>)>,
    mip0: Vec<u8>,
    distance: Vec<u8>,
    excluded_brushes: usize,
}

fn bits(v: [f32; 3]) -> [u32; 3] {
    v.map(f32::to_bits)
}

/// The pre-branch section (version 2): header of eight u32 after the version, probes of
/// source, box, capture point, layer and K five-lobe light entries, fluid bindings, then mip-0
/// tiles and distance tiles in layer order. Keyed by the sorted fluid faces each probe binds.
fn before_probes(body: &[u8], excluded: &[usize]) -> BTreeMap<Vec<(u32, u32)>, ProbeView> {
    let mut o = 0usize;
    let u16_at = |o: &mut usize| {
        let v = u16::from_le_bytes(body[*o..*o + 2].try_into().unwrap());
        *o += 2;
        v
    };
    let u32_at = |o: &mut usize| {
        let v = u32::from_le_bytes(body[*o..*o + 4].try_into().unwrap());
        *o += 4;
        v
    };
    let f3_at = |o: &mut usize| {
        let mut v = [0u32; 3];
        for c in &mut v {
            *c = u32::from_le_bytes(body[*o..*o + 4].try_into().unwrap());
            *o += 4;
        }
        v
    };
    assert_eq!(u16_at(&mut o), 2, "the pre-branch id 54 is version 2");
    let dim = u32_at(&mut o);
    let _border = u32_at(&mut o);
    let atlas = u32_at(&mut o);
    let dist_fmt = u32_at(&mut o);
    let k = u32_at(&mut o) as usize;
    let lobes = u32_at(&mut o) as usize;
    let probes = u32_at(&mut o) as usize;
    let fluid = u32_at(&mut o) as usize;
    assert_eq!(
        (k, lobes),
        (REFLECTION_PROBE_LIGHTS_PER_PROBE, BEFORE_LOBES)
    );
    let mut heads = Vec::with_capacity(probes);
    for _ in 0..probes {
        let source = u32_at(&mut o);
        let box_min = f3_at(&mut o);
        let box_max = f3_at(&mut o);
        let capture = f3_at(&mut o);
        let layer = u32_at(&mut o) as usize;
        let mut lights = Vec::with_capacity(k);
        for _ in 0..k {
            let roster = u32_at(&mut o);
            let reference = f3_at(&mut o);
            let shares = (0..lobes).map(|_| f3_at(&mut o)).collect();
            lights.push((roster, reference, shares));
        }
        heads.push((source, box_min, box_max, capture, layer, lights));
    }
    let mut faces: Vec<Vec<(u32, u32)>> = vec![Vec::new(); probes];
    for _ in 0..fluid {
        let (volume, face, probe) = (u32_at(&mut o), u32_at(&mut o), u32_at(&mut o));
        faces[probe as usize].push((volume, face));
    }
    let tile = reflection_probe_tile_bytes(atlas, dim).expect("atlas format");
    let dist = reflection_probe_distance_tile_bytes(dist_fmt, dim).expect("distance format");
    let tiles = &body[o..o + probes * tile];
    let dists = &body[o + probes * tile..o + probes * (tile + dist)];
    assert_eq!(
        o + probes * (tile + dist),
        body.len(),
        "pre-branch id 54 length"
    );
    let mut out = BTreeMap::new();
    for (p, (source, box_min, box_max, capture, layer, lights)) in heads.into_iter().enumerate() {
        let mut key = std::mem::take(&mut faces[p]);
        key.sort_unstable();
        let view = ProbeView {
            source_entity: source,
            box_min,
            box_max,
            capture_point: capture,
            lights,
            mip0: tiles[layer * tile..(layer + 1) * tile].to_vec(),
            distance: dists[layer * dist..(layer + 1) * dist].to_vec(),
            excluded_brushes: excluded[p],
        };
        assert!(
            out.insert(key, view).is_none(),
            "two pre-branch probes bind the same faces"
        );
    }
    out
}

/// The current section's probes binding no world face, keyed as `before_probes`; −Y shares are
/// checked zero here and dropped from the view.
fn after_probes(
    section: &ReflectionProbesSection,
    excluded: &[usize],
    map: &str,
) -> BTreeMap<Vec<(u32, u32)>, ProbeView> {
    let tile = reflection_probe_tile_bytes(section.atlas_format, section.tile_dimension).unwrap();
    let chain = reflection_probe_colour_chain_bytes(
        section.atlas_format,
        section.tile_dimension,
        section.colour_mip_count,
    )
    .unwrap();
    let dist =
        reflection_probe_distance_tile_bytes(section.distance_format, section.tile_dimension)
            .unwrap();
    let mut faces: Vec<Vec<(u32, u32)>> = vec![Vec::new(); section.probes.len()];
    for b in &section.fluid_bindings {
        faces[b.probe as usize].push((b.volume, b.face));
    }
    let mut world = vec![false; section.probes.len()];
    for b in &section.world_bindings {
        world[b.probe as usize] = true;
    }
    let mut out = BTreeMap::new();
    for (p, probe) in section.probes.iter().enumerate() {
        if world[p] {
            continue;
        }
        let layer = probe.layer as usize;
        let lights = probe
            .lights
            .iter()
            .map(|l| {
                assert_eq!(
                    bits(l.shares[BEFORE_LOBES]),
                    [0; 3],
                    "{map}: probe {p} light {} holds a −Y share",
                    l.roster_index
                );
                let water: Vec<[u32; 3]> =
                    l.shares[..BEFORE_LOBES].iter().map(|s| bits(*s)).collect();
                (l.roster_index, bits(l.reference), water)
            })
            .collect();
        let mut key = std::mem::take(&mut faces[p]);
        key.sort_unstable();
        let view = ProbeView {
            source_entity: probe.source_entity,
            box_min: bits(probe.box_min),
            box_max: bits(probe.box_max),
            capture_point: bits(probe.capture_point),
            lights,
            mip0: section.tiles[layer * chain..layer * chain + tile].to_vec(),
            distance: section.distances[layer * dist..(layer + 1) * dist].to_vec(),
            excluded_brushes: excluded[p],
        };
        assert!(
            out.insert(key, view).is_none(),
            "{map}: two probes bind the same faces"
        );
    }
    out
}

/// Per probe index, the `-v` log's "N brush(es) excluded from its capture" count.
fn excluded_counts(log: &Path, probes: usize) -> Vec<usize> {
    let text = std::fs::read_to_string(log).unwrap_or_else(|e| panic!("{}: {e}", log.display()));
    let mut out = vec![None; probes];
    for line in text.lines() {
        let Some(rest) = line.split("[ReflectionProbes] probe ").nth(1) else {
            continue;
        };
        let Some((index, tail)) = rest.split_once(": ") else {
            continue;
        };
        let Some(count) = tail.strip_suffix(" brush(es) excluded from its capture") else {
            continue;
        };
        let index: usize = index.parse().expect("probe index");
        assert!(
            index < probes,
            "{}: probe {index} past {probes}",
            log.display()
        );
        out[index] = Some(count.parse::<usize>().expect("excluded count"));
    }
    out.into_iter()
        .enumerate()
        .map(|(p, c)| {
            c.unwrap_or_else(|| panic!("{}: no excluded count for probe {p}", log.display()))
        })
        .collect()
}

#[test]
#[ignore = "opt-in A/B: set POSTRETRO_WATER_PARITY_BEFORE_DIR and POSTRETRO_WATER_PARITY_AFTER_DIR to fresh bakes (module doc)"]
fn every_water_probe_places_and_captures_as_the_build_before_glossy_reflections() {
    let before_dir = bake_dir(BEFORE_ENV);
    let after_dir = bake_dir(AFTER_ENV);
    let maps = water_maps();
    assert!(!maps.is_empty(), "no water map in content/dev");
    let mut compared = 0usize;
    for map in &maps {
        let before = id54(&before_dir.join(format!("{map}.prl")));
        let after = id54(&after_dir.join(format!("{map}.prl")));
        let (before, after) = match (before, after) {
            (None, None) => {
                eprintln!("{map}: no id 54 in either bake");
                continue;
            }
            (Some(b), Some(a)) => (b, a),
            (b, a) => panic!(
                "{map}: id 54 present before {} / after {}",
                b.is_some(),
                a.is_some()
            ),
        };
        let after = ReflectionProbesSection::from_bytes(&after).expect("current id 54 decodes");
        let before_count = u32::from_le_bytes(before[2 + 4 * 6..2 + 4 * 7].try_into().unwrap());
        let before = before_probes(
            &before,
            &excluded_counts(
                &before_dir.join(format!("{map}.log")),
                before_count as usize,
            ),
        );
        let after_views = after_probes(
            &after,
            &excluded_counts(&after_dir.join(format!("{map}.log")), after.probes.len()),
            map,
        );
        assert_eq!(
            before.keys().collect::<Vec<_>>(),
            after_views.keys().collect::<Vec<_>>(),
            "{map}: the probes binding no world face bind other fluid faces than before"
        );
        for (faces, view) in &after_views {
            let old = &before[faces];
            assert_eq!(
                old.source_entity, view.source_entity,
                "{map} {faces:?}: source entity"
            );
            assert_eq!(
                (old.box_min, old.box_max),
                (view.box_min, view.box_max),
                "{map} {faces:?}: box"
            );
            assert_eq!(
                old.capture_point, view.capture_point,
                "{map} {faces:?}: capture point"
            );
            assert_eq!(
                old.lights, view.lights,
                "{map} {faces:?}: lights or water-lobe shares"
            );
            assert!(old.mip0 == view.mip0, "{map} {faces:?}: mip-0 colour tile");
            assert!(
                old.distance == view.distance,
                "{map} {faces:?}: distance tile"
            );
            assert_eq!(
                old.excluded_brushes, view.excluded_brushes,
                "{map} {faces:?}: excluded brushes"
            );
        }
        eprintln!(
            "{map}: {} probe(s) binding no world face match the build before ({} world-bound)",
            after_views.len(),
            after.probes.len() - after_views.len()
        );
        compared += after_views.len();
    }
    assert!(compared > 0, "no probe compared");
}

// Reflection capture's SH mirror with probe depth tiles: parity for untiled probes, darkness behind a thin door.
// See: context/lib/resource_management.md §5.2 · context/lib/rendering_pipeline.md §4

use glam::{DVec3, UVec3, Vec3};
use postretro_level_format::lightmap::f32_to_f16_bits;
use postretro_level_format::probe_depth_tiles::{
    MAX_PROBE_DEPTH_TILES, PROBE_DEPTH_TILE_TEXELS, ProbeDepthTilesSection,
};
use postretro_level_format::sh_reconstruct::Level;

use super::*;
use crate::reflection_probes::reflection_probes_test_fixtures::dense_sh_volume;
use crate::sh_bake::depth_tiles_test_fixtures::DepthTileFixture;
use crate::sh_density::pack_indirect_section;

/// A 4³ volume whose probes carry near, distinct depth moments, so the
/// averaged Chebyshev weight bites at every sample.
fn near_moments_volume() -> OctahedralShVolumeSection {
    let mut dense = dense_sh_volume([4, 4, 4], |_, _, _| [1.0, 1.0, 1.0]);
    for (i, probe) in dense.probes.iter_mut().enumerate() {
        let mean = 0.3 + 0.01 * i as f32;
        probe.mean_distance = f32_to_f16_bits(mean);
        probe.mean_sq_distance = f32_to_f16_bits(mean * mean + 0.02);
    }
    pack_indirect_section(dense, Some(Level::L0)).unwrap().0
}

// A probe without a tile weighs exactly as with no id 59, bit for bit, even
// beside a tiled neighbour; the tiled probe weighs by its tile, here a
// zero-depth tile that holds it at the 0.03 floor.
#[test]
fn an_untiled_probe_beside_a_tiled_one_weighs_bit_for_bit_as_without_tiles() {
    let volume = near_moments_volume();
    let tiled = 1 + 4 + 16; // (1, 1, 1)
    let tiles = ProbeDepthTilesSection {
        probe_indices: vec![tiled],
        texels: vec![[0, 0]; PROBE_DEPTH_TILE_TEXELS],
    };
    let plain = ShVolumeSampler::new(&volume, true).unwrap();
    let with_tiles = ShVolumeSampler::new(&volume, true)
        .unwrap()
        .with_depth_tiles(&tiles)
        .unwrap();
    let samples = [
        Vec3::new(1.4, 1.3, 1.6),
        Vec3::new(0.2, 1.9, 1.1),
        Vec3::new(2.1, 0.6, 1.5),
    ];
    for probe in 0..64u32 {
        let idx = UVec3::new(probe % 4, (probe / 4) % 4, probe / 16);
        for sample in samples {
            let before = plain.depth_visibility(probe as usize, idx, sample);
            let after = with_tiles.depth_visibility(probe as usize, idx, sample);
            if probe == tiled {
                assert_eq!(after, 0.03, "{sample}");
            } else {
                assert_eq!(
                    after.to_bits(),
                    before.to_bits(),
                    "probe {probe} at {sample}"
                );
            }
        }
    }
    // An empty id 59 changes nothing either.
    let empty = ProbeDepthTilesSection::default();
    let none = ShVolumeSampler::new(&volume, true)
        .unwrap()
        .with_depth_tiles(&empty)
        .unwrap();
    for sample in samples {
        assert_eq!(
            none.indirect(sample, Vec3::Y).to_array().map(f32::to_bits),
            plain.indirect(sample, Vec3::Y).to_array().map(f32::to_bits)
        );
    }
}

// A tile naming an invalid probe is refused, as the loader refuses it.
#[test]
fn a_tile_naming_an_invalid_probe_is_refused() {
    let dense = dense_sh_volume(
        [4, 4, 4],
        |x, _, _| {
            if x == 0 { [f32::NAN; 3] } else { [1.0; 3] }
        },
    );
    let (volume, _) = pack_indirect_section(dense, Some(Level::L0)).unwrap();
    let tiles = ProbeDepthTilesSection {
        probe_indices: vec![0],
        texels: vec![[0, 0]; PROBE_DEPTH_TILE_TEXELS],
    };
    assert!(
        ShVolumeSampler::new(&volume, true)
            .unwrap()
            .with_depth_tiles(&tiles)
            .is_err()
    );
}

// The capture reads the closet floor behind the static door as the tiled
// runtime does: darker than the averaged moments leave it, which glow there.
#[test]
fn the_static_door_closet_floor_reads_darker_with_tiles() {
    let fixture = DepthTileFixture::load("mover-closet-static-door");
    let base = fixture.bake(MAX_PROBE_DEPTH_TILES, None);
    assert!(base.depth_tiles.section.tile_count() > 0);
    let (volume, _) = pack_indirect_section(base.section, Some(Level::L0)).unwrap();
    let plain = ShVolumeSampler::new(&volume, PRODUCTION_PROBE_OCCLUSION).unwrap();
    let tiled = ShVolumeSampler::new(&volume, PRODUCTION_PROBE_OCCLUSION)
        .unwrap()
        .with_depth_tiles(&base.depth_tiles.section)
        .unwrap();
    // The floor behind the door's closet face at y 504, 0.05 to 0.3 m back,
    // where the averaged moments glow: map units, floor top z 0
    // (mover-closet.README.md).
    for map_y in [506.0, 510.0, 516.0] {
        let point =
            (crate::parse::quake_to_engine(DVec3::new(320.0, map_y, 0.0)) * 0.0254).as_vec3();
        let without = plain.indirect(point, Vec3::Y);
        let with = tiled.indirect(point, Vec3::Y);
        eprintln!("map y {map_y}: without tiles {without}, with tiles {with}");
        assert!(
            without.max_element() > 0.0,
            "y {map_y}: the averaged moments leak"
        );
        assert!(
            with.max_element() < 0.5 * without.max_element(),
            "y {map_y}: tiles darken the floor behind the door: {with} vs {without}"
        );
    }
}

/// Largest share of its no-tile indirect a sprite behind the static door may
/// keep with tiles (A3). The first-slice pilot measured it; see the plan.
const SPRITE_FRACTION_CEILING: f32 = 0.10;

// A3, sprite half: a sprite on the closet floor at the door, sampled as the
// billboard pass samples it (capture draws no billboards), keeps at most a set
// share of its no-tile indirect with tiles.
#[test]
fn a_sprite_behind_the_static_door_darkens_with_tiles() {
    let fixture = DepthTileFixture::load("mover-closet-static-door");
    let base = fixture.bake(MAX_PROBE_DEPTH_TILES, None);
    let (volume, _) = pack_indirect_section(base.section, Some(Level::L0)).unwrap();
    let plain = ShVolumeSampler::new(&volume, PRODUCTION_PROBE_OCCLUSION).unwrap();
    let tiled = ShVolumeSampler::new(&volume, PRODUCTION_PROBE_OCCLUSION)
        .unwrap()
        .with_depth_tiles(&base.depth_tiles.section)
        .unwrap();
    // Station 3 faces −y at pitch −38°; a billboard's normal is camera forward.
    let pitch = (-38.0f64).to_radians();
    let forward = crate::parse::quake_to_engine(DVec3::new(0.0, -pitch.cos(), pitch.sin()))
        .as_vec3()
        .normalize();
    for (map_y, map_z) in [(508.0, 8.0), (512.0, 16.0), (520.0, 16.0)] {
        let point =
            (crate::parse::quake_to_engine(DVec3::new(320.0, map_y, map_z)) * 0.0254).as_vec3();
        let without = plain.indirect_as_billboard(point, forward).max_element();
        let with = tiled.indirect_as_billboard(point, forward).max_element();
        let fraction = with / without;
        eprintln!(
            "sprite at map y {map_y} z {map_z}: without {without}, with {with}, fraction {fraction}"
        );
        assert!(
            without > 0.0,
            "y {map_y}: the averaged moments leak onto the sprite"
        );
        assert!(
            fraction <= SPRITE_FRACTION_CEILING,
            "y {map_y}: the sprite keeps {fraction} of its no-tile indirect"
        );
    }
}

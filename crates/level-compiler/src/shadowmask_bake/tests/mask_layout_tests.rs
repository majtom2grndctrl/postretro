//! The per-block mask layout: which charts it packs, where plane B lies, and
//! the identity fallback that bounds it by its lightmap block.
use super::*;
use mask_layout::{MaskBlockLayout, assert_layout_sound, layout_mask_block, owns_plane_b};
use postretro_level_format::lightmap_pool::{PoolDemand, PoolKind};
use postretro_level_format::shadowmask_atlas::{FaceOwners, MaskExtents, ShadowmaskAtlasSection};

/// Owns channel 2: its chart samples plane B.
const B: FaceOwners = [0, 1, 2, E];
/// Owns channels 0/1 only.
const A: FaceOwners = [3, E, E, E];

fn layout_of(atlas: &Atlas, owners: &[FaceOwners]) -> MaskBlockLayout {
    let shared = atlas.shared();
    let faces: Vec<u32> = (0..atlas.charts.len() as u32).collect();
    let layout = layout_mask_block(&atlas.layout.blocks[0], &faces, owners, &shared);
    assert_layout_sound(&layout, owners, &shared);
    layout
}

fn rect_of(layout: &MaskBlockLayout, face: u32) -> (u32, u32, u32, u32) {
    let &(_, (x, y)) = layout
        .charts
        .iter()
        .find(|(f, _)| *f == face)
        .expect("the face is packed");
    (x, y, x + CHART, y + CHART)
}

// Every chart whose face owns channel 2 or 3 lies inside the plane-B
// sub-rect at the mask block's origin, so plane B never clamps a face onto a
// neighbour's texels.
#[test]
fn every_plane_b_chart_lies_inside_plane_b() {
    let atlas = Atlas::new(6);
    let owners = [B, A, NO_FACE_OWNERS, B, A, NO_FACE_OWNERS];
    let layout = layout_of(&atlas, &owners);
    assert!(!layout.identity, "four of six charts pack smaller");
    assert_eq!(layout.charts.len(), 4, "unowned faces take no place");
    for face in [0, 3] {
        let (_, _, x1, y1) = rect_of(&layout, face);
        assert!(
            x1 <= layout.plane_b.0 && y1 <= layout.plane_b.1,
            "face {face}"
        );
    }
    let [mask, plane_b] = [layout.extent, layout.plane_b];
    for (w, h) in [mask, plane_b] {
        assert!(w % 4 == 0 && h % 4 == 0, "whole BC blocks: {w}x{h}");
    }
    assert!(plane_b.0 <= mask.0 && plane_b.1 <= mask.1);
    assert!(
        u64::from(mask.0) * u64::from(mask.1) < u64::from(atlas.width) * u64::from(CHART),
        "the mask block is smaller than its lightmap block"
    );
}

// A channel-0/1 chart packs around plane B, never inside it: plane B holds
// only charts that sample it.
#[test]
fn a_channel_01_chart_may_sit_outside_plane_b() {
    let atlas = Atlas::new(6);
    let owners = [B, A, NO_FACE_OWNERS, B, A, NO_FACE_OWNERS];
    let layout = layout_of(&atlas, &owners);
    for face in [1, 4] {
        assert!(!owns_plane_b(&owners[face as usize]));
        let (x0, y0, _, _) = rect_of(&layout, face);
        assert!(
            x0 >= layout.plane_b.0 || y0 >= layout.plane_b.1,
            "face {face} at ({x0}, {y0}) sits inside plane B {:?}",
            layout.plane_b
        );
    }
}

// A block with no owned face has no mask block: zero extents, no chart,
// no stored plane, and so no mask-pool placement.
#[test]
fn a_block_with_no_owned_face_has_zero_extents_no_planes_no_placement() {
    let atlas = Atlas::new(2);
    let layout = layout_of(&atlas, &[NO_FACE_OWNERS; 2]);
    assert_eq!((layout.extent, layout.plane_b), ((0, 0), (0, 0)));
    assert!(layout.charts.is_empty());
    assert_eq!(layout.extents(), MaskExtents::NONE);
    assert_eq!(
        encode::encode_mask_block(&[], 0, 0, (0, 0)),
        [Vec::<u8>::new(), Vec::new()]
    );

    let lightmap = postretro_level_format::lightmap::LightmapSection {
        direction_texel_scale: 2,
        irradiance_format: postretro_level_format::lightmap::IRRADIANCE_FORMAT_BC6H,
        mode: postretro_level_format::lightmap::LightmapMode::Shadowed,
        blocks: vec![postretro_level_format::lightmap::LightmapBlock {
            codec: postretro_level_format::coded_plane::BlockCodec::Raw,
            cell_id: 0,
            width: 16,
            height: 8,
            irradiance: Vec::new(),
            direction: Vec::new(),
        }],
    };
    let section = ShadowmaskAtlasSection {
        owners: vec![NO_FACE_OWNERS; 2],
        mask_offsets: vec![[0; 2]; 2],
        blocks: vec![[Vec::new(), Vec::new()]],
        codecs: vec![postretro_level_format::coded_plane::BlockCodec::Zstd],
        extents: vec![layout.extents()],
    };
    let demand = PoolDemand::of_indices(&lightmap.index(), Some(&section.index()));
    assert!(demand.has_mask_pool());
    assert!(demand.items(PoolKind::Mask).is_empty(), "no mask placement");
    assert_eq!(demand.all_resident(2048).unwrap().mask, 0);
}

// Owned charts that pack no smaller than their block keep the lightmap
// layout: the mask pool never grows past whole-block planes.
#[test]
fn owned_charts_that_pack_no_smaller_keep_the_lightmap_layout() {
    let atlas = Atlas::new(2);
    let layout = layout_of(&atlas, &[B, A]);
    assert!(layout.identity);
    assert_eq!(layout.extent, (atlas.width, CHART));
    assert_eq!(layout.plane_b, layout.extent, "a face owns channel 2");
    let shared = atlas.shared();
    for &(face, origin) in &layout.charts {
        assert_eq!(
            mask_layout::face_offset(&atlas.layout.blocks[0], face, origin, &shared),
            [0, 0],
            "face {face} stays where the lightmap placed it"
        );
    }
    let a_only = layout_of(&atlas, &[A, A]);
    assert!(a_only.identity);
    assert_eq!(a_only.plane_b, (0, 0), "no face samples plane B");
}

// The layout depends only on the owned charts and their plane-B membership.
#[test]
fn the_layout_is_the_same_on_every_run() {
    let atlas = Atlas::new(6);
    let owners = [B, A, NO_FACE_OWNERS, B, A, NO_FACE_OWNERS];
    let first = layout_of(&atlas, &owners);
    let threads: Vec<MaskBlockLayout> = std::thread::scope(|scope| {
        (0..3)
            .map(|_| scope.spawn(|| layout_of(&atlas, &owners)))
            .collect::<Vec<_>>()
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect()
    });
    assert!(threads.iter().all(|layout| *layout == first));
}

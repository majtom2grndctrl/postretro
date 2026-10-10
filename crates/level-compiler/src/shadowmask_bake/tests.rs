// Shadowmask ownership tests: eligibility, admission and ranking, eviction,
// channel order, the drop report, and the memo entry.
// See: context/lib/build_pipeline.md §PRL (ShadowmaskAtlas ownership)

use glam::{DVec3, Vec3};
use postretro_level_format::entity_shadow_lights::EntityShadowLightsSection;
use postretro_level_format::shadowmask_atlas::{NO_FACE_OWNERS, SHADOWMASK_OWNER_EMPTY};

use super::*;
use crate::chart_raster::{CHART_PADDING_TEXELS, ChartPlacement};
use crate::light_namespaces::{AlphaLightsNs, StaticBakedLights};
use crate::lightmap_bake::{BlockLayout, CellBlock, Chart};
use crate::lightmap_layer::LayerTexel;
use crate::map_data::{FalloffModel, LightType, MapLight, ShadowType};
use ownership::{DropSite, LIT_TEXEL_FLOOR, OwnerDrop};

const E: u16 = SHADOWMASK_OWNER_EMPTY;
/// Interior edge: 4 × 4 = 16 texels, exactly the lit-texel floor.
const INTERIOR: u32 = 4;
const PAD: u32 = CHART_PADDING_TEXELS;
const CHART: u32 = INTERIOR + 2 * PAD;

/// Faces side by side in bake layer 0, one 1 m texel per interior texel, all
/// in one whole-layer block.
struct Atlas {
    charts: Vec<Chart>,
    placements: Vec<ChartPlacement>,
    layout: BlockLayout,
    width: u32,
}

impl Atlas {
    fn new(faces: usize) -> Self {
        let width = faces as u32 * CHART;
        let charts = (0..faces)
            .map(|face| Chart {
                origin: Vec3::new(face as f32 * 10.0, 0.0, 0.0),
                u_axis: Vec3::X,
                v_axis: Vec3::Z,
                uv_min: [0.0, 0.0],
                uv_extent: [INTERIOR as f32, INTERIOR as f32],
                normal: Vec3::Y,
                width_texels: CHART,
                height_texels: CHART,
                leaf_index: 0,
                window: None,
            })
            .collect();
        let placements = (0..faces)
            .map(|face| ChartPlacement {
                x: face as u32 * CHART,
                y: 0,
                layer: 0,
            })
            .collect();
        Self {
            charts,
            placements,
            layout: BlockLayout {
                direction_texel_scale: 2,
                blocks: vec![CellBlock {
                    cell_id: 0,
                    width,
                    height: CHART,
                    layer: 0,
                    x: 0,
                    y: 0,
                }],
                chart_blocks: vec![0; faces],
            },
            width,
        }
    }

    fn shared(&self) -> SharedAtlas<'_> {
        SharedAtlas {
            charts: &self.charts,
            placements: &self.placements,
            atlas_width: self.width,
            atlas_height: CHART,
            layout: &self.layout,
        }
    }

    /// Bake-layer index of interior texel `(tx, ty)` of `face`.
    fn idx(&self, face: usize, tx: u32, ty: u32) -> u32 {
        (PAD + ty) * self.width + face as u32 * CHART + PAD + tx
    }

    /// Every texel of `face`'s padded chart.
    fn padded(&self, face: usize) -> Vec<u32> {
        (0..CHART)
            .flat_map(|y| (0..CHART).map(move |x| (y, x)))
            .map(|(y, x)| y * self.width + face as u32 * CHART + x)
            .collect()
    }

    /// The first `count` interior texels of `face`, row-major, at `visibility`.
    fn lit(&self, face: usize, count: u32, visibility: f32) -> Vec<(u32, f32)> {
        (0..count)
            .map(|n| (self.idx(face, n % INTERIOR, n / INTERIOR), visibility))
            .collect()
    }
}

/// A partition of bake layer 0 holding `texels`, sorted as the walk emits.
fn partition(atlas: &Atlas, mut texels: Vec<(u32, f32)>) -> LightmapLayer {
    texels.sort_by_key(|&(idx, _)| idx);
    LightmapLayer {
        atlas_width: atlas.width,
        atlas_height: CHART,
        layer_count: 1,
        target_layer: 0,
        texels: texels
            .into_iter()
            .map(|(idx, raw_visibility)| LayerTexel {
                idx,
                raw_visibility,
            })
            .collect(),
    }
}

/// A point light 3 m above `face`'s interior; brighter means a larger
/// contribution on every texel.
fn light_over(face: usize, intensity: f32) -> MapLight {
    MapLight {
        origin: DVec3::new(face as f64 * 10.0 + 2.0, 3.0, 2.0),
        carrier: String::new(),
        light_type: LightType::Point,
        intensity,
        color: [1.0, 1.0, 1.0],
        falloff_model: FalloffModel::Linear,
        falloff_range: 20.0,
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
    }
}

fn eligible(light: &MapLight, spec_index: u16, promoted: bool) -> EligibleLight<'_> {
    EligibleLight {
        source_index: usize::from(spec_index),
        spec_index,
        promoted,
        light,
    }
}

struct Fill {
    section: Option<ShadowmaskAtlasSection>,
    report: OwnerReport,
    /// The raw fill the encoder consumed; empty when no section.
    raw: Vec<u8>,
}

impl Fill {
    fn owners(&self, face: usize) -> [u16; 4] {
        self.section
            .as_ref()
            .map_or(NO_FACE_OWNERS, |section| section.owners[face])
    }

    fn channel(&self, idx: u32, channel: usize) -> u8 {
        self.raw[idx as usize * 4 + channel]
    }
}

/// Consume `arrivals` in order through one owner fill.
fn run(atlas: &Atlas, arrivals: &[(&EligibleLight<'_>, LightmapLayer)]) -> Fill {
    let shared = atlas.shared();
    let mut fill = OwnershipFill::new(&shared, 1, None);
    for (light, partition) in arrivals {
        fill.consume(light, partition, &shared);
    }
    let (section, report) = fill.finish(&shared);
    let raw = if section.is_some() {
        take_last_raw_fill()
    } else {
        Vec::new()
    };
    Fill {
        section,
        report,
        raw,
    }
}

fn whole_face(atlas: &Atlas, face: usize) -> LightmapLayer {
    partition(atlas, atlas.lit(face, INTERIOR * INTERIOR, 1.0))
}

fn drop_at(face: u32, spec_index: u16, lit_texels: u32) -> OwnerDrop {
    OwnerDrop {
        site: DropSite::Face(face),
        spec_index,
        lit_texels,
    }
}

// AC 20: five eligible lights lit across one face yield four owners, chosen by
// largest shadowed contribution, and the fifth is reported; four yield none.
#[test]
fn five_lit_lights_keep_the_four_largest_contributions_and_report_the_fifth() {
    let atlas = Atlas::new(1);
    let lights: Vec<MapLight> = [3.0, 1.0, 5.0, 4.0, 2.0]
        .into_iter()
        .map(|intensity| light_over(0, intensity))
        .collect();
    let eligible: Vec<_> = lights
        .iter()
        .enumerate()
        .map(|(spec, light)| eligible(light, spec as u16, false))
        .collect();
    let arrivals: Vec<_> = eligible
        .iter()
        .map(|light| (light, whole_face(&atlas, 0)))
        .collect();
    let five = run(&atlas, &arrivals);
    assert_eq!(five.owners(0), [0, 2, 3, 4], "intensity 1 (spec 1) drops");
    assert_eq!(five.report.drops, vec![drop_at(0, 1, 16)]);
    assert_eq!(five.report.peak_face_demand, 5);

    let four = run(&atlas, &arrivals[..4]);
    assert_eq!(four.owners(0), [0, 1, 2, 3]);
    assert!(four.report.drops.is_empty());
    assert_eq!(four.report.peak_face_demand, 4);
}

// AC 22 (P24): owners sit in ascending light-index order whatever order the
// bake met them, and every owner's mask values sit in that owner's channel.
#[test]
fn owners_ascend_whatever_the_arrival_order_and_their_masks_move_with_them() {
    let atlas = Atlas::new(1);
    let lights: Vec<MapLight> = (0..4).map(|_| light_over(0, 2.0)).collect();
    // Spec 3 arrives first, spec 0 last; each writes its own visibility.
    let visibility = |spec: usize| 0.2 * (spec + 1) as f32;
    let eligible: Vec<_> = (0..4)
        .rev()
        .map(|spec| eligible(&lights[spec], spec as u16, false))
        .collect();
    let arrivals: Vec<_> = eligible
        .iter()
        .map(|light| {
            let spec = usize::from(light.spec_index);
            (light, partition(&atlas, atlas.lit(0, 16, visibility(spec))))
        })
        .collect();
    let fill = run(&atlas, &arrivals);
    assert_eq!(fill.owners(0), [0, 1, 2, 3]);
    let texel = atlas.idx(0, 1, 1);
    for spec in 0..4 {
        let expected = (visibility(spec) * 255.0 + 0.5) as u8;
        assert_eq!(
            fill.channel(texel, spec),
            expected,
            "owner {spec}'s mask must sit in channel {spec}"
        );
    }
}

// AC 24 (P4): when a later, higher-ranked light evicts an owner, none of the
// evicted light's values remain in that channel; texels the newcomer does not
// reach read zero, padding included.
#[test]
fn an_evicting_newcomer_leaves_none_of_the_evicted_values() {
    let atlas = Atlas::new(1);
    let dim = light_over(0, 1.0);
    let others: Vec<MapLight> = (0..3).map(|_| light_over(0, 3.0)).collect();
    let bright = light_over(0, 8.0);
    let dim_e = eligible(&dim, 0, false);
    let other_e: Vec<_> = others
        .iter()
        .enumerate()
        .map(|(i, light)| eligible(light, i as u16 + 1, false))
        .collect();
    let bright_e = eligible(&bright, 4, false);
    let mut arrivals = vec![(&dim_e, whole_face(&atlas, 0))];
    for light in &other_e {
        arrivals.push((light, whole_face(&atlas, 0)));
    }
    // The newcomer lights the whole face, half of it at half visibility, so a
    // stale value from the evicted light would show.
    let newcomer_texels: Vec<(u32, f32)> = (0..16)
        .map(|n| {
            let idx = atlas.idx(0, n % INTERIOR, n / INTERIOR);
            (idx, if n < 8 { 1.0 } else { 0.5 })
        })
        .collect();
    arrivals.push((&bright_e, partition(&atlas, newcomer_texels)));
    let fill = run(&atlas, &arrivals);
    assert_eq!(fill.owners(0), [1, 2, 3, 4], "the dim incumbent is evicted");
    assert_eq!(fill.report.drops, vec![drop_at(0, 0, 16)]);
    // The newcomer took the dim light's channel 0 and sorted to position 3.
    for idx in atlas.padded(0) {
        let value = fill.channel(idx, 3);
        let interior_row = (idx / atlas.width).checked_sub(PAD);
        let expected = match interior_row {
            Some(0 | 1) if (PAD..PAD + INTERIOR).contains(&(idx % atlas.width)) => 255,
            Some(2 | 3) if (PAD..PAD + INTERIOR).contains(&(idx % atlas.width)) => 128,
            _ => 0,
        };
        assert_eq!(value, expected, "texel {idx}");
    }
}

// AC 14 (P4): an owner occluded at a face edge reads zero there, including the
// padding a bilinear tap reaches; no owned channel keeps an unwritten value.
#[test]
fn an_owner_occluded_at_the_face_edge_reads_zero_there_and_in_padding() {
    let atlas = Atlas::new(1);
    let light = light_over(0, 2.0);
    let owner = eligible(&light, 0, false);
    // Lit everywhere except the last interior column.
    let texels: Vec<(u32, f32)> = (0..16)
        .map(|n| {
            let (tx, ty) = (n % INTERIOR, n / INTERIOR);
            (
                atlas.idx(0, tx, ty),
                if tx == INTERIOR - 1 { 0.0 } else { 1.0 },
            )
        })
        .collect();
    let fill = run(&atlas, &[(&owner, partition(&atlas, texels))]);
    assert_eq!(fill.owners(0), [0, E, E, E]);
    for ty in 0..INTERIOR {
        assert_eq!(fill.channel(atlas.idx(0, INTERIOR - 1, ty), 0), 0);
        assert_eq!(fill.channel(atlas.idx(0, 0, ty), 0), 255);
    }
    let interior: Vec<u32> = (0..16).map(|n| atlas.idx(0, n % 4, n / 4)).collect();
    for idx in atlas.padded(0) {
        if !interior.contains(&idx) {
            assert_eq!(fill.channel(idx, 0), 0, "padding texel {idx}");
        }
        for channel in 1..4 {
            assert_eq!(fill.channel(idx, channel), 0, "unowned channel {channel}");
        }
    }
}

// AC 25: with five above-floor candidates, a promoted light with the smallest
// contribution keeps its channel over a brighter specular-only light; with no
// promoted light among them, the smallest contribution drops.
#[test]
fn an_above_floor_promoted_light_outranks_brighter_specular_only_lights() {
    let atlas = Atlas::new(1);
    let lights: Vec<MapLight> = [1.0, 2.0, 3.0, 4.0, 5.0]
        .into_iter()
        .map(|intensity| light_over(0, intensity))
        .collect();
    for promote_dimmest in [true, false] {
        let eligible: Vec<_> = lights
            .iter()
            .enumerate()
            .map(|(spec, light)| eligible(light, spec as u16, promote_dimmest && spec == 0))
            .collect();
        let arrivals: Vec<_> = eligible
            .iter()
            .map(|light| (light, whole_face(&atlas, 0)))
            .collect();
        let fill = run(&atlas, &arrivals);
        if promote_dimmest {
            assert_eq!(fill.owners(0), [0, 2, 3, 4], "spec-only spec 1 drops");
            assert_eq!(fill.report.drops, vec![drop_at(0, 1, 16)]);
        } else {
            assert_eq!(fill.owners(0), [1, 2, 3, 4]);
            assert_eq!(fill.report.drops, vec![drop_at(0, 0, 16)]);
        }
    }
}

// AC 26 (P5): a promoted candidate below the floor drops silently and never
// takes a channel from an above-floor specular-only light.
#[test]
fn a_below_floor_promoted_candidate_never_displaces_an_above_floor_owner() {
    let atlas = Atlas::new(1);
    let lights: Vec<MapLight> = (0..5).map(|_| light_over(0, 1.0)).collect();
    // Spec 0: promoted, very bright per texel but on 8 texels only.
    let bright_promoted = light_over(0, 50.0);
    let promoted = eligible(&bright_promoted, 0, true);
    let spec_only: Vec<_> = (1..=4)
        .map(|spec| eligible(&lights[spec], spec as u16, false))
        .collect();
    let mut arrivals: Vec<_> = spec_only
        .iter()
        .map(|light| (light, whole_face(&atlas, 0)))
        .collect();
    arrivals.push((&promoted, partition(&atlas, atlas.lit(0, 8, 1.0))));
    let fill = run(&atlas, &arrivals);
    assert_eq!(fill.owners(0), [1, 2, 3, 4]);
    assert!(fill.report.drops.is_empty(), "a below-floor drop is silent");
}

// AC 27 (P19): a below-floor promoted light that arrives first and takes a free
// channel loses it once four above-floor specular-only lights arrive.
#[test]
fn a_below_floor_promoted_incumbent_is_evicted_by_four_above_floor_arrivals() {
    let atlas = Atlas::new(1);
    let bright_promoted = light_over(0, 50.0);
    let promoted = eligible(&bright_promoted, 0, true);
    let lights: Vec<MapLight> = (0..4).map(|_| light_over(0, 1.0)).collect();
    let spec_only: Vec<_> = (0..4)
        .map(|i| eligible(&lights[i], i as u16 + 1, false))
        .collect();
    let mut arrivals = vec![(&promoted, partition(&atlas, atlas.lit(0, 8, 1.0)))];
    for light in &spec_only {
        arrivals.push((light, whole_face(&atlas, 0)));
    }
    let fill = run(&atlas, &arrivals);
    assert_eq!(fill.owners(0), [1, 2, 3, 4]);
    assert!(fill.report.drops.is_empty());
    // Its eight texels were cleared from the channel it lost.
    for idx in atlas.padded(0) {
        let row = idx / atlas.width;
        let col = idx % atlas.width;
        let interior = (PAD..PAD + INTERIOR).contains(&row) && (PAD..PAD + INTERIOR).contains(&col);
        let held: Vec<u8> = (0..4).map(|c| fill.channel(idx, c)).collect();
        assert_eq!(held, if interior { vec![255; 4] } else { vec![0; 4] });
    }
}

// AC 28: with three above-floor candidates and two below-floor ones, the last
// channel goes to the brighter below-floor light, even when the dimmer one is
// promoted.
#[test]
fn the_last_channel_goes_to_the_brighter_below_floor_light_even_over_a_promoted_one() {
    let atlas = Atlas::new(1);
    let above: Vec<MapLight> = (0..3).map(|_| light_over(0, 1.0)).collect();
    let dim_promoted = light_over(0, 2.0);
    let bright_spec_only = light_over(0, 6.0);
    let mut eligible_lights: Vec<_> = above
        .iter()
        .enumerate()
        .map(|(spec, light)| eligible(light, spec as u16, false))
        .collect();
    eligible_lights.push(eligible(&dim_promoted, 3, true));
    eligible_lights.push(eligible(&bright_spec_only, 4, false));
    let arrivals: Vec<_> = eligible_lights
        .iter()
        .map(|light| {
            let texels = if light.spec_index >= 3 {
                atlas.lit(0, 8, 1.0)
            } else {
                atlas.lit(0, 16, 1.0)
            };
            (light, partition(&atlas, texels))
        })
        .collect();
    let fill = run(&atlas, &arrivals);
    assert_eq!(fill.owners(0), [0, 1, 2, 4]);
    assert!(
        fill.report.drops.is_empty(),
        "both extras are below the floor"
    );
}

// AC 30: a fifth candidate below the lit-area floor is dropped without a
// warning; the same light above the floor is warned.
#[test]
fn a_fifth_candidate_is_reported_only_above_the_floor() {
    let atlas = Atlas::new(1);
    let lights: Vec<MapLight> = (0..4).map(|_| light_over(0, 5.0)).collect();
    let fifth = light_over(0, 1.0);
    let owners: Vec<_> = lights
        .iter()
        .enumerate()
        .map(|(spec, light)| eligible(light, spec as u16, false))
        .collect();
    let fifth_e = eligible(&fifth, 4, false);
    for lit in [LIT_TEXEL_FLOOR - 1, LIT_TEXEL_FLOOR] {
        let mut arrivals: Vec<_> = owners
            .iter()
            .map(|light| (light, whole_face(&atlas, 0)))
            .collect();
        arrivals.push((&fifth_e, partition(&atlas, atlas.lit(0, lit, 1.0))));
        let fill = run(&atlas, &arrivals);
        assert_eq!(fill.owners(0), [0, 1, 2, 3]);
        if lit < LIT_TEXEL_FLOOR {
            assert!(fill.report.drops.is_empty());
        } else {
            assert_eq!(fill.report.drops, vec![drop_at(0, 4, lit)]);
        }
    }
}

// AC 31 (P7): two candidates with equal contribution for the last channel
// resolve to the lower light index, whichever arrives first.
#[test]
fn a_tie_for_the_last_channel_goes_to_the_lower_index_whatever_arrives_first() {
    let atlas = Atlas::new(1);
    let lights: Vec<MapLight> = (0..5)
        .map(|spec| light_over(0, if spec < 3 { 9.0 } else { 2.0 }))
        .collect();
    let eligible: Vec<_> = lights
        .iter()
        .enumerate()
        .map(|(spec, light)| eligible(light, spec as u16, false))
        .collect();
    for order in [[0, 1, 2, 3, 4], [0, 1, 2, 4, 3], [4, 3, 2, 1, 0]] {
        let arrivals: Vec<_> = order
            .iter()
            .map(|&spec| (&eligible[spec], whole_face(&atlas, 0)))
            .collect();
        let fill = run(&atlas, &arrivals);
        assert_eq!(fill.owners(0), [0, 1, 2, 3], "order {order:?}");
        assert_eq!(fill.report.drops, vec![drop_at(0, 4, 16)]);
    }
}

// AC 32 (P6): id 42 is byte-identical when the fill receives the same
// partitions in reversed light order, with lights spanning several faces.
#[test]
fn reversed_light_order_yields_identical_owners_and_bytes() {
    let atlas = Atlas::new(3);
    let lights: Vec<MapLight> = (0..6)
        .map(|spec| light_over(spec % 3, 1.0 + spec as f32))
        .collect();
    let eligible: Vec<_> = lights
        .iter()
        .enumerate()
        .map(|(spec, light)| eligible(light, spec as u16, spec == 2))
        .collect();
    // Every light reaches every face, at a visibility of its own.
    let arrival = |light: &EligibleLight<'_>| {
        let visibility = 0.3 + 0.1 * f32::from(light.spec_index);
        let texels = (0..3)
            .flat_map(|face| atlas.lit(face, 16, visibility))
            .collect();
        partition(&atlas, texels)
    };
    let forward: Vec<_> = eligible.iter().map(|l| (l, arrival(l))).collect();
    let reversed: Vec<_> = eligible.iter().rev().map(|l| (l, arrival(l))).collect();
    let a = run(&atlas, &forward);
    let b = run(&atlas, &reversed);
    assert_eq!(a.section, b.section);
    assert_eq!(a.report, b.report);
    assert_eq!(a.raw, b.raw);
    assert!(a.report.drops.len() >= 3, "every face overflows");
}

// AC 35 (P13): a texel with non-finite visibility neither admits a light nor
// adds to its contribution.
#[test]
fn non_finite_visibility_neither_admits_nor_counts() {
    let atlas = Atlas::new(1);
    let light = light_over(0, 2.0);
    let candidate = eligible(&light, 0, false);
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let fill = run(
            &atlas,
            &[(&candidate, partition(&atlas, atlas.lit(0, 16, bad)))],
        );
        assert!(fill.section.is_none(), "{bad} must not admit");
    }
    // Fifteen finite lit texels and one NaN stay below the floor of 16, so the
    // NaN never counts toward it.
    let mut texels = atlas.lit(0, 15, 1.0);
    texels.push((atlas.idx(0, 3, 3), f32::NAN));
    let others: Vec<MapLight> = (0..4).map(|_| light_over(0, 0.5)).collect();
    let other_e: Vec<_> = others
        .iter()
        .enumerate()
        .map(|(i, l)| eligible(l, i as u16 + 1, false))
        .collect();
    let mut arrivals: Vec<_> = other_e
        .iter()
        .map(|light| (light, whole_face(&atlas, 0)))
        .collect();
    arrivals.push((&candidate, partition(&atlas, texels)));
    let fill = run(&atlas, &arrivals);
    assert_eq!(
        fill.owners(0),
        [1, 2, 3, 4],
        "15 lit texels rank below the floor"
    );
    assert!(fill.report.drops.is_empty());
}

// AC 36: a light whose only lit texels on a face are padding takes no channel.
#[test]
fn a_light_lit_only_in_padding_takes_no_channel() {
    let atlas = Atlas::new(1);
    let light = light_over(0, 2.0);
    let candidate = eligible(&light, 0, false);
    let interior: Vec<u32> = (0..16).map(|n| atlas.idx(0, n % 4, n / 4)).collect();
    let padding: Vec<(u32, f32)> = atlas
        .padded(0)
        .into_iter()
        .filter(|idx| !interior.contains(idx))
        .map(|idx| (idx, 1.0))
        .collect();
    let fill = run(&atlas, &[(&candidate, partition(&atlas, padding))]);
    assert!(fill.section.is_none());
}

// AC 37 and AC 47: a light occluded on every texel of a face takes no channel
// there, and a level whose lights light no face interior emits no id 42; one
// lit texel takes a channel.
#[test]
fn an_occluded_light_takes_no_channel_and_one_lit_texel_takes_one() {
    let atlas = Atlas::new(2);
    let light = light_over(0, 2.0);
    let candidate = eligible(&light, 0, false);
    let dark = run(
        &atlas,
        &[(&candidate, partition(&atlas, atlas.lit(0, 16, 0.0)))],
    );
    assert!(dark.section.is_none(), "nothing owned means no id 42");
    // A visibility that quantizes to zero is occluded too.
    let dim = run(
        &atlas,
        &[(&candidate, partition(&atlas, atlas.lit(0, 16, 0.001)))],
    );
    assert!(dim.section.is_none());

    let mut texels = atlas.lit(0, 16, 0.0);
    texels[5].1 = 1.0;
    let one = run(&atlas, &[(&candidate, partition(&atlas, texels))]);
    assert_eq!(one.owners(0), [0, E, E, E]);
    assert_eq!(one.owners(1), NO_FACE_OWNERS);
    assert_eq!(one.section.as_ref().unwrap().owners.len(), 2);
}

// AC 1 (compiler half) and AC 2 (compiler half): a light owns only the faces it
// lights; the face it cannot see keeps its own owner.
#[test]
fn a_light_owns_only_the_faces_it_lights() {
    let atlas = Atlas::new(2);
    let a = light_over(0, 2.0);
    let b = light_over(1, 2.0);
    let a_e = eligible(&a, 0, false);
    let b_e = eligible(&b, 1, false);
    // A reaches face 1 analytically but is walled off there: visibility zero.
    let a_texels = [atlas.lit(0, 16, 1.0), atlas.lit(1, 16, 0.0)].concat();
    let fill = run(
        &atlas,
        &[
            (&a_e, partition(&atlas, a_texels)),
            (&b_e, whole_face(&atlas, 1)),
        ],
    );
    assert_eq!(fill.owners(0), [0, E, E, E]);
    assert_eq!(fill.owners(1), [1, E, E, E]);
}

// AC 34 (P8): dynamic, SDF and bake-only lights placed before a static light do
// not shift its owner index off the light that fills its channel; a bake-only
// light takes no channel, and id 40 marks only the promotion tier.
#[test]
fn eligibility_skips_dynamic_sdf_bake_only_and_animated_lights() {
    let base = light_over(0, 1.0);
    let mut dynamic = base.clone();
    dynamic.is_dynamic = true;
    let mut sdf = base.clone();
    sdf.shadow_type = ShadowType::Sdf;
    let mut bake_only = base.clone();
    bake_only.bake_only = true;
    let mut animated = base.clone();
    animated.animation = Some(crate::map_data::animated_light_placeholder(true));
    let mut sun = base.clone();
    sun.light_type = LightType::Directional;
    sun.cone_direction = Some([0.0, -1.0, 0.0]);
    let lights = vec![dynamic, sdf, bake_only, animated, base, sun];
    let static_lights = StaticBakedLights::from_lights(&lights);
    let alpha_lights = AlphaLightsNs::from_lights(&lights);
    // AlphaLights: dynamic, sdf, animated, base, sun (bake-only has no record).
    // Spec order (!is_dynamic): sdf 0, animated 1, base 2, sun 3.
    let selection = EntityShadowLightsSection {
        light_indices: vec![3],
    };
    let eligible = owner_eligible_lights(&static_lights, &alpha_lights, Some(&selection));
    let named: Vec<(usize, u16, bool)> = eligible
        .iter()
        .map(|l| (l.source_index, l.spec_index, l.promoted))
        .collect();
    assert_eq!(named, vec![(4, 2, true), (5, 3, false)]);
    assert_eq!(static_spec_light_count(&alpha_lights), 4);
}

// AC 48 (P11): more static spec lights than a u16 owner can name fails with a
// named error.
#[test]
fn the_owner_index_cap_is_the_u16_range_less_the_empty_sentinel() {
    assert!(check_owner_index_capacity(usize::from(u16::MAX)).is_ok());
    let error = check_owner_index_capacity(usize::from(u16::MAX) + 1).unwrap_err();
    assert_eq!(
        error,
        ShadowmaskBakeError::TooManyStaticSpecLights {
            count: 65_536,
            max: 65_535
        }
    );
    assert!(error.to_string().contains("65536 static spec lights"));
}

// AC 23 (P2): the memo entry carries the drop report, so a warm hit repeats it.
#[test]
fn the_memo_entry_round_trips_the_drop_report_and_an_absent_section() {
    let atlas = Atlas::new(1);
    let shared = atlas.shared();
    let dir = std::env::temp_dir().join(format!(
        "postretro_shadowmask_owner_memo_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let cache = StageCache::new(&dir).expect("memo test cache");
    let key = CacheKey::new(
        SHADOWMASK_ATLAS_STAGE_ID,
        SHADOWMASK_ATLAS_STAGE_VERSION,
        &[7; 32],
    );
    let bounds = ShadowmaskOwnerBounds {
        face_count: 1,
        static_spec_light_count: 8,
    };
    let report = OwnerReport {
        peak_face_demand: 9,
        drops: vec![
            drop_at(0, 4, 20),
            OwnerDrop {
                site: DropSite::CutFace {
                    parent: 3,
                    first_face: 5,
                    last_face: 7,
                },
                spec_index: 2,
                lit_texels: 400,
            },
        ],
    };
    memo::cache_shadowmask_section(&cache, &key, None, &report);
    let memo = memo::read_shadowmask_memo(&cache, &key, &shared, bounds).expect("memo hit");
    assert!(memo.section.is_none());
    assert_eq!(memo.report, report);

    let light = light_over(0, 1.0);
    let owner = eligible(&light, 0, false);
    let fill = run(&atlas, &[(&owner, whole_face(&atlas, 0))]);
    memo::cache_shadowmask_section(&cache, &key, fill.section.as_ref(), &fill.report);
    let memo = memo::read_shadowmask_memo(&cache, &key, &shared, bounds).expect("memo hit");
    assert_eq!(memo.section, fill.section);
    assert_eq!(memo.report, fill.report);
    drop(cache);
    let _ = std::fs::remove_dir_all(&dir);
}

// AC 43-adjacent: every input that can change id 42 changes the memo key.
#[test]
fn the_memo_key_covers_spec_indices_promotion_faces_and_estimate_parameters() {
    let light = light_over(0, 1.0);
    let hashes = [[1u8; 32], [2u8; 32]];
    let key = |eligible: &[EligibleLight<'_>], faces: usize, estimate: &[u8]| {
        shadowmask_atlas_input_hash(eligible, &hashes, faces, 8, 8, 2, estimate)
    };
    let base = [eligible(&light, 0, false)];
    let reference = key(&base, 4, &[]);
    assert_ne!(reference, key(&[eligible(&light, 1, false)], 4, &[]));
    assert_ne!(reference, key(&[eligible(&light, 0, true)], 4, &[]));
    assert_ne!(reference, key(&base, 5, &[]));
    assert_ne!(reference, key(&base, 4, &[4]));
    assert_eq!(reference, key(&base, 4, &[]));
}

// The raw fill lives only through the walk: one allocation, released before
// the memo write and before the section returns; encode scratch stays within
// three raw layers. A memo hit allocates nothing.
#[test]
fn the_raw_fill_is_released_before_the_memo_write_and_encode_scratch_stays_bounded() {
    let atlas = Atlas::new(2);
    let shared = atlas.shared();
    let light = light_over(0, 2.0);
    let dir = std::env::temp_dir().join(format!(
        "postretro_shadowmask_owner_residency_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let cache = StageCache::new(&dir).expect("residency test cache");
    let control = crate::bake_control::BakeControl::unrestricted();
    let raw_layer = atlas.width as usize * CHART as usize * 4;

    reset_shadowmask_output_allocation_count();
    reset_shadowmask_streamed_cache_write_count();
    let _ = encode::take_peak_encode_residency();
    let _ = take_raw_fill_live_bytes_at_cache_write();
    let mut plan = prepare_fused_shadowmask(
        vec![eligible(&light, 0, false)],
        &[[3u8; 32]],
        1,
        1,
        &shared,
        Some(&cache),
        &control,
    )
    .expect("prepare");
    assert!(plan.needs_source(0));
    plan.consume_partition(0, &whole_face(&atlas, 0), &shared);
    let miss = plan.finish(&shared);
    assert!(miss.section.is_some());
    assert_eq!(shadowmask_output_allocation_count(), 1);
    assert_eq!(shadowmask_streamed_cache_write_count(), 1);
    assert_eq!(
        take_raw_fill_live_bytes_at_cache_write(),
        Some(0),
        "the raw fill must be gone before the section is cached"
    );
    assert_eq!(raw_fill_live_bytes(), 0, "and before it is returned");
    let peak = encode::take_peak_encode_residency().expect("the miss encodes");
    assert_eq!(peak.raw_fill, raw_layer);
    assert_eq!(peak.output_capacity, raw_layer / 2, "one compressed output");
    assert!(peak.scratch <= 3 * raw_layer, "scratch {}", peak.scratch);

    reset_shadowmask_output_allocation_count();
    let plan = prepare_fused_shadowmask(
        vec![eligible(&light, 0, false)],
        &[[3u8; 32]],
        1,
        1,
        &shared,
        Some(&cache),
        &control,
    )
    .expect("prepare");
    assert!(!plan.needs_source(0), "a memo hit needs no partition");
    let hit = plan.finish(&shared);
    assert_eq!(hit.section, miss.section);
    assert_eq!(hit.report, miss.report);
    assert_eq!(shadowmask_output_allocation_count(), 0);
    drop(cache);
    let _ = std::fs::remove_dir_all(&dir);
}

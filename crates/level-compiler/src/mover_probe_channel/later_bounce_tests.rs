// A pose's later bounces against id 34's own, the mover-absent bake: a
// terminus re-traces them over every static patch; a mid pose over the
// static patches of its near-field domain and its posed sides, where a
// gather ray leaving the domain reads the mover-absent bounce; both keep id
// 34's bounces past their `light_bounces` (brief AC 8).

use std::sync::Arc;

use glam::{DVec3, Vec3};

use super::absent_bounces::AbsentLaterBounces;
use super::bounce::{LaterReach, PosedLaterBounces, PosedRadiance, add_posed_later_bounces};
use super::posed_scene::PosedScene;
use super::posed_solid::box_brush;
use super::tests::solid;
use crate::bake_control::BakeControl;
use crate::fixture_pipeline::FixturePipeline;
use crate::mover_probe_channel::PosedMovers;
use crate::sh_bake::RaytracingCtx;
use crate::surface_radiance::{
    LATER_BOUNCE_COUNT, PatchRadiance, PatchSet, Side, add_later_bounces_keeping,
};

const FILL: f32 = 0.7;

/// The SH base's bounce count at [`FILL`]: bounce one and every later one.
const ALL_BOUNCES: u32 = 1 + LATER_BOUNCE_COUNT;

/// Every side lit differently, so the later bounces carry values whose
/// float sum depends on its order.
fn varied(count: usize, sides: &[Side], scale: f32) -> PatchRadiance {
    let mut radiance = PatchRadiance::zeroed(count);
    for patch in 0..count as u32 {
        for &side in sides {
            let seed = patch as f32 * 0.618_034 + side as usize as f32 * 0.414_213;
            radiance.add_side(
                patch,
                side,
                Vec3::new(seed.fract(), (seed * 1.7).fract(), (seed * 2.3).fract()) * scale,
            );
        }
    }
    radiance
}

/// The closet's static patches lit by a synthetic bounce one, and id 34's
/// later bounces from it: every one, kept apart, and their sum at the fill.
struct Closet {
    fixture: FixturePipeline,
    patches: PatchSet,
    bounce_one: PatchRadiance,
    absent: AbsentLaterBounces,
    id34: PatchRadiance,
}

impl Closet {
    fn new() -> Self {
        let fixture = crate::fixture_pipeline::load_fixture("mover-closet");
        let patches = PatchSet::try_build(&fixture.geometry).expect("static patches");
        let bounce_one = varied(patches.len(), &Side::BOTH, 3.0);
        let mut id34 = bounce_one.clone();
        let layers = {
            let ctx = ctx(&fixture);
            add_later_bounces_keeping(
                &ctx,
                &patches,
                &mut id34,
                FILL,
                Some(0),
                &BakeControl::unrestricted(),
            )
        };
        assert_eq!(
            layers.len() as u32,
            LATER_BOUNCE_COUNT,
            "every later bounce"
        );
        Self {
            fixture,
            patches,
            bounce_one,
            absent: AbsentLaterBounces { layers },
            id34,
        }
    }

    /// The closet door at its first pose, its sides lit front only, as
    /// posed bounce one lights them.
    fn door(&self) -> (PosedScene, PatchRadiance) {
        let poses = self.fixture.placed_mover_poses(1.0);
        let posed = PosedMovers::resolve(
            &poses,
            &self.fixture.kinematic_movers,
            &self.fixture.geometry.texture_names,
        )
        .expect("the door poses");
        let solid = Arc::clone(&posed.movers()[0].solids[0]);
        let scene = PosedScene::new(solid, 0).expect("posed patches");
        let lit = varied(scene.patches.len(), &[Side::Front], 5.0);
        (scene, lit)
    }

    /// The static patches whose centre lies within `radius` metres of the
    /// door's patches' mean centre, ascending: a near-field domain.
    fn around(&self, scene: &PosedScene, radius: f32) -> Vec<u32> {
        let centres = scene.patches.patches().iter().map(|patch| patch.centroid);
        let centre = centres.clone().sum::<Vec3>() / scene.patches.len() as f32;
        (0..self.patches.len() as u32)
            .filter(|&patch| {
                self.patches.patches()[patch as usize]
                    .centroid
                    .distance(centre)
                    <= radius
            })
            .collect()
    }

    /// `bounces` posed bounces (bounce one given) over `reach`, with id
    /// 34's own past them.
    fn posed(
        &self,
        scene: &PosedScene,
        mover: &PatchRadiance,
        bounces: u32,
        reach: LaterReach<'_>,
    ) -> PosedRadiance {
        let mut radiance = PosedRadiance {
            world: self.bounce_one.clone(),
            mover: mover.clone(),
        };
        add_posed_later_bounces(
            &ctx(&self.fixture),
            &self.patches,
            scene,
            &mut radiance,
            FILL,
            PosedLaterBounces {
                posed: bounces - 1,
                absent: &self.absent,
                reach,
            },
            &BakeControl::unrestricted(),
        );
        radiance
    }
}

fn ctx(fixture: &FixturePipeline) -> RaytracingCtx<'_> {
    RaytracingCtx {
        bounce_materials: None,
        bvh: &fixture.bvh,
        primitives: &fixture.primitives,
        geometry: &fixture.geometry,
    }
}

/// Whether static patch `patch` reads the same, both sides, bit for bit.
fn same(a: &PatchRadiance, b: &PatchRadiance, patch: u32) -> bool {
    Side::BOTH
        .iter()
        .all(|&side| a.get(patch, side).to_array() == b.get(patch, side).to_array())
}

// Id 34's kept later bounces join a pose's running sum one by one, in bounce
// order, as id 34 adds them. With every `light_bounces` that keeps a tail,
// 1 (every later bounce is id 34's: the tail alone) through 4 (only id 34's
// last layer), a patch the pose never meets (here every static patch: the
// pose sits far outside the closet) sums id 34's later bounces bit for bit.
// Pre-summing the kept layers reassociates the float sum and misses.
#[test]
fn a_patch_the_pose_never_meets_sums_id_34s_later_bounces_bit_for_bit() {
    let closet = Closet::new();
    let far = solid(vec![box_brush(DVec3::splat(500.0), DVec3::splat(501.0))]);
    let scene = PosedScene::new(Arc::new(far), 0).expect("posed patches");
    let unlit = PatchRadiance::zeroed(scene.patches.len());
    for bounces in 1..ALL_BOUNCES {
        let posed = closet.posed(&scene, &unlit, bounces, LaterReach::Everywhere);
        assert!(
            posed.world == closet.id34,
            "light_bounces {bounces}: an untouched patch sums id 34's later bounces bit for bit"
        );
    }
}

// A near-field domain holding every static patch is the terminus's trace,
// bit for bit, at every `light_bounces`: the mid pose's later bounces are
// the terminus's rule, cut to the domain.
#[test]
fn a_domain_holding_every_patch_traces_the_terminus_later_bounces_bit_for_bit() {
    let closet = Closet::new();
    let (scene, lit) = closet.door();
    let every: Vec<u32> = (0..closet.patches.len() as u32).collect();
    for bounces in 1..=ALL_BOUNCES {
        let terminus = closet.posed(&scene, &lit, bounces, LaterReach::Everywhere);
        let near = closet.posed(&scene, &lit, bounces, LaterReach::NearField(&every));
        assert!(
            near.world == terminus.world && near.mover == terminus.mover,
            "light_bounces {bounces}: the whole map as the domain is the terminus's trace"
        );
    }
}

// At a mid pose, `light_bounces 1` is bounce one posed and id 34's later
// bounces: every static patch sums id 34's later bounces bit for bit, in or
// out of the domain, and the posed sides keep bounce one alone.
#[test]
fn light_bounces_one_at_a_mid_pose_is_bounce_one_posed_plus_id_34s_later_bounces() {
    let closet = Closet::new();
    let (scene, lit) = closet.door();
    let domain = closet.around(&scene, 1.5);
    assert!(!domain.is_empty() && domain.len() < closet.patches.len());
    let near = closet.posed(&scene, &lit, 1, LaterReach::NearField(&domain));
    assert!(
        near.world == closet.id34,
        "every static patch: bounce one plus id 34's later bounces"
    );
    assert!(near.mover == lit, "the posed sides keep bounce one alone");
}

// A mid pose's later bounces over part of the map. Past the domain, a patch
// takes the mover-absent bounce at every step, so it sums id 34's later
// bounces bit for bit. With one posed later bounce, a domain patch gathers
// it from the posed bounce one, as the terminus does, and keeps id 34's
// past it, so it reads the terminus's value bit for bit. With every later
// bounce posed, a domain patch's rays that leave the domain read the absent
// bounces, so near the domain's edge it reads neither the terminus's
// value nor id 34's; the posed sides gather posed later bounces.
#[test]
fn past_the_domain_a_patch_reads_id_34_and_inside_it_traces_posed() {
    let closet = Closet::new();
    let (scene, lit) = closet.door();
    let domain = closet.around(&scene, 1.5);
    let inside = |patch: u32| domain.binary_search(&patch).is_ok();
    let all = closet.patches.len() as u32;

    let two = closet.posed(&scene, &lit, 2, LaterReach::NearField(&domain));
    let terminus_two = closet.posed(&scene, &lit, 2, LaterReach::Everywhere);
    for patch in 0..all {
        let expected = if inside(patch) {
            &terminus_two.world
        } else {
            &closet.id34
        };
        assert!(
            same(&two.world, expected, patch),
            "light_bounces 2, patch {patch} ({} the domain)",
            if inside(patch) { "inside" } else { "outside" }
        );
    }
    assert!(
        two.mover == terminus_two.mover,
        "the posed sides' bounce two"
    );

    let full = closet.posed(&scene, &lit, ALL_BOUNCES, LaterReach::NearField(&domain));
    let terminus = closet.posed(&scene, &lit, ALL_BOUNCES, LaterReach::Everywhere);
    let (mut from_id34, mut from_terminus) = (0, 0);
    for patch in 0..all {
        if !inside(patch) {
            assert!(
                same(&full.world, &closet.id34, patch),
                "patch {patch} past the domain sums id 34's later bounces"
            );
            continue;
        }
        from_id34 += usize::from(!same(&full.world, &closet.id34, patch));
        from_terminus += usize::from(!same(&full.world, &terminus.world, patch));
    }
    eprintln!(
        "{} domain patch(es) of {all}: {from_id34} differ from id 34, {from_terminus} from the \
         terminus's full trace",
        domain.len()
    );
    assert!(from_id34 > 0, "the domain's patches trace posed");
    assert!(
        from_terminus > 0,
        "rays leaving the domain read the absent bounces"
    );
    assert!(
        full.mover != lit,
        "the posed sides gather posed later bounces"
    );
}

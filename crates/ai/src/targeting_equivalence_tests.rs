// Live target selection against the every-pawn scan the pawn index replaced, kept as a test-only
// reference (`targeting::reference`): the same stride-pricing nearest, selected
// target, fresh LOS verdict and retaliation latch, across randomized scenes
// and the named edge classes.
// See: context/lib/entity_model.md §7c

use super::*;
use crate::targeting::{self, TargetCandidate, TargetOffers, TargetPawn, TargetSelection};
use postretro_entities::components::brain::{RECENT_ATTACKER_LEDGER_CAPACITY, RecentAttacker};
use postretro_entities::components::inventory::Inventory;
use postretro_foundation::{CANDIDATE_DISTANCE_INPUT, CANDIDATE_HEALTH_INPUT};

type Ledger = [Option<RecentAttacker>; RECENT_ATTACKER_LEDGER_CAPACITY];

/// Authored factions take indices 2, 3 and 4.
const ALPHA: f32 = 2.0;
const BETA: f32 = 3.0;
const GAMMA: f32 = 4.0;
/// Pawn faction values, index or not, signed zero both ways.
const FACTION_VALUES: [f32; 9] = [0.0, -0.0, 1.0, ALPHA, BETA, GAMMA, 0.5, f32::NAN, 7.0];

/// Deterministic LCG in [0, 1).
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 40) as f32) / ((1u64 << 24) as f32)
    }

    fn below(&mut self, n: usize) -> usize {
        ((self.next() * n as f32) as usize).min(n - 1)
    }

    fn chance(&mut self, p: f32) -> bool {
        self.next() < p
    }

    fn pick<T: Copy>(&mut self, items: &[T]) -> T {
        items[self.below(items.len())]
    }
}

/// Asymmetric sentiment: alpha is hostile to beta, beta allied to alpha;
/// gamma and alpha one-way hostile; beta and gamma mutually hostile.
fn equivalence_factions() -> FactionRegistry {
    let pair = |from: &str, to: &str, sentiment: f32, tolerance: f32| FactionSentimentDescriptor {
        from_faction: from.to_string(),
        to_faction: to.to_string(),
        sentiment,
        tolerance,
        decay: None,
    };
    FactionRegistry::from_descriptors(
        ["eq.alpha", "eq.beta", "eq.gamma"]
            .into_iter()
            .map(|name| FactionDescriptor {
                name: name.to_string(),
            })
            .collect(),
    )
    .expect("factions are valid")
    .with_sentiments(vec![
        pair("eq.alpha", "eq.beta", -1.0, 4.0),
        pair("eq.beta", "eq.alpha", 0.5, 2.0),
        pair("eq.alpha", "eq.gamma", 0.0, 6.0),
        pair("eq.gamma", "eq.alpha", -0.5, f32::MAX),
        pair("eq.beta", "eq.gamma", -1.0, 1.0),
        pair("eq.gamma", "eq.beta", -1.0, 3.0),
    ])
    .expect("relationships resolve")
}

/// Everything one enemy's selection reads.
struct Scene {
    registry: EntityRegistry,
    factions: FactionRegistry,
    overlay: FactionSentimentState,
    enemy: EntityId,
    retained: Option<EntityId>,
    ledger: Ledger,
    latch: Option<EntityId>,
    retaliation: RetaliationDescriptor,
    filter: Option<BoundProgram<CandidateScope>>,
    /// Pawns whose line of sight fails.
    blind: Vec<EntityId>,
}

/// What a selection produces, formatted so NaN positions compare by value
/// and every field is checked: the stride's nearest, its price, and the
/// selection with its fresh LOS and latch.
fn outcome(
    nearest: Option<TargetCandidate>,
    stride_distance: Option<f32>,
    selection: Option<TargetSelection>,
) -> String {
    format!(
        "nearest {nearest:?} stride {:?} selection {selection:?}",
        stride_distance.map(f32::to_bits)
    )
}

/// The compute pass's targeting flow for one enemy, with offers and
/// selection supplied: retained lookup, offers excluding the retained target,
/// the stride price, then due-tick selection.
fn drive(
    scene: &Scene,
    offers: impl FnOnce(&LiveFactionSentiment<'_>, Vec3, f32, Option<EntityId>) -> TargetOffers,
    select: impl FnOnce(
        Option<TargetCandidate>,
        &TargetOffers,
        &LiveFactionSentiment<'_>,
        f32,
        &mut dyn FnMut(TargetPawn) -> Option<perception::RawTargetPerception>,
    ) -> Option<TargetSelection>,
) -> String {
    let registry = &scene.registry;
    let live = LiveFactionSentiment::new(&scene.factions, &scene.overlay);
    let from = registry
        .get_component::<Transform>(scene.enemy)
        .unwrap()
        .position;
    let enemy_faction = crate::compute::entity_faction(registry, scene.enemy);
    let retained = scene
        .retained
        .and_then(|entity| targeting::target_candidate(registry, entity, from));
    let offers = offers(
        &live,
        from,
        enemy_faction,
        retained.map(|r| r.target.entity),
    );
    let stride_distance = retained
        .map(|retained| retained.distance)
        .or(offers.nearest.map(|nearest| nearest.distance));
    let mut perception = |target: TargetPawn| {
        Some(perception::RawTargetPerception {
            target: target.entity,
            visible: !scene.blind.contains(&target.entity),
            enemy_eye: from,
            target_aim: target.position,
        })
    };
    let selection = select(retained, &offers, &live, enemy_faction, &mut perception);
    outcome(offers.nearest, stride_distance, selection)
}

fn reference_outcome(scene: &Scene) -> String {
    let registry = &scene.registry;
    drive(
        scene,
        |live, from, faction, exclude| {
            targeting::reference::target_offers(
                registry,
                live,
                from,
                faction,
                Some(scene.enemy),
                exclude,
            )
        },
        |retained, offers, live, faction, perception| {
            targeting::reference::select_target_with_attacker_ledger(
                retained,
                offers,
                registry,
                live,
                Some(scene.enemy),
                faction,
                scene.filter.as_ref(),
                &mut CandidateScope::for_validation(),
                &scene.ledger,
                scene.latch,
                scene.retaliation,
                TICK_MS,
                perception,
            )
        },
    )
}

/// The live selection, through the seams `compute` calls, over an index built
/// as the pass builds it.
fn live_outcome(scene: &Scene) -> String {
    let registry = &scene.registry;
    let index = crate::pawn_index::PawnIndex::for_test(registry);
    drive(
        scene,
        |live, from, faction, exclude| {
            targeting::target_offers(&index, live, from, faction, Some(scene.enemy), exclude)
        },
        |retained, offers, live, faction, perception| {
            targeting::select_target_with_attacker_ledger(
                retained,
                offers,
                &index,
                registry,
                live,
                Some(scene.enemy),
                faction,
                scene.filter.as_ref(),
                &mut CandidateScope::for_validation(),
                &scene.ledger,
                scene.latch,
                scene.retaliation,
                TICK_MS,
                perception,
            )
        },
    )
}

const TICK_MS: f32 = 1_000.0 / 60.0;

fn assert_live_matches_reference(scene: &Scene, label: &str) {
    assert_eq!(live_outcome(scene), reference_outcome(scene), "{label}");
}

fn set_faction(registry: &mut EntityRegistry, entity: EntityId, faction: f32) {
    registry
        .entity_state_mut(entity)
        .expect("live entity")
        .set(FACTION_STATE_FIELD, faction);
}

fn peer_brain(registry: &mut EntityRegistry, position: Vec3, health: f32) -> EntityId {
    spawn_enemy(
        registry,
        position,
        brain_with(test_graph_with(18.0, 18.0), TEST_IDLE_STATE),
        health,
    )
}

/// A lattice point: XZ on a 2 m grid so equal distances are common; Y varies,
/// which XZ distance ignores.
fn lattice(rng: &mut Lcg) -> Vec3 {
    Vec3::new(
        (rng.below(9) as f32 - 4.0) * 2.0,
        rng.pick(&[0.0, 0.5, 3.0]),
        (rng.below(9) as f32 - 4.0) * 2.0,
    )
}

fn random_scene(rng: &mut Lcg) -> Scene {
    let mut registry = EntityRegistry::new();
    let mut pawns = Vec::new();
    let mut others = Vec::new();

    // Interleave spawns so slot order and group order disagree.
    let enemy = peer_brain(&mut registry, lattice(rng), 50.0);
    set_faction(
        &mut registry,
        enemy,
        rng.pick(&[1.0, ALPHA, BETA, GAMMA, -0.0, 0.5]),
    );
    if rng.chance(0.4) {
        let tolerance = rng.pick(&[4.0, -1.0, 0.0, 100.0]);
        registry
            .entity_state_mut(enemy)
            .unwrap()
            .set(ARCHETYPE_TOLERANCE_STATE_FIELD, tolerance);
    }
    let players = 1 + rng.below(3);
    let peers = rng.below(14);
    let mut kinds: Vec<bool> = (0..players + peers).map(|i| i < players).collect();
    for i in (1..kinds.len()).rev() {
        kinds.swap(i, rng.below(i + 1));
    }
    for is_player in kinds {
        let position = lattice(rng);
        let entity = if is_player {
            spawn_player(&mut registry, position)
        } else {
            peer_brain(&mut registry, position, rng.pick(&[50.0, 20.0, 0.0]))
        };
        set_faction(&mut registry, entity, rng.pick(&FACTION_VALUES));
        pawns.push(entity);
    }
    // A prop between pawns: never a candidate, but it shifts slots.
    others.push(registry.spawn(Transform {
        position: lattice(rng),
        ..Transform::default()
    }));

    // Edge classes, each with some probability.
    if rng.chance(0.3) {
        // Holds player movement and a brain: walked once, as a movement holder.
        let both = spawn_player(&mut registry, lattice(rng));
        registry
            .set_component(
                both,
                brain_with(test_graph_with(18.0, 18.0), TEST_IDLE_STATE),
            )
            .unwrap();
        set_faction(&mut registry, both, rng.pick(&FACTION_VALUES));
        pawns.push(both);
    }
    if rng.chance(0.3) {
        // Inventory-owned: never a candidate.
        let held = peer_brain(&mut registry, lattice(rng), 50.0);
        let holder = pawns[rng.below(pawns.len())];
        let mut inventory = Inventory::default();
        inventory.wieldables[9] = Some(held);
        registry.set_component(holder, inventory).unwrap();
        others.push(held);
    }
    if rng.chance(0.3) {
        // No transform: never a candidate.
        let floating = peer_brain(&mut registry, lattice(rng), 50.0);
        registry.remove_component::<Transform>(floating).unwrap();
        others.push(floating);
    }
    if rng.chance(0.3) {
        // No faction state: reads as faction 0.
        let stateless = peer_brain(&mut registry, lattice(rng), 50.0);
        registry
            .remove_component::<EntityStateComponent>(stateless)
            .unwrap();
        pawns.push(stateless);
    }
    if rng.chance(0.3) {
        // A non-finite position.
        let lost = peer_brain(&mut registry, lattice(rng), 50.0);
        set_faction(&mut registry, lost, rng.pick(&FACTION_VALUES));
        let mut transform = *registry.get_component::<Transform>(lost).unwrap();
        transform.position = rng.pick(&[
            Vec3::new(f32::NAN, 0.0, 1.0),
            Vec3::new(f32::INFINITY, 0.0, 0.0),
            Vec3::new(1.0, 0.0, f32::NEG_INFINITY),
        ]);
        registry.set_component(lost, transform).unwrap();
        pawns.push(lost);
    }
    if rng.chance(0.3) {
        // Despawn queued: quiescent, still a candidate.
        let leaving = pawns[rng.below(pawns.len())];
        registry.mark_for_end_of_frame_removal(leaving).unwrap();
    }
    let despawned = registry.spawn(Transform::default());
    registry.despawn(despawned).unwrap();

    let mut overlay = FactionSentimentState::default();
    let factions = equivalence_factions();
    for _ in 0..rng.below(3) {
        let from = rng.pick(&[ALPHA, BETA, GAMMA, 1.0]);
        let to = rng.pick(&[ALPHA, BETA, GAMMA, 0.0, 1.0]);
        let value = rng.pick(&[-1.0, 0.0, 0.25]);
        let _ = overlay.set(from, to, value, factions.sentiment(from, to));
    }

    let retained = match rng.below(4) {
        0 | 1 => None,
        2 => Some(pawns[rng.below(pawns.len())]),
        _ => Some(rng.pick(&others)),
    };
    let ledger_pool: Vec<EntityId> = pawns
        .iter()
        .copied()
        .chain(others.iter().copied())
        .chain([despawned, enemy])
        .chain(retained)
        .collect();
    let mut ledger: Ledger = [None; RECENT_ATTACKER_LEDGER_CAPACITY];
    for entry in ledger
        .iter_mut()
        .take(rng.below(RECENT_ATTACKER_LEDGER_CAPACITY + 1))
    {
        *entry = Some(RecentAttacker {
            attacker: rng.pick(&ledger_pool),
            accumulated_damage: rng.pick(&[0.0, 3.0, 5.0, 10.0, 10.0]),
            time_since_damage_ms: rng.pick(&[0.0, 100.0, 100.0, 2_000.0]),
        });
    }
    let latch = match rng.below(3) {
        0 => None,
        1 => retained,
        _ => Some(rng.pick(&ledger_pool)),
    };
    let retaliation = RetaliationDescriptor {
        window_ms: rng.pick(&[1_500.0, 1_500.0, 0.0]),
        damage_weight: 1.0,
        recency_weight: rng.pick(&[0.001, 0.0]),
    };
    let filter = match rng.below(4) {
        0 => Some(bind_candidate_filter_for_test(IrNode::Ge {
            a: Box::new(brain_input(CANDIDATE_HEALTH_INPUT)),
            b: Box::new(IrNode::Const {
                value: IrValue::Number(30.0),
            }),
        })),
        1 => Some(bind_candidate_filter_for_test(IrNode::Ge {
            a: Box::new(brain_input(CANDIDATE_DISTANCE_INPUT)),
            b: Box::new(IrNode::Const {
                value: IrValue::Number(rng.pick(&[2.0, 4.0, 6.0])),
            }),
        })),
        _ => None,
    };
    let blind = pawns.iter().copied().filter(|_| rng.chance(0.3)).collect();
    Scene {
        registry,
        factions,
        overlay,
        enemy,
        retained,
        ledger,
        latch,
        retaliation,
        filter,
        blind,
    }
}

#[test]
fn live_target_selection_matches_the_every_pawn_reference_across_random_scenes() {
    let mut rng = Lcg(0x7a59e7);
    let (mut selected, mut latched, mut fresh, mut empty) = (0, 0, 0, 0);
    for case in 0..4_000 {
        let scene = random_scene(&mut rng);
        let reference = reference_outcome(&scene);
        assert_eq!(live_outcome(&scene), reference, "case {case}");
        selected += usize::from(reference.contains("selection Some"));
        latched += usize::from(reference.contains("retaliation_acquired_target: Some"));
        fresh += usize::from(reference.contains("fresh_perception: Some"));
        empty += usize::from(reference.contains("selection None"));
    }
    // The scenes reach every kind of result.
    let coverage = (selected, latched, fresh, empty);
    assert!(
        selected > 3_000 && latched > 500 && fresh > 2_500 && empty > 50,
        "{coverage:?}"
    );
}

/// A fixed scene: the enemy (alpha) at the origin, one player and a peer.
fn base_scene() -> (Scene, EntityId, EntityId) {
    let mut registry = EntityRegistry::new();
    let enemy = peer_brain(&mut registry, Vec3::ZERO, 50.0);
    set_faction(&mut registry, enemy, ALPHA);
    let player = spawn_player(&mut registry, Vec3::new(6.0, 0.0, 0.0));
    set_faction(&mut registry, player, BETA);
    let peer = peer_brain(&mut registry, Vec3::new(2.0, 0.0, 0.0), 50.0);
    set_faction(&mut registry, peer, ALPHA);
    let scene = Scene {
        registry,
        factions: equivalence_factions(),
        overlay: FactionSentimentState::default(),
        enemy,
        retained: None,
        ledger: [None; RECENT_ATTACKER_LEDGER_CAPACITY],
        latch: None,
        retaliation: RetaliationDescriptor {
            window_ms: 1_500.0,
            damage_weight: 1.0,
            recency_weight: 0.001,
        },
        filter: None,
        blind: Vec::new(),
    };
    (scene, player, peer)
}

/// The reference scan's fresh selection: (target, retaliation latch).
fn reference_choice(scene: &Scene) -> (Option<EntityId>, Option<EntityId>) {
    let registry = &scene.registry;
    let live = LiveFactionSentiment::new(&scene.factions, &scene.overlay);
    let from = registry
        .get_component::<Transform>(scene.enemy)
        .unwrap()
        .position;
    let faction = crate::compute::entity_faction(registry, scene.enemy);
    let offers = targeting::reference::target_offers(
        registry,
        &live,
        from,
        faction,
        Some(scene.enemy),
        None,
    );
    let mut perception = |target: TargetPawn| {
        Some(perception::RawTargetPerception {
            target: target.entity,
            visible: true,
            enemy_eye: from,
            target_aim: target.position,
        })
    };
    let selection = targeting::reference::select_target_with_attacker_ledger(
        None,
        &offers,
        registry,
        &live,
        Some(scene.enemy),
        faction,
        None,
        &mut CandidateScope::for_validation(),
        &scene.ledger,
        None,
        scene.retaliation,
        TICK_MS,
        &mut perception,
    );
    (
        selection.map(|selection| selection.target.entity),
        selection.and_then(|selection| selection.retaliation_acquired_target),
    )
}

/// Give the scene's enemy an archetype tolerance, which outranks the faction
/// pair's (alpha toward alpha declares none: `f32::MAX`).
fn tolerate(scene: &mut Scene, tolerance: f32) {
    scene
        .registry
        .entity_state_mut(scene.enemy)
        .unwrap()
        .set(ARCHETYPE_TOLERANCE_STATE_FIELD, tolerance);
}

fn attack(scene: &mut Scene, slot: usize, attacker: EntityId, damage: f32) {
    scene.ledger[slot] = Some(RecentAttacker {
        attacker,
        accumulated_damage: damage,
        time_since_damage_ms: 100.0,
    });
}

#[test]
fn named_edge_scenes_match_the_reference() {
    // Zero candidates, and zero hostile buckets.
    let (mut scene, player, peer) = base_scene();
    scene.registry.despawn(player).unwrap();
    assert_live_matches_reference(&scene, "only an allied peer");
    scene.registry.despawn(peer).unwrap();
    assert_live_matches_reference(&scene, "no candidate at all");

    // A non-hostile peer admitted only by retaliation over tolerance (4).
    let (mut scene, _, peer) = base_scene();
    tolerate(&mut scene, 4.0);
    attack(&mut scene, 0, peer, 10.0);
    assert_eq!(
        reference_choice(&scene),
        (Some(peer), Some(peer)),
        "the peer ranks"
    );
    assert_live_matches_reference(&scene, "over-tolerance allied attacker");
    // A hostile ledger attacker beyond the nearest hostile.
    let (mut scene, player, _) = base_scene();
    let far = spawn_player(&mut scene.registry, Vec3::new(9.0, 0.0, 0.0));
    set_faction(&mut scene.registry, far, BETA);
    attack(&mut scene, 0, far, 10.0);
    assert_live_matches_reference(&scene, "farther hostile attacker");
    // Retained target with a hysteresis tie, and as the latched attacker.
    scene.retained = Some(player);
    assert_live_matches_reference(&scene, "retained nearest");
    scene.latch = Some(player);
    attack(&mut scene, 1, player, 10.0);
    assert_live_matches_reference(&scene, "retained retaliation target");
    // The evaluating enemy in its own ledger.
    let enemy = scene.enemy;
    attack(&mut scene, 2, enemy, 50.0);
    assert_live_matches_reference(&scene, "self in ledger");

    // Two over-tolerance attackers of equal rank, recorded in reverse of scan
    // order: the earlier in scan order wins.
    let (mut scene, _, _) = base_scene();
    let a = peer_brain(&mut scene.registry, Vec3::new(0.0, 0.0, 4.0), 50.0);
    let b = peer_brain(&mut scene.registry, Vec3::new(0.0, 0.0, -4.0), 50.0);
    set_faction(&mut scene.registry, a, ALPHA);
    set_faction(&mut scene.registry, b, ALPHA);
    tolerate(&mut scene, 4.0);
    attack(&mut scene, 0, b, 10.0);
    attack(&mut scene, 1, a, 10.0);
    assert_eq!(
        reference_choice(&scene),
        (Some(a), Some(a)),
        "equal ranks: the earlier in scan order"
    );
    assert_live_matches_reference(&scene, "equal-rank attackers");

    // A candidacy predicate and failed LOS rejecting the nearest several.
    let (mut scene, _, _) = base_scene();
    let mut ring = Vec::new();
    for (i, x) in [1.0, 2.0, 3.0, 4.0, 5.0].into_iter().enumerate() {
        let hostile = spawn_player(&mut scene.registry, Vec3::new(x, 0.0, 0.0));
        set_faction(&mut scene.registry, hostile, BETA);
        if i < 3 {
            scene.blind.push(hostile);
        }
        ring.push(hostile);
    }
    assert_live_matches_reference(&scene, "LOS rejects the nearest three");
    scene.blind.clear();
    scene.filter = Some(bind_candidate_filter_for_test(IrNode::Ge {
        a: Box::new(brain_input(CANDIDATE_DISTANCE_INPUT)),
        b: Box::new(IrNode::Const {
            value: IrValue::Number(4.0),
        }),
    }));
    assert_live_matches_reference(&scene, "candidacy rejects the nearest three");

    // NaN and 0.5 faction values, and -0.0 beside 0.0.
    for (a, b) in [(f32::NAN, 0.5), (-0.0, 0.0), (0.0, -0.0)] {
        let (mut scene, player, peer) = base_scene();
        set_faction(&mut scene.registry, player, a);
        set_faction(&mut scene.registry, peer, b);
        set_faction(&mut scene.registry, scene.enemy, 1.0);
        assert_live_matches_reference(&scene, &format!("factions {a} and {b}"));
    }
}

// Fixtures for the clip-timed attack proofs: a clip rig (mesh states and the
// model clips carrying their events), jab graphs, and a scene that ticks the AI
// with the clips at 60 Hz.
// See: context/lib/entity_model.md §7c (Clip-timed attacks)

use std::sync::Arc;

use postretro_entities::provenance::{DescriptorProvenance, DescriptorSpawnPath};
use postretro_model::clip_events::ClipEvent;
use postretro_model::skeleton::{AnimationClip, Skeleton};

use super::*;
use crate::scripting_systems::hit_zones::ModelHitZones;

/// One 60 Hz tick, so a moment's tick is its time × 60.
pub(super) const DT: f32 = 1.0 / 60.0;
pub(super) const RIG_MODEL: &str = "clip_rig";
pub(super) const JAB: &str = "jab";
pub(super) const JAB_DAMAGE: f32 = 8.0;
/// The entity class every fixture enemy is materialized from.
pub(super) const CLASS: &str = "brute";

/// One mesh animation state and the clip it plays.
pub(super) struct ClipSpec {
    pub(super) state: &'static str,
    /// The clip's name; `{state}_clip` when `None`.
    pub(super) clip: Option<&'static str>,
    pub(super) duration: f32,
    pub(super) looping: bool,
    pub(super) events: Vec<(&'static str, f32)>,
    /// The state's clip resolved at load. An unresolved state names a clip
    /// the model lacks and holds no clip index.
    pub(super) resolved: bool,
}

pub(super) fn one_shot(
    state: &'static str,
    duration: f32,
    events: &[(&'static str, f32)],
) -> ClipSpec {
    ClipSpec {
        state,
        clip: None,
        duration,
        looping: false,
        events: events.to_vec(),
        resolved: true,
    }
}

pub(super) fn looping(
    state: &'static str,
    duration: f32,
    events: &[(&'static str, f32)],
) -> ClipSpec {
    ClipSpec {
        looping: true,
        ..one_shot(state, duration, events)
    }
}

/// `spec` playing a clip named `clip`.
pub(super) fn playing(clip: &'static str, spec: ClipSpec) -> ClipSpec {
    ClipSpec {
        clip: Some(clip),
        ..spec
    }
}

pub(super) fn unresolved(state: &'static str) -> ClipSpec {
    ClipSpec {
        resolved: false,
        ..one_shot(state, 1.0, &[])
    }
}

/// The states every rig carries unless a test redefines them: the rest and
/// travel loops, and the clip an impact effect switches to.
fn base_specs() -> Vec<ClipSpec> {
    vec![
        looping("idle", 2.0, &[]),
        looping("walk", 1.0, &[]),
        one_shot("hit_reaction", 0.4, &[]),
    ]
}

/// The mesh declaring every spec's state, and the clip store holding every
/// resolved spec's clip, each state indexing its own clip.
pub(super) fn rig(specs: Vec<ClipSpec>) -> (MeshComponent, HitZoneStore) {
    let mut all = base_specs();
    all.retain(|base| specs.iter().all(|spec| spec.state != base.state));
    all.extend(specs);

    let mut states = std::collections::HashMap::new();
    let mut clips = Vec::new();
    for spec in &all {
        let clip = spec
            .clip
            .map_or_else(|| format!("{}_clip", spec.state), str::to_string);
        let clip_index = spec.resolved.then(|| {
            clips.push(AnimationClip {
                name: clip.clone(),
                duration: spec.duration,
                joints: Vec::new(),
                travel_speed: None,
                events: spec
                    .events
                    .iter()
                    .map(|&(name, time)| ClipEvent {
                        name: name.to_string(),
                        time,
                    })
                    .collect(),
            });
            clips.len() - 1
        });
        states.insert(
            spec.state.to_string(),
            AnimationState {
                clip,
                looping: spec.looping,
                crossfade_ms: 0.0,
                interrupt: InterruptPolicy::Smooth,
                travel_speed: None,
                clip_index,
            },
        );
    }
    let mesh = MeshComponent {
        model: RIG_MODEL.into(),
        animation: Some(MeshAnimation::new(states, "idle".into())),
        origin_offset: Vec3::ZERO,
        shadow_bias_scale: 1.0,
        shadow_only: false,
        attachments: Vec::new(),
        pose_inputs: None,
    };
    let mut store = HitZoneStore::new();
    store.insert_for_test(
        postretro_model::ModelHandle::from(RIG_MODEL),
        ModelHitZones::for_test(
            Arc::new(Skeleton { joints: Vec::new() }),
            Arc::new(clips),
            Vec::new(),
            std::collections::HashMap::new(),
            None,
            Vec::new(),
            Default::default(),
        ),
    );
    (mesh, store)
}

/// Fire `attack` at clip event `event`.
pub(super) fn timed(attack: &str, event: &str) -> ActionVerb {
    ActionVerb::Attack {
        name: attack.to_string(),
        at: Some(event.to_string()),
    }
}

pub(super) fn untimed(attack: &str) -> ActionVerb {
    ActionVerb::Attack {
        name: attack.to_string(),
        at: None,
    }
}

/// A standing activity on `animation`. An action fired `at` an event carries
/// the turn mark descriptor load implies for it.
pub(super) fn act(animation: &str, action: Option<ActionVerb>) -> BehaviorActivityDescriptor {
    let mut activity = authored_state(animation, MotionVerb::Hold, action);
    if let Some(ActionVerb::Attack {
        name, at: Some(_), ..
    }) = activity.action.as_ref()
    {
        activity.attack_turn = Some(name.clone());
    }
    activity
}

pub(super) fn contact_attack(cooldown_ms: f32) -> AttackParams {
    AttackParams {
        sound: None,
        weapon: None,
        damage: Some(JAB_DAMAGE),
        max_range: Some(2.0),
        cooldown_ms: Some(cooldown_ms),
        engagement_radius: None,
        standoff_distance: None,
        damage_type: None,
        draws: None,
    }
}

pub(super) fn clip_graph(
    initial: &str,
    activities: Vec<(&str, BehaviorActivityDescriptor)>,
    transitions: Vec<(&str, Vec<GuardedRow>)>,
    attacks: Vec<(&str, AttackParams)>,
) -> BehaviorGraphDescriptor {
    test_behavior_graph!({
        initial: initial.to_string(),
        activities: activities
            .into_iter()
            .map(|(name, activity)| (name.to_string(), activity))
            .collect(),
        transitions: transitions
            .into_iter()
            .map(|(from, rows)| (from.to_string(), rows))
            .collect(),
        candidate_filter: None,
        retaliation: None,
        patrol: None,
        attacks: attacks
            .into_iter()
            .map(|(name, attack)| (name.to_string(), attack))
            .collect(),
        engagement_radius: None,
        move_speed: TEST_MOVE_SPEED,
    })
}

/// `stalk` on `walk` and `jab` on `attack_jab` firing `action`, with `rows`
/// leaving `jab` and no way back but a test's own entry.
pub(super) fn jab_graph(action: ActionVerb, rows: Vec<GuardedRow>) -> BehaviorGraphDescriptor {
    clip_graph(
        "stalk",
        vec![
            ("stalk", act("walk", None)),
            ("jab", act("attack_jab", Some(action))),
        ],
        vec![("jab", rows)],
        vec![(JAB, contact_attack(10.0))],
    )
}

pub(super) fn guard_done() -> IrNode {
    brain_input(postretro_foundation::BRAIN_ANIMATION_DONE_INPUT)
}

/// `@brain.timeInActivityMs ≥ ms`.
pub(super) fn after_ms(ms: f32) -> IrNode {
    IrNode::Ge {
        a: Box::new(brain_input(BRAIN_TIME_IN_ACTIVITY_MS_INPUT)),
        b: Box::new(IrNode::Const {
            value: IrValue::Number(ms),
        }),
    }
}

/// A player standing within reach and facing tolerance of every fixture
/// enemy, and the enemies, ticked with their clips.
pub(super) struct Scene {
    pub(super) reg: EntityRegistry,
    pub(super) runtime: AiRuntime,
    pub(super) mesh: MeshComponent,
    pub(super) clips: HitZoneStore,
    pub(super) pawn: EntityId,
    pub(super) descriptors: Vec<EntityTypeDescriptor>,
}

pub(super) fn player_spot() -> Vec3 {
    Vec3::new(1.5, 0.0, 0.0)
}

impl Scene {
    pub(super) fn new(specs: Vec<ClipSpec>) -> Self {
        let (mesh, clips) = rig(specs);
        let mut reg = EntityRegistry::new();
        let pawn = spawn_player(&mut reg, player_spot());
        Self {
            reg,
            runtime: AiRuntime::new(),
            mesh,
            clips,
            pawn,
            descriptors: Vec::new(),
        }
    }

    /// Spawn the `index`th enemy on the rig, grounded, seated in `state` as an
    /// entry: the first tick is its entry tick.
    pub(super) fn spawn(
        &mut self,
        index: usize,
        graph: &BehaviorGraphDescriptor,
        state: &str,
    ) -> EntityId {
        let enemy = spawn_enemy(
            &mut self.reg,
            Vec3::new(0.0, 0.0, -0.3 + 0.2 * index as f32),
            authored_brain(graph, state),
            50.0,
        );
        self.reg.set_component(enemy, self.mesh.clone()).unwrap();
        let mut agent = self.agent(enemy);
        agent.ground = postretro_foundation::GroundRef::World;
        self.reg.set_component(enemy, agent).unwrap();
        self.reg
            .set_component(
                enemy,
                DescriptorProvenance {
                    canonical_name: CLASS.to_string(),
                    owned_components: Default::default(),
                    map_overrides: Default::default(),
                    spawn_path: DescriptorSpawnPath::MapPlacement,
                },
            )
            .unwrap();
        enemy
    }

    pub(super) fn tick(&mut self) -> AiTickResult {
        self.tick_with_impact(|_| {})
    }

    pub(super) fn tick_with_impact(
        &mut self,
        on_impact: impl FnMut(&mut EntityRegistry),
    ) -> AiTickResult {
        run_ai_tick_with_navigation_and_impact(
            &mut self.reg,
            &mut self.runtime,
            DT,
            AiTickInputs {
                mover_nav: crate::nav::MoverNav::none(),
                nav_graph: None,
                collision_world: Some(&CollisionWorld::new()),
                descriptors: &self.descriptors,
                descriptor_generation: 0,
                factions: &FactionRegistry::default(),
                faction_sentiment: &RefCell::new(FactionSentimentState::default()),
                clips: Some(&self.clips),
            },
            on_impact,
        )
    }

    /// Run `ticks` ticks and return the indices of those on which a contact
    /// attack hit the player, the first run tick being 0.
    pub(super) fn hits(&mut self, ticks: usize) -> Vec<usize> {
        self.hits_with(ticks, |_, _| {})
    }

    /// [`Self::hits`], calling `before` ahead of each tick with its index.
    pub(super) fn hits_with(
        &mut self,
        ticks: usize,
        mut before: impl FnMut(&mut Self, usize),
    ) -> Vec<usize> {
        let mut hits = Vec::new();
        for tick in 0..ticks {
            before(self, tick);
            let hp = player_hp(&self.reg, self.pawn);
            self.tick();
            if player_hp(&self.reg, self.pawn) < hp {
                hits.push(tick);
            }
        }
        hits
    }

    pub(super) fn state(&self, enemy: EntityId) -> String {
        enemy_state_path(&self.reg, enemy)
    }

    pub(super) fn agent(&self, enemy: EntityId) -> AgentComponent {
        self.reg
            .get_component::<AgentComponent>(enemy)
            .unwrap()
            .clone()
    }

    pub(super) fn set_agent(&mut self, enemy: EntityId, edit: impl FnOnce(&mut AgentComponent)) {
        let mut agent = self.agent(enemy);
        edit(&mut agent);
        self.reg.set_component(enemy, agent).unwrap();
    }

    /// Enter root activity `state` as a transition would.
    pub(super) fn enter(&mut self, enemy: EntityId, state: &str) {
        let mut brain = self.brain(enemy);
        let index = graph_activity_index(&brain.graph, state).expect("the graph declares it");
        assert!(brain.enter_activity_at(0, index));
        self.reg.set_component(enemy, brain).unwrap();
    }

    pub(super) fn brain(&self, enemy: EntityId) -> BrainComponent {
        self.reg
            .get_component::<BrainComponent>(enemy)
            .unwrap()
            .clone()
    }

    /// Switch the mesh as an impact effect or a reaction does.
    pub(super) fn switch(&mut self, enemy: EntityId, state: &str) {
        assert_eq!(
            switch_animation_state(&mut self.reg, enemy, state),
            postretro_entities::components::mesh::SwitchResult::Switched,
            "fixture: the mesh switches to `{state}`"
        );
    }

    /// Set the playback rate the pose stage writes after the AI.
    pub(super) fn set_rate(&mut self, enemy: EntityId, rate: f32) {
        let mut mesh = self
            .reg
            .get_component::<MeshComponent>(enemy)
            .unwrap()
            .clone();
        mesh.animation.as_mut().unwrap().rate = rate;
        self.reg.set_component(enemy, mesh).unwrap();
    }
}

/// The ticks, counted from a loop's restart tick (0), whose clip-time window
/// holds a pass of a moment at `event` on a clip of `duration` started at the
/// drawn `phase`. Proofs derive their expected ticks from the phase the
/// renderer draws rather than restating them.
pub(super) fn loop_pass_ticks(phase: f32, event: f32, duration: f32, ticks: usize) -> Vec<usize> {
    const SLACK: f64 = 1.0e-4;
    let (phase, event, duration) = (f64::from(phase), f64::from(event), f64::from(duration));
    let tick = f64::from(DT);
    let passes = |time: f64| ((time + SLACK - event) / duration).floor();
    (0..ticks)
        .filter(|&k| {
            let (open, close) = if k == 0 {
                (phase - 2.0 * SLACK, phase)
            } else {
                (phase + tick * (k - 1) as f64, phase + tick * k as f64)
            };
            passes(close) > passes(open)
        })
        .collect()
}

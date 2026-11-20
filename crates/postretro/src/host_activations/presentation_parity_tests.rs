//! Fog/emissive presentation parity (emissive-volumes A11): one case per cited
//! ordering pin, each run against both volume kinds through the shipped
//! primitives, bridges, capture, wire and client apply. An emissive colour write
//! follows fog's tint lifecycle (replication, late join, demote/promote,
//! restart, map change), not its range.
//! See: context/lib/networking.md §Presentation state · scripting.md §10.2, §10.2a
//!
//! Pin map (`context/plans/in-progress/emissive-volumes/research.md`):
//! P6 `p06_*` · P7 `p07_*` · P8 `p08_*` · P9 `p09_*` · P10 `p10_*` · P11 `p11_*`
//! · P13 `p13_*` · P19 `p19_*` · P20 `p20_*` · P21 `p21_*` · P22 `p22_*`
//! · P23 `p23_*` · P24 `p24_*` · P25 `p25_*` · P26 `p26_*` · P35 `p35_*`
//! · P45 `p45a_*` to `p45d_*` · P55 `p55_*`.
use super::*;
use postretro_entities::presentation_overrides::PresentationOverride;
use postretro_entities::{EmissiveVolumeComponent, FireOrigin, FireSubject, FogVolumeComponent};
use postretro_level_format::emissive::EmissiveVolumeRecord;
use postretro_level_loader::LevelWorld;
use postretro_test_log_capture::LogCapture;

/// The emissive fixture volume's authored colour; fog's authored tint is white.
pub(super) const EMISSIVE_COLOR: [f32; 3] = [0.5, 0.75, 1.0];
pub(super) const RED: [f32; 3] = [1.0, 0.05, 0.02];
pub(super) const GREEN: [f32; 3] = [0.02, 1.0, 0.05];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Fog,
    Emissive,
}
pub(super) const KINDS: [Kind; 2] = [Kind::Fog, Kind::Emissive];

/// What a volume presents: the animated scalar (fog density, emissive
/// brightness), the `[0, 1]` scalar (fog glow, emissive brightness), the
/// colour (fog tint, emissive colour) and whether a curve runs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Shown {
    pub(super) value: f32,
    pub(super) unit: f32,
    pub(super) tint: [f32; 3],
    pub(super) animating: bool,
}

impl Kind {
    pub(super) fn tag(self) -> &'static str {
        match self {
            Kind::Fog => "fog",
            Kind::Emissive => "sign",
        }
    }
    fn binding_kind(self) -> u8 {
        match self {
            Kind::Fog => 1,
            Kind::Emissive => 3,
        }
    }
    pub(super) fn component(self) -> ComponentKind {
        match self {
            Kind::Fog => ComponentKind::FogVolume,
            Kind::Emissive => ComponentKind::EmissiveVolume,
        }
    }
    pub(super) fn volume(self, app: &App, index: usize) -> EntityId {
        let session = app.session.as_ref().unwrap();
        match self {
            Kind::Fog => session.fog_volume_bridge.entity_for_map_index(index),
            Kind::Emissive => session.emissive_volume_bridge.entity_for_map_index(index),
        }
        .expect("fixture volume")
    }
    pub(super) fn shown_at(self, app: &App, index: usize) -> Shown {
        let id = self.volume(app, index);
        let ctx = script_ctx(app);
        let registry = ctx.registry.borrow();
        match self {
            Kind::Fog => {
                let fog = registry.get_component::<FogVolumeComponent>(id).unwrap();
                Shown {
                    value: fog.density,
                    unit: fog.glow,
                    tint: fog.tint,
                    animating: fog.animation.is_some(),
                }
            }
            Kind::Emissive => {
                let volume = registry
                    .get_component::<EmissiveVolumeComponent>(id)
                    .unwrap();
                Shown {
                    value: volume.brightness,
                    unit: volume.brightness,
                    tint: volume.color,
                    animating: volume.animation.is_some(),
                }
            }
        }
    }
    /// The running curve's key count on volume 0.
    fn curve_len(self, app: &App) -> Option<usize> {
        let id = self.volume(app, 0);
        let ctx = script_ctx(app);
        let registry = ctx.registry.borrow();
        match self {
            Kind::Fog => registry
                .get_component::<FogVolumeComponent>(id)
                .unwrap()
                .animation
                .as_ref()
                .and_then(|animation| animation.density.as_ref().map(Vec::len)),
            Kind::Emissive => registry
                .get_component::<EmissiveVolumeComponent>(id)
                .unwrap()
                .animation
                .as_ref()
                .map(|animation| animation.brightness.len()),
        }
    }
    pub(super) fn shown(self, app: &App) -> Shown {
        self.shown_at(app, 0)
    }
    /// The no-script state of the fixture's volume 0.
    pub(super) fn authored(self) -> Shown {
        match self {
            Kind::Fog => Shown {
                value: 0.5,
                unit: 0.0,
                tint: [1.0; 3],
                animating: false,
            },
            Kind::Emissive => Shown {
                value: 1.0,
                unit: 1.0,
                tint: EMISSIVE_COLOR,
                animating: false,
            },
        }
    }
    /// Write the animated scalar. Fog's density write leaves a running curve
    /// in place; parity cases never write it over one (the differences row).
    pub(super) fn set_value(self, value: f32) -> (&'static str, Value) {
        match self {
            Kind::Fog => ("setFogDensity", json!({ "density": value })),
            Kind::Emissive => ("setEmissiveBrightness", json!({ "brightness": value })),
        }
    }
    /// Write the `[0, 1]` scalar: fog's glow, emissive brightness.
    fn set_unit(self, value: f32) -> (&'static str, Value) {
        match self {
            Kind::Fog => ("setFogGlow", json!({ "glow": value })),
            Kind::Emissive => self.set_value(value),
        }
    }
    pub(super) fn set_tint(self, tint: [f32; 3]) -> (&'static str, Value) {
        match self {
            Kind::Fog => ("setFogParams", json!({ "tint": tint })),
            Kind::Emissive => ("setEmissiveColor", json!({ "color": tint })),
        }
    }
    pub(super) fn animate(
        self,
        period_ms: f64,
        play_count: Option<u32>,
        keys: &[f32],
    ) -> (&'static str, Value) {
        match self {
            Kind::Fog => (
                "setFogAnimation",
                json!({ "periodMs": period_ms, "playCount": play_count, "density": keys }),
            ),
            Kind::Emissive => (
                "setEmissiveAnimation",
                json!({ "periodMs": period_ms, "playCount": play_count, "brightness": keys }),
            ),
        }
    }
    pub(super) fn clear_animation(self) -> (&'static str, Value) {
        match self {
            Kind::Fog => ("setFogAnimation", Value::Null),
            Kind::Emissive => ("setEmissiveAnimation", Value::Null),
        }
    }
    /// One tag-targeted reaction body step on this kind's volume 0.
    pub(super) fn step(self, (verb, args): (&'static str, Value)) -> NamedReaction {
        primitive("parity", verb, Some(self.tag()), args)
    }
}

pub(super) fn emissive_record(origin: [f32; 3], tags: &[&str]) -> EmissiveVolumeRecord {
    EmissiveVolumeRecord {
        min: 0.0,
        max: 2.0,
        color: EMISSIVE_COLOR,
        origin,
        faces: vec![],
        tags: tags.iter().map(|tag| (*tag).to_owned()).collect(),
    }
}

/// The conditioned fixture plus one emissive volume. Fog record 0 and emissive
/// volume 0 share index and origin, so only the binding kind tells them apart
/// (P23). Both also carry the tag `both` (P11).
pub(super) fn parity_world() -> LevelWorld {
    let mut world = fixture_world();
    world.fog_volumes[0].tags = vec!["fog".into(), "both".into()];
    world.emissive.volumes = vec![emissive_record([0.0; 3], &["sign", "both"])];
    world
}

pub(super) fn host_only() -> FireOrigin {
    FireOrigin::HostOnly(FireSubject::None)
}

/// Run `reactions` as one body on `app` at script time `clock`, under
/// `origin`, as a frame's reaction drain does. Level reactions accumulate, so
/// each body installs under a name no earlier body used.
pub(super) fn fire_on(
    app: &App,
    clock: f64,
    origin: FireOrigin,
    mut reactions: Vec<NamedReaction>,
) {
    thread_local! {
        static BODY: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    }
    let name = BODY.with(|body| {
        body.set(body.get() + 1);
        format!("parity.{}", body.get())
    });
    for reaction in &mut reactions {
        reaction.name.clone_from(&name);
    }
    let ctx = script_ctx(app);
    ctx.presentation_script_time.set(clock);
    ctx.presentation_overrides.borrow_mut().begin_frame();
    ctx.data_registry
        .borrow_mut()
        .populate_level(reactions, vec![], &[]);
    let session = app.session.as_ref().unwrap();
    crate::drain_named_events_with_sequences(
        [(name.as_str(), None)],
        origin,
        &ctx.data_registry.borrow(),
        &session.scripting.sequence_registry,
        &session.scripting.reaction_registry,
        &session.scripting.system_registry,
        &ctx,
    );
}

/// A host-only body on the harness host, then that frame's bridge tick.
pub(super) fn host_write(h: &mut Harness, reactions: Vec<NamedReaction>) {
    fire_on(&h.host, h.clock, host_only(), reactions);
    sample(&mut h.host, h.clock);
}

/// A machine installed alone (host role), its bridges at script time 0.
fn local() -> App {
    installed_with_reactions(&parity_world(), false, Vec::new())
}

/// One local frame: the body, then the bridge tick at `clock`.
fn local_write(app: &mut App, clock: f64, reactions: Vec<NamedReaction>) {
    fire_on(app, clock, host_only(), reactions);
    sample(app, clock);
}

pub(super) fn record(app: &App, kind: Kind) -> Option<PresentationOverride> {
    let id = kind.volume(app, 0);
    script_ctx(app)
        .presentation_overrides
        .borrow()
        .records
        .get(&(id, kind.component()))
        .cloned()
}

pub(super) fn approx(actual: f32, expected: f32, what: &str) {
    assert!(
        (actual - expected).abs() < 1e-4,
        "{what}: {actual} vs {expected}"
    );
}

/// A harness on `world` whose peer A participates.
pub(super) fn joined(world: LevelWorld) -> Harness {
    let mut h = Harness::with_world(world);
    h.advance(40);
    assert!(h.server.is_participating(CLIENT_A));
    h
}

/// Hold peer `index`, as a client un-revealing its level does: the host's
/// Demoted arm drops the slot's replication, and the client's Holding arm drops
/// its replication bookkeeping while keeping its own level content and every
/// applied presentation value.
pub(super) fn demote(h: &mut Harness, index: usize) {
    let id = h.peers[index].id;
    h.peers[index].client.set_revealed_level(None);
    for _ in 0..60 {
        h.step();
        if !h.server.is_participating(id) && !h.peers[index].client.is_participating() {
            break;
        }
    }
    assert!(
        !h.server.is_participating(id) && !h.peers[index].client.is_participating(),
        "peer {index} held on both ends"
    );
    h.replication.remove_client(id);
    h.state = Default::default();
    let peer = &mut h.peers[index];
    peer.replication = Default::default();
    peer.state = Default::default();
}

/// Re-reveal peer `index`: a new participation epoch and fresh baselines.
pub(super) fn promote(h: &mut Harness, index: usize) {
    let id = h.peers[index].id;
    postretro_net::harness::reveal_both(&mut h.server, &mut h.peers[index].client, "presentation");
    for _ in 0..60 {
        h.step();
        if h.server.is_participating(id) && h.peers[index].client.is_participating() {
            break;
        }
    }
    assert!(
        h.server.is_participating(id) && h.peers[index].client.is_participating(),
        "peer {index} promoted on both ends"
    );
}

/// A host level reload onto `world` (a same-map restart, or a map change):
/// every slot is held while the host and each active peer reinstall, then
/// each re-reveals under a new participation epoch. Anything the host sent
/// before the reload is still in flight and must never bind.
pub(super) fn reload(h: &mut Harness, world: LevelWorld) {
    h.server.set_revealed_level(None);
    let active: Vec<usize> = (0..h.peers.len()).filter(|i| h.peers[*i].active).collect();
    // A client reinstalls only once it has seen the hold, as the app's
    // Holding arm runs before any reload; a snapshot sent before the hold then
    // either lands on the old install or is refused by epoch.
    let held = |h: &Harness, i: usize| {
        !h.server.is_participating(h.peers[i].id) && !h.peers[i].client.is_participating()
    };
    for _ in 0..60 {
        h.step();
        if active.iter().all(|i| held(h, *i)) {
            break;
        }
    }
    for &index in &active {
        assert!(held(h, index), "peer {index} held");
    }
    h.world = world;
    h.host = installed_with_reactions(&h.world, false, Vec::new());
    h.allocator = netcode::NetworkIdAllocator::for_test();
    h.set = Default::default();
    h.tracked = Default::default();
    h.replication = Default::default();
    h.state = Default::default();
    h.last = None;
    for &index in &active {
        let peer = &mut h.peers[index];
        peer.app = installed_with_reactions(&h.world, true, Vec::new());
        peer.replication = Default::default();
        peer.state = Default::default();
        peer.prediction = Default::default();
    }
    for &index in &active {
        postretro_net::harness::reveal_both(
            &mut h.server,
            &mut h.peers[index].client,
            "presentation",
        );
    }
    let joined = |h: &Harness, i: usize| {
        h.server.is_participating(h.peers[i].id) && h.peers[i].client.is_participating()
    };
    for _ in 0..60 {
        h.step();
        if active.iter().all(|i| joined(h, *i)) {
            break;
        }
    }
    for &index in &active {
        assert!(joined(h, index), "peer {index} promoted");
    }
}

// ---------------------------------------------------------------------------
// Local pins: primitives, capture and the bridge on one machine
// ---------------------------------------------------------------------------

// P6: an identical animation re-installed while it runs keeps its start (the
// bridge's and the capture's); a running finite curve does not restart. Once a
// finite run settles, the same install is a new run.
#[test]
fn p06_identical_reinstall_keeps_start_until_a_finite_run_settles() {
    for kind in KINDS {
        let mut app = local();
        let fade = kind.animate(1000.0, Some(1), &[0.0, 1.0]);
        local_write(&mut app, 0.0, vec![kind.step(fade.clone())]);
        approx(kind.shown(&app).value, 0.0, "first sample");
        sample(&mut app, 0.5);
        local_write(&mut app, 0.5, vec![kind.step(fade.clone())]);
        assert_eq!(
            record(&app, kind).unwrap().anim_start,
            Some(0.0),
            "{kind:?}"
        );
        sample(&mut app, 0.75);
        approx(kind.shown(&app).value, 0.75, "the run continues");
        sample(&mut app, 1.1);
        let settled = kind.shown(&app);
        assert!(!settled.animating, "{kind:?} settled");
        approx(settled.value, 1.0, "settles on its last sample");

        local_write(&mut app, 1.2, vec![kind.step(fade)]);
        assert_eq!(
            record(&app, kind).unwrap().anim_start,
            Some(1.2),
            "{kind:?}"
        );
        sample(&mut app, 1.45);
        approx(kind.shown(&app).value, 0.25, "the settled curve restarts");
    }
}

// P7: a non-positive period or an empty curve is refused with a warning; the
// running animation and the value stay. (A `flicker` of rate 0 has an infinite
// period, which no JSON number carries; the Rust boundary refuses it as fog's
// does.)
#[test]
fn p07_invalid_period_or_empty_curve_is_refused_and_leaves_the_running_curve() {
    for kind in KINDS {
        let mut app = local();
        local_write(
            &mut app,
            0.0,
            vec![kind.step(kind.animate(1000.0, None, &[0.0, 1.0]))],
        );
        sample(&mut app, 0.25);
        let before = record(&app, kind);
        let logs = LogCapture::start();
        for bad in [
            kind.animate(0.0, None, &[0.5]),
            kind.animate(-10.0, None, &[0.5]),
            kind.animate(1000.0, None, &[]),
        ] {
            fire_on(&app, 0.25, host_only(), vec![kind.step(bad)]);
        }
        logs.assert_logged(log::Level::Warn, "periodMs must be > 0");
        logs.assert_logged(log::Level::Warn, "curve is empty");
        // A refused install changes nothing a record carries but the sample
        // the bridge last wrote: the run, its start and its curve hold.
        let after = record(&app, kind);
        assert_eq!(
            after.as_ref().map(|r| r.anim_start),
            before.as_ref().map(|r| r.anim_start),
            "{kind:?}"
        );
        assert_eq!(kind.curve_len(&app), Some(2), "{kind:?}");
        assert!(kind.shown(&app).animating);
        sample(&mut app, 0.5);
        approx(kind.shown(&app).value, 0.5, "the prior run continues");
    }
}

// P8: playCount 0 reads 1, with a warning, and settles once.
#[test]
fn p08_zero_play_count_reads_one_and_settles_once() {
    for kind in KINDS {
        let mut app = local();
        let logs = LogCapture::start();
        local_write(
            &mut app,
            0.0,
            vec![kind.step(kind.animate(500.0, Some(0), &[0.0, 0.8]))],
        );
        logs.assert_logged(log::Level::Warn, "playCount of 0");
        sample(&mut app, 0.25);
        assert!(kind.shown(&app).animating);
        sample(&mut app, 0.6);
        let settled = kind.shown(&app);
        assert!(!settled.animating, "{kind:?} one period, then done");
        approx(settled.value, 0.8, "settles on its last sample");
    }
}

// P9: a flat `fade` (from == to) holds its value every frame and settles on it.
#[test]
fn p09_flat_fade_holds_and_settles_on_its_value() {
    for kind in KINDS {
        let mut app = local();
        local_write(
            &mut app,
            0.0,
            vec![kind.step(kind.animate(500.0, Some(1), &[0.4, 0.4]))],
        );
        for clock in [0.1, 0.25, 0.45] {
            sample(&mut app, clock);
            approx(kind.shown(&app).value, 0.4, "flat");
        }
        sample(&mut app, 0.6);
        let settled = kind.shown(&app);
        assert!(!settled.animating);
        approx(settled.value, 0.4, "settled");
    }
}

// P10: a `[0, 1]` scalar out of range clamps with one warning per write, and a
// negative curve sample clamps to 0. (NaN cannot cross JSON; the Rust dispatch
// boundary reads it as 0 for both kinds — sim's fog glow and emissive tests.)
// The emissive curve also clamps above 1, where fog's density curve has no
// ceiling: range is not part of parity.
#[test]
fn p10_out_of_range_scalars_and_curve_samples_clamp_with_one_warning() {
    for kind in KINDS {
        let mut app = local();
        for (written, read) in [(-0.5, 0.0), (1.5, 1.0)] {
            let logs = LogCapture::start();
            local_write(&mut app, 0.0, vec![kind.step(kind.set_unit(written))]);
            approx(kind.shown(&app).unit, read, "clamped");
            assert_eq!(
                logs.records()
                    .iter()
                    .filter(|r| r.level == log::Level::Warn && r.message.contains("clamping"))
                    .count(),
                1,
                "{kind:?}: one warning per write"
            );
        }
        let logs = LogCapture::start();
        local_write(
            &mut app,
            1.0,
            vec![kind.step(kind.animate(1000.0, None, &[-0.5, 1.5]))],
        );
        logs.assert_logged(log::Level::Warn, "clamping");
        approx(kind.shown(&app).value, 0.0, "negative sample reads 0");
        if kind == Kind::Emissive {
            sample(&mut app, 1.999);
            assert!(kind.shown(&app).value <= 1.0);
        }
    }
}

// P11: a tag matching no volume is a debug no-op; a tag matching another kind's
// entity warns, skips it, and still writes this kind's volume.
#[test]
fn p11_empty_tag_is_silent_and_a_foreign_target_warns_and_is_skipped() {
    for kind in KINDS {
        let other = if kind == Kind::Fog {
            Kind::Emissive
        } else {
            Kind::Fog
        };
        let mut app = local();
        let logs = LogCapture::start();
        let (verb, args) = kind.set_value(0.25);
        local_write(
            &mut app,
            0.0,
            vec![primitive("parity", verb, Some("nothing"), args.clone())],
        );
        logs.assert_nothing_above(log::Level::Info);
        assert_eq!(kind.shown(&app), kind.authored());

        local_write(
            &mut app,
            0.0,
            vec![primitive("parity", verb, Some("both"), args)],
        );
        logs.assert_logged_once(log::Level::Warn, "skipping");
        approx(kind.shown(&app).value, 0.25, "this kind's volume written");
        assert_eq!(other.shown(&app), other.authored(), "{kind:?}");
    }
}

// P13: while the script clock is frozen the bridge samples it, so the value
// holds its sample across presented frames, and resumes from there. (The flash
// clamp's aging on presented-frame time is the limiter's.)
#[test]
fn p13_frozen_script_clock_holds_the_sample() {
    for kind in KINDS {
        let mut app = local();
        local_write(
            &mut app,
            0.0,
            vec![kind.step(kind.animate(1000.0, None, &[0.0, 1.0]))],
        );
        sample(&mut app, 0.25);
        for _ in 0..5 {
            sample_delta(&mut app, 0.25, DT as f32);
            approx(kind.shown(&app).value, 0.25, "frozen");
        }
        sample(&mut app, 0.5);
        approx(kind.shown(&app).value, 0.5, "resumed");
    }
}

// ---------------------------------------------------------------------------
// Co-op pins: host capture → wire → client binding and apply
// ---------------------------------------------------------------------------

// P19: a host-only write, then a host restart: host and client both present
// the authored state after reinstall; the old life's record never binds,
// though a snapshot carrying it was in flight across the reload.
#[test]
fn p19_restart_presents_authored_state_and_the_old_record_never_binds() {
    for kind in KINDS {
        let mut h = joined(parity_world());
        host_write(&mut h, vec![kind.step(kind.set_value(0.0))]);
        h.advance(12);
        approx(kind.shown(&h.peers[0].app).value, 0.0, "replicated");
        host_write(&mut h, vec![kind.step(kind.set_value(0.25))]);
        h.step();
        reload(&mut h, parity_world());
        h.advance(30);
        assert_eq!(kind.shown(&h.host), kind.authored(), "{kind:?} host");
        assert_eq!(
            kind.shown(&h.peers[0].app),
            kind.authored(),
            "{kind:?} client"
        );
        assert!(record(&h.host, kind).is_none());
    }
}

// P20: a `levelLoad` sequence `wait 2000` then a write. The host lands it and
// records an override. Every machine installs the same reaction, but the
// client never lands it: it presents the value only once the record crosses
// the link, never at its own reading of the host's tick.
#[test]
fn p20_level_load_wait_tail_lands_on_the_host_and_reaches_the_client_as_state() {
    for kind in KINDS {
        let (verb, args) = kind.set_value(0.0);
        let reactions = vec![
            NamedReaction {
                name: "levelLoad".into(),
                descriptor: ReactionDescriptor::Sequence(vec![
                    SequenceStep {
                        id: SequenceTarget::Wait,
                        primitive: "wait".into(),
                        args: json!({ "durationMs": 2000 }),
                    },
                    SequenceStep {
                        id: SequenceTarget::Fire,
                        primitive: "fire".into(),
                        args: json!({ "event": "parity.tail" }),
                    },
                ]),
            },
            primitive("parity.tail", verb, Some(kind.tag()), args),
        ];
        let mut h = Harness::with_shared_reactions(parity_world(), reactions);
        let mut host_landed = None;
        let mut client_changed = None;
        while h.clock < 3.0 {
            h.step();
            if host_landed.is_none() && kind.shown(&h.host).value == 0.0 {
                host_landed = Some(h.clock);
                assert!(record(&h.host, kind).is_some(), "{kind:?}: host records");
                assert_eq!(
                    kind.shown(&h.peers[0].app),
                    kind.authored(),
                    "{kind:?}: the client did not land it"
                );
            }
            if client_changed.is_none() && kind.shown(&h.peers[0].app).value == 0.0 {
                client_changed = Some(h.clock);
            }
        }
        let host_landed = host_landed.expect("host landed the tail");
        assert!(
            (host_landed - 2.0).abs() <= 2.0 * DT + 1e-6,
            "{host_landed}"
        );
        // The 80 ms link delivers the record in the fourth frame after the
        // host's; a local landing, at a reading of the host's tick that runs
        // half an RTT ahead, could come no later than the host's own.
        let client_changed = client_changed.expect("client received the record");
        assert!(
            client_changed - host_landed >= 3.0 * DT - 1e-6,
            "{kind:?}: the client's change crossed the link: host {host_landed}, client {client_changed}"
        );
    }
}

// P21: a late joiner, 1 s into the host's 2 s fade, rebases on the host's
// script time; its fade settles when the host's does.
#[test]
fn p21_late_joiner_rebases_a_running_fade_and_settles_with_the_host() {
    for kind in KINDS {
        let mut h = joined(parity_world());
        host_write(
            &mut h,
            vec![kind.step(kind.animate(2000.0, Some(1), &[1.0, 0.0]))],
        );
        let start = h.clock;
        while h.clock < start + 1.0 {
            h.step();
        }
        h.connect_late();
        let mut finishes = [None; 3];
        while h.clock < start + 2.5 {
            h.step();
            for (index, app) in std::iter::once(&h.host)
                .chain(h.peers.iter().map(|peer| &peer.app))
                .enumerate()
            {
                let shown = kind.shown(app);
                if finishes[index].is_none() && !shown.animating && shown.value == 0.0 {
                    finishes[index] = Some(h.clock);
                }
            }
        }
        let host = finishes[0].expect("host settled");
        assert!((host - start - 2.0).abs() <= 2.0 * DT + 1e-6);
        for client in &finishes[1..] {
            let client = client.expect("client settled");
            assert!(
                (client - host).abs() <= 2.0 * DT + 1e-6,
                "{kind:?}: {finishes:?}"
            );
        }
    }
}

// P22: a late joiner after the host's fade ended presents the last sample from
// its first world frame; it never replays the curve.
#[test]
fn p22_late_joiner_after_a_settled_fade_presents_its_last_sample_without_replay() {
    for kind in KINDS {
        let mut h = joined(parity_world());
        host_write(
            &mut h,
            vec![kind.step(kind.animate(500.0, Some(1), &[1.0, 0.0]))],
        );
        h.advance(60);
        approx(kind.shown(&h.host).value, 0.0, "host settled");
        h.connect_late();
        let mut applied = false;
        for _ in 0..60 {
            h.step();
            let shown = kind.shown(&h.peers[1].app);
            if shown != kind.authored() {
                applied = true;
                approx(shown.value, 0.0, "first changed frame is the last sample");
                assert!(!shown.animating, "{kind:?}: no replay");
            }
        }
        assert!(applied, "{kind:?}: the joiner received the record");
    }
}

// P23: the kind is part of the key. Fog record 0 and emissive volume 0 share
// index and origin; a write to one kind reaches only that kind's volume.
#[test]
fn p23_kind_is_part_of_the_binding_key() {
    for kind in KINDS {
        let other = if kind == Kind::Fog {
            Kind::Emissive
        } else {
            Kind::Fog
        };
        let mut h = joined(parity_world());
        host_write(
            &mut h,
            vec![
                kind.step(kind.set_value(0.25)),
                kind.step(kind.set_tint(RED)),
            ],
        );
        h.advance(12);
        let peer = &h.peers[0];
        let nid = h.allocator.stamp_for_test(kind.volume(&h.host, 0));
        assert_eq!(client_entity(peer, nid), kind.volume(&peer.app, 0));
        let shown = kind.shown(&peer.app);
        approx(shown.value, 0.25, "this kind's volume");
        assert_eq!(shown.tint, RED);
        assert_eq!(other.shown(&peer.app), other.authored(), "{kind:?}");
    }
}

// P24: demotion unbinds and keeps the client's last applied record, animation
// included; re-promotion's baseline brings the host's current state.
#[test]
fn p24_demotion_keeps_the_last_record_and_promotion_brings_the_hosts_current_state() {
    for kind in KINDS {
        let mut h = joined(parity_world());
        host_write(
            &mut h,
            vec![kind.step(kind.animate(1000.0, None, &[0.2, 0.8]))],
        );
        h.advance(12);
        assert!(kind.shown(&h.peers[0].app).animating);
        demote(&mut h, 0);
        h.advance(12);
        assert!(
            kind.shown(&h.peers[0].app).animating,
            "{kind:?}: demotion keeps the applied curve"
        );
        host_write(
            &mut h,
            vec![
                kind.step(kind.clear_animation()),
                kind.step(kind.set_value(0.25)),
            ],
        );
        h.advance(12);
        assert!(
            kind.shown(&h.peers[0].app).animating,
            "{kind:?}: still held"
        );
        promote(&mut h, 0);
        h.advance(12);
        let shown = kind.shown(&h.peers[0].app);
        assert!(!shown.animating, "{kind:?}");
        approx(shown.value, 0.25, "the host's current value");
    }
}

// P25: a host-only write to every volume of the kind (emissive's 256 cap,
// fog's 16) reaches a late joiner across snapshots under the baseline budget.
// The budget's order (bound presentation records after gameplay) is the net
// tracker's: `presentation_budget_tests`.
#[test]
fn p25_a_write_to_every_volume_reaches_a_late_joiner() {
    for kind in KINDS {
        let mut world = parity_world();
        let count = match kind {
            Kind::Fog => postretro_level_format::fog_volumes::MAX_FOG_VOLUMES,
            Kind::Emissive => postretro_level_format::emissive::MAX_EMISSIVE_VOLUMES,
        };
        match kind {
            Kind::Fog => {
                let template = world.fog_volumes[0].clone();
                world.fog_volumes = (0..count)
                    .map(|index| {
                        let mut fog = template.clone();
                        fog.center = [index as f32 * 4.0, 0.0, 0.0];
                        fog.tags = vec!["all".into()];
                        fog
                    })
                    .collect();
            }
            Kind::Emissive => {
                world.emissive.volumes = (0..count)
                    .map(|index| emissive_record([index as f32 * 4.0, 0.0, 0.0], &["all"]))
                    .collect();
            }
        }
        let mut h = joined(world);
        let (verb, args) = kind.set_tint(RED);
        host_write(&mut h, vec![primitive("parity", verb, Some("all"), args)]);
        h.advance(4);
        h.connect_late();
        h.advance(90);
        assert!(h.server.is_participating(CLIENT_B));
        for index in 0..count {
            assert_eq!(
                kind.shown_at(&h.peers[1].app, index).tint,
                RED,
                "{kind:?} volume {index}"
            );
        }
    }
}

// P26: a curve past the 64-key wire cap presents on the host, cannot encode,
// warns once, and the client keeps the last valid record.
#[test]
fn p26_an_unencodable_curve_presents_on_the_host_and_the_client_keeps_the_last_valid_record() {
    for kind in KINDS {
        let mut h = joined(parity_world());
        host_write(
            &mut h,
            vec![kind.step(kind.animate(1000.0, None, &[0.2, 0.8]))],
        );
        h.advance(12);
        let valid = record(&h.host, kind).unwrap();
        let oversized = vec![0.5; postretro_net::wire::MAX_WIRE_CURVE_KEYS + 1];
        let logs = LogCapture::start();
        host_write(
            &mut h,
            vec![kind.step(kind.animate(1000.0, None, &oversized))],
        );
        host_write(
            &mut h,
            vec![kind.step(kind.animate(1000.0, None, &oversized))],
        );
        logs.assert_logged_once(log::Level::Warn, "cannot be encoded");
        assert_eq!(record(&h.host, kind), Some(valid), "{kind:?}");
        h.advance(12);
        approx(kind.shown(&h.host).value, 0.5, "the host presents it");
        assert_eq!(kind.curve_len(&h.host), Some(oversized.len()));
        assert_eq!(
            kind.curve_len(&h.peers[0].app),
            Some(2),
            "{kind:?}: the client keeps the last valid curve"
        );
    }
}

// P35: a host record and a client-local write in one client frame: the local
// write lands after the record and holds, and the client re-applies only when
// the host's record changes.
#[test]
fn p35_a_local_write_after_the_record_holds_until_the_host_record_changes() {
    for kind in KINDS {
        let mut h = joined(parity_world());
        host_write(&mut h, vec![kind.step(kind.set_value(0.2))]);
        let mut landed = false;
        for _ in 0..30 {
            h.step();
            if kind.shown(&h.peers[0].app).value == 0.2 {
                landed = true;
                break;
            }
        }
        assert!(landed, "{kind:?}: the record applied");
        // The same frame's drain, after SnapshotApply, then RenderPrep.
        let clock = h.peers[0].clock;
        let peer = &mut h.peers[0].app;
        fire_on(
            peer,
            clock,
            FireOrigin::EveryMachine(postretro_entities::MachineSource::HostMirrored),
            vec![kind.step(kind.set_value(0.7))],
        );
        sample(peer, clock);
        for _ in 0..20 {
            h.step();
            approx(kind.shown(&h.peers[0].app).value, 0.7, "local write holds");
        }
        host_write(&mut h, vec![kind.step(kind.set_value(0.4))]);
        h.advance(12);
        approx(
            kind.shown(&h.peers[0].app).value,
            0.4,
            "a changed record applies",
        );
    }
}

// P45a: a host-only colour write reaches a late joiner from its first world
// frame with the binding.
#[test]
fn p45a_colour_reaches_a_late_joiner_from_its_first_bound_frame() {
    for kind in KINDS {
        let mut h = joined(parity_world());
        host_write(&mut h, vec![kind.step(kind.set_tint(RED))]);
        h.advance(30);
        h.connect_late();
        let nid = h.allocator.stamp_for_test(kind.volume(&h.host, 0));
        let mut bound = false;
        for _ in 0..60 {
            h.step();
            let peer = &h.peers[1];
            if peer
                .replication
                .entity_network_ids()
                .values()
                .any(|id| *id == nid)
            {
                bound = true;
                assert_eq!(kind.shown(&peer.app).tint, RED, "{kind:?}");
            }
        }
        assert!(bound, "{kind:?}: the joiner bound the volume");
    }
}

// P45b: demote, host writes green, promote: re-promotion brings green.
#[test]
fn p45b_colour_written_while_demoted_arrives_on_promotion() {
    for kind in KINDS {
        let mut h = joined(parity_world());
        host_write(&mut h, vec![kind.step(kind.set_tint(RED))]);
        h.advance(12);
        assert_eq!(kind.shown(&h.peers[0].app).tint, RED);
        demote(&mut h, 0);
        host_write(&mut h, vec![kind.step(kind.set_tint(GREEN))]);
        h.advance(12);
        assert_eq!(kind.shown(&h.peers[0].app).tint, RED, "{kind:?}: held");
        promote(&mut h, 0);
        h.advance(12);
        assert_eq!(kind.shown(&h.peers[0].app).tint, GREEN, "{kind:?}");
    }
}

// P45c: a host restart after a colour write: host and client both present
// the authored colour.
#[test]
fn p45c_restart_presents_the_authored_colour() {
    for kind in KINDS {
        let mut h = joined(parity_world());
        host_write(&mut h, vec![kind.step(kind.set_tint(RED))]);
        h.advance(12);
        assert_eq!(kind.shown(&h.peers[0].app).tint, RED);
        reload(&mut h, parity_world());
        h.advance(30);
        assert_eq!(kind.shown(&h.host).tint, kind.authored().tint);
        assert_eq!(kind.shown(&h.peers[0].app).tint, kind.authored().tint);
    }
}

// P45d: a change to map B: B's volume 0 starts at its own authored colour.
#[test]
fn p45d_map_change_starts_at_the_new_maps_authored_colour() {
    const B_COLOR: [f32; 3] = [0.25, 0.5, 0.125];
    for kind in KINDS {
        let mut h = joined(parity_world());
        host_write(&mut h, vec![kind.step(kind.set_tint(RED))]);
        h.advance(12);
        let mut map_b = parity_world();
        map_b.fog_volumes[0].tint = B_COLOR;
        map_b.emissive.volumes[0].color = B_COLOR;
        reload(&mut h, map_b);
        h.advance(30);
        assert_eq!(kind.shown(&h.host).tint, B_COLOR, "{kind:?} host");
        assert_eq!(kind.shown(&h.peers[0].app).tint, B_COLOR, "{kind:?} client");
    }
}

// P55: the client's volume 0 sits 2 cm from the host's: the client warns once
// and drops every update; it never writes this or another volume.
#[test]
fn p55_a_2cm_origin_mismatch_warns_once_and_writes_nothing() {
    for kind in KINDS {
        let mut h = joined(parity_world());
        let mut shifted = parity_world();
        shifted.fog_volumes[0].center[0] += 0.02;
        shifted.emissive.volumes[0].origin[0] += 0.02;
        h.peers[0].app = installed_with_reactions(&shifted, true, Vec::new());
        h.peers[0].replication = Default::default();
        let logs = LogCapture::start();
        host_write(&mut h, vec![kind.step(kind.set_value(0.25))]);
        h.advance(12);
        host_write(&mut h, vec![kind.step(kind.set_tint(RED))]);
        h.advance(12);
        logs.assert_logged_once(
            log::Level::Warn,
            &format!("presentation binding kind {} index 0", kind.binding_kind()),
        );
        for any in KINDS {
            assert_eq!(any.shown(&h.peers[0].app), any.authored(), "{kind:?}");
        }
    }
}

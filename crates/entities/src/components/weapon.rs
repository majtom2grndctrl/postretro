// Weapon descriptor tuning plus live magazine, cooldown, reload, and input-edge state.
// See: context/lib/entity_model.md §4, §5

use glam::Vec3;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
#[cfg(debug_assertions)]
use std::sync::Once;

use crate::components::wieldable_state::WieldableState;
use crate::data_descriptors::{
    FireMode, ProjectileDescriptor, ReloadStyle, ResolutionMode, WeaponDescriptor, WeaponResource,
};

pub const UNKNOWN_WEAPON_CREDIT_SOURCE: &str = "weapon.unknown";

#[cfg(debug_assertions)]
static WARNED_UNKNOWN_CREDIT_SOURCE: Once = Once::new();

#[derive(Debug, Clone, PartialEq)]
pub struct EffectiveAmmoStats<'a> {
    pub ammo_type: &'a str,
    pub capacity: u32,
    pub cost_per_shot: u32,
    pub reload_ms: u32,
    pub reload_style: ReloadStyle,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EffectiveStats<'a> {
    pub damage: f32,
    pub pellet_count: u32,
    pub spread_degrees: f32,
    pub range: f32,
    pub cooldown_ms: f32,
    pub fire_mode: FireMode,
    pub resolution: ResolutionMode,
    pub projectile: Option<&'a ProjectileDescriptor>,
    /// Model-local projectile origin authored on this weapon.
    pub muzzle_offset: Option<Vec3>,
    pub lower_ms: u32,
    pub raise_ms: u32,
    /// Per-weapon override of the mod-global reload-interrupt policy. This is
    /// deliberately unresolved: only the App-owned commit gates know the mod
    /// default.
    pub block_during_reload: Option<bool>,
    pub credit_source: &'a str,
    pub ammo: Option<EffectiveAmmoStats<'a>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WeaponAmmoTuning {
    pub ammo_type: String,
    pub capacity: u32,
    pub cost_per_shot: u32,
    pub reload_ms: u32,
    #[serde(default = "default_reload_style")]
    pub reload_style: ReloadStyle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReloadFeedback {
    Started,
    Completed,
}

const RELOAD_FEEDBACK_STREAM_CAPACITY: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReloadFeedbackConsumer {
    Hud,
    OwnerProjection,
}

/// Endpoint metadata exposed to frame-rate consumers.
///
/// Consecutive equal endpoints produced in one simulation tick share one
/// observation. `occurrences` reports how many endpoints it represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReloadFeedbackObservation {
    pub feedback: ReloadFeedback,
    pub producer_tick: u64,
    pub occurrences: u32,
    pub coalesced: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReloadFeedbackSample {
    pub progress: f32,
    pub active: bool,
    pub endpoint: Option<ReloadFeedbackObservation>,
    /// Endpoints evicted before this consumer acknowledged them.
    pub lost_endpoints: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ReloadFeedbackEntry {
    sequence: u64,
    feedback: ReloadFeedback,
    producer_tick: u64,
    occurrences: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct ReloadFeedbackCursor {
    next_sequence: u64,
    lost_endpoints: u64,
    last_endpoint: Option<ReloadFeedback>,
}

/// One bounded endpoint stream shared by HUD and owner-private projection.
///
/// Equal adjacent endpoints coalesce only when one machine tick produced them
/// and neither consumer has consumed the run. When 32 retained runs are full,
/// the oldest run is evicted; affected cursors report its occurrence count as
/// loss. Retained runs always remain FIFO, so a new endpoint never bypasses an
/// older separator or endpoint. Occurrence and loss counters saturate at their
/// integer maxima. Consumers acknowledge independently.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReloadFeedbackStream {
    entries: VecDeque<ReloadFeedbackEntry>,
    next_sequence: u64,
    producer_tick: u64,
    hud: ReloadFeedbackCursor,
    owner_projection: ReloadFeedbackCursor,
}

impl ReloadFeedbackStream {
    fn begin_producer_tick(&mut self) -> u64 {
        self.producer_tick = self.producer_tick.wrapping_add(1);
        self.producer_tick
    }

    fn push(&mut self, feedback: ReloadFeedback, producer_tick: u64) {
        let tail_is_unread = self.entries.back().is_some_and(|tail| {
            self.hud.next_sequence <= tail.sequence
                && self.owner_projection.next_sequence <= tail.sequence
        });
        if tail_is_unread
            && let Some(tail) = self.entries.back_mut()
            && tail.feedback == feedback
            && tail.producer_tick == producer_tick
        {
            tail.occurrences = tail.occurrences.saturating_add(1);
            return;
        }

        if self.entries.len() == RELOAD_FEEDBACK_STREAM_CAPACITY {
            self.evict_oldest();
        }
        if self.entries.is_empty() {
            self.entries.reserve(RELOAD_FEEDBACK_STREAM_CAPACITY);
        }
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.wrapping_add(1);
        self.entries.push_back(ReloadFeedbackEntry {
            sequence,
            feedback,
            producer_tick,
            occurrences: 1,
        });
    }

    fn evict_oldest(&mut self) {
        let Some(entry) = self.entries.pop_front() else {
            return;
        };
        Self::record_eviction(&mut self.hud, entry);
        Self::record_eviction(&mut self.owner_projection, entry);
    }

    fn record_eviction(cursor: &mut ReloadFeedbackCursor, entry: ReloadFeedbackEntry) {
        if cursor.next_sequence <= entry.sequence {
            cursor.next_sequence = entry.sequence.wrapping_add(1);
            cursor.lost_endpoints = cursor
                .lost_endpoints
                .saturating_add(u64::from(entry.occurrences));
        }
    }

    fn cursor(&self, consumer: ReloadFeedbackConsumer) -> &ReloadFeedbackCursor {
        match consumer {
            ReloadFeedbackConsumer::Hud => &self.hud,
            ReloadFeedbackConsumer::OwnerProjection => &self.owner_projection,
        }
    }

    fn cursor_mut(&mut self, consumer: ReloadFeedbackConsumer) -> &mut ReloadFeedbackCursor {
        match consumer {
            ReloadFeedbackConsumer::Hud => &mut self.hud,
            ReloadFeedbackConsumer::OwnerProjection => &mut self.owner_projection,
        }
    }

    fn next_entry(&self, consumer: ReloadFeedbackConsumer) -> Option<ReloadFeedbackEntry> {
        let next_sequence = self.cursor(consumer).next_sequence;
        self.entries
            .iter()
            .copied()
            .find(|entry| entry.sequence >= next_sequence)
    }

    fn retain_same_tick_completion(&mut self, producer_tick: u64) {
        self.entries.retain(|entry| {
            entry.feedback == ReloadFeedback::Completed && entry.producer_tick == producer_tick
        });
        self.reseat_after_filter(ReloadFeedbackConsumer::Hud);
        self.reseat_after_filter(ReloadFeedbackConsumer::OwnerProjection);
    }

    fn reseat_after_filter(&mut self, consumer: ReloadFeedbackConsumer) {
        let current = self.cursor(consumer).next_sequence;
        let next = self
            .entries
            .iter()
            .find(|entry| entry.sequence >= current)
            .map_or(self.next_sequence, |entry| entry.sequence);
        self.cursor_mut(consumer).next_sequence = next;
    }

    fn acknowledge(&mut self, consumer: ReloadFeedbackConsumer) -> bool {
        let next = self.next_entry(consumer);
        let cursor = self.cursor_mut(consumer);
        let had_last_endpoint = cursor.last_endpoint.is_some();
        if let Some(entry) = next {
            if cursor.last_endpoint == Some(entry.feedback) {
                // A consumed endpoint value needs a live-state separator before an
                // identical endpoint can be consumed. Wait for the next publication.
                cursor.last_endpoint = None;
            } else {
                cursor.next_sequence = entry.sequence.wrapping_add(1);
                cursor.last_endpoint = Some(entry.feedback);
            }
        } else {
            cursor.last_endpoint = None;
        }
        let advanced = next.is_some() || cursor.lost_endpoints > 0 || had_last_endpoint;
        cursor.lost_endpoints = 0;
        self.discard_fully_consumed();
        advanced
    }

    fn discard_fully_consumed(&mut self) {
        let consumed_before = self
            .hud
            .next_sequence
            .min(self.owner_projection.next_sequence);
        while self
            .entries
            .front()
            .is_some_and(|entry| entry.sequence < consumed_before)
        {
            self.entries.pop_front();
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WeaponComponent {
    pub damage: f32,
    #[serde(default)]
    pub pellet_count: u32,
    #[serde(default)]
    pub spread_degrees: f32,
    pub range: f32,
    pub cooldown_ms: f32,
    pub fire_mode: FireMode,
    pub resolution: ResolutionMode,
    #[serde(default)]
    pub projectile: Option<ProjectileDescriptor>,
    /// Model-local projectile origin authored on this weapon.
    #[serde(default)]
    pub muzzle_offset: Option<Vec3>,
    #[serde(default)]
    pub lower_ms: u32,
    #[serde(default)]
    pub raise_ms: u32,
    #[serde(default)]
    pub block_during_reload: Option<bool>,
    pub cooldown_remaining_ms: f32,
    #[serde(default)]
    pub shoot_press_consumed: bool,
    #[serde(default)]
    pub reload_press_consumed: bool,
    #[serde(default = "default_credit_source")]
    pub credit_source: String,
    #[serde(default)]
    pub ammo: Option<WeaponAmmoTuning>,
    #[serde(default)]
    pub magazine: u32,
    #[serde(default)]
    pub state: WieldableState,
    #[serde(default)]
    pub state_remaining_ms: u32,
    #[serde(default)]
    pub state_total_ms: u32,
    /// Fractional elapsed milliseconds carried between fixed ticks. Public HUD
    /// and replication fields remain integer milliseconds; this remainder keeps
    /// their countdown from accumulating per-tick rounding bias.
    #[serde(default)]
    pub state_elapsed_sub_ms: f64,
    /// Rounds credited during the active reload. This is reset on every state
    /// transition into or out of reload activity, never inferred from magazine.
    #[serde(default)]
    pub reload_credited: u32,
    /// Monotonic per-instance shell counter used to seed deterministic pellet spread.
    #[serde(default)]
    pub shells_fired: u32,
    /// Bounded endpoint stream with independent HUD and owner-projection cursors.
    #[serde(skip)]
    pub reload_feedback: ReloadFeedbackStream,
}

impl WeaponComponent {
    pub fn from_descriptor(desc: &WeaponDescriptor) -> Self {
        Self::from_descriptor_with_canonical(desc, None)
    }

    pub fn from_descriptor_with_canonical(
        desc: &WeaponDescriptor,
        canonical_name: Option<&str>,
    ) -> Self {
        let ammo = ammo_tuning(desc);
        let magazine = ammo.as_ref().map_or(0, |ammo| ammo.capacity);
        Self {
            damage: desc.damage,
            pellet_count: desc.pellet_count,
            spread_degrees: desc.spread_degrees,
            range: desc.range,
            cooldown_ms: desc.cooldown_ms,
            fire_mode: desc.fire_mode,
            resolution: desc.resolution,
            projectile: desc.projectile.clone(),
            muzzle_offset: desc.muzzle_offset.map(Vec3::from_array),
            lower_ms: desc.lower_ms,
            raise_ms: desc.raise_ms,
            block_during_reload: desc.block_during_reload,
            cooldown_remaining_ms: 0.0,
            shoot_press_consumed: false,
            reload_press_consumed: false,
            credit_source: resolve_credit_source(desc, canonical_name),
            ammo,
            magazine,
            state: WieldableState::Idle,
            state_remaining_ms: 0,
            state_total_ms: 0,
            state_elapsed_sub_ms: 0.0,
            reload_credited: 0,
            shells_fired: 0,
            reload_feedback: ReloadFeedbackStream::default(),
        }
    }

    pub fn effective(&self) -> EffectiveStats<'_> {
        EffectiveStats {
            damage: self.damage,
            pellet_count: self.pellet_count,
            spread_degrees: self.spread_degrees,
            range: self.range,
            cooldown_ms: self.cooldown_ms,
            fire_mode: self.fire_mode,
            resolution: self.resolution,
            projectile: self.projectile.as_ref(),
            muzzle_offset: self.muzzle_offset,
            lower_ms: self.lower_ms,
            raise_ms: self.raise_ms,
            block_during_reload: self.block_during_reload,
            credit_source: &self.credit_source,
            ammo: self.ammo.as_ref().map(|ammo| EffectiveAmmoStats {
                ammo_type: &ammo.ammo_type,
                capacity: ammo.capacity,
                cost_per_shot: ammo.cost_per_shot,
                reload_ms: ammo.reload_ms,
                reload_style: ammo.reload_style,
            }),
        }
    }

    pub fn refresh_from_descriptor(&mut self, desc: &WeaponDescriptor) {
        self.damage = desc.damage;
        self.pellet_count = desc.pellet_count;
        self.spread_degrees = desc.spread_degrees;
        self.range = desc.range;
        self.cooldown_ms = desc.cooldown_ms;
        self.fire_mode = desc.fire_mode;
        self.resolution = desc.resolution;
        self.projectile = desc.projectile.clone();
        self.muzzle_offset = desc.muzzle_offset.map(Vec3::from_array);
        self.lower_ms = desc.lower_ms;
        self.raise_ms = desc.raise_ms;
        self.block_during_reload = desc.block_during_reload;
        if let Some(credit_source) = desc.credit_source.as_ref() {
            self.credit_source = credit_source.clone();
        }
        self.ammo = ammo_tuning(desc);
        // Cooldown, input edges, magazine, state, timed-state fields, reload credit,
        // and shells fired are live instance state. Hot reload changes authored tuning,
        // not the active state sample or whether this instance is mid-cooldown. An
        // absent `creditSource` also keeps the already-resolved spawn-time default so
        // canonical defaults do not regress to `weapon.unknown` on reload.
    }

    pub fn reload_status(&self) -> (f32, bool) {
        let sample = self.reload_feedback_sample(ReloadFeedbackConsumer::Hud);
        (sample.progress, sample.active)
    }

    pub fn owner_reload_status(&self) -> (f32, bool) {
        let sample = self.reload_feedback_sample(ReloadFeedbackConsumer::OwnerProjection);
        (sample.progress, sample.active)
    }

    pub fn begin_reload_feedback_tick(&mut self) -> u64 {
        self.reload_feedback.begin_producer_tick()
    }

    pub fn publish_reload_feedback(&mut self, feedback: ReloadFeedback, producer_tick: u64) {
        self.reload_feedback.push(feedback, producer_tick);
    }

    pub fn acknowledge_reload_feedback(&mut self, consumer: ReloadFeedbackConsumer) -> bool {
        self.reload_feedback.acknowledge(consumer)
    }

    pub fn clear_cancelled_reload_feedback(&mut self, producer_tick: u64) {
        self.reload_feedback
            .retain_same_tick_completion(producer_tick);
    }

    /// Discard every reload endpoint when an equip transition takes ownership
    /// of this instance. Unlike cancellation, a switch must not preserve a
    /// same-tick completion: the next active weapon needs a fresh endpoint
    /// observation even when it publishes the same value.
    pub fn clear_equip_reload_feedback(&mut self) {
        self.reload_feedback.entries.clear();
        self.reload_feedback
            .reseat_after_filter(ReloadFeedbackConsumer::Hud);
        self.reload_feedback
            .reseat_after_filter(ReloadFeedbackConsumer::OwnerProjection);
        self.reload_feedback.hud.last_endpoint = None;
        self.reload_feedback.owner_projection.last_endpoint = None;
        self.reload_feedback.hud.lost_endpoints = 0;
        self.reload_feedback.owner_projection.lost_endpoints = 0;
    }

    pub fn reload_feedback_sample(&self, consumer: ReloadFeedbackConsumer) -> ReloadFeedbackSample {
        let cursor = self.reload_feedback.cursor(consumer);
        let endpoint = self.reload_feedback.next_entry(consumer);
        if let Some(entry) = endpoint
            && cursor.last_endpoint != Some(entry.feedback)
        {
            let (progress, active) = match entry.feedback {
                ReloadFeedback::Started => (0.0, true),
                ReloadFeedback::Completed => (1.0, true),
            };
            return ReloadFeedbackSample {
                progress,
                active,
                endpoint: Some(ReloadFeedbackObservation {
                    feedback: entry.feedback,
                    producer_tick: entry.producer_tick,
                    occurrences: entry.occurrences,
                    coalesced: entry.occurrences > 1,
                }),
                lost_endpoints: cursor.lost_endpoints,
            };
        }

        let (progress, active) = match () {
            () if self.state.is_reload_activity()
                && self.state_remaining_ms > 0
                && self.state_total_ms > 0 =>
            {
                (
                    (1.0 - self.state_remaining_ms as f32 / self.state_total_ms as f32)
                        .clamp(0.0, 1.0),
                    true,
                )
            }
            () if self.state.is_reload_activity() => (0.0, true),
            () => (0.0, false),
        };
        ReloadFeedbackSample {
            progress,
            active,
            endpoint: None,
            lost_endpoints: cursor.lost_endpoints,
        }
    }
}

fn ammo_tuning(desc: &WeaponDescriptor) -> Option<WeaponAmmoTuning> {
    desc.resource.as_ref().map(|resource| match resource {
        WeaponResource::Ammo(ammo) => WeaponAmmoTuning {
            ammo_type: ammo.ammo_type.clone(),
            capacity: ammo.magazine,
            cost_per_shot: ammo.cost_per_shot,
            reload_ms: ammo.reload_ms,
            reload_style: ammo.reload_style,
        },
    })
}

fn resolve_credit_source(desc: &WeaponDescriptor, canonical_name: Option<&str>) -> String {
    if let Some(credit_source) = desc.credit_source.as_ref() {
        return credit_source.clone();
    }
    if let Some(canonical_name) = canonical_name {
        return canonical_name.to_string();
    }
    warn_unknown_credit_source_once();
    UNKNOWN_WEAPON_CREDIT_SOURCE.to_string()
}

fn default_credit_source() -> String {
    UNKNOWN_WEAPON_CREDIT_SOURCE.to_string()
}

const fn default_reload_style() -> ReloadStyle {
    ReloadStyle::Magazine
}

#[cfg(debug_assertions)]
fn warn_unknown_credit_source_once() {
    WARNED_UNKNOWN_CREDIT_SOURCE.call_once(|| {
        log::warn!(
            "weapon descriptor materialized without authored creditSource or canonical name; using {UNKNOWN_WEAPON_CREDIT_SOURCE}"
        );
    });
}

#[cfg(not(debug_assertions))]
fn warn_unknown_credit_source_once() {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_descriptors::AmmoResource;

    fn descriptor(damage: f32, range: f32, cooldown_ms: f32) -> WeaponDescriptor {
        WeaponDescriptor {
            damage,
            pellet_count: 1,
            spread_degrees: 0.0,
            range,
            cooldown_ms,
            fire_mode: FireMode::Semi,
            resolution: ResolutionMode::Hitscan,
            projectile: None,
            credit_source: None,
            third_person_model: None,
            viewmodel: None,
            placement: None,
            muzzle_offset: None,
            resource: None,
            lower_ms: 0,
            raise_ms: 0,
            block_during_reload: None,
        }
    }

    fn ammo_descriptor(
        ammo_type: &str,
        capacity: u32,
        cost_per_shot: u32,
        reload_ms: u32,
    ) -> WeaponDescriptor {
        let mut descriptor = descriptor(10.0, 20.0, 100.0);
        descriptor.resource = Some(WeaponResource::Ammo(AmmoResource {
            ammo_type: ammo_type.to_string(),
            magazine: capacity,
            cost_per_shot,
            reserve: 48,
            reload_ms,
            reload_style: ReloadStyle::Magazine,
        }));
        descriptor
    }

    #[test]
    fn from_descriptor_seeds_ammo_tuning_full_magazine_and_idle_reload() {
        let component =
            WeaponComponent::from_descriptor(&ammo_descriptor("bullets.light", 12, 2, 850));

        assert_eq!(
            component.ammo,
            Some(WeaponAmmoTuning {
                ammo_type: "bullets.light".to_string(),
                capacity: 12,
                cost_per_shot: 2,
                reload_ms: 850,
                reload_style: ReloadStyle::Magazine,
            })
        );
        assert_eq!(component.magazine, 12);
        assert_eq!(component.state, WieldableState::Idle);
        assert_eq!(component.state_remaining_ms, 0);
        assert_eq!(component.state_total_ms, 0);
        assert!((component.state_elapsed_sub_ms - 0.0).abs() < f64::EPSILON);
        assert_eq!(component.reload_credited, 0);
        assert_eq!(component.reload_feedback, ReloadFeedbackStream::default());
    }

    #[test]
    fn from_descriptor_without_resource_preserves_unlimited_fire_state() {
        let component = WeaponComponent::from_descriptor(&descriptor(10.0, 20.0, 100.0));

        assert_eq!(component.ammo, None);
        assert_eq!(component.magazine, 0);
        assert_eq!(component.state, WieldableState::Idle);
        assert_eq!(component.state_remaining_ms, 0);
        assert_eq!(component.state_total_ms, 0);
        assert_eq!(component.effective().ammo, None);
    }

    #[test]
    fn muzzle_offset_materializes_refreshes_and_surfaces_effectively() {
        let mut descriptor = descriptor(10.0, 20.0, 100.0);
        descriptor.muzzle_offset = Some([0.2, -0.1, -0.7]);
        let mut component = WeaponComponent::from_descriptor(&descriptor);
        assert_eq!(component.muzzle_offset, Some(Vec3::new(0.2, -0.1, -0.7)));
        assert_eq!(component.effective().muzzle_offset, component.muzzle_offset);

        descriptor.muzzle_offset = Some([-0.3, 0.4, -1.1]);
        component.refresh_from_descriptor(&descriptor);
        assert_eq!(component.muzzle_offset, Some(Vec3::new(-0.3, 0.4, -1.1)));
    }

    #[test]
    fn equip_timing_materializes_and_refreshes_with_descriptor_tuning() {
        let mut descriptor = descriptor(10.0, 20.0, 100.0);
        descriptor.lower_ms = 25;
        descriptor.raise_ms = 40;
        descriptor.block_during_reload = Some(true);
        let mut component = WeaponComponent::from_descriptor(&descriptor);

        assert_eq!(component.lower_ms, 25);
        assert_eq!(component.raise_ms, 40);
        assert_eq!(component.effective().lower_ms, 25);
        assert_eq!(component.effective().raise_ms, 40);
        assert_eq!(component.effective().block_during_reload, Some(true));

        descriptor.lower_ms = 60;
        descriptor.raise_ms = 75;
        descriptor.block_during_reload = Some(false);
        component.refresh_from_descriptor(&descriptor);

        assert_eq!(component.lower_ms, 60);
        assert_eq!(component.raise_ms, 75);
        assert_eq!(component.effective().lower_ms, 60);
        assert_eq!(component.effective().raise_ms, 75);
        assert_eq!(component.effective().block_during_reload, Some(false));
    }

    #[test]
    fn effective_projects_authored_ammo_stats() {
        let mut descriptor = ammo_descriptor("shells.heavy", 8, 1, 1200);
        let Some(WeaponResource::Ammo(ammo)) = descriptor.resource.as_mut() else {
            panic!("expected ammo resource");
        };
        ammo.reload_style = ReloadStyle::PerShell;
        let component = WeaponComponent::from_descriptor(&descriptor);

        assert_eq!(
            component.effective().ammo,
            Some(EffectiveAmmoStats {
                ammo_type: "shells.heavy",
                capacity: 8,
                cost_per_shot: 1,
                reload_ms: 1200,
                reload_style: ReloadStyle::PerShell,
            })
        );
    }

    #[test]
    fn refresh_updates_ammo_tuning_and_preserves_all_live_state() {
        let mut component =
            WeaponComponent::from_descriptor(&ammo_descriptor("bullets", 12, 1, 800));
        component.magazine = 3;
        component.state = WieldableState::Reloading;
        component.state_remaining_ms = 275;
        component.state_total_ms = 800;
        component.state_elapsed_sub_ms = 0.625;
        let feedback_tick = component.begin_reload_feedback_tick();
        component.publish_reload_feedback(ReloadFeedback::Started, feedback_tick);
        component.reload_credited = 2;
        component.cooldown_remaining_ms = 42.0;
        component.shoot_press_consumed = true;
        component.reload_press_consumed = true;

        component.refresh_from_descriptor(&ammo_descriptor("cells", 30, 3, 1400));

        assert_eq!(
            component.ammo,
            Some(WeaponAmmoTuning {
                ammo_type: "cells".to_string(),
                capacity: 30,
                cost_per_shot: 3,
                reload_ms: 1400,
                reload_style: ReloadStyle::Magazine,
            })
        );
        assert_eq!(component.effective().ammo.unwrap().reload_ms, 1400);
        assert_eq!(component.magazine, 3);
        assert_eq!(component.state, WieldableState::Reloading);
        assert_eq!(component.state_remaining_ms, 275);
        assert_eq!(component.state_total_ms, 800);
        assert!((component.state_elapsed_sub_ms - 0.625).abs() < f64::EPSILON);
        assert_eq!(
            component
                .reload_feedback_sample(ReloadFeedbackConsumer::Hud)
                .endpoint
                .map(|endpoint| endpoint.feedback),
            Some(ReloadFeedback::Started)
        );
        assert_eq!(component.reload_credited, 2);
        assert!((component.cooldown_remaining_ms - 42.0).abs() < f32::EPSILON);
        assert!(component.shoot_press_consumed);
        assert!(component.reload_press_consumed);
    }

    #[test]
    fn weapon_ammo_tuning_persistence_defaults_and_round_trips_reload_style() {
        let legacy: WeaponAmmoTuning = serde_json::from_value(serde_json::json!({
            "ammo_type": "shells.heavy",
            "capacity": 8,
            "cost_per_shot": 1,
            "reload_ms": 1200,
        }))
        .unwrap();
        assert_eq!(legacy.reload_style, ReloadStyle::Magazine);

        let per_shell = WeaponAmmoTuning {
            ammo_type: "shells.heavy".to_string(),
            capacity: 8,
            cost_per_shot: 1,
            reload_ms: 1200,
            reload_style: ReloadStyle::PerShell,
        };
        let persisted = serde_json::to_value(&per_shell).unwrap();
        assert_eq!(persisted["reload_style"], serde_json::json!("perShell"));
        assert_eq!(
            serde_json::from_value::<WeaponAmmoTuning>(persisted).unwrap(),
            per_shell
        );
    }

    #[test]
    fn refresh_can_remove_ammo_tuning_without_aborting_live_reload() {
        let mut component =
            WeaponComponent::from_descriptor(&ammo_descriptor("bullets", 12, 1, 800));
        component.magazine = 4;
        component.state = WieldableState::Reloading;
        component.state_remaining_ms = 300;
        component.state_total_ms = 800;

        component.refresh_from_descriptor(&descriptor(10.0, 20.0, 100.0));

        assert_eq!(component.ammo, None);
        assert_eq!(component.magazine, 4);
        assert_eq!(component.state_remaining_ms, 300);
        assert_eq!(component.state_total_ms, 800);
        let (progress, is_reloading) = component.reload_status();
        assert!((progress - 0.625).abs() < f32::EPSILON);
        assert!(is_reloading);
    }

    #[test]
    fn reload_status_exposes_lifecycle_endpoints_and_timer_without_ammo_tuning() {
        let mut component =
            WeaponComponent::from_descriptor(&ammo_descriptor("bullets", 12, 1, 800));
        component.state_remaining_ms = 600;
        component.state_total_ms = 800;
        component.state = WieldableState::Reloading;
        let (progress, is_reloading) = component.reload_status();
        assert!((progress - 0.25).abs() < f32::EPSILON);
        assert!(is_reloading);

        let feedback_tick = component.begin_reload_feedback_tick();
        component.publish_reload_feedback(ReloadFeedback::Started, feedback_tick);
        let (progress, is_reloading) = component.reload_status();
        assert!((progress - 0.0).abs() < f32::EPSILON);
        assert!(is_reloading);
        component.reload_feedback = ReloadFeedbackStream::default();
        let feedback_tick = component.begin_reload_feedback_tick();
        component.publish_reload_feedback(ReloadFeedback::Completed, feedback_tick);
        component.state_remaining_ms = 0;
        let (progress, is_reloading) = component.reload_status();
        assert!((progress - 1.0).abs() < f32::EPSILON);
        assert!(is_reloading);

        component.ammo = None;
        component.reload_feedback = ReloadFeedbackStream::default();
        component.state_remaining_ms = 400;
        let (progress, is_reloading) = component.reload_status();
        assert!((progress - 0.5).abs() < f32::EPSILON);
        assert!(is_reloading);
    }

    #[test]
    fn reload_feedback_consumers_advance_independently_in_endpoint_order() {
        let mut component = WeaponComponent::from_descriptor(&descriptor(10.0, 20.0, 100.0));
        let first_tick = component.begin_reload_feedback_tick();
        component.publish_reload_feedback(ReloadFeedback::Started, first_tick);
        let second_tick = component.begin_reload_feedback_tick();
        component.publish_reload_feedback(ReloadFeedback::Completed, second_tick);

        let (hud_progress, hud_active) = component.reload_status();
        let (owner_progress, owner_active) = component.owner_reload_status();
        assert!((hud_progress - 0.0).abs() < f32::EPSILON);
        assert!(hud_active);
        assert!((owner_progress - 0.0).abs() < f32::EPSILON);
        assert!(owner_active);

        component.acknowledge_reload_feedback(ReloadFeedbackConsumer::OwnerProjection);
        let (hud_progress, _) = component.reload_status();
        let (owner_progress, _) = component.owner_reload_status();
        assert!((hud_progress - 0.0).abs() < f32::EPSILON);
        assert!((owner_progress - 1.0).abs() < f32::EPSILON);

        component.acknowledge_reload_feedback(ReloadFeedbackConsumer::Hud);
        let (hud_progress, _) = component.reload_status();
        assert!((hud_progress - 1.0).abs() < f32::EPSILON);
    }

    // Regression: a newly produced endpoint bypassed an older identical endpoint
    // while that older endpoint still needed a live separator.
    #[test]
    fn reload_feedback_live_separator_does_not_invert_fifo() {
        let mut component = WeaponComponent::from_descriptor(&descriptor(10.0, 20.0, 100.0));
        let first_tick = component.begin_reload_feedback_tick();
        component.publish_reload_feedback(ReloadFeedback::Completed, first_tick);
        component.acknowledge_reload_feedback(ReloadFeedbackConsumer::Hud);

        let older_tick = component.begin_reload_feedback_tick();
        component.publish_reload_feedback(ReloadFeedback::Completed, older_tick);
        let newer_tick = component.begin_reload_feedback_tick();
        component.publish_reload_feedback(ReloadFeedback::Started, newer_tick);

        let separator = component.reload_feedback_sample(ReloadFeedbackConsumer::Hud);
        assert!(separator.endpoint.is_none());
        component.acknowledge_reload_feedback(ReloadFeedbackConsumer::Hud);

        let older = component
            .reload_feedback_sample(ReloadFeedbackConsumer::Hud)
            .endpoint
            .unwrap();
        assert_eq!(older.feedback, ReloadFeedback::Completed);
        assert_eq!(older.producer_tick, older_tick);
        component.acknowledge_reload_feedback(ReloadFeedbackConsumer::Hud);

        let newer = component
            .reload_feedback_sample(ReloadFeedbackConsumer::Hud)
            .endpoint
            .unwrap();
        assert_eq!(newer.feedback, ReloadFeedback::Started);
        assert_eq!(newer.producer_tick, newer_tick);
    }

    // Regression: cancellation retained stale completion endpoints from earlier
    // simulation ticks.
    #[test]
    fn cancellation_retains_only_same_tick_completed_endpoints() {
        let mut component = WeaponComponent::from_descriptor(&descriptor(10.0, 20.0, 100.0));
        let stale_tick = component.begin_reload_feedback_tick();
        component.publish_reload_feedback(ReloadFeedback::Started, stale_tick);
        component.publish_reload_feedback(ReloadFeedback::Completed, stale_tick);
        let cancel_tick = component.begin_reload_feedback_tick();
        component.publish_reload_feedback(ReloadFeedback::Started, cancel_tick);
        component.publish_reload_feedback(ReloadFeedback::Completed, cancel_tick);

        component.clear_cancelled_reload_feedback(cancel_tick);

        for consumer in [
            ReloadFeedbackConsumer::Hud,
            ReloadFeedbackConsumer::OwnerProjection,
        ] {
            let endpoint = component
                .reload_feedback_sample(consumer)
                .endpoint
                .expect("same-tick completion survives");
            assert_eq!(endpoint.feedback, ReloadFeedback::Completed);
            assert_eq!(endpoint.producer_tick, cancel_tick);
            assert_eq!(endpoint.occurrences, 1);
        }
    }

    #[test]
    fn feedback_stream_coalesces_same_tick_run_with_observable_count() {
        let mut component = WeaponComponent::from_descriptor(&descriptor(10.0, 20.0, 100.0));
        let tick = component.begin_reload_feedback_tick();
        for _ in 0..300 {
            component.publish_reload_feedback(ReloadFeedback::Completed, tick);
        }

        for consumer in [
            ReloadFeedbackConsumer::Hud,
            ReloadFeedbackConsumer::OwnerProjection,
        ] {
            let sample = component.reload_feedback_sample(consumer);
            let endpoint = sample.endpoint.unwrap();
            assert_eq!(endpoint.occurrences, 300);
            assert!(endpoint.coalesced);
            assert_eq!(sample.lost_endpoints, 0);
        }
    }

    // Regression: a full backlog silently overwrote the tail endpoint type.
    #[test]
    fn feedback_stream_overflow_drops_oldest_and_reports_loss_to_both_consumers() {
        let mut component = WeaponComponent::from_descriptor(&descriptor(10.0, 20.0, 100.0));
        for index in 0..(RELOAD_FEEDBACK_STREAM_CAPACITY + 5) {
            let tick = component.begin_reload_feedback_tick();
            let feedback = if index % 2 == 0 {
                ReloadFeedback::Started
            } else {
                ReloadFeedback::Completed
            };
            component.publish_reload_feedback(feedback, tick);
        }

        for consumer in [
            ReloadFeedbackConsumer::Hud,
            ReloadFeedbackConsumer::OwnerProjection,
        ] {
            let sample = component.reload_feedback_sample(consumer);
            assert_eq!(sample.lost_endpoints, 5);
            let endpoint = sample.endpoint.unwrap();
            assert_eq!(endpoint.producer_tick, 6);
            assert_eq!(endpoint.feedback, ReloadFeedback::Completed);
        }
    }

    #[test]
    fn refresh_from_descriptor_updates_stats_and_preserves_live_state() {
        let mut component = WeaponComponent::from_descriptor_with_canonical(
            &descriptor(10.0, 20.0, 100.0),
            Some("reference_pistol"),
        );
        component.cooldown_remaining_ms = 42.0;
        component.shoot_press_consumed = true;
        component.state = WieldableState::Reloading;
        component.state_remaining_ms = 600;
        component.state_total_ms = 800;
        component.state_elapsed_sub_ms = 0.5;
        component.reload_credited = 3;
        component.shells_fired = 7;

        let mut reloaded = descriptor(25.0, 80.0, 250.0);
        reloaded.pellet_count = 8;
        reloaded.spread_degrees = 4.0;
        component.refresh_from_descriptor(&reloaded);

        assert!((component.damage - 25.0).abs() < f32::EPSILON);
        assert_eq!(component.pellet_count, 8);
        assert!((component.spread_degrees - 4.0).abs() < f32::EPSILON);
        assert!((component.range - 80.0).abs() < f32::EPSILON);
        assert!((component.cooldown_ms - 250.0).abs() < f32::EPSILON);
        assert!((component.cooldown_remaining_ms - 42.0).abs() < f32::EPSILON);
        assert!(component.shoot_press_consumed);
        assert_eq!(component.state, WieldableState::Reloading);
        assert_eq!(component.state_remaining_ms, 600);
        assert_eq!(component.state_total_ms, 800);
        assert!((component.state_elapsed_sub_ms - 0.5).abs() < f64::EPSILON);
        assert_eq!(component.reload_credited, 3);
        assert_eq!(component.shells_fired, 7);
        assert_eq!(component.credit_source, "reference_pistol");
    }

    #[test]
    fn from_descriptor_prefers_authored_credit_source_over_canonical_name() {
        let mut descriptor = descriptor(10.0, 20.0, 100.0);
        descriptor.credit_source = Some("plasma.primary".to_string());

        let component =
            WeaponComponent::from_descriptor_with_canonical(&descriptor, Some("plasma_rifle"));

        assert_eq!(component.credit_source, "plasma.primary");
        assert_eq!(component.effective().credit_source, "plasma.primary");
    }

    #[test]
    fn from_descriptor_uses_canonical_name_when_credit_source_is_absent() {
        let component = WeaponComponent::from_descriptor_with_canonical(
            &descriptor(10.0, 20.0, 100.0),
            Some("reference_pistol"),
        );

        assert_eq!(component.credit_source, "reference_pistol");
        assert_eq!(component.effective().credit_source, "reference_pistol");
    }

    #[test]
    fn from_descriptor_uses_unknown_fallback_without_authored_or_canonical_source() {
        let component = WeaponComponent::from_descriptor(&descriptor(10.0, 20.0, 100.0));

        assert_eq!(component.credit_source, UNKNOWN_WEAPON_CREDIT_SOURCE);
    }

    #[test]
    fn refresh_from_descriptor_updates_authored_credit_source_when_present() {
        let mut component = WeaponComponent::from_descriptor_with_canonical(
            &descriptor(10.0, 20.0, 100.0),
            Some("reference_pistol"),
        );
        let mut reloaded = descriptor(25.0, 80.0, 250.0);
        reloaded.credit_source = Some("pistol.alt".to_string());

        component.refresh_from_descriptor(&reloaded);

        assert_eq!(component.credit_source, "pistol.alt");
    }
}

// Data-script registries: active level definitions plus engine-global entity,
// faction, map, reaction, crossing, trigger-event, and trigger-pool snapshots used by
// startup and staged reloads.
// See: context/lib/scripting.md §2 (Data context lifecycle)
//
// Held inside `ScriptCtx` (not directly on `App`) so primitive closures can
// access it via the same captured handle they use for the entity registry.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};

use super::data_descriptors::{
    CrossingDescriptor, EntityTypeDescriptor, NamedReaction, TriggerEventDescriptor,
    TriggerPoolDescriptor,
};
use postretro_foundation::{ModMapEntry, WeaponPlacementDescriptor};

/// Engine-global reaction definition plus its optional level-tag scope.
/// Empty `levels` means all levels; activation/composition happens separately.
#[derive(Clone, Debug, PartialEq)]
pub struct ScopedReaction {
    pub reaction: NamedReaction,
    pub levels: Vec<String>,
}

/// Engine-global state-crossing definition plus its optional level-tag scope.
/// Empty `levels` means all levels; activation/composition happens separately.
#[derive(Clone, Debug, PartialEq)]
pub struct ScopedCrossing {
    pub crossing: CrossingDescriptor,
    pub levels: Vec<String>,
}

pub type ScopedTriggerEvent = TriggerEventDescriptor;
pub type ScopedTriggerPool = TriggerPoolDescriptor;

/// Stable, manifest-authored faction name. The runtime stores only the resolved
/// scalar index on an entity; names stay in this engine-global content registry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FactionDescriptor {
    pub name: String,
}

/// One authored directional relationship entry. Names exist only at manifest
/// drain time; [`FactionRegistry`] resolves them into sparse index-pair
/// overrides before the AI tick can read the relationship.
#[derive(Clone, Debug, PartialEq)]
pub struct FactionSentimentDescriptor {
    pub from_faction: String,
    pub to_faction: String,
    pub sentiment: f32,
    pub tolerance: f32,
    /// Optional per-pair rate for easing a live sentiment value back to this
    /// authored baseline. `None` resolves to the manifest-wide default.
    pub decay: Option<f32>,
}

/// A directional relationship resolved from the faction registry. `tolerance`
/// is present only for an authored pair override; AI resolves absent values
/// through its entity and compatibility tolerance rules.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FactionRelationship {
    pub sentiment: f32,
    pub tolerance: Option<f32>,
    /// An authored pair override for the live sentiment decay rate.
    pub decay: Option<f32>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct FactionRelationshipOverride {
    from: usize,
    to: usize,
    relationship: FactionRelationship,
}

/// The absent faction carried by player pawns.
pub const PLAYER_FACTION_INDEX: f32 = 0.0;
/// The compatibility faction used by brain-bearing archetypes without an
/// authored faction declaration.
pub const DEFAULT_ENEMY_FACTION_INDEX: f32 = 1.0;
/// Namespace reserved for durable engine-only faction identities. Authored
/// faction names cannot enter it, so persisted reserved keys never collide.
pub const ENGINE_RESERVED_FACTION_NAME_PREFIX: &str = "@postretro.";
/// Compatibility sentiment for an unlisted same-faction pair.
pub const SAME_FACTION_DEFAULT_SENTIMENT: f32 = 0.0;
/// Compatibility sentiment for an unlisted cross-faction pair.
pub const CROSS_FACTION_DEFAULT_SENTIMENT: f32 = -1.0;
const FIRST_AUTHORED_FACTION_INDEX: f32 = 2.0;
const MAX_EXACT_FACTION_INDEX: usize = 1 << 24;
/// Largest faction index that mutable sentiment can replicate losslessly.
pub const MAX_FACTION_SENTIMENT_INDEX: usize = u16::MAX as usize;
/// Largest sparse live overlay that one snapshot record can carry.
pub const MAX_FACTION_SENTIMENT_OVERRIDES: usize = 4096;
/// A decaying live value this close to its immutable baseline returns to the
/// baseline exactly and leaves the sparse overlay.
const SENTIMENT_DECAY_BASELINE_EPSILON: f32 = 1.0e-6;
static NEXT_FACTION_SENTIMENT_GENERATION: AtomicU64 = AtomicU64::new(1);

/// Return the adjacent finite `f32` in the requested numeric direction.
///
/// This is the MSRV-compatible equivalent of `next_up` / `next_down` for the
/// finite values held by the live overlay. `direction_is_positive` selects the
/// next greater representable value; zero steps to the smallest magnitude value
/// on the matching side of zero.
fn adjacent_finite_f32(value: f32, direction_is_positive: bool) -> f32 {
    debug_assert!(value.is_finite());
    if direction_is_positive {
        if value == 0.0 {
            f32::from_bits(1)
        } else if value.is_sign_positive() {
            f32::from_bits(value.to_bits() + 1)
        } else {
            f32::from_bits(value.to_bits() - 1)
        }
    } else if value == 0.0 {
        f32::from_bits(1 | (1 << 31))
    } else if value.is_sign_positive() {
        f32::from_bits(value.to_bits() - 1)
    } else {
        f32::from_bits(value.to_bits() + 1)
    }
}

/// Manifest faction names resolved to compact, stable entity-state indices.
///
/// Indices 0 and 1 stay reserved for the player and the built-in default enemy
/// faction. Author declarations retain manifest order and begin at index 2.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FactionRegistry {
    descriptors: Vec<FactionDescriptor>,
    /// Sorted authored overrides keyed by `(from, to)` numeric indices.
    /// Candidate-scan lookup is allocation-free; absent pairs use the
    /// compatibility relationship without reserving an N x N matrix.
    relationship_overrides: Vec<FactionRelationshipOverride>,
    /// Manifest-wide default for pairs without an authored decay override.
    /// Its `0.0` default intentionally preserves the pre-decay behavior.
    faction_sentiment_decay: f32,
}

/// Engine-owned, live faction sentiment values that diverge from immutable
/// authored faction content.
///
/// Pairs stay sorted by their resolved faction indices so the AI can read the
/// sparse overlay without building an N x N matrix. A value equal to its
/// authored baseline is deliberately absent: the registry remains the one
/// source of truth for the decay target and content compatibility.
#[derive(Clone, Debug, Default)]
pub struct FactionSentimentState {
    overrides: Vec<FactionSentimentOverride>,
    /// Changes only when the sparse set changes. Snapshot production uses this
    /// to reuse its lowered wire cache on unchanged frames.
    generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct FactionSentimentOverride {
    from: usize,
    to: usize,
    current: f32,
}

/// Why a live sentiment write cannot enter the host-authoritative overlay.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FactionSentimentMutationError {
    NonFiniteValue,
    InvalidFactionIndex(f32),
    FactionIndexNotWireRepresentable(usize),
    CapacityExceeded,
}

impl std::fmt::Display for FactionSentimentMutationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NonFiniteValue => write!(f, "sentiment value and baseline must be finite"),
            Self::InvalidFactionIndex(value) => {
                write!(
                    f,
                    "faction index {value} is not an exact non-negative integer"
                )
            }
            Self::FactionIndexNotWireRepresentable(index) => write!(
                f,
                "faction index {index} exceeds the u16 sentiment wire representation"
            ),
            Self::CapacityExceeded => write!(
                f,
                "live sentiment overlay exceeds {MAX_FACTION_SENTIMENT_OVERRIDES} diverged pairs"
            ),
        }
    }
}

impl std::error::Error for FactionSentimentMutationError {}

impl PartialEq for FactionSentimentState {
    fn eq(&self, other: &Self) -> bool {
        self.overrides == other.overrides
    }
}

/// A transient AI read over immutable faction content and the live sentiment
/// overlay. The overlay changes sentiment only; tolerance always remains
/// authored baseline content.
#[derive(Clone, Copy, Debug)]
pub struct LiveFactionSentiment<'a> {
    baseline: &'a FactionRegistry,
    overlay: &'a FactionSentimentState,
}

impl FactionSentimentState {
    /// Return a live override for this directional pair, if it has diverged
    /// from its authored baseline.
    pub fn get(&self, from: f32, to: f32) -> Option<f32> {
        self.resolved_pair(from, to)
            .ok()
            .and_then(|pair| self.get_resolved(pair))
    }

    /// Set a live value for a directional pair. Supplying the baseline keeps
    /// this runtime state independent of content and removes a pair as soon as
    /// it returns to that baseline. The write fails before mutation when its
    /// indices or sparse-set size cannot cross the snapshot wire unchanged.
    pub fn set(
        &mut self,
        from: f32,
        to: f32,
        value: f32,
        baseline: f32,
    ) -> Result<bool, FactionSentimentMutationError> {
        if !value.is_finite() || !baseline.is_finite() {
            return Err(FactionSentimentMutationError::NonFiniteValue);
        }
        let pair = self.resolved_pair(from, to)?;
        self.set_resolved(pair, value, baseline)
    }

    /// Add `delta` to the live value for a directional pair. An absent pair
    /// starts at the supplied authored baseline. The same wire-representability
    /// invariant as [`Self::set`] applies.
    pub fn adjust(
        &mut self,
        from: f32,
        to: f32,
        delta: f32,
        baseline: f32,
    ) -> Result<bool, FactionSentimentMutationError> {
        if !delta.is_finite() || !baseline.is_finite() {
            return Err(FactionSentimentMutationError::NonFiniteValue);
        }
        let pair = self.resolved_pair(from, to)?;
        let current = self.get_resolved(pair).unwrap_or(baseline);
        let next = current + delta;
        if !current.is_finite() || !next.is_finite() {
            return Err(FactionSentimentMutationError::NonFiniteValue);
        }
        self.set_resolved(pair, next, baseline)
    }

    /// Mutation stamp for allocation-free snapshot cache invalidation.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Iterate the sparse diverged set in ascending `(from_idx, to_idx)`
    /// order. Consumers that serialize or replicate it can therefore retain a
    /// stable order without allocating a sorted copy.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = (usize, usize, f32)> + '_ {
        self.overrides
            .iter()
            .map(|override_| (override_.from, override_.to, override_.current))
    }

    /// Generic decay substrate for the fixed-tick owner. This type knows only
    /// how to move an already-diverged value toward a supplied baseline; rate
    /// authoring and tick scheduling remain outside the entities crate.
    pub fn decay_step(
        &mut self,
        tick_dt: f32,
        mut rate_for: impl FnMut(usize, usize) -> f32,
        mut baseline_for: impl FnMut(usize, usize) -> f32,
    ) {
        if !tick_dt.is_finite() || tick_dt <= 0.0 {
            return;
        }

        let mut changed = false;
        let mut index = 0;
        while index < self.overrides.len() {
            let entry = self.overrides[index];
            let rate = rate_for(entry.from, entry.to);
            let baseline = baseline_for(entry.from, entry.to);
            if !rate.is_finite()
                || rate <= 0.0
                || !baseline.is_finite()
                || !entry.current.is_finite()
            {
                index += 1;
                continue;
            }

            let distance = baseline - entry.current;
            let step = rate * tick_dt;
            if distance.abs() <= SENTIMENT_DECAY_BASELINE_EPSILON || distance.abs() <= step {
                self.overrides.remove(index);
                changed = true;
            } else {
                let stepped = entry.current + distance.signum() * step;
                // `rate * dt` can be smaller than one ULP of an unclamped,
                // far-from-baseline sentiment. In that case ordinary f32
                // addition returns `current` unchanged forever. A one-ULP
                // floor preserves monotonic motion for every positive rate;
                // the normal linear step remains unchanged whenever it is
                // representable.
                let next = if stepped == entry.current {
                    adjacent_finite_f32(entry.current, distance.is_sign_positive())
                } else {
                    stepped
                };
                if (baseline - next).abs() <= SENTIMENT_DECAY_BASELINE_EPSILON {
                    self.overrides.remove(index);
                    changed = true;
                } else {
                    self.overrides[index].current = next;
                    changed = true;
                    index += 1;
                }
            }
        }
        if changed {
            self.mark_mutated();
        }
    }

    fn resolved_pair(
        &self,
        from: f32,
        to: f32,
    ) -> Result<(usize, usize), FactionSentimentMutationError> {
        let from =
            faction_index(from).ok_or(FactionSentimentMutationError::InvalidFactionIndex(from))?;
        let to = faction_index(to).ok_or(FactionSentimentMutationError::InvalidFactionIndex(to))?;
        if from > MAX_FACTION_SENTIMENT_INDEX {
            return Err(FactionSentimentMutationError::FactionIndexNotWireRepresentable(from));
        }
        if to > MAX_FACTION_SENTIMENT_INDEX {
            return Err(FactionSentimentMutationError::FactionIndexNotWireRepresentable(to));
        }
        Ok((from, to))
    }

    fn get_resolved(&self, pair: (usize, usize)) -> Option<f32> {
        self.overrides
            .binary_search_by_key(&pair, |override_| (override_.from, override_.to))
            .ok()
            .map(|index| self.overrides[index].current)
    }

    fn set_resolved(
        &mut self,
        pair: (usize, usize),
        value: f32,
        baseline: f32,
    ) -> Result<bool, FactionSentimentMutationError> {
        if !value.is_finite() || !baseline.is_finite() {
            return Err(FactionSentimentMutationError::NonFiniteValue);
        }
        let changed = match self
            .overrides
            .binary_search_by_key(&pair, |override_| (override_.from, override_.to))
        {
            Ok(index) if value == baseline => {
                self.overrides.remove(index);
                true
            }
            Ok(index) if self.overrides[index].current == value => false,
            Ok(index) => {
                self.overrides[index].current = value;
                true
            }
            Err(_) if value == baseline => false,
            Err(index) => {
                if self.overrides.len() >= MAX_FACTION_SENTIMENT_OVERRIDES {
                    return Err(FactionSentimentMutationError::CapacityExceeded);
                }
                self.overrides.insert(
                    index,
                    FactionSentimentOverride {
                        from: pair.0,
                        to: pair.1,
                        current: value,
                    },
                );
                true
            }
        };
        if changed {
            self.mark_mutated();
        }
        Ok(changed)
    }

    fn mark_mutated(&mut self) {
        self.generation = NEXT_FACTION_SENTIMENT_GENERATION.fetch_add(1, Ordering::Relaxed);
    }
}

impl<'a> LiveFactionSentiment<'a> {
    pub fn new(baseline: &'a FactionRegistry, overlay: &'a FactionSentimentState) -> Self {
        Self { baseline, overlay }
    }

    /// Construct a baseline-only live view for focused read-path tests. Normal
    /// runtime AI construction always supplies the session-owned overlay.
    pub fn with_empty_overlay(baseline: &'a FactionRegistry) -> Self {
        static EMPTY_OVERLAY: FactionSentimentState = FactionSentimentState {
            overrides: Vec::new(),
            generation: 0,
        };
        Self::new(baseline, &EMPTY_OVERLAY)
    }

    /// Sentiment resolves the live sparse value first, then the authored
    /// baseline when no runtime write has diverged this pair.
    pub fn sentiment(&self, from: f32, to: f32) -> f32 {
        self.overlay
            .get(from, to)
            .unwrap_or_else(|| self.baseline.sentiment(from, to))
    }

    /// Tolerance remains immutable authored content for this feature.
    pub fn tolerance(&self, from: f32, to: f32) -> Option<f32> {
        self.baseline.tolerance(from, to)
    }

    /// Construct the complete relationship explicitly: delegating to the
    /// baseline relationship here would accidentally reintroduce stale
    /// baseline sentiment for an overridden pair.
    pub fn relationship(&self, from: f32, to: f32) -> FactionRelationship {
        FactionRelationship {
            sentiment: self.sentiment(from, to),
            tolerance: self.tolerance(from, to),
            decay: self.baseline.relationship(from, to).decay,
        }
    }
}

/// Borrowed faction content used by compatibility hashing.
///
/// Construction and iteration exhaustively bind the registry's private state,
/// so a new behavior field requires an explicit compatibility decision.
pub struct FactionCompatibilitySnapshot<'a> {
    descriptors: &'a [FactionDescriptor],
    relationship_overrides: &'a [FactionRelationshipOverride],
    faction_sentiment_decay: f32,
}

impl<'a> FactionCompatibilitySnapshot<'a> {
    pub fn descriptors(&self) -> &'a [FactionDescriptor] {
        self.descriptors
    }

    /// Authored sparse relationship overrides in ascending `(from, to)` order.
    pub fn relationships(
        &self,
    ) -> impl ExactSizeIterator<Item = (usize, usize, FactionRelationship)> + 'a {
        let relationship_overrides = self.relationship_overrides;
        relationship_overrides.iter().map(|relationship_override| {
            let FactionRelationshipOverride {
                from,
                to,
                relationship,
            } = relationship_override;
            (*from, *to, *relationship)
        })
    }

    pub fn faction_sentiment_decay(&self) -> f32 {
        self.faction_sentiment_decay
    }
}

impl FactionRegistry {
    pub fn from_descriptors(descriptors: Vec<FactionDescriptor>) -> Result<Self, String> {
        let mut names = HashSet::with_capacity(descriptors.len());
        for descriptor in &descriptors {
            if descriptor.name.is_empty() {
                return Err("faction name must be a non-empty string".to_string());
            }
            if descriptor
                .name
                .starts_with(ENGINE_RESERVED_FACTION_NAME_PREFIX)
            {
                return Err(format!(
                    "faction name `{}` must not use reserved engine namespace `{ENGINE_RESERVED_FACTION_NAME_PREFIX}`",
                    descriptor.name
                ));
            }
            if !names.insert(descriptor.name.as_str()) {
                return Err(format!("duplicate faction name `{}`", descriptor.name));
            }
        }
        // A f32 exactly represents every integer in this range. Reserving the
        // two built-ins keeps authored values from colliding with player/default
        // semantics even when the registry is later read directly by the AI.
        if descriptors.len() > MAX_EXACT_FACTION_INDEX - FIRST_AUTHORED_FACTION_INDEX as usize + 1 {
            return Err("too many authored factions for f32 index storage".to_string());
        }
        Ok(Self {
            descriptors,
            relationship_overrides: Vec::new(),
            faction_sentiment_decay: 0.0,
        })
    }

    /// Set the manifest-wide sentiment decay default. A zero rate holds every
    /// pair without an authored per-pair override.
    pub fn with_sentiment_decay(mut self, decay: f32) -> Result<Self, String> {
        if !decay.is_finite() || decay < 0.0 {
            return Err("faction sentiment decay must be finite and >= 0".to_string());
        }
        self.faction_sentiment_decay = decay;
        Ok(self)
    }

    /// Resolve strict manifest-authored directional relationships into the
    /// sparse faction-index overrides. Both endpoint names must name declared
    /// factions; player and default-enemy compatibility rows use the unlisted
    /// pair fallback.
    pub fn with_sentiments(
        mut self,
        sentiments: impl AsRef<[FactionSentimentDescriptor]>,
    ) -> Result<Self, String> {
        for entry in sentiments.as_ref() {
            if !entry.sentiment.is_finite() {
                return Err(format!(
                    "sentiment from `{}` to `{}` must be finite",
                    entry.from_faction, entry.to_faction
                ));
            }
            if !entry.tolerance.is_finite() {
                return Err(format!(
                    "tolerance from `{}` to `{}` must be finite",
                    entry.from_faction, entry.to_faction
                ));
            }
            if let Some(decay) = entry.decay
                && (!decay.is_finite() || decay < 0.0)
            {
                return Err(format!(
                    "decay from `{}` to `{}` must be finite and >= 0",
                    entry.from_faction, entry.to_faction
                ));
            }
            let from = self.index_for_name(&entry.from_faction).ok_or_else(|| {
                format!(
                    "sentiment references undeclared from faction `{}`",
                    entry.from_faction
                )
            })?;
            let to = self.index_for_name(&entry.to_faction).ok_or_else(|| {
                format!(
                    "sentiment references undeclared to faction `{}`",
                    entry.to_faction
                )
            })?;
            let pair = (
                faction_index(from).expect("declared faction index is exact"),
                faction_index(to).expect("declared faction index is exact"),
            );
            match self
                .relationship_overrides
                .binary_search_by_key(&pair, |override_| (override_.from, override_.to))
            {
                Ok(_) => {
                    return Err(format!(
                        "duplicate sentiment entry from `{}` to `{}`",
                        entry.from_faction, entry.to_faction
                    ));
                }
                Err(index) => self.relationship_overrides.insert(
                    index,
                    FactionRelationshipOverride {
                        from: pair.0,
                        to: pair.1,
                        relationship: FactionRelationship {
                            sentiment: entry.sentiment,
                            tolerance: Some(entry.tolerance),
                            decay: entry.decay,
                        },
                    },
                ),
            }
        }
        Ok(self)
    }

    /// Resolve an authored stable name to its entity-state scalar.
    pub fn index_for_name(&self, name: &str) -> Option<f32> {
        self.descriptors
            .iter()
            .position(|descriptor| descriptor.name == name)
            .map(|offset| FIRST_AUTHORED_FACTION_INDEX + offset as f32)
    }

    pub fn descriptors(&self) -> &[FactionDescriptor] {
        &self.descriptors
    }

    /// Authoritative borrowed content for peer compatibility hashing.
    pub fn compatibility_snapshot(&self) -> FactionCompatibilitySnapshot<'_> {
        let Self {
            descriptors,
            relationship_overrides,
            faction_sentiment_decay,
        } = self;
        FactionCompatibilitySnapshot {
            descriptors,
            relationship_overrides,
            faction_sentiment_decay: *faction_sentiment_decay,
        }
    }

    /// Sentiment from the evaluating faction toward a candidate faction.
    /// Unlisted rows preserve the prior faction-inequality behavior exactly:
    /// equal indices are neutral, different indices are hostile.
    pub fn sentiment(&self, from: f32, to: f32) -> f32 {
        self.relationship(from, to).sentiment
    }

    /// Pair tolerance only when the authored relationship declared one.
    pub fn tolerance(&self, from: f32, to: f32) -> Option<f32> {
        self.relationship(from, to).tolerance
    }

    /// Resolve the authored per-pair decay override first, then the global
    /// manifest default. Missing content therefore resolves to zero (hold).
    pub fn sentiment_decay(&self, from: f32, to: f32) -> f32 {
        self.relationship(from, to)
            .decay
            .unwrap_or(self.faction_sentiment_decay)
    }

    /// Global manifest default, exposed for compatibility hashing and tests.
    pub fn faction_sentiment_decay(&self) -> f32 {
        self.faction_sentiment_decay
    }

    pub fn relationship(&self, from: f32, to: f32) -> FactionRelationship {
        self.resolved_pair(from, to)
            .and_then(|pair| {
                self.relationship_overrides
                    .binary_search_by_key(&pair, |override_| (override_.from, override_.to))
                    .ok()
            })
            .map(|index| self.relationship_overrides[index].relationship)
            .unwrap_or(FactionRelationship {
                sentiment: if from == to {
                    SAME_FACTION_DEFAULT_SENTIMENT
                } else {
                    CROSS_FACTION_DEFAULT_SENTIMENT
                },
                tolerance: None,
                decay: None,
            })
    }

    fn resolved_pair(&self, from: f32, to: f32) -> Option<(usize, usize)> {
        let from = faction_index(from)?;
        let to = faction_index(to)?;
        let faction_count = self.descriptors.len() + FIRST_AUTHORED_FACTION_INDEX as usize;
        (from < faction_count && to < faction_count).then_some((from, to))
    }
}

fn faction_index(index: f32) -> Option<usize> {
    if !index.is_finite() || index < 0.0 || index.fract() != 0.0 {
        return None;
    }
    let resolved = index as usize;
    (resolved as f32 == index).then_some(resolved)
}

/// Data registries collected from script execution.
/// `reactions`, `crossings`, `trigger_events`, and `trigger_pools` are per-level
/// and cleared on unload; entity, faction, map, and global
/// reaction/crossing/trigger-event/trigger-pool definitions survive level unload.
#[derive(Debug, Default)]
pub struct DataRegistry {
    /// Active reactions for this level after composing matching mod-global
    /// definitions with level-local definitions. Existing dispatch reads here.
    pub reactions: Vec<NamedReaction>,
    /// Active state-crossing watchers for this level after composition (M13 HUD
    /// dynamics). Per-level — cleared on unload with `reactions`. The crossing
    /// detector reads these to know which slots to watch.
    pub crossings: Vec<CrossingDescriptor>,
    pub trigger_events: Vec<TriggerEventDescriptor>,
    trigger_pools: Vec<TriggerPoolDescriptor>,
    /// Engine-global reaction definitions from `ModManifest.reactions`.
    /// These are durable definitions, not the currently active per-level set.
    pub global_reactions: Vec<ScopedReaction>,
    /// Engine-global crossing definitions from `ModManifest.crossings`.
    /// These are durable definitions, not the currently active per-level set.
    pub global_crossings: Vec<ScopedCrossing>,
    pub global_trigger_events: Vec<ScopedTriggerEvent>,
    pub global_trigger_pools: Vec<ScopedTriggerPool>,
    /// Level-local reaction definitions from `setupLevel()`. Retained so a
    /// staged mod-init reload can recompose active globals without rerunning the
    /// level data script.
    level_reactions: Vec<NamedReaction>,
    /// Level-local crossing definitions from `setupLevel()`. Retained for the
    /// same staged-reload recomposition path as [`Self::level_reactions`].
    level_crossings: Vec<CrossingDescriptor>,
    level_trigger_events: Vec<TriggerEventDescriptor>,
    level_trigger_pools: Vec<TriggerPoolDescriptor>,
    /// Entity-type descriptors. Engine-global — survive level unload.
    /// Populated by the boot caller after `run_mod_init`: it drains the
    /// `entities` field of the validated mod manifest into here
    /// via [`Self::upsert_entity_type`]. Read by the data-archetype spawn
    /// sweep. Not populated from `setupLevel()`.
    pub entities: Vec<EntityTypeDescriptor>,
    /// Generation identity for the committed entity-descriptor snapshot.
    /// Consumers of derived descriptor data use this to refresh once per
    /// registry change without fingerprinting descriptors in a hot path.
    entity_types_generation: u64,
    /// Manifest-authored named faction registry. Engine-global like entity
    /// descriptors: level unload clears no faction declarations or indices.
    pub factions: FactionRegistry,
    /// Mod map catalog entries. Engine-global — survive level unload.
    /// Populated by the boot caller from `ModManifest.maps` so the
    /// frontend and catalog-id load path can discover maps before a level is
    /// loaded. Not populated from `setupLevel()`.
    pub maps: Vec<ModMapEntry>,
    /// Optional mod-global first-person weapon-placement default. Engine-global
    /// content committed with descriptor snapshots and resolved at the render
    /// seam; never copied into session-local presentation state.
    pub default_weapon_placement: Option<WeaponPlacementDescriptor>,
}

impl DataRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append level-local definitions, then recompose the active per-level
    /// reaction/crossing/trigger-event/trigger-pool sets from matching globals
    /// plus locals.
    /// Existing level-local definitions are preserved — call [`Self::clear`]
    /// first for a fresh population. Entity-type descriptors arrive separately
    /// via `ModManifest.entities` (they outlive level unload). UI trees are
    /// drained by the runtime caller and never enter `DataRegistry`.
    pub fn populate_level(
        &mut self,
        reactions: Vec<NamedReaction>,
        crossings: Vec<CrossingDescriptor>,
        tags: &[String],
    ) {
        self.populate_level_with_trigger_events(reactions, crossings, Vec::new(), Vec::new(), tags);
    }

    pub fn populate_level_with_trigger_events(
        &mut self,
        reactions: Vec<NamedReaction>,
        crossings: Vec<CrossingDescriptor>,
        trigger_events: Vec<TriggerEventDescriptor>,
        trigger_pools: Vec<TriggerPoolDescriptor>,
        tags: &[String],
    ) {
        self.set_level_reactions(reactions);
        self.set_level_crossings(crossings);
        self.level_trigger_events.extend(trigger_events);
        self.level_trigger_pools.extend(trigger_pools);
        self.recompose(tags);
    }

    /// Append level-local reaction definitions retained for recomposition.
    pub fn set_level_reactions(&mut self, reactions: Vec<NamedReaction>) {
        self.level_reactions.extend(reactions);
    }

    /// Append level-local crossing definitions retained for recomposition.
    pub fn set_level_crossings(&mut self, crossings: Vec<CrossingDescriptor>) {
        self.level_crossings.extend(crossings);
    }

    /// Rebuild active level-local sets from retained local definitions and
    /// matching globals.
    pub fn recompose(&mut self, tags: &[String]) {
        self.recompose_active_sets(tags);
    }

    /// Rebuild the active per-level sets from durable globals plus retained
    /// level-local definitions. Empty `levels` scopes match every level;
    /// otherwise matching is exact, case-sensitive intersection with `tags`.
    pub fn recompose_active_sets(&mut self, tags: &[String]) {
        let mut reactions: Vec<NamedReaction> = self
            .global_reactions
            .iter()
            .filter(|reaction| Self::levels_match(&reaction.levels, tags))
            .map(|reaction| reaction.reaction.clone())
            .collect();
        reactions.extend(self.level_reactions.iter().cloned());
        Self::warn_duplicate_reaction_names(&reactions);

        let mut crossings: Vec<CrossingDescriptor> = self
            .global_crossings
            .iter()
            .filter(|crossing| Self::levels_match(&crossing.levels, tags))
            .map(|crossing| crossing.crossing.clone())
            .collect();
        crossings.extend(self.level_crossings.iter().cloned());

        let mut trigger_events: Vec<TriggerEventDescriptor> = self
            .global_trigger_events
            .iter()
            .filter(|descriptor| Self::levels_match(&descriptor.levels, tags))
            .cloned()
            .collect();
        trigger_events.extend(
            self.level_trigger_events
                .iter()
                .filter(|descriptor| Self::levels_match(&descriptor.levels, tags))
                .cloned(),
        );
        let mut seen = Vec::new();
        trigger_events.retain(|descriptor| {
            if seen.contains(descriptor) {
                log::warn!("[Loader] duplicate trigger-event descriptor for tag `{}` event `{}`; ignoring duplicate", descriptor.tag, descriptor.event);
                false
            } else {
                seen.push(descriptor.clone());
                true
            }
        });

        let matching_global_pools: Vec<&ScopedTriggerPool> = self
            .global_trigger_pools
            .iter()
            .filter(|descriptor| Self::levels_match(&descriptor.levels, tags))
            .collect();
        let level_pool_tags: HashSet<&str> = self
            .level_trigger_pools
            .iter()
            .map(|descriptor| descriptor.tag.as_str())
            .collect();
        for pool in &self.level_trigger_pools {
            if matching_global_pools
                .iter()
                .any(|global| global.tag == pool.tag)
            {
                log::info!(
                    "[Loader] level-local trigger pool `{}` overrides matching mod-global pool",
                    pool.tag,
                );
            }
        }
        let mut trigger_pools: Vec<TriggerPoolDescriptor> = matching_global_pools
            .into_iter()
            .filter(|descriptor| !level_pool_tags.contains(descriptor.tag.as_str()))
            .cloned()
            .collect();
        // Level-local pools always apply to the level whose setupLevel() script
        // declared them. Their retained `levels` field scopes only mod globals.
        trigger_pools.extend(self.level_trigger_pools.iter().cloned());

        self.reactions = reactions;
        self.crossings = crossings;
        self.trigger_events = trigger_events;
        self.trigger_pools = trigger_pools;
    }

    fn levels_match(levels: &[String], tags: &[String]) -> bool {
        levels.is_empty()
            || levels
                .iter()
                .any(|level| tags.iter().any(|tag| tag == level))
    }

    fn warn_duplicate_reaction_names(reactions: &[NamedReaction]) {
        let mut seen = HashSet::new();
        for reaction in reactions {
            if !seen.insert(reaction.name.as_str()) {
                log::warn!(
                    "[Loader] duplicate active reaction name `{}` in composed reaction set; all matching reactions will fire",
                    reaction.name,
                );
            }
        }
    }

    /// Insert (or overwrite) an entity-type descriptor. Identical re-inserts
    /// keyed on `canonical_name` are silent no-ops; differing re-inserts
    /// overwrite and log at `debug!`. Descriptors with `canonical_name = None`
    /// are always appended — they have no addressable name to dedup against.
    /// Survives level unload — only invoke from the mod-init path (after
    /// mod manifest commits), not during per-level data-script execution.
    pub fn upsert_entity_type(&mut self, descriptor: EntityTypeDescriptor) {
        let descriptor_name = descriptor.canonical_name.clone();
        if let Some(name) = descriptor_name.as_deref() {
            if let Some(existing) = self
                .entities
                .iter_mut()
                .find(|e| e.canonical_name.as_deref() == Some(name))
            {
                if *existing == descriptor {
                    return;
                }
                log::debug!(
                    "[Loader] upsert_entity_type: overwriting existing descriptor for `{}`",
                    name,
                );
                *existing = descriptor;
                self.entity_types_generation = self.entity_types_generation.wrapping_add(1);
                return;
            }
        }
        self.entities.push(descriptor);
        self.entity_types_generation = self.entity_types_generation.wrapping_add(1);
    }

    /// Dedup a complete entity descriptor snapshot before hot-reload commit,
    /// keeping the LAST occurrence per `canonical_name` so the result matches
    /// startup's `upsert_entity_type` last-write-wins (where a descriptor
    /// spread later in `ModManifest.entities` overwrites an earlier one
    /// with the same name). Each collision logs at `warn!`. Descriptors with
    /// no `canonical_name` pass through untouched. Surviving entries keep their
    /// last-appearance order.
    pub fn dedup_entity_type_snapshot(
        descriptors: Vec<EntityTypeDescriptor>,
    ) -> Vec<EntityTypeDescriptor> {
        // First pass records the last index each name appears at; second pass
        // keeps only that occurrence, preserving last-appearance order.
        let mut last_index: HashMap<String, usize> = HashMap::new();
        for (index, descriptor) in descriptors.iter().enumerate() {
            if let Some(name) = descriptor.canonical_name.as_deref() {
                if last_index.insert(name.to_string(), index).is_some() {
                    log::warn!(
                        "[Loader] duplicate entity descriptor canonicalName `{name}` in replacement snapshot; later declaration wins"
                    );
                }
            }
        }
        descriptors
            .into_iter()
            .enumerate()
            .filter(
                |(index, descriptor)| match descriptor.canonical_name.as_deref() {
                    Some(name) => last_index.get(name) == Some(index),
                    None => true,
                },
            )
            .map(|(_, descriptor)| descriptor)
            .collect()
    }

    /// Replace the engine-global descriptor snapshot as one complete commit.
    /// Used by dev hot reload after a staged manifest has validated and its
    /// live refresh plan has applied. Startup may continue to use upsert.
    /// Duplicate `canonical_name`s are deduped last-write-wins to match
    /// startup, so this is infallible.
    pub fn replace_entity_types(&mut self, descriptors: Vec<EntityTypeDescriptor>) {
        self.entities = Self::dedup_entity_type_snapshot(descriptors);
        self.entity_types_generation = self.entity_types_generation.wrapping_add(1);
    }

    /// Replace the complete committed faction snapshot. Startup accepts any
    /// validated declaration order. Staged reload verifies that the name-to-index
    /// mapping is unchanged before calling this, while still allowing authored
    /// relationship overrides to refresh.
    pub fn replace_factions(&mut self, factions: FactionRegistry) {
        self.factions = factions;
    }

    /// Identity of the current complete entity-descriptor snapshot.
    pub fn entity_types_generation(&self) -> u64 {
        self.entity_types_generation
    }

    /// Replace the engine-global map catalog snapshot as one complete commit.
    /// Used by startup and successful staged mod-init commits.
    pub fn replace_maps(&mut self, maps: Vec<ModMapEntry>) {
        self.maps = maps;
    }

    /// Replace the mod-global first-person weapon-placement default as part of
    /// a successful manifest snapshot commit. `None` clears a prior default
    /// when the replacement manifest omits the field.
    pub fn set_default_weapon_placement(&mut self, placement: Option<WeaponPlacementDescriptor>) {
        self.default_weapon_placement = placement;
    }

    /// Replace the engine-global reaction definition snapshot as one complete
    /// commit. No dedupe: same-name collisions are preserved intentionally.
    pub fn replace_global_reactions(&mut self, reactions: Vec<ScopedReaction>) {
        self.global_reactions = reactions;
    }

    /// Replace the engine-global crossing definition snapshot as one complete
    /// commit. No dedupe: collisions are preserved intentionally.
    pub fn replace_global_crossings(&mut self, crossings: Vec<ScopedCrossing>) {
        self.global_crossings = crossings;
    }

    pub fn replace_global_trigger_events(&mut self, events: Vec<ScopedTriggerEvent>) {
        self.global_trigger_events = events;
    }

    pub fn replace_global_trigger_pools(&mut self, pools: Vec<ScopedTriggerPool>) {
        self.global_trigger_pools = pools;
    }

    /// Active pools for the installed level, ordered as matching mod globals
    /// followed by level locals after same-tag overrides are applied.
    pub fn trigger_pools(&self) -> &[TriggerPoolDescriptor] {
        &self.trigger_pools
    }

    /// Drop every active per-level reaction/crossing/trigger-event/trigger-pool
    /// definition. Engine-global entity, faction, map, and global reaction/crossing/
    /// trigger-event/trigger-pool definitions outlive the clear. Called on level unload.
    /// See [`Self::upsert_entity_type`].
    pub fn clear(&mut self) {
        self.reactions.clear();
        self.crossings.clear();
        self.trigger_events.clear();
        self.trigger_pools.clear();
        self.level_reactions.clear();
        self.level_crossings.clear();
        self.level_trigger_events.clear();
        self.level_trigger_pools.clear();
    }

    /// Returns `true` only when every registry collection is empty. After level
    /// unload, engine-global definitions, `entities`, or `maps` may still be
    /// populated, so this returns `false` — production code should use
    /// `reactions.is_empty()` for level-unload checks.
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.reactions.is_empty()
            && self.crossings.is_empty()
            && self.trigger_events.is_empty()
            && self.trigger_pools.is_empty()
            && self.global_reactions.is_empty()
            && self.global_crossings.is_empty()
            && self.global_trigger_events.is_empty()
            && self.global_trigger_pools.is_empty()
            && self.level_reactions.is_empty()
            && self.level_crossings.is_empty()
            && self.level_trigger_events.is_empty()
            && self.level_trigger_pools.is_empty()
            && self.entities.is_empty()
            && self.factions.descriptors().is_empty()
            && self.maps.is_empty()
            && self.default_weapon_placement.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_descriptors::{
        CrossingCondition, CrossingDescriptor, EntityTypeDescriptor, NamedReaction,
        PrimitiveDescriptor, ReactionDescriptor, TriggerPoolArm, TriggerPoolDescriptor,
    };

    fn sample_level_reactions() -> Vec<NamedReaction> {
        vec![NamedReaction {
            name: "wave1Complete".to_string(),
            descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                primitive: "moveGeometry".to_string(),
                target: None,
                tag: Some("reactorChambers".to_string()),
                on_complete: None,
                args: serde_json::Value::Object(Default::default()),
            }),
        }]
    }

    fn grunt_descriptor() -> EntityTypeDescriptor {
        EntityTypeDescriptor {
            faction: None,
            tolerance: None,
            canonical_name: Some("grunt".to_string()),
            inventory: None,
            light: None,
            emitter: None,
            movement: None,
            weapon: None,
            touchable: None,
            mesh: None,
            health: None,
            behavior: None,
        }
    }

    fn sample_map(id: &str) -> ModMapEntry {
        ModMapEntry {
            id: id.to_string(),
            path: format!("maps/{id}.prl"),
            name: id.to_string(),
            tags: vec!["campaign".to_string()],
        }
    }

    fn sample_trigger_event() -> TriggerEventDescriptor {
        TriggerEventDescriptor {
            tag: "airlock".to_string(),
            event: "enter".to_string(),
            fire: vec!["openAirlock".to_string()],
            levels: Vec::new(),
        }
    }

    fn sample_trigger_pool(tag: &str, levels: &[&str]) -> TriggerPoolDescriptor {
        TriggerPoolDescriptor {
            tag: tag.to_string(),
            arm: TriggerPoolArm::Count(2),
            levels: levels.iter().map(|level| level.to_string()).collect(),
        }
    }

    fn sample_global_reaction(name: &str) -> ScopedReaction {
        sample_scoped_reaction(name, &["campaign"])
    }

    fn sample_scoped_reaction(name: &str, levels: &[&str]) -> ScopedReaction {
        sample_scoped_reaction_with_on_complete(name, levels, None)
    }

    fn sample_scoped_reaction_with_on_complete(
        name: &str,
        levels: &[&str],
        on_complete: Option<&str>,
    ) -> ScopedReaction {
        ScopedReaction {
            reaction: NamedReaction {
                name: name.to_string(),
                descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                    primitive: "moveGeometry".to_string(),
                    target: None,
                    tag: Some("reactorChambers".to_string()),
                    on_complete: on_complete.map(str::to_string),
                    args: serde_json::Value::Object(Default::default()),
                }),
            },
            levels: levels.iter().map(|level| level.to_string()).collect(),
        }
    }

    fn sample_global_crossing(slot: &str) -> ScopedCrossing {
        sample_scoped_crossing(slot, &["campaign"])
    }

    fn sample_scoped_crossing(slot: &str, levels: &[&str]) -> ScopedCrossing {
        ScopedCrossing {
            crossing: CrossingDescriptor {
                slot: Some(slot.to_string()),
                condition: CrossingCondition::Below { threshold: 0.5 },
                max: 1.0,
                edge: None,
                fire: vec!["lowHealth".to_string()],
            },
            levels: levels.iter().map(|level| level.to_string()).collect(),
        }
    }

    fn tags(tags: &[&str]) -> Vec<String> {
        tags.iter().map(|tag| tag.to_string()).collect()
    }

    fn reaction_names(registry: &DataRegistry) -> Vec<String> {
        registry
            .reactions
            .iter()
            .map(|reaction| reaction.name.clone())
            .collect()
    }

    fn crossing_slots(registry: &DataRegistry) -> Vec<String> {
        registry
            .crossings
            .iter()
            .map(|crossing| crossing.slot.clone().expect("sample crossings have slots"))
            .collect()
    }

    fn trigger_pool_tags(registry: &DataRegistry) -> Vec<String> {
        registry
            .trigger_pools()
            .iter()
            .map(|pool| pool.tag.clone())
            .collect()
    }

    #[test]
    fn new_registry_is_empty() {
        let r = DataRegistry::new();
        assert!(r.is_empty());
    }

    #[test]
    fn populate_appends_manifest_entries() {
        let mut r = DataRegistry::new();
        r.populate_level(sample_level_reactions(), Vec::new(), &[]);
        assert_eq!(r.reactions.len(), 1);
        assert!(!r.is_empty());
    }

    #[test]
    fn trigger_event_collections_make_registry_nonempty() {
        let trigger_event = sample_trigger_event();
        let trigger_pool = sample_trigger_pool("closet", &[]);

        let mut registry = DataRegistry::new();
        registry.trigger_events.push(trigger_event.clone());
        assert!(!registry.is_empty());

        let mut registry = DataRegistry::new();
        registry.global_trigger_events.push(trigger_event.clone());
        assert!(!registry.is_empty());

        let mut registry = DataRegistry::new();
        registry.level_trigger_events.push(trigger_event);
        assert!(!registry.is_empty());

        let mut registry = DataRegistry::new();
        registry.trigger_pools.push(trigger_pool.clone());
        assert!(!registry.is_empty());

        let mut registry = DataRegistry::new();
        registry.global_trigger_pools.push(trigger_pool.clone());
        assert!(!registry.is_empty());

        let mut registry = DataRegistry::new();
        registry.level_trigger_pools.push(trigger_pool);
        assert!(!registry.is_empty());
    }

    #[test]
    fn trigger_pools_compose_matching_globals_before_unfiltered_level_locals() {
        let mut registry = DataRegistry::new();
        registry.replace_global_trigger_pools(vec![
            sample_trigger_pool("campaign", &["campaign"]),
            sample_trigger_pool("deathmatch", &["deathmatch"]),
            sample_trigger_pool("all-levels", &[]),
        ]);
        registry.populate_level_with_trigger_events(
            Vec::new(),
            Vec::new(),
            Vec::new(),
            vec![sample_trigger_pool("local", &["not-active"])],
            &tags(&["campaign"]),
        );

        assert_eq!(
            trigger_pool_tags(&registry),
            ["campaign", "all-levels", "local"],
            "only mod-global pools use their levels selector; locals retain declaration order",
        );
    }

    #[test]
    fn level_local_trigger_pool_replaces_matching_global_with_the_same_tag() {
        let mut registry = DataRegistry::new();
        registry.replace_global_trigger_pools(vec![
            sample_trigger_pool("shared", &["campaign"]),
            sample_trigger_pool("global-only", &["campaign"]),
        ]);
        let local = TriggerPoolDescriptor {
            tag: "shared".to_string(),
            arm: TriggerPoolArm::Percentage(50.0),
            levels: vec!["ignored-at-level-scope".to_string()],
        };
        registry.populate_level_with_trigger_events(
            Vec::new(),
            Vec::new(),
            Vec::new(),
            vec![local.clone()],
            &tags(&["campaign"]),
        );

        assert_eq!(trigger_pool_tags(&registry), ["global-only", "shared"]);
        assert_eq!(registry.trigger_pools()[1], local);
    }

    #[test]
    fn clear_drops_level_trigger_pools_but_retains_globals_for_next_install() {
        let mut registry = DataRegistry::new();
        let global = sample_trigger_pool("global", &[]);
        registry.replace_global_trigger_pools(vec![global.clone()]);
        registry.populate_level_with_trigger_events(
            Vec::new(),
            Vec::new(),
            Vec::new(),
            vec![sample_trigger_pool("local", &[])],
            &[],
        );

        registry.clear();

        assert!(registry.trigger_pools().is_empty());
        assert_eq!(registry.global_trigger_pools, vec![global.clone()]);

        registry.populate_level_with_trigger_events(
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            &[],
        );
        assert_eq!(registry.trigger_pools(), [global]);
    }

    #[test]
    fn populate_composes_unscoped_global_reactions_into_every_level() {
        let mut r = DataRegistry::new();
        r.replace_global_reactions(vec![sample_scoped_reaction("globalLoad", &[])]);

        r.populate_level(sample_level_reactions(), Vec::new(), &tags(&["deathmatch"]));

        assert_eq!(
            reaction_names(&r),
            vec!["globalLoad".to_string(), "wave1Complete".to_string()]
        );
    }

    #[test]
    fn populate_filters_scoped_global_reactions_by_exact_tag_intersection() {
        let mut r = DataRegistry::new();
        r.replace_global_reactions(vec![
            sample_scoped_reaction("campaignLoad", &["campaign"]),
            sample_scoped_reaction("deathmatchLoad", &["deathmatch"]),
            sample_scoped_reaction("caseMismatchLoad", &["Campaign"]),
        ]);

        r.populate_level(Vec::new(), Vec::new(), &tags(&["campaign"]));

        assert_eq!(reaction_names(&r), vec!["campaignLoad".to_string()]);
    }

    #[test]
    fn recompose_isolates_disjoint_campaign_and_deathmatch_scopes() {
        let mut r = DataRegistry::new();
        r.replace_global_reactions(vec![
            sample_scoped_reaction("campaignLoad", &["campaign"]),
            sample_scoped_reaction("deathmatchLoad", &["deathmatch"]),
        ]);

        r.populate_level(Vec::new(), Vec::new(), &tags(&["campaign"]));
        assert_eq!(reaction_names(&r), vec!["campaignLoad".to_string()]);

        r.clear();
        r.populate_level(Vec::new(), Vec::new(), &tags(&["deathmatch"]));
        assert_eq!(reaction_names(&r), vec!["deathmatchLoad".to_string()]);
    }

    #[test]
    fn populate_appends_level_local_reactions_after_matching_globals() {
        let mut r = DataRegistry::new();
        r.replace_global_reactions(vec![sample_scoped_reaction("globalLoad", &["campaign"])]);

        r.populate_level(
            sample_level_reactions(),
            Vec::new(),
            &tags(&["campaign", "intro"]),
        );

        assert_eq!(
            reaction_names(&r),
            vec!["globalLoad".to_string(), "wave1Complete".to_string()]
        );
    }

    #[test]
    fn populate_preserves_same_name_active_reactions() {
        let mut r = DataRegistry::new();
        r.replace_global_reactions(vec![sample_scoped_reaction("levelLoad", &[])]);

        r.populate_level(
            vec![NamedReaction {
                name: "levelLoad".to_string(),
                descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                    primitive: "activateGroup".to_string(),
                    target: None,
                    tag: Some("local".to_string()),
                    on_complete: None,
                    args: serde_json::Value::Object(Default::default()),
                }),
            }],
            Vec::new(),
            &tags(&["campaign"]),
        );
        assert_eq!(
            reaction_names(&r),
            vec!["levelLoad".to_string(), "levelLoad".to_string()]
        );
    }

    #[test]
    fn populate_composes_crossings_from_matching_globals_and_level_locals() {
        let mut r = DataRegistry::new();
        r.replace_global_crossings(vec![
            sample_scoped_crossing("global.health", &["campaign"]),
            sample_scoped_crossing("deathmatch.health", &["deathmatch"]),
            sample_scoped_crossing("global.ammo", &[]),
        ]);

        r.populate_level(
            Vec::new(),
            vec![sample_global_crossing("local.health").crossing],
            &tags(&["campaign"]),
        );

        assert_eq!(
            crossing_slots(&r),
            vec![
                "global.health".to_string(),
                "global.ammo".to_string(),
                "local.health".to_string(),
            ]
        );
    }

    #[test]
    fn replace_global_definitions_then_recompose_updates_active_sets() {
        let mut r = DataRegistry::new();
        let active_tags = tags(&["campaign"]);
        r.replace_global_reactions(vec![sample_scoped_reaction("oldGlobal", &[])]);
        r.replace_global_crossings(vec![sample_scoped_crossing("old.health", &[])]);
        r.populate_level(sample_level_reactions(), Vec::new(), &active_tags);

        r.replace_global_reactions(vec![
            sample_scoped_reaction("newCampaignGlobal", &["campaign"]),
            sample_scoped_reaction("newDeathmatchGlobal", &["deathmatch"]),
        ]);
        r.replace_global_crossings(vec![
            sample_scoped_crossing("new.health", &["campaign"]),
            sample_scoped_crossing("excluded.health", &["deathmatch"]),
        ]);
        r.recompose_active_sets(&active_tags);

        assert_eq!(
            reaction_names(&r),
            vec!["newCampaignGlobal".to_string(), "wave1Complete".to_string(),]
        );
        assert_eq!(crossing_slots(&r), vec!["new.health".to_string()]);
    }

    #[test]
    fn skipped_staged_replace_leaves_active_sets_unchanged() {
        let mut r = DataRegistry::new();
        let active_tags = tags(&["campaign"]);
        r.replace_global_reactions(vec![sample_scoped_reaction("oldGlobal", &[])]);
        r.replace_global_crossings(vec![sample_scoped_crossing("old.health", &[])]);
        r.populate_level(sample_level_reactions(), Vec::new(), &active_tags);
        let active_reactions_before = r.reactions.clone();
        let active_crossings_before = r.crossings.clone();
        let global_reactions_before = r.global_reactions.clone();
        let global_crossings_before = r.global_crossings.clone();
        let skipped_reactions = vec![sample_scoped_reaction("newGlobal", &[])];
        let skipped_crossings = vec![sample_scoped_crossing("new.health", &[])];

        // Simulates the stale/failed staged-manifest early-return path: the
        // candidate snapshot exists, but the registry receives no replacement.
        drop((skipped_reactions, skipped_crossings));
        r.recompose_active_sets(&active_tags);

        assert_eq!(r.global_reactions, global_reactions_before);
        assert_eq!(r.global_crossings, global_crossings_before);
        assert_eq!(r.reactions, active_reactions_before);
        assert_eq!(r.crossings, active_crossings_before);
    }

    #[test]
    fn clear_drops_reactions_but_keeps_entity_descriptors() {
        let mut r = DataRegistry::new();
        r.populate_level(sample_level_reactions(), Vec::new(), &[]);
        r.upsert_entity_type(grunt_descriptor());
        r.clear();
        assert_eq!(r.reactions.len(), 0);
        assert_eq!(r.entities.len(), 1, "entities survive level unload");
    }

    #[test]
    fn faction_registry_reserves_builtin_indices_and_preserves_manifest_order() {
        let factions = FactionRegistry::from_descriptors(vec![
            FactionDescriptor {
                name: "cabal".to_string(),
            },
            FactionDescriptor {
                name: "resistance".to_string(),
            },
        ])
        .expect("distinct non-empty faction names are valid");

        assert_eq!(PLAYER_FACTION_INDEX, 0.0);
        assert_eq!(DEFAULT_ENEMY_FACTION_INDEX, 1.0);
        assert_eq!(factions.index_for_name("cabal"), Some(2.0));
        assert_eq!(factions.index_for_name("resistance"), Some(3.0));
    }

    #[test]
    fn faction_registry_rejects_names_in_engine_reserved_persistence_namespace() {
        let error = FactionRegistry::from_descriptors(vec![FactionDescriptor {
            name: "@postretro.player".to_string(),
        }])
        .expect_err("authored names cannot collide with durable engine identities");

        assert!(error.contains(ENGINE_RESERVED_FACTION_NAME_PREFIX));
    }

    #[test]
    fn faction_compatibility_snapshot_preserves_authored_and_pair_order() {
        let factions = FactionRegistry::from_descriptors(vec![
            FactionDescriptor {
                name: "cabal".to_string(),
            },
            FactionDescriptor {
                name: "resistance".to_string(),
            },
            FactionDescriptor {
                name: "wild".to_string(),
            },
        ])
        .expect("valid faction declarations")
        .with_sentiments([
            FactionSentimentDescriptor {
                from_faction: "wild".to_string(),
                to_faction: "cabal".to_string(),
                sentiment: 0.25,
                tolerance: 8.0,
                decay: None,
            },
            FactionSentimentDescriptor {
                from_faction: "cabal".to_string(),
                to_faction: "resistance".to_string(),
                sentiment: -0.75,
                tolerance: 4.0,
                decay: None,
            },
        ])
        .expect("declared endpoint names resolve");

        let snapshot = factions.compatibility_snapshot();
        assert_eq!(
            snapshot
                .descriptors()
                .iter()
                .map(|descriptor| descriptor.name.as_str())
                .collect::<Vec<_>>(),
            vec!["cabal", "resistance", "wild"],
        );
        assert_eq!(
            snapshot.relationships().collect::<Vec<_>>(),
            vec![
                (
                    2,
                    3,
                    FactionRelationship {
                        sentiment: -0.75,
                        tolerance: Some(4.0),
                        decay: None,
                    },
                ),
                (
                    4,
                    2,
                    FactionRelationship {
                        sentiment: 0.25,
                        tolerance: Some(8.0),
                        decay: None,
                    },
                ),
            ],
        );
    }

    #[test]
    fn faction_registry_resolves_directional_sentiment_and_compatibility_defaults() {
        let factions = FactionRegistry::from_descriptors(vec![
            FactionDescriptor {
                name: "cabal".to_string(),
            },
            FactionDescriptor {
                name: "resistance".to_string(),
            },
        ])
        .expect("valid faction declarations")
        .with_sentiments(vec![
            FactionSentimentDescriptor {
                from_faction: "cabal".to_string(),
                to_faction: "resistance".to_string(),
                sentiment: -0.75,
                tolerance: 4.0,
                decay: None,
            },
            FactionSentimentDescriptor {
                from_faction: "resistance".to_string(),
                to_faction: "cabal".to_string(),
                sentiment: 0.0,
                tolerance: 9.0,
                decay: None,
            },
        ])
        .expect("declared endpoint names resolve");

        assert!((factions.sentiment(2.0, 3.0) - -0.75).abs() <= f32::EPSILON);
        assert_eq!(factions.sentiment(3.0, 2.0), 0.0);
        assert_eq!(factions.tolerance(2.0, 3.0), Some(4.0));
        assert_eq!(factions.tolerance(3.0, 2.0), Some(9.0));
        assert_eq!(factions.sentiment(2.0, 2.0), 0.0, "same defaults neutral");
        assert_eq!(
            factions.sentiment(PLAYER_FACTION_INDEX, DEFAULT_ENEMY_FACTION_INDEX),
            -1.0,
            "unlisted cross-faction pairs retain the prior hostile rule"
        );
        assert_eq!(factions.tolerance(2.0, 2.0), None);
        assert!(factions.faction_sentiment_decay().abs() <= f32::EPSILON);
        assert!(factions.sentiment_decay(2.0, 3.0).abs() <= f32::EPSILON);
    }

    #[test]
    fn faction_registry_allocates_only_authored_sparse_relationship_overrides() {
        let descriptors = (0..128)
            .map(|index| FactionDescriptor {
                name: format!("faction-{index}"),
            })
            .collect();
        let factions = FactionRegistry::from_descriptors(descriptors)
            .expect("many distinct faction declarations are valid");

        assert!(
            factions.relationship_overrides.is_empty(),
            "declaring factions must not eagerly allocate an N x N relationship matrix"
        );

        let factions = factions
            .with_sentiments([FactionSentimentDescriptor {
                from_faction: "faction-127".to_string(),
                to_faction: "faction-0".to_string(),
                sentiment: 0.5,
                tolerance: 3.0,
                decay: None,
            }])
            .expect("one sparse relationship override resolves");
        assert_eq!(factions.relationship_overrides.len(), 1);
        assert!((factions.sentiment(129.0, 2.0) - 0.5).abs() <= f32::EPSILON);
        assert_eq!(
            factions.sentiment(2.0, 129.0),
            -1.0,
            "the reverse directed pair remains on the compatibility default"
        );
    }

    #[test]
    fn faction_sentiment_state_tracks_only_sorted_pairs_diverged_from_baseline() {
        let mut state = FactionSentimentState::default();

        state.set(3.0, 2.0, -0.25, -1.0).unwrap();
        state.adjust(2.0, 3.0, 0.5, -1.0).unwrap();
        state.adjust(2.0, 3.0, 0.5, -1.0).unwrap();

        assert_eq!(
            state.iter().collect::<Vec<_>>(),
            vec![(2, 3, 0.0), (3, 2, -0.25)],
            "entries sort by directional faction-index pair"
        );
        assert_eq!(state.get(2.0, 3.0), Some(0.0));
        assert_eq!(state.get(3.0, 2.0), Some(-0.25));

        state.set(2.0, 3.0, -1.0, -1.0).unwrap();
        assert_eq!(
            state.iter().collect::<Vec<_>>(),
            vec![(3, 2, -0.25)],
            "a value restored to its baseline is absent from the sparse overlay"
        );
    }

    #[test]
    fn faction_sentiment_state_rejects_non_finite_set_and_adjust_without_clamping() {
        let mut state = FactionSentimentState::default();
        state.set(2.0, 3.0, f32::MAX, 0.0).unwrap();
        let before = state.iter().collect::<Vec<_>>();

        assert!(state.set(2.0, 3.0, f32::NAN, 0.0).is_err());
        assert!(state.set(2.0, 3.0, f32::INFINITY, 0.0).is_err());
        assert!(state.set(2.0, 3.0, 1.0, f32::NAN).is_err());
        assert!(state.adjust(2.0, 3.0, f32::NAN, 0.0).is_err());
        assert!(state.adjust(2.0, 3.0, f32::INFINITY, 0.0).is_err());
        assert!(state.adjust(2.0, 3.0, 1.0, f32::NAN).is_err());
        assert!(state.adjust(2.0, 3.0, f32::MAX, 0.0).is_err());

        assert_eq!(
            state.iter().collect::<Vec<_>>(),
            before,
            "non-finite inputs and finite-adjust overflow leave the sparse overlay unchanged"
        );
        assert_eq!(
            state.get(2.0, 3.0),
            Some(f32::MAX),
            "the finite sentiment remains unclamped"
        );
    }

    #[test]
    fn faction_sentiment_state_accepts_reserved_indices_and_rejects_unrepresentable_index() {
        let mut state = FactionSentimentState::default();
        state
            .set(
                PLAYER_FACTION_INDEX,
                DEFAULT_ENEMY_FACTION_INDEX,
                -0.5,
                CROSS_FACTION_DEFAULT_SENTIMENT,
            )
            .expect("reserved engine factions are legitimate live sentiment endpoints");

        let out_of_range = (MAX_FACTION_SENTIMENT_INDEX + 1) as f32;
        assert!(matches!(
            state.set(out_of_range, PLAYER_FACTION_INDEX, 0.5, -1.0),
            Err(FactionSentimentMutationError::FactionIndexNotWireRepresentable(index))
                if index == MAX_FACTION_SENTIMENT_INDEX + 1
        ));
        assert_eq!(state.iter().len(), 1);
    }

    #[test]
    fn faction_sentiment_state_rejects_new_pair_past_wire_capacity_but_allows_removal() {
        let mut state = FactionSentimentState::default();
        for from in 0..64 {
            for to in 0..64 {
                state
                    .set(from as f32, to as f32, 0.0, -1.0)
                    .expect("fixture fills the representable sparse set");
            }
        }
        assert_eq!(state.iter().len(), MAX_FACTION_SENTIMENT_OVERRIDES);
        assert_eq!(
            state.set(64.0, 0.0, 0.0, -1.0),
            Err(FactionSentimentMutationError::CapacityExceeded)
        );

        state
            .set(0.0, 0.0, -1.0, -1.0)
            .expect("returning an existing pair to baseline remains legal at capacity");
        state
            .set(64.0, 0.0, 0.0, -1.0)
            .expect("freed capacity accepts the next representable pair");
        assert_eq!(state.iter().len(), MAX_FACTION_SENTIMENT_OVERRIDES);
    }

    #[test]
    fn faction_sentiment_generation_changes_only_with_sparse_state() {
        let mut state = FactionSentimentState::default();
        let empty_generation = state.generation();
        assert!(!state.set(2.0, 3.0, -1.0, -1.0).unwrap());
        assert_eq!(state.generation(), empty_generation);

        assert!(state.set(2.0, 3.0, -0.5, -1.0).unwrap());
        let diverged_generation = state.generation();
        assert_ne!(diverged_generation, empty_generation);
        assert!(!state.set(2.0, 3.0, -0.5, -1.0).unwrap());
        assert_eq!(state.generation(), diverged_generation);

        assert!(state.set(2.0, 3.0, -1.0, -1.0).unwrap());
        assert_ne!(state.generation(), diverged_generation);
    }

    #[test]
    fn faction_sentiment_decay_converges_monotonically_and_removes_at_baseline() {
        let mut state = FactionSentimentState::default();
        state.set(2.0, 3.0, -1.0, 0.0).unwrap();

        let mut values = Vec::new();
        for _ in 0..4 {
            state.decay_step(1.0, |_, _| 0.25, |_, _| 0.0);
            values.push(state.get(2.0, 3.0));
        }

        let expected = [-0.75, -0.5, -0.25];
        for (actual, expected) in values[..3].iter().zip(expected) {
            let actual = actual.expect("pair remains diverged before the final decay step");
            assert!((actual - expected).abs() <= f32::EPSILON);
        }
        assert_eq!(values[3], None);
        assert!(state.iter().next().is_none());
    }

    #[test]
    fn faction_sentiment_decay_clamps_to_baseline_without_crossing() {
        let mut state = FactionSentimentState::default();
        state.set(2.0, 3.0, 0.1, 0.0).unwrap();

        state.decay_step(1.0, |_, _| 1.0, |_, _| 0.0);

        assert_eq!(state.get(2.0, 3.0), None);
        assert!(state.iter().next().is_none());
    }

    #[test]
    fn faction_sentiment_decay_zero_rate_holds_a_diverged_pair() {
        let mut state = FactionSentimentState::default();
        state.set(2.0, 3.0, -1.0, 0.0).unwrap();

        state.decay_step(1.0, |_, _| 0.0, |_, _| 0.0);

        let held = state.get(2.0, 3.0).expect("zero decay keeps the pair");
        assert!((held - -1.0).abs() <= f32::EPSILON);
    }

    #[test]
    fn faction_sentiment_decay_uses_pair_override_before_global_default() {
        let factions = FactionRegistry::from_descriptors(vec![
            FactionDescriptor {
                name: "cabal".to_string(),
            },
            FactionDescriptor {
                name: "resistance".to_string(),
            },
        ])
        .expect("valid factions")
        .with_sentiment_decay(0.1)
        .expect("non-negative global decay is valid")
        .with_sentiments([FactionSentimentDescriptor {
            from_faction: "cabal".to_string(),
            to_faction: "resistance".to_string(),
            sentiment: 0.0,
            tolerance: 1.0,
            decay: Some(0.25),
        }])
        .expect("valid pair override");

        assert!((factions.sentiment_decay(2.0, 3.0) - 0.25).abs() <= f32::EPSILON);
        assert!((factions.sentiment_decay(3.0, 2.0) - 0.1).abs() <= f32::EPSILON);
        assert!((factions.sentiment_decay(2.0, 2.0) - 0.1).abs() <= f32::EPSILON);
    }

    #[test]
    fn faction_sentiment_decay_uses_an_ulp_floor_when_linear_step_rounds_away() {
        let mut state = FactionSentimentState::default();
        state.set(2.0, 3.0, 1.0, 0.0).unwrap();
        let smaller_than_one_ulp = f32::EPSILON / 4.0;

        state.decay_step(1.0, |_, _| smaller_than_one_ulp, |_, _| 0.0);

        assert_eq!(
            state.get(2.0, 3.0),
            Some(f32::from_bits(1.0_f32.to_bits() - 1)),
            "a positive rate must not strand a far-from-baseline value when its linear step is below one ULP"
        );
    }

    #[test]
    fn live_faction_sentiment_uses_overlay_for_sentiment_and_baseline_for_tolerance() {
        let baseline = FactionRegistry::from_descriptors(vec![
            FactionDescriptor {
                name: "cabal".to_string(),
            },
            FactionDescriptor {
                name: "resistance".to_string(),
            },
        ])
        .expect("valid factions")
        .with_sentiments([
            FactionSentimentDescriptor {
                from_faction: "cabal".to_string(),
                to_faction: "resistance".to_string(),
                sentiment: -0.75,
                tolerance: 4.0,
                decay: None,
            },
            FactionSentimentDescriptor {
                from_faction: "resistance".to_string(),
                to_faction: "cabal".to_string(),
                sentiment: 0.25,
                tolerance: 9.0,
                decay: None,
            },
        ])
        .expect("valid authored relationships");
        let mut overlay = FactionSentimentState::default();

        let live = LiveFactionSentiment::new(&baseline, &overlay);
        assert_eq!(live.sentiment(2.0, 3.0), -0.75);
        assert_eq!(live.tolerance(2.0, 3.0), Some(4.0));

        overlay.set(2.0, 3.0, 0.5, -0.75).unwrap();
        let live = LiveFactionSentiment::new(&baseline, &overlay);
        assert_eq!(live.sentiment(2.0, 3.0), 0.5);
        assert_eq!(live.sentiment(3.0, 2.0), 0.25, "pairs stay directional");
        assert_eq!(live.tolerance(2.0, 3.0), Some(4.0));
        assert_eq!(
            live.relationship(2.0, 3.0),
            FactionRelationship {
                sentiment: 0.5,
                tolerance: Some(4.0),
                decay: None,
            },
            "relationship must not fall through to stale baseline sentiment"
        );
    }

    #[test]
    fn faction_registry_rejects_duplicate_or_unknown_sentiment_pairs() {
        let factions = FactionRegistry::from_descriptors(vec![FactionDescriptor {
            name: "cabal".to_string(),
        }])
        .expect("valid faction declaration");
        let unknown = factions
            .clone()
            .with_sentiments(vec![FactionSentimentDescriptor {
                from_faction: "missing".to_string(),
                to_faction: "cabal".to_string(),
                sentiment: -1.0,
                tolerance: 1.0,
                decay: None,
            }])
            .expect_err("unknown endpoint rejects the manifest");
        assert!(unknown.contains("undeclared from faction `missing`"));

        let unknown = factions
            .clone()
            .with_sentiments([FactionSentimentDescriptor {
                from_faction: "cabal".to_string(),
                to_faction: "missing".to_string(),
                sentiment: -1.0,
                tolerance: 1.0,
                decay: None,
            }])
            .expect_err("unknown destination rejects the manifest");
        assert!(unknown.contains("undeclared to faction `missing`"));

        let duplicate = factions
            .with_sentiments(vec![
                FactionSentimentDescriptor {
                    from_faction: "cabal".to_string(),
                    to_faction: "cabal".to_string(),
                    sentiment: 0.0,
                    tolerance: 1.0,
                    decay: None,
                },
                FactionSentimentDescriptor {
                    from_faction: "cabal".to_string(),
                    to_faction: "cabal".to_string(),
                    sentiment: -1.0,
                    tolerance: 2.0,
                    decay: None,
                },
            ])
            .expect_err("ambiguous duplicate pair rejects the manifest");
        assert!(duplicate.contains("duplicate sentiment entry"));
    }

    #[test]
    fn clear_keeps_manifest_factions_for_the_next_level() {
        let mut registry = DataRegistry::new();
        let factions = FactionRegistry::from_descriptors(vec![FactionDescriptor {
            name: "cabal".to_string(),
        }])
        .expect("valid faction declaration");
        registry.replace_factions(factions);
        registry.populate_level(sample_level_reactions(), Vec::new(), &[]);

        registry.clear();

        assert!(registry.reactions.is_empty());
        assert_eq!(registry.factions.index_for_name("cabal"), Some(2.0));
    }

    #[test]
    fn clear_drops_reactions_but_keeps_map_catalog() {
        let mut r = DataRegistry::new();
        r.populate_level(sample_level_reactions(), Vec::new(), &[]);
        r.replace_maps(vec![sample_map("e1m1")]);

        r.clear();

        assert_eq!(r.reactions.len(), 0);
        assert_eq!(r.maps, vec![sample_map("e1m1")]);
    }

    #[test]
    fn default_weapon_placement_replaces_whole_manifest_value_and_survives_unload() {
        let mut r = DataRegistry::new();
        let first = postretro_foundation::WeaponPlacementDescriptor {
            offset: postretro_foundation::PlacementOffset {
                right: 0.32,
                up: -0.28,
                forward: 0.62,
            },
            rotation: Default::default(),
        };
        let replacement = postretro_foundation::WeaponPlacementDescriptor {
            offset: postretro_foundation::PlacementOffset {
                right: 0.20,
                up: -0.40,
                forward: 0.70,
            },
            rotation: Default::default(),
        };

        r.set_default_weapon_placement(Some(first));
        r.set_default_weapon_placement(Some(replacement.clone()));
        r.clear();
        assert_eq!(r.default_weapon_placement, Some(replacement));

        r.set_default_weapon_placement(None);
        assert_eq!(r.default_weapon_placement, None);
    }

    #[test]
    fn clear_drops_active_sets_but_keeps_global_reactions_and_crossings() {
        let mut r = DataRegistry::new();
        r.populate_level(sample_level_reactions(), Vec::new(), &[]);
        let global_reactions = vec![sample_global_reaction("levelLoad")];
        let global_crossings = vec![sample_global_crossing("player.health")];
        r.replace_global_reactions(global_reactions.clone());
        r.replace_global_crossings(global_crossings.clone());

        r.clear();

        assert!(r.reactions.is_empty());
        assert!(r.crossings.is_empty());
        assert_eq!(r.global_reactions, global_reactions);
        assert_eq!(r.global_crossings, global_crossings);
    }

    #[test]
    fn global_definitions_survive_clear_and_recompose_after_reload() {
        let mut r = DataRegistry::new();
        let global_reactions = vec![sample_global_reaction("levelLoad")];
        let global_crossings = vec![sample_global_crossing("player.health")];
        r.replace_global_reactions(global_reactions.clone());
        r.replace_global_crossings(global_crossings.clone());
        r.populate_level(Vec::new(), Vec::new(), &tags(&["campaign"]));

        r.clear();

        assert!(r.reactions.is_empty());
        assert!(r.crossings.is_empty());
        assert_eq!(r.global_reactions, global_reactions);
        assert_eq!(r.global_crossings, global_crossings);

        r.populate_level(Vec::new(), Vec::new(), &tags(&["campaign"]));

        assert_eq!(reaction_names(&r), vec!["levelLoad".to_string()]);
        assert_eq!(crossing_slots(&r), vec!["player.health".to_string()]);
    }

    #[test]
    fn replace_global_reactions_preserves_collisions() {
        let mut r = DataRegistry::new();
        let first = sample_global_reaction("levelLoad");
        let second = sample_global_reaction("levelLoad");

        r.replace_global_reactions(vec![first.clone(), second.clone()]);

        assert_eq!(r.global_reactions, vec![first, second]);
    }

    #[test]
    fn upsert_entity_type_inserts_new_descriptor() {
        let mut r = DataRegistry::new();
        r.upsert_entity_type(grunt_descriptor());
        assert_eq!(r.entities.len(), 1);
        assert_eq!(r.entities[0].canonical_name.as_deref(), Some("grunt"));
    }

    #[test]
    fn upsert_entity_type_replays_identical_descriptor_silently() {
        let mut r = DataRegistry::new();
        r.upsert_entity_type(grunt_descriptor());
        r.upsert_entity_type(grunt_descriptor());
        assert_eq!(r.entities.len(), 1);
    }

    #[test]
    fn upsert_entity_type_overwrites_when_different() {
        let mut r = DataRegistry::new();
        r.upsert_entity_type(grunt_descriptor());
        let mut next = grunt_descriptor();
        next.light = Some(crate::data_descriptors::LightDescriptor {
            color: [1.0, 0.0, 0.0],
            intensity: 1.0,
            range: 5.0,
            is_dynamic: true,
        });
        r.upsert_entity_type(next.clone());
        assert_eq!(r.entities.len(), 1);
        assert_eq!(r.entities[0], next);
    }

    #[test]
    fn entity_type_generation_tracks_descriptor_snapshot_commits() {
        let mut r = DataRegistry::new();
        assert_eq!(r.entity_types_generation(), 0);

        r.upsert_entity_type(grunt_descriptor());
        assert_eq!(r.entity_types_generation(), 1);

        r.upsert_entity_type(grunt_descriptor());
        assert_eq!(
            r.entity_types_generation(),
            1,
            "an identical startup replay does not create a new snapshot"
        );

        let mut changed = grunt_descriptor();
        changed.light = Some(crate::data_descriptors::LightDescriptor {
            color: [1.0, 0.0, 0.0],
            intensity: 1.0,
            range: 5.0,
            is_dynamic: true,
        });
        r.upsert_entity_type(changed.clone());
        assert_eq!(r.entity_types_generation(), 2);

        r.replace_entity_types(vec![changed]);
        assert_eq!(
            r.entity_types_generation(),
            3,
            "a staged replacement is a new committed snapshot even when equal"
        );
    }

    #[test]
    fn replace_entity_types_removes_absent_descriptors() {
        let mut r = DataRegistry::new();
        r.upsert_entity_type(grunt_descriptor());
        let mut replacement = grunt_descriptor();
        replacement.canonical_name = Some("enforcer".to_string());

        r.replace_entity_types(vec![replacement.clone()]);

        assert_eq!(r.entities, vec![replacement]);
    }

    #[test]
    fn replace_entity_types_dedups_duplicate_canonical_names_last_wins() {
        let mut r = DataRegistry::new();
        r.upsert_entity_type(grunt_descriptor());
        let earlier = grunt_descriptor();
        let mut later = grunt_descriptor();
        later.light = Some(crate::data_descriptors::LightDescriptor {
            color: [1.0, 0.0, 0.0],
            intensity: 1.0,
            range: 5.0,
            is_dynamic: true,
        });

        r.replace_entity_types(vec![earlier, later.clone()]);

        assert_eq!(
            r.entities,
            vec![later],
            "later declaration wins on collision"
        );
    }

    #[test]
    fn dedup_entity_type_snapshot_keeps_last_occurrence_and_passes_through_unnamed() {
        let earlier = grunt_descriptor();
        let mut later = grunt_descriptor();
        later.light = Some(crate::data_descriptors::LightDescriptor {
            color: [0.0, 1.0, 0.0],
            intensity: 2.0,
            range: 8.0,
            is_dynamic: false,
        });
        let mut unnamed = grunt_descriptor();
        unnamed.canonical_name = None;

        let deduped =
            DataRegistry::dedup_entity_type_snapshot(vec![earlier, unnamed.clone(), later.clone()]);

        // The named collision collapses to its last occurrence (last-appearance
        // order), and the unnamed descriptor passes through untouched.
        assert_eq!(deduped, vec![unnamed, later]);
    }
}

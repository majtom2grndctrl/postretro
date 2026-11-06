// Target-owned contributor ledger: bounded per-source damage attribution
// recorded at the damage chokepoint and frozen into kill credit at the latch.
//
// See: context/lib/entity_model.md §2 (Health component)

use crate::registry::EntityId;

/// Maximum number of exact contributor source ids retained per target before
/// later distinct sources collapse into the overflow bucket.
pub const CONTRIBUTOR_LEDGER_CAPACITY: usize = 8;

/// Facts supplied to [`HealthComponent::record_contributor_damage`].
///
/// This is intentionally narrower than [`DamageContext`]: it describes only
/// the already-mitigated hit recorded in the target-owned contributor ledger.
#[derive(Debug, Clone, PartialEq)]
pub struct ContributorLedgerRecord {
    pub source_id: String,
    pub damage: f32,
    pub zone: Option<String>,
    pub attacker: Option<EntityId>,
    pub weapon: Option<EntityId>,
}

impl ContributorLedgerRecord {
    pub fn new(source_id: impl Into<String>, damage: f32) -> Self {
        Self {
            source_id: source_id.into(),
            damage,
            zone: None,
            attacker: None,
            weapon: None,
        }
    }
}

/// Exact per-source contributor facts retained by a health component.
#[derive(Debug, Clone, PartialEq)]
pub struct ContributorLedgerEntry {
    pub source_id: String,
    pub accumulated_damage: f32,
    pub hit_count: u32,
    pub last_hit_damage: f32,
    pub last_hit_zone: Option<String>,
    pub last_attacker: Option<EntityId>,
    pub last_weapon: Option<EntityId>,
}

impl ContributorLedgerEntry {
    fn from_record(record: ContributorLedgerRecord) -> Self {
        Self {
            source_id: record.source_id,
            accumulated_damage: record.damage,
            hit_count: 1,
            last_hit_damage: record.damage,
            last_hit_zone: record.zone,
            last_attacker: record.attacker,
            last_weapon: record.weapon,
        }
    }

    fn record_hit(&mut self, record: ContributorLedgerRecord) {
        self.accumulated_damage += record.damage;
        self.hit_count += 1;
        self.last_hit_damage = record.damage;
        self.last_hit_zone = record.zone;
        self.last_attacker = record.attacker;
        self.last_weapon = record.weapon;
    }
}

/// Reduced facts for distinct source ids that arrived after the exact ledger
/// filled. It deliberately has no source id, so exact-source queries never
/// mistake overflow damage for a retained contributor.
#[derive(Debug, Clone, PartialEq)]
pub struct ContributorLedgerOverflow {
    pub accumulated_damage: f32,
    pub hit_count: u32,
    pub last_hit_damage: f32,
    pub last_hit_zone: Option<String>,
    pub last_attacker: Option<EntityId>,
    pub last_weapon: Option<EntityId>,
}

impl ContributorLedgerOverflow {
    fn from_record(record: ContributorLedgerRecord) -> Self {
        Self {
            accumulated_damage: record.damage,
            hit_count: 1,
            last_hit_damage: record.damage,
            last_hit_zone: record.zone,
            last_attacker: record.attacker,
            last_weapon: record.weapon,
        }
    }

    fn record_hit(&mut self, record: ContributorLedgerRecord) {
        self.accumulated_damage += record.damage;
        self.hit_count += 1;
        self.last_hit_damage = record.damage;
        self.last_hit_zone = record.zone;
        self.last_attacker = record.attacker;
        self.last_weapon = record.weapon;
    }
}

/// Bounded target-owned contributor ledger. The first
/// [`CONTRIBUTOR_LEDGER_CAPACITY`] distinct source ids remain exact; any later
/// distinct source rolls into `overflow`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ContributorLedger {
    entries: Vec<ContributorLedgerEntry>,
    overflow: Option<ContributorLedgerOverflow>,
}

/// Frozen non-player kill credit captured at the first zero-HP latch.
///
/// This lives with the latch rather than in a simulation-side map so a direct
/// registry despawn drops it with the component and an eventually recycled id
/// cannot inherit stale credit. The frame-end deferred-removal pass hands a
/// pre-despawn snapshot to its callback only after successful removal.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingKillCredit {
    pub tags: Vec<String>,
    pub contributor_ledger: ContributorLedger,
}

impl ContributorLedger {
    pub fn record(&mut self, record: ContributorLedgerRecord) {
        if !record.damage.is_finite() || record.damage <= 0.0 {
            return;
        }

        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.source_id == record.source_id)
        {
            entry.record_hit(record);
            return;
        }

        if self.entries.len() < CONTRIBUTOR_LEDGER_CAPACITY {
            self.entries
                .push(ContributorLedgerEntry::from_record(record));
            return;
        }

        match &mut self.overflow {
            Some(overflow) => overflow.record_hit(record),
            None => self.overflow = Some(ContributorLedgerOverflow::from_record(record)),
        }
    }

    pub fn entries(&self) -> &[ContributorLedgerEntry] {
        &self.entries
    }

    pub fn overflow(&self) -> Option<&ContributorLedgerOverflow> {
        self.overflow.as_ref()
    }

    pub fn recorded_damage_by_source(&self, source_id: &str) -> Option<f32> {
        self.entries
            .iter()
            .find(|entry| entry.source_id == source_id)
            .map(|entry| entry.accumulated_damage)
    }

    pub fn total_recorded_damage(&self) -> f32 {
        let exact = self
            .entries
            .iter()
            .map(|entry| entry.accumulated_damage)
            .sum::<f32>();
        exact
            + self
                .overflow
                .as_ref()
                .map(|overflow| overflow.accumulated_damage)
                .unwrap_or(0.0)
    }

    pub fn total_recorded_hits(&self) -> u32 {
        let exact = self
            .entries
            .iter()
            .map(|entry| entry.hit_count)
            .sum::<u32>();
        exact
            + self
                .overflow
                .as_ref()
                .map(|overflow| overflow.hit_count)
                .unwrap_or(0)
    }
}

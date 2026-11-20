// Which `occupancy` families exist: the mod's references, replaced at each mod
// commit, plus the installed level's `setupLevel` references.
// See: context/lib/scripting.md §5 (Occupancy)

use std::collections::BTreeSet;

use postretro_entities::engine_state_collections::OCCUPANCY_FAMILY;
use postretro_entities::slot_table::SlotTable;
use postretro_scripting_core::occupancy_references::FamilyReferences;
use postretro_scripting_core::ui::descriptor::AnchoredTree;

/// The mod's references: every committed global reaction and crossing, every
/// store accumulator, and the mod's UI trees. Read from the whole committed
/// set, not the level-filtered active one.
pub(crate) fn mod_scope_references<'a>(
    script_ctx: &postretro_entities::ScriptCtx,
    ui_trees: impl IntoIterator<Item = &'a AnchoredTree>,
) -> FamilyReferences {
    let mut refs = FamilyReferences::default();
    {
        let registry = script_ctx.data_registry.borrow();
        for scoped in &registry.global_reactions {
            refs.note_reaction(&scoped.reaction.descriptor);
        }
        for scoped in &registry.global_crossings {
            refs.note_crossing(&scoped.crossing);
        }
    }
    refs.note_accumulators(&script_ctx.slot_table.borrow());
    for tree in ui_trees {
        refs.note_ui_tree(tree);
    }
    refs
}

fn write_count(slot_table: &mut SlotTable, name: &str, count: u32) {
    let value = postretro_entities::SlotValue::Number(count as f32);
    if let Some(slot) = slot_table.get_mut(name)
        && slot.value.as_ref() != Some(&value)
    {
        slot.write_value(Some(value));
    }
}

/// Every peer runs the same scripts, so each derives the same sets at the same
/// points; nothing carries them on the wire.
#[derive(Debug, Default)]
pub(crate) struct OccupancyFamilies {
    mod_keys: BTreeSet<String>,
    level_keys: BTreeSet<String>,
    /// Each existing member's tag and slot names, rebuilt when the set changes
    /// so a tick's writes format nothing.
    members: Vec<MemberSlots>,
    /// Scratch for one tag's distinct occupant pawns; reused across ticks.
    pawns: Vec<postretro_entities::EntityId>,
}

#[derive(Debug)]
struct MemberSlots {
    tag: String,
    occupied_count: String,
    occupants: String,
}

impl OccupancyFamilies {
    /// A mod commit replaces the mod-scope references whole. Members kept
    /// across the commit keep their values; the installed level's stay.
    pub(crate) fn commit_mod(&mut self, refs: &FamilyReferences, slot_table: &mut SlotTable) {
        refs.warn_unformable("the mod");
        self.mod_keys = refs.keys.clone();
        self.apply(slot_table);
    }

    /// A level installs: its `setupLevel` references join the mod's, and every
    /// member reads 0 until the first authoritative tick writes it. A level
    /// reference to a tag no trigger volume in the level carries warns once.
    pub(crate) fn install_level(
        &mut self,
        refs: &FamilyReferences,
        volume_tags: &BTreeSet<String>,
        slot_table: &mut SlotTable,
    ) {
        refs.warn_unformable("setupLevel");
        for tag in refs.keys.difference(volume_tags) {
            log::warn!(
                "[Scripting] setupLevel references occupancy tag `{tag}`, but no trigger volume or level seam in this level carries it; it reads 0"
            );
        }
        self.level_keys = refs.keys.clone();
        self.apply(slot_table);
        slot_table.reset_family_values(&OCCUPANCY_FAMILY);
    }

    /// The level unloads: its references go, except tags the mod also
    /// references, and every remaining member reads 0.
    pub(crate) fn unload_level(&mut self, slot_table: &mut SlotTable) {
        self.level_keys.clear();
        self.apply(slot_table);
        slot_table.reset_family_values(&OCCUPANCY_FAMILY);
    }

    /// The tags whose families exist, sorted.
    #[cfg(test)]
    pub(crate) fn keys(&self) -> impl Iterator<Item = &str> {
        self.mod_keys.union(&self.level_keys).map(String::as_str)
    }

    /// One authoritative tick, beside the player counts: each family from the
    /// volumes this tick's trigger stage left occupied. `occupiedCount` counts
    /// volumes carrying the tag with a counted player inside (a tag a volume
    /// repeats counts it once); `occupants` counts distinct counted players
    /// inside any of them. With no session level every family reads 0, as the
    /// player counts do.
    pub(crate) fn publish_tick(
        &mut self,
        session_level: bool,
        registry: &postretro_entities::EntityRegistry,
        occupied_volumes: &[(
            postretro_entities::EntityId,
            Vec<postretro_entities::EntityId>,
        )],
        slot_table: &mut SlotTable,
    ) {
        for member in &self.members {
            let mut volumes = 0u32;
            self.pawns.clear();
            if session_level {
                for (volume, inside) in occupied_volumes {
                    let carries = registry
                        .get_tags(*volume)
                        .is_ok_and(|tags| tags.contains(&member.tag));
                    if !carries {
                        continue;
                    }
                    // Only a seat's pawn occupies: the capture counts that same
                    // pawn into its seat's `inPlay`, so `occupants` never
                    // exceeds it. An unseated pawn (a fixture, an unmarked
                    // fallback pawn) holds nothing.
                    let before = self.pawns.len();
                    self.pawns.extend(
                        inside
                            .iter()
                            .copied()
                            .filter(|&pawn| registry.seat_for_pawn(pawn).is_some()),
                    );
                    if self.pawns.len() > before {
                        volumes += 1;
                    }
                }
            }
            self.pawns.sort_unstable();
            self.pawns.dedup();
            write_count(slot_table, &member.occupied_count, volumes);
            write_count(slot_table, &member.occupants, self.pawns.len() as u32);
        }
    }

    fn apply(&mut self, slot_table: &mut SlotTable) {
        let keys: BTreeSet<String> = self.mod_keys.union(&self.level_keys).cloned().collect();
        slot_table.replace_family_members(&OCCUPANCY_FAMILY, &keys);
        self.members = keys
            .into_iter()
            .map(|tag| MemberSlots {
                occupied_count: OCCUPANCY_FAMILY.slot_name(&tag, "occupiedCount"),
                occupants: OCCUPANCY_FAMILY.slot_name(&tag, "occupants"),
                tag,
            })
            .collect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use log::Level;
    use postretro_entities::SlotValue;
    use postretro_test_log_capture::LogCapture;

    fn refs(tags: &[&str]) -> FamilyReferences {
        let mut refs = FamilyReferences::default();
        for tag in tags {
            refs.note_slot(&format!("occupancy.{tag}.occupants"));
        }
        refs
    }

    fn tags(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|name| (*name).to_string()).collect()
    }

    fn occupants(table: &SlotTable, tag: &str) -> Option<SlotValue> {
        table
            .get(&format!("occupancy.{tag}.occupants"))
            .and_then(|slot| slot.value.clone())
    }

    fn hold(table: &mut SlotTable, tag: &str, value: f32) {
        table
            .get_mut(&format!("occupancy.{tag}.occupants"))
            .unwrap()
            .write_value(Some(SlotValue::Number(value)));
    }

    // C3: a tag with `.` or `:` warns once where it is collected and gets no
    // family; the other references get theirs.
    #[test]
    fn unformable_tag_warns_once_and_gets_no_family() {
        let capture = LogCapture::start();
        let mut table = SlotTable::new();
        let mut families = OccupancyFamilies::default();
        families.commit_mod(&refs(&["hill", "a.b", "c:d"]), &mut table);
        assert_eq!(table.family_members(&OCCUPANCY_FAMILY), tags(&["hill"]));
        capture.assert_logged_once(Level::Warn, "occupancy tag `a.b`, which cannot form");
        capture.assert_logged_once(Level::Warn, "occupancy tag `c:d`, which cannot form");
    }

    // C4: a level reference to a tag no volume carries reads 0 and warns once;
    // a mod-scope reference reads 0 on that level silently.
    #[test]
    fn absent_tag_warns_for_level_reference_only() {
        let capture = LogCapture::start();
        let mut table = SlotTable::new();
        let mut families = OccupancyFamilies::default();
        families.commit_mod(&refs(&["hill"]), &mut table);
        families.install_level(
            &refs(&["airlock", "plates"]),
            &tags(&["plates"]),
            &mut table,
        );
        assert_eq!(occupants(&table, "airlock"), Some(SlotValue::Number(0.0)));
        assert_eq!(occupants(&table, "hill"), Some(SlotValue::Number(0.0)));
        capture.assert_logged_once(
            Level::Warn,
            "occupancy tag `airlock`, but no trigger volume",
        );
        capture.assert_not_logged(Level::Warn, "occupancy tag `hill`");
        capture.assert_not_logged(Level::Warn, "occupancy tag `plates`");
    }

    // C7 (P2), C10 (P23), C11 (P21): after a level change the families are
    // the mod's plus the new level's; a tag the mod shares with the unloaded
    // level keeps its family; every family reads 0 from unload through install.
    #[test]
    fn level_change_replaces_level_families_and_zeroes_every_family() {
        let mut table = SlotTable::new();
        let mut families = OccupancyFamilies::default();
        families.commit_mod(&refs(&["hill"]), &mut table);
        families.install_level(&refs(&["hill", "a"]), &tags(&["hill", "a"]), &mut table);
        assert_eq!(
            table.family_members(&OCCUPANCY_FAMILY),
            tags(&["a", "hill"])
        );
        hold(&mut table, "hill", 2.0);
        hold(&mut table, "a", 1.0);

        families.unload_level(&mut table);
        assert_eq!(table.family_members(&OCCUPANCY_FAMILY), tags(&["hill"]));
        assert_eq!(occupants(&table, "hill"), Some(SlotValue::Number(0.0)));

        hold(&mut table, "hill", 3.0);
        families.install_level(&refs(&["b"]), &tags(&["b", "hill"]), &mut table);
        assert_eq!(
            table.family_members(&OCCUPANCY_FAMILY),
            tags(&["b", "hill"])
        );
        assert_eq!(occupants(&table, "hill"), Some(SlotValue::Number(0.0)));
        assert_eq!(occupants(&table, "a"), None);
    }

    // P22: a mod commit mid-level keeps the installed level's families and
    // every kept family's value, and declares a reference it adds.
    #[test]
    fn staged_commit_mid_level_keeps_families_and_values() {
        let mut table = SlotTable::new();
        let mut families = OccupancyFamilies::default();
        families.commit_mod(&refs(&["hill"]), &mut table);
        families.install_level(&refs(&["airlock"]), &tags(&["airlock", "hill"]), &mut table);
        hold(&mut table, "hill", 2.0);
        hold(&mut table, "airlock", 1.0);

        families.commit_mod(&refs(&["hill", "arena"]), &mut table);
        assert_eq!(occupants(&table, "hill"), Some(SlotValue::Number(2.0)));
        assert_eq!(occupants(&table, "airlock"), Some(SlotValue::Number(1.0)));
        assert_eq!(occupants(&table, "arena"), Some(SlotValue::Number(0.0)));
        assert_eq!(
            families.keys().collect::<Vec<_>>(),
            vec!["airlock", "arena", "hill"]
        );
    }

    fn counts(table: &SlotTable, tag: &str) -> (Option<SlotValue>, Option<SlotValue>) {
        (
            table
                .get(&format!("occupancy.{tag}.occupiedCount"))
                .and_then(|slot| slot.value.clone()),
            occupants(table, tag),
        )
    }

    fn n(value: f32) -> Option<SlotValue> {
        Some(SlotValue::Number(value))
    }

    // O1, O2, O4 (P11): volumes with a counted player inside count once per
    // volume carrying the tag, players once however many volumes they stand in,
    // and a volume repeating a tag counts it once.
    #[test]
    fn occupancy_counts_volumes_and_distinct_players() {
        use postretro_entities::{EntityRegistry, Transform};
        let mut registry = EntityRegistry::new();
        let mut volume = |tags: &[&str]| {
            let id = registry.spawn(Transform::default());
            registry
                .set_tags(id, tags.iter().map(|tag| (*tag).to_string()).collect())
                .unwrap();
            id
        };
        let plate = volume(&["plates"]);
        let other_plate = volume(&["plates"]);
        let overlap = volume(&["room", "plates"]);
        let doubled = volume(&["hill", "hill"]);
        let (a, b) = (
            registry.spawn(Transform::default()),
            registry.spawn(Transform::default()),
        );
        registry.bind_pawn_seat(a, postretro_foundation::Seat(1));
        registry.bind_pawn_seat(b, postretro_foundation::Seat(2));
        let unseated = registry.spawn(Transform::default());

        let mut table = SlotTable::new();
        let mut families = OccupancyFamilies::default();
        families.commit_mod(&refs(&["plates", "room", "hill"]), &mut table);

        // Two players on one plate.
        families.publish_tick(true, &registry, &[(plate, vec![a, b])], &mut table);
        assert_eq!(counts(&table, "plates"), (n(1.0), n(2.0)));
        // One player on each of two plates sharing a tag.
        families.publish_tick(
            true,
            &registry,
            &[(plate, vec![a]), (other_plate, vec![b])],
            &mut table,
        );
        assert_eq!(counts(&table, "plates"), (n(2.0), n(2.0)));
        // One player inside two overlapping volumes sharing a tag.
        families.publish_tick(
            true,
            &registry,
            &[(plate, vec![a]), (overlap, vec![a])],
            &mut table,
        );
        assert_eq!(counts(&table, "plates"), (n(2.0), n(1.0)));
        assert_eq!(counts(&table, "room"), (n(1.0), n(1.0)));
        // A volume repeating its tag counts once.
        families.publish_tick(true, &registry, &[(doubled, vec![a])], &mut table);
        assert_eq!(counts(&table, "hill"), (n(1.0), n(1.0)));
        assert_eq!(counts(&table, "plates"), (n(0.0), n(0.0)));
        // An unseated pawn occupies nothing (S6).
        families.publish_tick(true, &registry, &[(doubled, vec![unseated])], &mut table);
        assert_eq!(counts(&table, "hill"), (n(0.0), n(0.0)));
        // No session level: every family reads 0.
        families.publish_tick(false, &registry, &[(doubled, vec![a])], &mut table);
        assert_eq!(counts(&table, "hill"), (n(0.0), n(0.0)));
    }
}

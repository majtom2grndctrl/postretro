// The mod author's optional `ModManifest.saves` block, as both SDK drains
// produce it: how many character and run save slots the mod keeps.
// See: context/lib/scripting.md §5 (Durable State Store)

/// Engine cap on `saves.characterSlots`. Catalog rows are fixed-capacity, so
/// the cap sizes the engine slot table and the frontend's `forEach` unroll.
pub const MAX_CHARACTER_SLOTS: usize = 16;
/// Engine cap on `saves.runSlots`, for the same reason as
/// [`MAX_CHARACTER_SLOTS`].
pub const MAX_RUN_SLOTS: usize = 16;

/// Declared save-slot counts. Each is at least 1 and at most its cap; the
/// resolver clamps or defaults an authored value with a warning and never
/// rejects the manifest. A mod with no `saves` block carries `None` and gets
/// one implicit character and one implicit run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SavesManifest {
    pub character_slots: usize,
    pub run_slots: usize,
}

// Source-format-specific translators. Each `<name>.rs` converts mapper-facing
// FGD properties from one source format into the canonical map light format
// defined in `map_data.rs`. Downstream stages (SH baker, runtime direct path)
// never see source-format vocabulary.
// See: context/plans/in-progress/lighting-foundation/1-fgd-canonical.md

pub mod quake_map;

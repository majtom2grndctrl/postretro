// Source-format adapter boundary. One module per input format. Each converts
// that format's vocabulary — axes, units, angle encoding, intensity reference,
// classnames, authoring-property names, editor-only containers and keys — into
// the canonical map representation in `map_data.rs`. Shared compiler stages and
// PRL sections never branch on source format and never see source vocabulary.
// Format-specific helpers belong here, not in shared code.
// See: context/lib/build_pipeline.md §Source-format neutrality

pub mod quake_map;

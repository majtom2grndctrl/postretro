// Compiler per-stage host-RAM working-set ceiling.
// See: context/lib/build_pipeline.md §Build Cache

pub(crate) const DEFAULT_MAX_BYTES: u64 = 16 * 1024 * 1024 * 1024;

// The reach cull's verify-mode switch: an invariant check compiled into every
// build, off by default outside tests, set by POSTRETRO_LIGHTMAP_REACH_VERIFY.
// See: context/lib/development_guide.md §6.4 · build_pipeline.md step 12 (Exact skips)

use std::sync::OnceLock;

/// `1` turns reach verify mode on, `0` off; unset keeps the default.
pub(crate) const REACH_VERIFY_ENV: &str = "POSTRETRO_LIGHTMAP_REACH_VERIFY";

/// Off in every build profile, so a dev-profile compile neither re-bakes the
/// lightmap stage cold nor stops a level designer at a named gap. Always on
/// in tests.
const REACH_VERIFY_DEFAULT: bool = cfg!(test);

/// The switch from `value`: `Ok` with the setting, or `Err` with `default`
/// for a malformed value, which the caller warns about once.
pub(super) fn resolve_reach_verify(value: Option<&str>, default: bool) -> Result<bool, bool> {
    match value.map(str::trim) {
        None => Ok(default),
        Some("1") => Ok(true),
        Some("0") => Ok(false),
        Some(_) => Err(default),
    }
}

/// Whether reach verify mode is on, read from the environment once per
/// process.
pub(super) fn reach_verify_enabled() -> bool {
    #[cfg(test)]
    if let Some(on) = VERIFY_OVERRIDE.with(std::cell::Cell::get) {
        return on;
    }
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        let value = std::env::var(REACH_VERIFY_ENV).ok();
        resolve_reach_verify(value.as_deref(), REACH_VERIFY_DEFAULT).unwrap_or_else(|default| {
            log::warn!(
                "[Compiler] {REACH_VERIFY_ENV}={:?} is neither 1 nor 0; lightmap reach verify \
                 mode stays {}",
                value.unwrap_or_default(),
                if default { "on" } else { "off" },
            );
            default
        })
    })
}

#[cfg(test)]
thread_local! {
    static VERIFY_OVERRIDE: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// Run `f` with reach verify mode set to `on` for stages run on this thread:
/// for tests that count cache reads and writes, which need production's
/// walk and caches, as the brief's warm gate rows do.
#[cfg(test)]
pub(crate) fn with_reach_verify<T>(on: bool, f: impl FnOnce() -> T) -> T {
    let previous = VERIFY_OVERRIDE.with(|cell| cell.replace(Some(on)));
    let result = f();
    VERIFY_OVERRIDE.with(|cell| cell.set(previous));
    result
}

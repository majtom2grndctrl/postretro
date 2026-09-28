// Reduced motion and contrast from the OS via `mundy`, with change events.
// See: context/lib/player_options.md §5

use std::sync::mpsc::Sender;

use mundy::{Contrast, Interest, Preferences, ReducedMotion, Subscription};

use super::OsUpdate;

/// Subscribe on the calling (main) thread. The first callback carries the
/// initial values; later ones carry changes. Dropping the subscription
/// cancels it.
pub(super) fn subscribe(tx: Sender<OsUpdate>) -> Subscription {
    Preferences::subscribe(Interest::ReducedMotion | Interest::Contrast, move |prefs| {
        let _ = tx.send(OsUpdate::Preferences {
            reduce_motion: reduce_motion(prefs.reduced_motion),
            increased_contrast: increased_contrast(prefs.contrast),
        });
    })
}

/// No preference (including a platform or portal that cannot say) leaves the
/// engine default in charge.
fn reduce_motion(value: ReducedMotion) -> Option<bool> {
    match value {
        ReducedMotion::Reduce => Some(true),
        ReducedMotion::NoPreference => None,
    }
}

/// Forced colors (Windows high-contrast themes) counts as increased contrast.
fn increased_contrast(value: Contrast) -> Option<bool> {
    match value {
        Contrast::More | Contrast::Custom => Some(true),
        Contrast::Less | Contrast::NoPreference => None,
    }
}

// Slider value steps captured from nav intents.
// See: context/lib/ui.md §4

use crate::input::ui_nav::NavIntent;
use postretro_ui::tree::NodeInteraction;

/// Resolve a captured-nav value step for a focused `slider` (M13 Goal F, Task 4).
/// Partitions `nav_intents` into captured — removed in place — and uncaptured —
/// retained for the focus engine. Each captured directional intent steps the value
/// by the slider's `step`: `nav.right`/`nav.up` increase, `nav.left`/`nav.down`
/// decrease; any other captured name is swallowed without moving the value. Returns
/// `Some(next)` — the value stepped from `current` and clamped to `[min, max]` —
/// when a net step occurred, else `None`. A non-`Slider` interaction returns `None`
/// and leaves `nav_intents` untouched. The app emits a `setState { slot, next }`
/// for a `Some` result, applied at the game-logic command drain (the bound slot
/// changes on the N+1 frame — the system's defining N→N+1 ordering).
pub fn capture_slider_step(
    interaction: &NodeInteraction,
    current: f32,
    nav_intents: &mut Vec<NavIntent>,
) -> Option<f32> {
    let NodeInteraction::Slider {
        min,
        max,
        step,
        captures_nav,
        ..
    } = interaction
    else {
        return None;
    };
    if captures_nav.is_empty() {
        return None;
    }

    let mut retained = Vec::with_capacity(nav_intents.len());
    let mut delta_steps = 0i32;
    for &nav in nav_intents.iter() {
        if captures_nav.iter().any(|name| name == nav.wire_name()) {
            match nav {
                NavIntent::Right | NavIntent::Up => delta_steps += 1,
                NavIntent::Left | NavIntent::Down => delta_steps -= 1,
                _ => {}
            }
        } else {
            retained.push(nav);
        }
    }
    *nav_intents = retained;

    if delta_steps == 0 {
        return None;
    }
    Some((current + step * delta_steps as f32).clamp(*min, *max))
}

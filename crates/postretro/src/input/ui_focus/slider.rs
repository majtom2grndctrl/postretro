// Slider value steps captured from nav intents.
// See: context/lib/ui.md §4

use crate::input::ui_nav::NavIntent;
use postretro_ui::tree::NodeInteraction;

/// A focused slider's captured nav for one frame: the net signed steps and the
/// last captured direction, which arms its hold-to-repeat.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SliderCapture {
    pub steps: i32,
    pub held: NavIntent,
    min: f32,
    max: f32,
    step: f32,
}

impl SliderCapture {
    /// The value `steps` lands on from `current`, clamped to the slider's
    /// bounds without overshoot.
    #[cfg(test)]
    pub fn value_from(&self, current: f32) -> f32 {
        slider_value(current, self.steps, self.step, self.min, self.max)
    }
}

/// `current` moved by `steps` of `step`, clamped to `[min, max]`.
pub fn slider_value(current: f32, steps: i32, step: f32, min: f32, max: f32) -> f32 {
    (current + step * steps as f32).clamp(min, max)
}

/// Capture nav for a focused `slider` (M13 Goal F, Task 4). Partitions
/// `nav_intents` into captured — removed in place — and uncaptured — retained
/// for the focus engine. Each captured directional intent is one step:
/// `nav.right`/`nav.up` increase, `nav.left`/`nav.down` decrease; any other
/// captured name is swallowed. Returns the net steps when they are non-zero. A
/// non-`Slider` interaction returns `None` and leaves `nav_intents` untouched.
/// The app applies the steps relative to the slot's value when the command
/// drain runs, so an external write on the same frame is never overwritten
/// by a value computed before it (P17).
pub fn capture_slider_step(
    interaction: &NodeInteraction,
    nav_intents: &mut Vec<NavIntent>,
) -> Option<SliderCapture> {
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
    let mut held = None;
    for &nav in nav_intents.iter() {
        if captures_nav.iter().any(|name| name == nav.wire_name()) {
            match nav {
                NavIntent::Right | NavIntent::Up => delta_steps += 1,
                NavIntent::Left | NavIntent::Down => delta_steps -= 1,
                _ => continue,
            }
            held = Some(nav);
        } else {
            retained.push(nav);
        }
    }
    *nav_intents = retained;

    if delta_steps == 0 {
        return None;
    }
    Some(SliderCapture {
        steps: delta_steps,
        held: held?,
        min: *min,
        max: *max,
        step: *step,
    })
}

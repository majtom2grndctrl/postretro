// Change-driven projection of picked mode and countdown into readonly UI slots.
// See: context/lib/ui.md §3 · context/lib/player_options.md §7

use super::policy::Controller;
use postretro_entities::{SlotValue, slot_table::SlotTable};
use std::time::Instant;

pub(super) fn project(controller: &Controller, table: &mut SlotTable, now: Instant) {
    let mode = controller.picked.as_ref();
    for (name, value) in [
        (
            "window.displayModeWidth",
            mode.map_or(0.0, |mode| mode.width as f32),
        ),
        (
            "window.displayModeHeight",
            mode.map_or(0.0, |mode| mode.height as f32),
        ),
        (
            "window.displayModeRefreshHz",
            mode.map_or(0.0, |mode| mode.refresh_millihertz as f32 / 1000.0),
        ),
        (
            "window.displayModeBitDepth",
            mode.map_or(0.0, |mode| mode.bit_depth as f32),
        ),
        (
            "window.displayModeRevertSeconds",
            controller.seconds_remaining(now),
        ),
    ] {
        let slot = table.get_mut(name).expect("catalog declares display slots");
        let changed = !matches!(slot.value.as_ref(), Some(SlotValue::Number(current)) if (*current - value).abs() < f32::EPSILON);
        if changed {
            slot.write_value(Some(SlotValue::Number(value)));
        }
    }
    let monitor = mode.map_or("", |mode| mode.monitor.as_str());
    let slot = table
        .get_mut("window.displayModeMonitor")
        .expect("catalog declares monitor slot");
    if !matches!(slot.value.as_ref(), Some(SlotValue::String(current)) if current == monitor) {
        slot.write_value(Some(SlotValue::String(monitor.into())));
    }
}

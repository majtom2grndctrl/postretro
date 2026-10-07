// The device family glyphs follow: the last family the player used, one per
// frame, set only by deliberate input.
// See: context/lib/input.md §6 (gamepad) · context/lib/ui.md §4

use super::input_names::DeviceClass;

/// Sony's USB vendor id.
const SONY_VENDOR_ID: u16 = 0x054C;
/// Nintendo's USB vendor id.
const NINTENDO_VENDOR_ID: u16 = 0x057E;

/// Which glyph art set draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DeviceFamily {
    #[default]
    KeyboardMouse,
    Xbox,
    PlayStation,
    Nintendo,
}

impl DeviceFamily {
    /// The family of a pad by its vendor id: Sony draws PlayStation, Nintendo
    /// draws Nintendo, and any other pad, or one with no vendor id, draws Xbox.
    pub const fn of_pad(vendor_id: Option<u16>) -> Self {
        match vendor_id {
            Some(SONY_VENDOR_ID) => DeviceFamily::PlayStation,
            Some(NINTENDO_VENDOR_ID) => DeviceFamily::Nintendo,
            _ => DeviceFamily::Xbox,
        }
    }

    /// The binding class this family reads.
    pub const fn class(self) -> DeviceClass {
        match self {
            DeviceFamily::KeyboardMouse => DeviceClass::KeyboardMouse,
            DeviceFamily::Xbox | DeviceFamily::PlayStation | DeviceFamily::Nintendo => {
                DeviceClass::Gamepad
            }
        }
    }
}

/// Collects this frame's deliberate input and settles one family per frame.
/// Only presses, stick crossings past the dead zone, and a pointer-mode switch
/// vote, so a resting device's drift never changes the family (P22).
#[derive(Debug, Default)]
pub struct DeviceFamilyTracker {
    current: DeviceFamily,
    /// The last family voted this frame.
    vote: Option<DeviceFamily>,
}

impl DeviceFamilyTracker {
    /// A key or mouse button press, or the input mode settling on pointer.
    pub fn note_keyboard_mouse(&mut self) {
        self.vote = Some(DeviceFamily::KeyboardMouse);
    }

    /// A pad press or a stick past its dead zone, from a pad with this vendor.
    pub fn note_pad(&mut self, vendor_id: Option<u16>) {
        self.vote = Some(DeviceFamily::of_pad(vendor_id));
    }

    /// Settle the frame: the last input voted wins, so a frame with keyboard
    /// and pad input sets exactly one family.
    pub fn end_frame(&mut self) -> DeviceFamily {
        if let Some(family) = self.vote.take() {
            self.current = family;
        }
        self.current
    }

    pub fn current(&self) -> DeviceFamily {
        self.current
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pads_map_to_families_by_vendor() {
        assert_eq!(DeviceFamily::of_pad(Some(0x054C)), DeviceFamily::PlayStation);
        assert_eq!(DeviceFamily::of_pad(Some(0x057E)), DeviceFamily::Nintendo);
        assert_eq!(DeviceFamily::of_pad(Some(0x045E)), DeviceFamily::Xbox);
        assert_eq!(DeviceFamily::of_pad(Some(0x2DC8)), DeviceFamily::Xbox);
        assert_eq!(DeviceFamily::of_pad(None), DeviceFamily::Xbox);
    }

    #[test]
    fn one_family_per_frame_and_quiet_frames_keep_it() {
        let mut tracker = DeviceFamilyTracker::default();
        tracker.note_keyboard_mouse();
        tracker.note_pad(Some(0x054C));
        assert_eq!(tracker.end_frame(), DeviceFamily::PlayStation);
        assert_eq!(
            tracker.end_frame(),
            DeviceFamily::PlayStation,
            "a frame with no deliberate input keeps the family"
        );
        tracker.note_pad(Some(0x054C));
        tracker.note_keyboard_mouse();
        assert_eq!(tracker.end_frame(), DeviceFamily::KeyboardMouse);
    }
}

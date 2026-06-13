// App-side consumer that applies the mod-declared `audio.master` slot to the
// audio main bus volume. The slider in the demo pause menu writes an amplitude
// in `[0, 1]` to `audio.master` (via `setState`); this reads that amplitude,
// converts it to decibels, and hands the dB value to the App to apply to the
// audio main track — the cross-subsystem store-consumer pattern (like the
// `screen.flash` flash-decay consumer), keeping the audio module out of the
// scripting surface.
//
// `audio.master` is NOT an engine-owned slot: the demo mod declares it via
// `defineStore` (writable). This consumer only reads it and amplifies to dB;
// the slot's existence is content, the consumer is engine wiring.
// See: context/lib/audio.md §1 (Mixer bus tree) · context/lib/scripting.md §5

use crate::scripting::ctx::ScriptCtx;
use crate::scripting::primitives::store::read_store_slot;
use crate::scripting::slot_table::SlotValue;

/// Dotted name of the mod-declared master-volume slot. Absent until the demo
/// mod declares it, so the consumer no-ops cleanly when no mod declares it.
const MASTER_SLOT: &str = "audio.master";

/// Decibel floor a zero (or non-positive) amplitude maps to — effectively
/// silent. kira treats this as the mute endpoint; using a finite floor instead
/// of `-inf` avoids feeding a non-finite value into the volume command.
const MUTE_FLOOR_DB: f32 = -80.0;

/// Convert a linear amplitude in `[0, 1]` to decibels. `1.0` → `0 dB` (unity
/// gain), `0.0` (or any non-positive amplitude) → [`MUTE_FLOOR_DB`] (mute), and
/// intermediate values follow `20 * log10(amplitude)`. Amplitudes above `1.0`
/// boost (positive dB) — the slider clamps to `[0, 1]`, but the conversion does
/// not assume it.
pub(crate) fn amplitude_to_decibels(amplitude: f32) -> f32 {
    if amplitude <= 0.0 {
        MUTE_FLOOR_DB
    } else {
        // 20*log10(amp). Clamp the floor so a tiny positive amplitude doesn't
        // produce a hugely-negative dB the floor already represents.
        (20.0 * amplitude.log10()).max(MUTE_FLOOR_DB)
    }
}

/// Reads `audio.master` each frame and reports the master-track dB to apply when
/// the amplitude changes. Holds a clone of the engine's `ScriptCtx` (cheap `Rc`
/// bump). Change-gated: returns `Some(db)` only when the read amplitude differs
/// from the last applied one, so the audio command is issued on slider moves,
/// not every frame.
pub(crate) struct AudioMasterConsumer {
    ctx: ScriptCtx,
    /// The last amplitude applied, or `None` before the first apply (so the
    /// initial declared value is applied once).
    last_amplitude: Option<f32>,
}

impl AudioMasterConsumer {
    /// Build a consumer holding a clone of the engine's `ScriptCtx`.
    pub(crate) fn new(ctx: ScriptCtx) -> Self {
        Self {
            ctx,
            last_amplitude: None,
        }
    }

    /// Forget the last applied amplitude so the next `poll` re-applies. Called on
    /// level load (a new mod may re-declare `audio.master` at a different value).
    pub(crate) fn reset(&mut self) {
        self.last_amplitude = None;
    }

    /// Read `audio.master` and, when its amplitude changed since the last apply,
    /// return the master-track dB to set. Returns `None` when the slot is absent
    /// (no mod declared it), non-numeric, or unchanged — so the caller issues no
    /// audio command. The App applies the returned dB via `Audio::set_main_volume`.
    pub(crate) fn poll(&mut self) -> Option<f32> {
        let amplitude = match read_store_slot(&self.ctx, MASTER_SLOT) {
            Ok(SlotValue::Number(value)) => value,
            // Slot absent (no mod declared it) or wrong type: nothing to apply.
            _ => return None,
        };

        // Change-gate on the raw amplitude so float dB jitter never re-issues.
        if self.last_amplitude == Some(amplitude) {
            return None;
        }
        self.last_amplitude = Some(amplitude);
        Some(amplitude_to_decibels(amplitude))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::primitives::store::write_store_slot;
    use crate::scripting::slot_table::{
        NumericRange, SlotOwnership, SlotRecord, SlotSchema, SlotType,
    };

    /// Declare a writable mod-style `audio.master` Number slot at `value`,
    /// mirroring the demo mod's `defineStore`.
    fn ctx_with_master(value: f32) -> ScriptCtx {
        let ctx = ScriptCtx::new();
        ctx.slot_table
            .borrow_mut()
            .insert_namespace(
                "audio",
                vec![(
                    "master".to_string(),
                    SlotRecord::new(SlotSchema {
                        slot_type: SlotType::Number,
                        default: Some(SlotValue::Number(value)),
                        range: Some(NumericRange { min: 0.0, max: 1.0 }),
                        persist: false,
                        readonly: false,
                        ownership: SlotOwnership::Mod,
                    }),
                )],
            )
            .expect("audio.master declares cleanly");
        ctx
    }

    #[test]
    fn unity_amplitude_maps_to_zero_decibels() {
        assert!((amplitude_to_decibels(1.0) - 0.0).abs() < 1e-4);
    }

    #[test]
    fn zero_amplitude_maps_to_the_mute_floor() {
        // 0 maps to the mute floor, not -inf, so the volume command stays finite.
        assert_eq!(amplitude_to_decibels(0.0), MUTE_FLOOR_DB);
        // A negative amplitude (out of range) also floors rather than NaNs.
        assert_eq!(amplitude_to_decibels(-0.5), MUTE_FLOOR_DB);
    }

    #[test]
    fn half_amplitude_is_about_minus_six_decibels() {
        // 20*log10(0.5) ≈ -6.02 dB.
        assert!((amplitude_to_decibels(0.5) - (-6.0206)).abs() < 1e-2);
    }

    #[test]
    fn poll_applies_initial_value_then_only_on_change() {
        let ctx = ctx_with_master(1.0);
        let mut consumer = AudioMasterConsumer::new(ctx.clone());

        // First poll applies the declared value (0 dB at unity).
        let first = consumer.poll().expect("initial value applies once");
        assert!((first - 0.0).abs() < 1e-4);

        // Unchanged: no further command.
        assert_eq!(consumer.poll(), None);

        // Slider lowers the amplitude → a new dB is reported once.
        write_store_slot(&ctx, MASTER_SLOT, SlotValue::Number(0.5)).unwrap();
        let lowered = consumer.poll().expect("a changed amplitude re-applies");
        assert!((lowered - (-6.0206)).abs() < 1e-2);
        assert_eq!(consumer.poll(), None, "unchanged again → no command");
    }

    #[test]
    fn poll_is_a_noop_when_slot_absent() {
        // No mod declared `audio.master`: the consumer applies nothing.
        let ctx = ScriptCtx::new();
        let mut consumer = AudioMasterConsumer::new(ctx);
        assert_eq!(consumer.poll(), None);
    }

    #[test]
    fn mute_via_zero_amplitude_applies_the_floor() {
        let ctx = ctx_with_master(0.0);
        let mut consumer = AudioMasterConsumer::new(ctx);
        assert_eq!(consumer.poll(), Some(MUTE_FLOOR_DB));
    }
}

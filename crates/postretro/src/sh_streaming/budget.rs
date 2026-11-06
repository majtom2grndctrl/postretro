//! Checked host-phase ledgers for SH payloads on their way to the renderer.
//! GPU capacity is renderer truth and has no ledger here.
//! See: context/lib/rendering_pipeline.md §4

use super::controller::ShResidencyControllerError;

/// A current/high-water host phase. Payload ownership moves between phases,
/// so the controller uses this type for every transition instead of adding the
/// phases together into a misleading residency total.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct BytePhase {
    pub(crate) current_bytes: u64,
    pub(crate) high_water_bytes: u64,
}

impl BytePhase {
    pub(crate) fn add(
        &mut self,
        bytes: u64,
        label: &'static str,
    ) -> Result<(), ShResidencyControllerError> {
        self.current_bytes = self
            .current_bytes
            .checked_add(bytes)
            .ok_or(ShResidencyControllerError::AccountingOverflow(label))?;
        self.high_water_bytes = self.high_water_bytes.max(self.current_bytes);
        Ok(())
    }

    pub(crate) fn remove(
        &mut self,
        bytes: u64,
        label: &'static str,
    ) -> Result<(), ShResidencyControllerError> {
        self.current_bytes = self
            .current_bytes
            .checked_sub(bytes)
            .ok_or(ShResidencyControllerError::AccountingUnderflow(label))?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct CpuPhaseLedger {
    pub(crate) encoded: BytePhase,
    pub(crate) decoding: BytePhase,
    pub(crate) ready: BytePhase,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_phases_track_current_and_high_water_separately() {
        let mut cpu = CpuPhaseLedger::default();
        cpu.encoded.add(7, "encoded").unwrap();
        cpu.encoded.remove(7, "encoded").unwrap();
        cpu.decoding.add(9, "decoding").unwrap();
        cpu.decoding.remove(9, "decoding").unwrap();
        cpu.ready.add(11, "ready").unwrap();

        assert_eq!(cpu.encoded.current_bytes, 0);
        assert_eq!(cpu.encoded.high_water_bytes, 7);
        assert_eq!(cpu.decoding.current_bytes, 0);
        assert_eq!(cpu.decoding.high_water_bytes, 9);
        assert_eq!(cpu.ready.current_bytes, 11);
        assert_eq!(cpu.ready.high_water_bytes, 11);
    }

    #[test]
    fn ledger_transitions_reject_overflow_and_underflow() {
        let mut phase = BytePhase::default();
        phase.add(u64::MAX, "ready").unwrap();
        assert!(matches!(
            phase.add(1, "ready"),
            Err(ShResidencyControllerError::AccountingOverflow("ready"))
        ));
        let mut phase = BytePhase::default();
        assert!(matches!(
            phase.remove(1, "ready"),
            Err(ShResidencyControllerError::AccountingUnderflow("ready"))
        ));
    }
}

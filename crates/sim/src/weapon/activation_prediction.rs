//! Connected-client fixed-tick timing. Render catch-up retains every due attempt;
//! resource projections deliberately do not enter this semantic path.
use postretro_combat_model::activation::{ActivationAdvance, advance_activation, start_activation};
use postretro_foundation::{ActivationCursor, ActivationInput, ActivationProgram, ActivationToken};

#[derive(Debug, Default)]
pub struct ClientActivationTiming {
    pub cursor: Option<ActivationCursor>,
}
impl ClientActivationTiming {
    /// Idle/recovery/trigger arbitration is owned by the central weapon machine.
    pub fn start(
        &mut self,
        token: ActivationToken,
        pawn: u32,
        tick: u32,
        program: &ActivationProgram,
    ) -> bool {
        if self.cursor.is_some() {
            return false;
        }
        self.cursor = Some(start_activation(token, pawn, tick, program));
        true
    }
    pub fn tick(
        &mut self,
        program: &ActivationProgram,
        tick: u32,
        input: ActivationInput,
    ) -> ActivationAdvance {
        let Some(cursor) = self.cursor.as_mut() else {
            return ActivationAdvance::default();
        };
        let result = advance_activation(cursor, program, tick, true, input);
        if result.terminal.is_some() {
            self.cursor = None;
        }
        result
    }
    /// Call `tick` for each logical tick in a rendered frame and resolve each returned
    /// shot at rendered aim. A zero-tick frame calls neither start nor advancement.
    pub fn cancel(&mut self, token: ActivationToken) {
        if self.cursor.is_some_and(|cursor| cursor.token == token) {
            self.cursor = None;
        }
    }
}

// Payloads the leaf crate itself writes: run bracketing, producer states and
// captured log records. Every other payload lives with its producer.
// See: context/lib/development_guide.md §6.5

use serde::{Deserialize, Serialize};

/// Whether a producer's events can appear in this run, so an absent family
/// is explained rather than read as zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProducerState {
    Enabled,
    /// Off; `variable` names what turns it on.
    Disabled,
    /// Not known yet; a later `producer_state` settles it.
    Pending,
    /// Cannot run here; `reason` says why.
    Unsupported,
}

/// One producer's state, in `run_start` and as a `producer_state` payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProducerEntry {
    pub producer: String,
    pub state: ProducerState,
    pub variable: Option<String>,
    pub reason: Option<String>,
}

impl ProducerEntry {
    pub fn enabled(producer: &str) -> Self {
        Self::new(producer, ProducerState::Enabled, None, None)
    }

    pub fn disabled(producer: &str, variable: &str) -> Self {
        Self::new(producer, ProducerState::Disabled, Some(variable), None)
    }

    pub fn pending(producer: &str, variable: &str) -> Self {
        Self::new(producer, ProducerState::Pending, Some(variable), None)
    }

    pub fn unsupported(producer: &str, reason: &str) -> Self {
        Self::new(producer, ProducerState::Unsupported, None, Some(reason))
    }

    /// The variable that gates this producer, kept on every later state of a
    /// gated producer so its settle names the same switch its launch did.
    pub fn with_variable(mut self, variable: &str) -> Self {
        self.variable = Some(variable.to_string());
        self
    }

    /// A variable-gated producer: enabled when `value` is `1`, else
    /// disabled naming `variable`.
    pub fn gated(producer: &str, variable: &str, value: Option<&str>) -> Self {
        if value == Some("1") {
            Self::new(producer, ProducerState::Enabled, Some(variable), None)
        } else {
            Self::disabled(producer, variable)
        }
    }

    fn new(
        producer: &str,
        state: ProducerState,
        variable: Option<&str>,
        reason: Option<&str>,
    ) -> Self {
        Self {
            producer: producer.to_string(),
            state,
            variable: variable.map(str::to_string),
            reason: reason.map(str::to_string),
        }
    }
}

/// The first event of every run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunStart {
    /// Wall-clock milliseconds since the Unix epoch.
    pub epoch_ms: u64,
    pub argv: Vec<String>,
    pub version: String,
    /// The file sink's absolute path, `None` without one.
    pub sink: Option<String>,
    pub producers: Vec<ProducerEntry>,
}

/// How a run ended. A kill writes no `run_end`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitReason {
    Clean,
    Error,
    Panic,
}

/// The last event of every run the process ends itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunEnd {
    pub reason: ExitReason,
    /// The error, the panic message, or the mode that exited (`capture`,
    /// `headless`).
    pub detail: Option<String>,
}

/// A captured `log` record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogRecord {
    /// `error`, `warn` or `info`.
    pub level: String,
    pub target: String,
    pub message: String,
    /// The message was cut at the message cap.
    pub truncated: bool,
}

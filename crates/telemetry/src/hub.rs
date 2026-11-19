// The per-process hub: sequence, running level, ring and file sink behind one
// lock, so every event's sequence, ring slot and file line agree.
// See: context/lib/development_guide.md §6.5

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, TryLockError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::envelope::{Kind, Level, Source, format_line};
use crate::payload::{ExitReason, LogRecord, ProducerEntry, ProducerState, RunEnd, RunStart};
use crate::ring::{Entry, EventsError, EventsPage, PinKey, Ring, RingCaps};

/// A captured log message longer than this is cut at a char boundary and
/// flagged `truncated`, in the file and the ring alike.
pub const MESSAGE_CAP_BYTES: usize = 4096;

/// How a process starts its hub.
#[derive(Debug, Clone)]
pub struct HubConfig {
    pub source: Source,
    pub version: String,
    pub argv: Vec<String>,
    /// Every producer this process knows, in `run_start` order.
    pub producers: Vec<ProducerEntry>,
    /// The file sink as the variable named it; a relative path resolves
    /// against the working directory.
    pub sink: Option<PathBuf>,
    /// The live ring's caps; `None` keeps no ring.
    pub ring: Option<RingCaps>,
    /// Keep every line in memory, for tests.
    pub memory: bool,
}

impl HubConfig {
    pub fn new(source: Source, version: &str) -> Self {
        Self {
            source,
            version: version.to_string(),
            argv: Vec::new(),
            producers: Vec::new(),
            sink: None,
            ring: None,
            memory: false,
        }
    }
}

pub struct Hub {
    run: String,
    run_json: String,
    source: Source,
    started: Instant,
    state: Mutex<State>,
    startup_warning: Mutex<Option<String>>,
}

struct State {
    next_seq: u64,
    level: Option<Level>,
    file: Option<File>,
    sink_path: Option<PathBuf>,
    ring: Option<Ring>,
    memory: Option<Vec<String>>,
    producers: Vec<ProducerEntry>,
    ended: bool,
}

/// Which level an event's envelope names.
enum LevelChoice<'a> {
    Current,
    Explicit(Option<&'a Level>),
}

impl Hub {
    /// Start a run: resolve and open the sink, then write `run_start` at
    /// sequence 0. A sink that cannot open leaves a warning for
    /// [`Hub::take_startup_warning`], to log once a logger exists.
    pub fn start(config: HubConfig) -> Hub {
        let started = Instant::now();
        let epoch_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_millis() as u64;
        let run = format!("{epoch_ms:x}-{:x}", std::process::id());
        let mut warning = None;
        let sink_path = config.sink.as_deref().map(absolute_sink_path);
        let file = sink_path.as_deref().and_then(|path| {
            match OpenOptions::new().create(true).append(true).open(path) {
                Ok(file) => Some(file),
                Err(error) => {
                    warning = Some(format!(
                        "[Telemetry] cannot open sink {}: {error}; telemetry file disabled",
                        path.display()
                    ));
                    None
                }
            }
        });
        let run_start = RunStart {
            epoch_ms,
            argv: config.argv,
            version: config.version,
            sink: sink_path.as_ref().map(|path| path.display().to_string()),
            producers: config.producers.clone(),
        };
        let hub = Hub {
            run_json: serde_json::to_string(&run).expect("a string serializes"),
            run,
            source: config.source,
            started,
            state: Mutex::new(State {
                next_seq: 0,
                level: None,
                file,
                sink_path,
                ring: config.ring.map(Ring::new),
                memory: config.memory.then(Vec::new),
                producers: config.producers,
                ended: false,
            }),
            startup_warning: Mutex::new(warning),
        };
        hub.emit(Kind::RunStart, &run_start);
        hub
    }

    pub fn run_id(&self) -> &str {
        &self.run
    }

    pub fn source(&self) -> Source {
        self.source
    }

    /// The sink's absolute path, whether or not it opened.
    pub fn sink_path(&self) -> Option<PathBuf> {
        self.lock().sink_path.clone()
    }

    pub fn has_ring(&self) -> bool {
        self.lock().ring.is_some()
    }

    /// The sequence the next event will take.
    pub fn head(&self) -> u64 {
        self.lock().next_seq
    }

    /// The warning a failed sink open left, once.
    pub fn take_startup_warning(&self) -> Option<String> {
        self.startup_warning
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
    }

    /// Every line written so far, newline included, when started with
    /// `memory`.
    pub fn lines(&self) -> Vec<String> {
        self.lock().memory.clone().unwrap_or_default()
    }

    pub fn level(&self) -> Option<Level> {
        self.lock().level.clone()
    }

    /// The running level every later envelope names, until replaced.
    pub fn set_level(&self, level: Option<Level>) {
        self.lock().level = level;
    }

    /// Write `kind` under the running level.
    pub fn emit<P: Serialize + ?Sized>(&self, kind: Kind, payload: &P) {
        self.record(kind, LevelChoice::Current, payload, pin_for(kind));
    }

    /// Write `kind` naming `level` rather than the running level: an unload
    /// and its partial windows name the level they end.
    pub fn emit_for_level<P: Serialize + ?Sized>(
        &self,
        kind: Kind,
        level: Option<&Level>,
        payload: &P,
    ) {
        self.record(kind, LevelChoice::Explicit(level), payload, pin_for(kind));
    }

    /// Settle one producer's state and write it as `producer_state`.
    pub fn set_producer_state(&self, entry: ProducerEntry) {
        let Some(payload) = to_json(&entry) else {
            return;
        };
        let pin = Some(PinKey::Producer(entry.producer.clone()));
        let warning = {
            let mut state = self.lock();
            if state.ended {
                return;
            }
            match state
                .producers
                .iter_mut()
                .find(|known| known.producer == entry.producer)
            {
                Some(known) => *known = entry,
                None => state.producers.push(entry),
            }
            self.write_locked(
                &mut state,
                Kind::ProducerState,
                LevelChoice::Current,
                &payload,
                pin,
            )
        };
        emit_warning(warning);
    }

    /// The state `run_start` or the latest `producer_state` gave `producer`.
    pub fn producer_state(&self, producer: &str) -> Option<ProducerState> {
        self.lock()
            .producers
            .iter()
            .find(|known| known.producer == producer)
            .map(|known| known.state)
    }

    /// Capture one log record: its message cut at [`MESSAGE_CAP_BYTES`].
    pub fn log_record(&self, level: log::Level, target: &str, message: &str) {
        let (message, truncated) = truncate_message(message, MESSAGE_CAP_BYTES);
        let record = LogRecord {
            level: level.as_str().to_ascii_lowercase(),
            target: target.to_string(),
            message: message.to_string(),
            truncated,
        };
        self.emit(Kind::Log, &record);
    }

    /// End the run: settle any producer still pending as unsupported, then
    /// write `run_end`. Nothing writes after it, from any thread.
    pub fn end_run(&self, reason: ExitReason, detail: Option<&str>) {
        let mut state = self.lock();
        let warning = self.end_run_locked(&mut state, reason, detail);
        drop(state);
        emit_warning(warning);
    }

    /// [`Hub::end_run`] for a panic: never waits on a lock another thread
    /// may hold for long. Retries briefly, then gives up. Returns whether
    /// `run_end` was written.
    pub fn end_run_without_waiting(&self, reason: ExitReason, detail: Option<&str>) -> bool {
        for _ in 0..100 {
            match self.state.try_lock() {
                Ok(mut state) => {
                    let _ = self.end_run_locked(&mut state, reason, detail);
                    return true;
                }
                Err(TryLockError::Poisoned(poisoned)) => {
                    let mut state = poisoned.into_inner();
                    let _ = self.end_run_locked(&mut state, reason, detail);
                    return true;
                }
                Err(TryLockError::WouldBlock) => std::thread::sleep(Duration::from_millis(1)),
            }
        }
        false
    }

    pub fn has_ended(&self) -> bool {
        self.lock().ended
    }

    /// One `events` page, copied out under the lock. `kinds` names wire
    /// kinds; an unknown one is an error.
    pub fn page(
        &self,
        since: u64,
        max: usize,
        kinds: Option<&[String]>,
    ) -> Result<EventsPage, EventsError> {
        let kinds = kinds.map(|names| {
            names
                .iter()
                .map(|name| Kind::parse(name).ok_or(name))
                .collect::<Result<Vec<Kind>, &String>>()
        });
        let state = self.lock();
        let head = state.next_seq;
        let Some(ring) = state.ring.as_ref() else {
            return Err(EventsError::NoRing {
                head,
                run: self.run.clone(),
            });
        };
        let kinds = match kinds.transpose() {
            Ok(kinds) => kinds,
            Err(unknown) => {
                return Err(EventsError::UnknownKind {
                    kind: unknown.clone(),
                    head,
                    run: self.run.clone(),
                });
            }
        };
        if since > head {
            return Err(EventsError::SinceBeyondHead {
                since,
                head,
                run: self.run.clone(),
            });
        }
        let (events, next) = ring.page(since, max, kinds.as_deref());
        Ok(EventsPage {
            events,
            next,
            head,
            oldest: ring.oldest(head),
            dropped: ring.dropped(),
            run: self.run.clone(),
        })
    }

    fn end_run_locked(
        &self,
        state: &mut State,
        reason: ExitReason,
        detail: Option<&str>,
    ) -> Option<String> {
        if state.ended {
            return None;
        }
        let mut warning = None;
        let pending: Vec<String> = state
            .producers
            .iter()
            .filter(|known| known.state == ProducerState::Pending)
            .map(|known| known.producer.clone())
            .collect();
        for producer in pending {
            let entry = ProducerEntry::unsupported(&producer, "the run ended before it settled");
            if let Some(known) = state.producers.iter_mut().find(|k| k.producer == producer) {
                *known = entry.clone();
            }
            if let Some(payload) = to_json(&entry) {
                warning = warning.or(self.write_locked(
                    state,
                    Kind::ProducerState,
                    LevelChoice::Current,
                    &payload,
                    Some(PinKey::Producer(producer)),
                ));
            }
        }
        let run_end = RunEnd {
            reason,
            detail: detail.map(str::to_string),
        };
        if let Some(payload) = to_json(&run_end) {
            warning = warning.or(self.write_locked(
                state,
                Kind::RunEnd,
                LevelChoice::Current,
                &payload,
                None,
            ));
        }
        state.ended = true;
        warning
    }

    fn record<P: Serialize + ?Sized>(
        &self,
        kind: Kind,
        level: LevelChoice<'_>,
        payload: &P,
        pin: Option<PinKey>,
    ) {
        // Serialized before the lock: only the sequence, the envelope and the
        // writes need it.
        let Some(payload) = to_json(payload) else {
            return;
        };
        let warning = {
            let mut state = self.lock();
            if state.ended {
                return;
            }
            self.write_locked(&mut state, kind, level, &payload, pin)
        };
        emit_warning(warning);
    }

    /// Take the sequence, then write the line to the file, the memory copy
    /// and the ring, all under the caller's lock. Returns a failed write's
    /// warning for the caller to log after releasing the lock.
    fn write_locked(
        &self,
        state: &mut State,
        kind: Kind,
        level: LevelChoice<'_>,
        payload: &str,
        pin: Option<PinKey>,
    ) -> Option<String> {
        let t_ms = self.started.elapsed().as_millis() as u64;
        let seq = state.next_seq;
        state.next_seq += 1;
        let line = {
            let level = match level {
                LevelChoice::Current => state.level.as_ref(),
                LevelChoice::Explicit(level) => level,
            };
            format_line(&self.run_json, seq, t_ms, self.source, level, kind, payload)
        };
        let mut warning = None;
        if let Some(file) = state.file.as_mut()
            && let Err(error) = file.write_all(line.as_bytes())
        {
            state.file = None;
            warning = Some(format!(
                "[Telemetry] write to {} failed: {error}; telemetry file disabled",
                state
                    .sink_path
                    .as_deref()
                    .map(Path::display)
                    .map_or_else(String::new, |path| path.to_string())
            ));
        }
        if let Some(memory) = state.memory.as_mut() {
            memory.push(line.clone());
        }
        if let Some(ring) = state.ring.as_mut() {
            ring.insert(
                Entry {
                    seq,
                    kind,
                    line: Arc::from(line.trim_end_matches('\n')),
                },
                pin,
            );
        }
        warning
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Test seam: make the file sink fail on its next write.
    #[cfg(any(test, feature = "test-support"))]
    pub fn break_sink_for_test(&self) {
        let mut state = self.lock();
        if state.file.is_some() {
            // A read-only handle fails every write.
            let path = state.sink_path.clone().expect("an open sink has a path");
            state.file = File::open(path).ok();
        }
    }
}

/// The ring pin an event takes, if any. `producer_state` pins through
/// [`Hub::set_producer_state`], which knows the producer.
fn pin_for(kind: Kind) -> Option<PinKey> {
    match kind {
        Kind::RunStart => Some(PinKey::RunStart),
        kind if kind.is_load_family() => Some(PinKey::Load(kind)),
        _ => None,
    }
}

fn to_json<P: Serialize + ?Sized>(payload: &P) -> Option<String> {
    // Payloads are plain structs with string keys; a failure here is a
    // producer bug, and telemetry never stops the run for it.
    serde_json::to_string(payload).ok()
}

/// Logged outside the hub lock: the tee captures this record too, and
/// capture takes the lock.
fn emit_warning(warning: Option<String>) {
    if let Some(warning) = warning {
        log::warn!("{warning}");
    }
}

fn absolute_sink_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    match std::env::current_dir() {
        Ok(cwd) => cwd.join(path),
        Err(_) => path.to_path_buf(),
    }
}

/// `message` cut to at most `cap` bytes at a char boundary.
pub(crate) fn truncate_message(message: &str, cap: usize) -> (&str, bool) {
    if message.len() <= cap {
        return (message, false);
    }
    let mut end = cap;
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    (&message[..end], true)
}

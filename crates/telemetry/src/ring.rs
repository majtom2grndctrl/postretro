// The bounded in-process ring the live `events` verb pages from.
// See: context/lib/development_guide.md §6.5 · context/lib/networking.md §Not netcode: the live introspection channel

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::envelope::Kind;

/// Separate count and byte caps for typed events and for log records, so a
/// log storm cannot evict the windows and phases a client came for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RingCaps {
    pub event_count: usize,
    pub event_bytes: usize,
    pub log_count: usize,
    pub log_bytes: usize,
}

impl RingCaps {
    /// Sized from the File sink's rates (plan of record, Delegated answers).
    pub const DEFAULT: RingCaps = RingCaps {
        event_count: 4096,
        event_bytes: 4 * 1024 * 1024,
        log_count: 2048,
        log_bytes: 1024 * 1024,
    };
}

impl Default for RingCaps {
    fn default() -> Self {
        Self::DEFAULT
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Entry {
    pub(crate) seq: u64,
    pub(crate) kind: Kind,
    /// The envelope line without its newline.
    pub(crate) line: Arc<str>,
}

/// What stays outside the caps: the run header, the latest state of each
/// producer, and the latest event of each load-family kind.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum PinKey {
    RunStart,
    Producer(String),
    Load(Kind),
}

#[derive(Debug)]
pub(crate) struct Ring {
    caps: RingCaps,
    events: BTreeMap<u64, Entry>,
    event_bytes: usize,
    logs: BTreeMap<u64, Entry>,
    log_bytes: usize,
    pins: BTreeMap<PinKey, Entry>,
    /// Every event below this sequence was evicted; a superseded pin below it
    /// would already have been evicted under the caps, so it drops.
    events_evicted_below: u64,
    dropped: u64,
}

/// One page copied out under the hub lock and serialized outside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventsPage {
    /// Envelope lines, in sequence order, without newlines.
    pub events: Vec<Arc<str>>,
    /// The resume cursor; equals `since` when nothing was consumed.
    pub next: u64,
    /// The sequence the next event will take.
    pub head: u64,
    /// The oldest retained sequence, pins ignored; `head` when nothing
    /// unpinned is retained.
    pub oldest: u64,
    pub dropped: u64,
    pub run: String,
}

impl EventsPage {
    /// The `events` reply body: `{"events":[…],"next":…,"head":…,"oldest":…,"dropped":…,"run":…}`.
    pub fn to_json(&self) -> String {
        let mut body = String::with_capacity(
            64 + self.events.iter().map(|line| line.len() + 1).sum::<usize>(),
        );
        body.push_str("{\"events\":[");
        for (index, line) in self.events.iter().enumerate() {
            if index > 0 {
                body.push(',');
            }
            body.push_str(line);
        }
        body.push_str(&format!(
            "],\"next\":{},\"head\":{},\"oldest\":{},\"dropped\":{},\"run\":{}}}",
            self.next,
            self.head,
            self.oldest,
            self.dropped,
            serde_json::to_string(&self.run).expect("a string serializes"),
        ));
        body
    }
}

/// Why a page could not be served. Both name the run and `head` so a client
/// can tell a restarted process from its own bad cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventsError {
    SinceBeyondHead {
        since: u64,
        head: u64,
        run: String,
    },
    UnknownKind {
        kind: String,
        head: u64,
        run: String,
    },
    /// The process keeps no ring (it was not launched with `--observe-live`).
    NoRing {
        head: u64,
        run: String,
    },
}

impl std::fmt::Display for EventsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EventsError::SinceBeyondHead { since, head, run } => {
                write!(f, "since {since} is beyond head {head} of run {run}")
            }
            EventsError::UnknownKind { kind, head, run } => {
                write!(f, "unknown event kind `{kind}` (run {run}, head {head})")
            }
            EventsError::NoRing { head, run } => {
                write!(f, "run {run} keeps no event ring (head {head})")
            }
        }
    }
}

impl std::error::Error for EventsError {}

impl Ring {
    pub(crate) fn new(caps: RingCaps) -> Self {
        Self {
            caps,
            events: BTreeMap::new(),
            event_bytes: 0,
            logs: BTreeMap::new(),
            log_bytes: 0,
            pins: BTreeMap::new(),
            events_evicted_below: 0,
            dropped: 0,
        }
    }

    pub(crate) fn insert(&mut self, entry: Entry, pin: Option<PinKey>) {
        if let Some(key) = pin {
            if let Some(superseded) = self.pins.insert(key, entry) {
                self.fall_under_caps(superseded);
            }
            return;
        }
        if entry.kind == Kind::Log {
            self.log_bytes += entry.line.len();
            self.logs.insert(entry.seq, entry);
            self.enforce_log_caps();
        } else {
            self.event_bytes += entry.line.len();
            self.events.insert(entry.seq, entry);
            self.enforce_event_caps();
        }
    }

    /// A superseded pin is retained as if it had never been pinned: held in
    /// place under the caps, or dropped at once if eviction has passed it.
    fn fall_under_caps(&mut self, entry: Entry) {
        if entry.seq < self.events_evicted_below {
            self.dropped += 1;
            return;
        }
        self.event_bytes += entry.line.len();
        self.events.insert(entry.seq, entry);
        self.enforce_event_caps();
    }

    fn enforce_event_caps(&mut self) {
        while self.events.len() > self.caps.event_count || self.event_bytes > self.caps.event_bytes
        {
            let Some((seq, evicted)) = self.events.pop_first() else {
                break;
            };
            self.event_bytes -= evicted.line.len();
            self.events_evicted_below = self.events_evicted_below.max(seq + 1);
            self.dropped += 1;
        }
    }

    fn enforce_log_caps(&mut self) {
        while self.logs.len() > self.caps.log_count || self.log_bytes > self.caps.log_bytes {
            let Some((_, evicted)) = self.logs.pop_first() else {
                break;
            };
            self.log_bytes -= evicted.line.len();
            self.dropped += 1;
        }
    }

    pub(crate) fn oldest(&self, head: u64) -> u64 {
        let events = self.events.first_key_value().map(|(seq, _)| *seq);
        let logs = self.logs.first_key_value().map(|(seq, _)| *seq);
        match (events, logs) {
            (Some(a), Some(b)) => a.min(b),
            (Some(a), None) | (None, Some(a)) => a,
            (None, None) => head,
        }
    }

    pub(crate) fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Pins at or after `since`, then everything retained from `since`, in one
    /// merged sequence order; a pin is served once, in its place. Filtered-out
    /// kinds still advance the cursor. Stops before the `max + 1`th match.
    pub(crate) fn page(
        &self,
        since: u64,
        max: usize,
        kinds: Option<&[Kind]>,
    ) -> (Vec<Arc<str>>, u64) {
        let mut out = Vec::new();
        let mut next = since;
        if max == 0 {
            return (out, next);
        }
        let mut pins: Vec<&Entry> = self.pins.values().filter(|e| e.seq >= since).collect();
        pins.sort_by_key(|entry| entry.seq);
        let mut pins = pins.into_iter().peekable();
        let mut events = self.events.range(since..).map(|(_, e)| e).peekable();
        let mut logs = self.logs.range(since..).map(|(_, e)| e).peekable();
        loop {
            let candidates = [
                pins.peek().map(|e| e.seq),
                events.peek().map(|e| e.seq),
                logs.peek().map(|e| e.seq),
            ];
            let Some((source, _)) = candidates
                .iter()
                .enumerate()
                .filter_map(|(index, seq)| seq.map(|seq| (index, seq)))
                .min_by_key(|(_, seq)| *seq)
            else {
                break;
            };
            let entry = match source {
                0 => pins.peek(),
                1 => events.peek(),
                _ => logs.peek(),
            }
            .copied()
            .expect("the chosen source has a next entry");
            let wanted = kinds.is_none_or(|kinds| kinds.contains(&entry.kind));
            if wanted && out.len() == max {
                break;
            }
            if wanted {
                out.push(Arc::clone(&entry.line));
            }
            next = entry.seq + 1;
            match source {
                0 => {
                    pins.next();
                }
                1 => {
                    events.next();
                }
                _ => {
                    logs.next();
                }
            }
        }
        (out, next)
    }
}

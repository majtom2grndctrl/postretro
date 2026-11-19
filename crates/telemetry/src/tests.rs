// Hub, ring and tee contracts: one lock for sequence, ring and file; caps,
// pins and paging; sink failure; log capture.
// See: context/lib/development_guide.md §6.5

use std::path::PathBuf;
use std::sync::{Arc, Barrier, Mutex};

use log::{Level as LogLevel, LevelFilter, Log, Metadata, Record};
use postretro_test_log_capture::LogCapture;

use crate::*;

fn config(ring: Option<RingCaps>) -> HubConfig {
    HubConfig {
        ring,
        memory: true,
        producers: vec![
            ProducerEntry::gated("cpu_timing", "POSTRETRO_CPU_TIMING", None),
            ProducerEntry::pending("gpu_timing", "POSTRETRO_GPU_TIMING"),
        ],
        ..HubConfig::new(Source::Engine, "test")
    }
}

fn parse(line: &str) -> Envelope {
    Envelope::parse_line(line).unwrap_or_else(|error| panic!("{error}: {line}"))
}

fn seqs(page: &EventsPage) -> Vec<u64> {
    page.events.iter().map(|line| parse(line).seq).collect()
}

fn kinds(page: &EventsPage) -> Vec<Kind> {
    page.events.iter().map(|line| parse(line).kind).collect()
}

fn scratch_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "postretro-telemetry-{name}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[derive(serde::Serialize)]
struct Probe {
    n: u64,
}

// --- run bracketing -------------------------------------------------------

#[test]
fn run_start_is_sequence_zero_and_lists_every_producer() {
    let hub = Hub::start(config(None));
    let lines = hub.lines();
    let first = parse(&lines[0]);
    assert_eq!(first.seq, 0);
    assert_eq!(first.kind, Kind::RunStart);
    assert_eq!(first.level, None);
    let start: RunStart = serde_json::from_value(first.payload).unwrap();
    assert_eq!(start.producers.len(), 2);
    assert_eq!(start.producers[0].state, ProducerState::Disabled);
    assert_eq!(
        start.producers[0].variable.as_deref(),
        Some("POSTRETRO_CPU_TIMING")
    );
    assert_eq!(start.producers[1].state, ProducerState::Pending);
}

#[test]
fn relative_sink_resolves_against_the_working_directory() {
    let hub = Hub::start(HubConfig {
        sink: Some(PathBuf::from("some/relative.jsonl")),
        ..config(None)
    });
    let expected = std::env::current_dir().unwrap().join("some/relative.jsonl");
    assert_eq!(hub.sink_path(), Some(expected.clone()));
    let start: RunStart = serde_json::from_value(parse(&hub.lines()[0]).payload).unwrap();
    assert_eq!(start.sink, Some(expected.display().to_string()));
    // The directory does not exist, so the open failed and warned.
    assert!(hub.take_startup_warning().is_some());
    assert!(hub.take_startup_warning().is_none(), "warns once");
}

#[test]
fn run_end_settles_pending_producers_and_nothing_writes_after_it() {
    let hub = Hub::start(config(Some(RingCaps::DEFAULT)));
    hub.emit(Kind::ShStreamingWindow, &Probe { n: 1 });
    hub.end_run(ExitReason::Clean, None);
    hub.emit(Kind::ShStreamingWindow, &Probe { n: 2 });
    hub.log_record(LogLevel::Info, "t", "after the end");
    hub.end_run(ExitReason::Error, Some("again"));
    let lines = hub.lines();
    let envelopes: Vec<Envelope> = lines.iter().map(|l| parse(l)).collect();
    let last = envelopes.last().unwrap();
    assert_eq!(last.kind, Kind::RunEnd);
    let end: RunEnd = serde_json::from_value(last.payload.clone()).unwrap();
    assert_eq!(end.reason, ExitReason::Clean);
    let settled = &envelopes[envelopes.len() - 2];
    assert_eq!(settled.kind, Kind::ProducerState);
    let entry: ProducerEntry = serde_json::from_value(settled.payload.clone()).unwrap();
    assert_eq!(entry.producer, "gpu_timing");
    assert_eq!(entry.state, ProducerState::Unsupported);
    assert_eq!(
        hub.producer_state("gpu_timing"),
        Some(ProducerState::Unsupported)
    );
    assert_eq!(envelopes.len(), 4, "run_start, window, settle, run_end");
    // The ring ends at run_end too.
    let page = hub.page(0, 100, None).unwrap();
    assert_eq!(*kinds(&page).last().unwrap(), Kind::RunEnd);
}

#[test]
fn a_caught_panic_writes_no_run_end_and_one_leaving_the_guard_does() {
    let hub = Arc::new(Hub::start(config(None)));
    let _scope = scoped::install(Arc::clone(&hub));
    let caught = std::panic::catch_unwind(|| run_guarded(|| ()));
    assert!(caught.is_ok());
    // A worker's panic caught inside the guarded body ends nothing.
    run_guarded(|| {
        let _ = std::thread::spawn(|| panic!("worker")).join();
    });
    assert!(!hub.has_ended());
    emit(Kind::ShStreamingWindow, &Probe { n: 1 });
    let escaped = std::panic::catch_unwind(|| run_guarded(|| panic!("boom at main")));
    assert!(escaped.is_err());
    let lines = hub.lines();
    let last = parse(lines.last().unwrap());
    assert_eq!(last.kind, Kind::RunEnd);
    let end: RunEnd = serde_json::from_value(last.payload).unwrap();
    assert_eq!(end.reason, ExitReason::Panic);
    assert_eq!(end.detail.as_deref(), Some("boom at main"));
    assert!(
        lines
            .iter()
            .any(|l| parse(l).kind == Kind::ShStreamingWindow)
    );
}

// --- envelope level ---------------------------------------------------------

#[test]
fn envelopes_name_the_running_level_or_the_one_given() {
    let hub = Hub::start(config(None));
    let old = Level {
        generation: 1,
        map: String::from("a.prl"),
    };
    hub.set_level(Some(old.clone()));
    hub.emit(Kind::CpuTimingWindow, &Probe { n: 1 });
    hub.set_level(None);
    hub.emit_for_level(Kind::Unload, Some(&old), &Probe { n: 2 });
    hub.emit(Kind::GeometryInstall, &Probe { n: 3 });
    let envelopes: Vec<Envelope> = hub.lines().iter().map(|l| parse(l)).collect();
    assert_eq!(envelopes[1].level, Some(old.clone()));
    assert_eq!(envelopes[2].level, Some(old));
    assert_eq!(envelopes[3].level, None);
}

// --- file sink ---------------------------------------------------------------

#[test]
fn file_sink_appends_one_whole_line_per_event() {
    let dir = scratch_dir("append");
    let path = dir.join("t.jsonl");
    let hub = Hub::start(HubConfig {
        sink: Some(path.clone()),
        ..config(None)
    });
    hub.emit(Kind::BakeStage, &Probe { n: 1 });
    hub.end_run(ExitReason::Clean, None);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.ends_with('\n'));
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 4, "run_start, stage, settle, run_end");
    for (index, line) in lines.iter().enumerate() {
        assert_eq!(parse(line).seq, index as u64);
    }
}

// P10: the warning is formatted and logged outside the sink lock, so a tee
// capturing it cannot deadlock against the write that failed.
#[test]
fn a_write_failing_mid_run_warns_once_disables_the_file_and_the_ring_continues() {
    let logs = LogCapture::start();
    let dir = scratch_dir("midrun");
    let path = dir.join("t.jsonl");
    let hub = Arc::new(Hub::start(HubConfig {
        sink: Some(path.clone()),
        ..config(Some(RingCaps::DEFAULT))
    }));
    let _scope = scoped::install(Arc::clone(&hub));
    hub.emit(Kind::CpuTimingWindow, &Probe { n: 1 });
    hub.break_sink_for_test();
    let before = std::fs::read_to_string(&path).unwrap();
    hub.emit(Kind::CpuTimingWindow, &Probe { n: 2 });
    hub.emit(Kind::CpuTimingWindow, &Probe { n: 3 });
    logs.assert_logged_once(LogLevel::Warn, "telemetry file disabled");
    // A tee capturing a record takes the hub lock: it must be free again.
    let tee = TeeLogger::new(Box::new(CaptureLogger::default()));
    tee.log(
        &Record::builder()
            .level(LogLevel::Warn)
            .target("t")
            .args(format_args!("after the failure"))
            .build(),
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    let page = hub.page(0, 100, None).unwrap();
    assert_eq!(page.events.len(), 5, "run_start, three windows, the log");
    assert_eq!(*kinds(&page).last().unwrap(), Kind::Log);
}

#[test]
fn an_unwritable_sink_warns_once_and_the_run_completes() {
    let dir = scratch_dir("unwritable");
    // A directory cannot be opened for append.
    let hub = Hub::start(HubConfig {
        sink: Some(dir.clone()),
        ..config(None)
    });
    let warning = hub.take_startup_warning().expect("the open failed");
    assert!(warning.contains("telemetry file disabled"), "{warning}");
    assert!(hub.take_startup_warning().is_none());
    hub.emit(Kind::CpuTimingWindow, &Probe { n: 1 });
    hub.end_run(ExitReason::Clean, None);
    // run_start, the window, the pending GPU producer settled, run_end.
    assert_eq!(hub.lines().len(), 4);
}

// --- message cap --------------------------------------------------------------

#[test]
fn an_over_cap_message_truncates_at_a_char_boundary_and_is_flagged() {
    let hub = Hub::start(config(Some(RingCaps::DEFAULT)));
    let long = "é".repeat(MESSAGE_CAP_BYTES); // two bytes each
    hub.log_record(LogLevel::Warn, "t", &long);
    hub.log_record(LogLevel::Info, "t", "short");
    let lines = hub.lines();
    let record: LogRecord = serde_json::from_value(parse(&lines[1]).payload).unwrap();
    assert!(record.truncated);
    assert!(record.message.len() <= MESSAGE_CAP_BYTES);
    assert_eq!(record.level, "warn");
    let short: LogRecord = serde_json::from_value(parse(&lines[2]).payload).unwrap();
    assert!(!short.truncated);
}

// --- ring caps and eviction ---------------------------------------------------

fn small_caps() -> RingCaps {
    RingCaps {
        event_count: 4,
        event_bytes: 1 << 20,
        log_count: 3,
        log_bytes: 1 << 20,
    }
}

#[test]
fn overfilling_events_by_count_evicts_oldest_first_and_counts_drops() {
    let hub = Hub::start(config(Some(small_caps())));
    for n in 0..10 {
        hub.emit(Kind::CpuTimingWindow, &Probe { n });
    }
    let page = hub.page(0, 100, None).unwrap();
    // run_start is pinned; four windows retained.
    assert_eq!(seqs(&page), vec![0, 7, 8, 9, 10]);
    assert_eq!(page.dropped, 6);
    assert_eq!(page.oldest, 7);
    assert_eq!(page.head, 11);
}

#[test]
fn overfilling_events_by_bytes_evicts_oldest_first() {
    let hub = Hub::start(config(Some(RingCaps {
        event_bytes: 300,
        ..small_caps()
    })));
    for n in 0..4 {
        hub.emit(Kind::CpuTimingWindow, &Probe { n });
    }
    let page = hub.page(0, 100, None).unwrap();
    let retained: Vec<u64> = seqs(&page).into_iter().filter(|s| *s > 0).collect();
    let bytes: usize = page.events.iter().skip(1).map(|l| l.len()).sum();
    assert!(bytes <= 300, "{bytes}");
    assert!(page.dropped >= 1);
    assert!(retained.windows(2).all(|w| w[0] < w[1]));
    assert_eq!(*retained.last().unwrap(), 4);
}

#[test]
fn overfilling_logs_by_count_and_bytes_evicts_only_logs() {
    let hub = Hub::start(config(Some(small_caps())));
    hub.emit(Kind::CpuTimingWindow, &Probe { n: 0 });
    for n in 0..5 {
        hub.log_record(LogLevel::Info, "t", &format!("record {n}"));
    }
    let page = hub.page(0, 100, None).unwrap();
    assert_eq!(seqs(&page), vec![0, 1, 4, 5, 6]);
    assert_eq!(page.dropped, 2);

    let hub = Hub::start(config(Some(RingCaps {
        log_bytes: 500,
        ..small_caps()
    })));
    for n in 0..3 {
        hub.log_record(LogLevel::Info, "t", &format!("record {n}"));
    }
    let page = hub.page(0, 100, None).unwrap();
    let log_bytes: usize = page.events.iter().skip(1).map(|l| l.len()).sum();
    assert!(log_bytes <= 500);
    assert!(page.dropped >= 1);
}

// --- paging --------------------------------------------------------------------

#[test]
fn since_at_head_is_empty_and_beyond_head_names_run_and_head() {
    let hub = Hub::start(config(Some(RingCaps::DEFAULT)));
    let head = hub.head();
    let page = hub.page(head, 10, None).unwrap();
    assert!(page.events.is_empty());
    assert_eq!(page.next, head);
    assert_eq!(page.oldest, head, "nothing unpinned retained");
    assert_eq!(
        page.to_json(),
        format!(
            "{{\"events\":[],\"next\":{head},\"head\":{head},\"oldest\":{head},\"dropped\":0,\"run\":\"{}\"}}",
            hub.run_id()
        )
    );
    match hub.page(head + 1, 10, None) {
        Err(EventsError::SinceBeyondHead { head: h, run, .. }) => {
            assert_eq!(h, head);
            assert_eq!(run, hub.run_id());
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn max_zero_returns_head_and_polling_from_it_sees_only_newer_events() {
    let hub = Hub::start(config(Some(RingCaps::DEFAULT)));
    hub.emit(Kind::CpuTimingWindow, &Probe { n: 1 });
    let probe = hub.page(0, 0, None).unwrap();
    assert!(probe.events.is_empty());
    assert_eq!(probe.next, 0);
    assert_eq!(probe.head, 2);
    hub.emit(Kind::CpuTimingWindow, &Probe { n: 2 });
    let page = hub.page(probe.head, 10, None).unwrap();
    assert_eq!(seqs(&page), vec![2]);
}

#[test]
fn since_older_than_oldest_returns_from_the_oldest_and_shows_the_gap() {
    let hub = Hub::start(config(Some(small_caps())));
    for n in 0..10 {
        hub.emit(Kind::CpuTimingWindow, &Probe { n });
    }
    let page = hub.page(1, 100, None).unwrap();
    assert_eq!(seqs(&page), vec![7, 8, 9, 10]);
    assert!(page.oldest > 1);
    assert_eq!(page.dropped, 6);
}

#[test]
fn kinds_filter_returns_only_those_kinds_and_advances_past_the_rest() {
    let hub = Hub::start(config(Some(RingCaps::DEFAULT)));
    hub.emit(Kind::CpuTimingWindow, &Probe { n: 1 });
    hub.emit(Kind::GpuTimingWindow, &Probe { n: 2 });
    hub.emit(Kind::CpuTimingWindow, &Probe { n: 3 });
    hub.emit(Kind::GpuTimingWindow, &Probe { n: 4 });
    let wanted = vec![String::from("cpu_timing_window")];
    let page = hub.page(0, 10, Some(&wanted)).unwrap();
    assert_eq!(seqs(&page), vec![1, 3]);
    assert_eq!(page.next, 5);
    let unknown = vec![String::from("no_such_kind")];
    assert!(matches!(
        hub.page(0, 10, Some(&unknown)),
        Err(EventsError::UnknownKind { .. })
    ));
}

#[test]
fn two_polls_are_disjoint_and_contiguous_from_the_first_since() {
    let hub = Hub::start(config(Some(RingCaps::DEFAULT)));
    hub.emit(Kind::CpuTimingWindow, &Probe { n: 1 });
    let first = hub.page(0, 100, None).unwrap();
    hub.emit(Kind::CpuTimingWindow, &Probe { n: 2 });
    hub.log_record(LogLevel::Info, "t", "between");
    let second = hub.page(first.next, 100, None).unwrap();
    let mut all = seqs(&first);
    all.extend(seqs(&second));
    assert_eq!(all, (0..hub.head()).collect::<Vec<_>>());
}

// P2: late attach after eviction past the latest load.
#[test]
fn a_late_client_reads_pins_first_then_the_retained_run() {
    let hub = Hub::start(config(Some(small_caps())));
    hub.set_producer_state(ProducerEntry::enabled("gpu_timing"));
    hub.emit(Kind::LoadPhases, &Probe { n: 1 }); // seq 2
    for n in 0..8 {
        hub.emit(Kind::CpuTimingWindow, &Probe { n });
    }
    let page = hub.page(0, 100, None).unwrap();
    assert_eq!(
        kinds(&page)[..3],
        [Kind::RunStart, Kind::ProducerState, Kind::LoadPhases]
    );
    assert_eq!(seqs(&page), vec![0, 1, 2, 7, 8, 9, 10]);
    assert!(page.oldest > 2);

    // A newer load mid-pagination: the superseded phases fall under the caps,
    // already evicted past, so they drop.
    let dropped = page.dropped;
    hub.emit(Kind::LoadPhases, &Probe { n: 2 });
    let page = hub.page(0, 100, None).unwrap();
    assert_eq!(page.dropped, dropped + 1);
    assert_eq!(
        kinds(&page)
            .iter()
            .filter(|k| **k == Kind::LoadPhases)
            .count(),
        1
    );
}

// P2: paging from 0 with a small max while the latest load is pinned and
// newer than the oldest retained event.
#[test]
fn paging_with_a_small_max_reads_every_event_once_in_order() {
    let hub = Hub::start(config(Some(small_caps())));
    for n in 0..6 {
        hub.emit(Kind::CpuTimingWindow, &Probe { n });
    }
    hub.emit(Kind::LoadPhases, &Probe { n: 9 }); // pinned, newer than oldest
    hub.emit(Kind::CpuTimingWindow, &Probe { n: 7 });
    hub.log_record(LogLevel::Info, "t", "a log");
    let mut since = 0;
    let mut read = Vec::new();
    loop {
        let page = hub.page(since, 2, None).unwrap();
        if page.events.is_empty() {
            break;
        }
        read.extend(seqs(&page));
        since = page.next;
    }
    let mut sorted = read.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(read, sorted, "every event once, in order");
    assert!(read.contains(&7), "the pinned load is served in its place");
    assert_eq!(*read.last().unwrap(), hub.head() - 1);
}

// P5: concurrent emitters on several threads.
#[test]
fn concurrent_emitters_land_in_strictly_increasing_sequence_and_a_poller_misses_none() {
    let hub = Arc::new(Hub::start(config(Some(RingCaps::DEFAULT))));
    let threads = 4;
    let per_thread: usize = 200;
    let barrier = Arc::new(Barrier::new(threads + 1));
    let mut handles = Vec::new();
    for t in 0..threads {
        let hub = Arc::clone(&hub);
        let barrier = Arc::clone(&barrier);
        handles.push(std::thread::spawn(move || {
            barrier.wait();
            for n in 0..per_thread {
                if t % 2 == 0 {
                    hub.emit(Kind::CpuTimingWindow, &Probe { n: n as u64 });
                } else {
                    hub.log_record(LogLevel::Info, "t", "x");
                }
            }
        }));
    }
    barrier.wait();
    let mut since = 0;
    let mut polled = Vec::new();
    while polled.len() < threads * per_thread + 1 {
        let page = hub.page(since, 37, None).unwrap();
        polled.extend(seqs(&page));
        since = page.next;
    }
    for handle in handles {
        handle.join().unwrap();
    }
    let lines = hub.lines();
    let file_seqs: Vec<u64> = lines.iter().map(|l| parse(l).seq).collect();
    assert_eq!(file_seqs, (0..lines.len() as u64).collect::<Vec<_>>());
    assert_eq!(polled, file_seqs);
}

// --- tee logger -----------------------------------------------------------------

#[derive(Clone, Default)]
struct CaptureLogger {
    records: Arc<Mutex<Vec<(LogLevel, String)>>>,
}

impl Log for CaptureLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        // stderr's default filter: errors only.
        metadata.level() <= LevelFilter::Error
    }
    fn log(&self, record: &Record<'_>) {
        self.records
            .lock()
            .unwrap()
            .push((record.level(), record.args().to_string()));
    }
    fn flush(&self) {}
}

fn record(level: LogLevel, message: &str) -> (LogLevel, String) {
    (level, message.to_string())
}

#[test]
fn with_a_consumer_info_is_captured_while_the_stderr_filter_hides_it() {
    let hub = Arc::new(Hub::start(config(Some(RingCaps::DEFAULT))));
    let _scope = scoped::install(Arc::clone(&hub));
    let inner = CaptureLogger::default();
    let tee = TeeLogger::new(Box::new(inner.clone()));
    for (level, message) in [
        record(LogLevel::Info, "an info"),
        record(LogLevel::Debug, "a debug"),
        record(LogLevel::Error, "an error"),
    ] {
        tee.log(
            &Record::builder()
                .level(level)
                .target("t")
                .args(format_args!("{message}"))
                .build(),
        );
    }
    assert_eq!(
        *inner.records.lock().unwrap(),
        vec![record(LogLevel::Error, "an error")],
        "stderr's filter is unchanged"
    );
    let captured: Vec<String> = hub
        .lines()
        .iter()
        .map(|l| parse(l))
        .filter(|e| e.kind == Kind::Log)
        .map(|e| {
            serde_json::from_value::<LogRecord>(e.payload)
                .unwrap()
                .message
        })
        .collect();
    assert_eq!(captured, vec!["an info", "an error"]);
    assert!(tee.enabled(&Metadata::builder().level(LogLevel::Info).build()));
    assert!(!tee.enabled(&Metadata::builder().level(LogLevel::Debug).build()));
}

// A log eviction past `since` shows as a gap even while an older event is
// still retained in the other class, and a superseded pin falling under the
// caps never moves `oldest` back past an eviction.
#[test]
fn a_gap_in_either_class_shows_in_oldest_and_oldest_never_moves_back() {
    let hub = Hub::start(config(Some(small_caps())));
    hub.emit(Kind::GeometryInstall, &Probe { n: 0 }); // seq 1, pinned
    hub.emit(Kind::CpuTimingWindow, &Probe { n: 0 }); // seq 2, retained
    for n in 0..6 {
        hub.log_record(LogLevel::Info, "t", &format!("record {n}")); // seq 3..8
    }
    let page = hub.page(3, 100, None).unwrap();
    assert!(page.oldest > 3, "logs 3..5 were evicted: {}", page.oldest);
    let before = page.oldest;
    hub.emit(Kind::GeometryInstall, &Probe { n: 1 }); // supersedes seq 1
    let page = hub.page(0, 100, None).unwrap();
    assert!(page.oldest >= before, "{} < {before}", page.oldest);
}

// The tee formats before it marks the thread capturing, so a panicking
// `Display` leaves capture open for that thread's later records.
#[test]
fn a_record_whose_display_panics_does_not_stop_later_capture_on_its_thread() {
    struct Boom;
    impl std::fmt::Display for Boom {
        fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            panic!("display panicked")
        }
    }
    let hub = Arc::new(Hub::start(config(Some(RingCaps::DEFAULT))));
    let _scope = scoped::install(Arc::clone(&hub));
    let tee = TeeLogger::new(Box::new(CaptureLogger::default()));
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        tee.log(
            &Record::builder()
                .level(LogLevel::Info)
                .target("t")
                .args(format_args!("{}", Boom))
                .build(),
        );
    }));
    assert!(caught.is_err());
    tee.log(
        &Record::builder()
            .level(LogLevel::Info)
            .target("t")
            .args(format_args!("after"))
            .build(),
    );
    let logs = hub
        .lines()
        .iter()
        .filter(|l| parse(l).kind == Kind::Log)
        .count();
    assert_eq!(logs, 1);
}

#[test]
fn end_level_names_the_level_then_clears_it_and_needs_one() {
    let hub = Hub::start(config(None));
    hub.end_level(Kind::Unload, |_| Probe { n: 0 });
    assert_eq!(hub.lines().len(), 1, "no level, nothing written");
    let level = Level {
        generation: 2,
        map: String::from("m.prl"),
    };
    hub.set_level(Some(level.clone()));
    hub.end_level(Kind::Unload, |level| Probe {
        n: level.generation,
    });
    assert_eq!(hub.level(), None);
    let unload = parse(hub.lines().last().unwrap());
    assert_eq!(unload.level, Some(level));
    assert_eq!(unload.payload["n"], 2);
}

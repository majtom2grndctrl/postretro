//! Decode pool: tier pickup across resources, retirement releases every job.
//! See: context/lib/testing_guide.md

use std::sync::atomic::AtomicI64;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use super::*;
use crate::streaming::request::StreamResource;

/// What happened to each test job, in order, and the bytes jobs still hold.
#[derive(Default)]
struct Journal {
    events: Mutex<Vec<(&'static str, String)>>,
    held: AtomicI64,
}

impl Journal {
    fn push(&self, what: &'static str, name: &str) {
        self.events.lock().unwrap().push((what, name.to_string()));
    }

    fn names(&self, what: &str) -> Vec<String> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|(event, _)| *event == what)
            .map(|(_, name)| name.clone())
            .collect()
    }
}

/// A job charged `bytes` from submission until it delivers, is discarded,
/// or finds the pool retiring. A gated job waits for its gate after it
/// starts.
struct TestJob {
    name: String,
    tier: ReadTier,
    bytes: i64,
    journal: Arc<Journal>,
    started: Option<Sender<()>>,
    gate: Option<Receiver<()>>,
}

impl TestJob {
    fn new(
        journal: &Arc<Journal>,
        resource: StreamResource,
        key: u32,
        tier: ReadTier,
    ) -> Box<Self> {
        journal.held.fetch_add(100, Ordering::SeqCst);
        Box::new(Self {
            name: format!("{}:{key}:{tier:?}", resource.label()),
            tier,
            bytes: 100,
            journal: Arc::clone(journal),
            started: None,
            gate: None,
        })
    }

    fn gated(mut self: Box<Self>) -> (Box<Self>, Receiver<()>, Sender<()>) {
        let (started, started_rx) = channel();
        let (release, gate) = channel();
        self.started = Some(started);
        self.gate = Some(gate);
        (self, started_rx, release)
    }
}

impl DecodeJob for TestJob {
    fn tier(&self) -> ReadTier {
        self.tier
    }

    fn run(self: Box<Self>, retiring: &AtomicBool) {
        self.journal.push("start", &self.name);
        if let Some(started) = &self.started {
            started.send(()).unwrap();
        }
        if let Some(gate) = &self.gate {
            gate.recv().unwrap();
        }
        let what = if retiring.load(Ordering::Acquire) {
            "released"
        } else {
            "delivered"
        };
        self.journal.push(what, &self.name);
        self.journal.held.fetch_sub(self.bytes, Ordering::SeqCst);
    }

    fn discard(self: Box<Self>) {
        self.journal.push("discarded", &self.name);
        self.journal.held.fetch_sub(self.bytes, Ordering::SeqCst);
    }
}

fn wait_for(journal: &Journal, what: &str, count: usize) {
    for _ in 0..2000 {
        if journal.names(what).len() >= count {
            return;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    panic!("timed out waiting for {count} `{what}` events");
}

#[test]
fn decode_pool_size_is_half_the_cores_between_one_and_three() {
    for (available, expected) in [(1, 1), (2, 1), (3, 1), (4, 2), (6, 3), (16, 3)] {
        assert_eq!(decode_pool_size(available), expected, "{available} cores");
    }
}

// P3: a mandatory decode of either resource is taken before every queued
// optional decode of either resource, and waits only for the running one.
#[test]
fn a_mandatory_job_of_either_resource_is_taken_before_every_queued_optional_job() {
    use ReadTier::{Mandatory, Optional};
    use StreamResource::{LightmapBlock, Sh};
    let journal = Arc::new(Journal::default());
    let pool = DecodePool::spawn(1, 8).unwrap();
    let queue = pool.queue();
    let (running, started, release) = TestJob::new(&journal, LightmapBlock, 0, Optional).gated();
    assert!(queue.submit(running));
    started.recv().unwrap();
    for job in [
        TestJob::new(&journal, Sh, 1, Optional),
        TestJob::new(&journal, LightmapBlock, 2, Optional),
        TestJob::new(&journal, LightmapBlock, 3, Mandatory),
        TestJob::new(&journal, Sh, 4, Mandatory),
    ] {
        assert!(queue.submit(job));
    }
    assert_eq!(
        journal.names("start"),
        ["lightmap streaming:0:Optional"],
        "queued jobs wait only for the running one"
    );
    release.send(()).unwrap();
    wait_for(&journal, "delivered", 5);
    assert_eq!(
        journal.names("start"),
        [
            "lightmap streaming:0:Optional",
            "lightmap streaming:3:Mandatory",
            "SH streaming:4:Mandatory",
            "SH streaming:1:Optional",
            "lightmap streaming:2:Optional",
        ]
    );
    drop(pool);
    assert_eq!(journal.held.load(Ordering::SeqCst), 0);
}

// Retirement releases every job: queued ones are discarded at once, the
// running one releases instead of delivering, and a submission after
// retirement is discarded on the spot.
#[test]
fn retiring_releases_every_queued_running_and_late_job() {
    use ReadTier::{Mandatory, Optional};
    use StreamResource::{LightmapBlock, Sh};
    let journal = Arc::new(Journal::default());
    let pool = DecodePool::spawn(1, 8).unwrap();
    let queue = pool.queue();
    let (running, started, release) = TestJob::new(&journal, Sh, 0, Mandatory).gated();
    assert!(queue.submit(running));
    started.recv().unwrap();
    assert!(queue.submit(TestJob::new(&journal, LightmapBlock, 1, Optional)));
    assert!(queue.submit(TestJob::new(&journal, Sh, 2, Mandatory)));

    let handles = pool.begin_retirement();
    assert_eq!(
        journal.names("discarded"),
        ["SH streaming:2:Mandatory", "lightmap streaming:1:Optional"]
    );
    assert!(!queue.submit(TestJob::new(&journal, LightmapBlock, 3, Mandatory)));
    release.send(()).unwrap();
    for handle in handles {
        handle.join().unwrap();
    }
    assert_eq!(journal.names("released"), ["SH streaming:0:Mandatory"]);
    assert!(journal.names("delivered").is_empty());
    assert_eq!(
        journal.held.load(Ordering::SeqCst),
        0,
        "every charge returned"
    );
}

#[test]
fn pool_threads_are_named_for_the_shared_pool() {
    let journal = Arc::new(Journal::default());
    let pool = DecodePool::spawn(2, 4).unwrap();
    struct Named(Arc<Journal>);
    impl DecodeJob for Named {
        fn tier(&self) -> ReadTier {
            ReadTier::Mandatory
        }
        fn run(self: Box<Self>, _: &AtomicBool) {
            let name = std::thread::current()
                .name()
                .unwrap_or_default()
                .to_string();
            self.0.push("delivered", &name);
        }
        fn discard(self: Box<Self>) {}
    }
    assert!(pool.queue().submit(Box::new(Named(Arc::clone(&journal)))));
    wait_for(&journal, "delivered", 1);
    assert!(
        journal.names("delivered")[0].starts_with(DECODE_THREAD_NAME_PREFIX),
        "{:?}",
        journal.names("delivered")
    );
}

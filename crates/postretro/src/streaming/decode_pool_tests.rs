//! Decode pool: rank pickup across resources, retirement releases every job.
//! See: context/lib/testing_guide.md

use std::sync::atomic::AtomicI64;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use super::*;
use crate::streaming::target_set::TargetSet;

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

/// A job charged `bytes` from submission until it delivers, fails, is
/// discarded, finds the pool retiring, or drops. A gated job waits for its
/// gate after it starts; a job over a target set ranks by its key's live
/// class there; a panicking job panics in `run`, and a doubly panicking one
/// in `panicked` too.
struct TestJob {
    name: String,
    resource: StreamResource,
    key: u32,
    class: DrainClass,
    live: Option<Arc<TargetSet>>,
    panics: bool,
    panics_again: bool,
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
        class: DrainClass,
    ) -> Box<Self> {
        journal.held.fetch_add(100, Ordering::SeqCst);
        Box::new(Self {
            name: format!("{}:{key}:{class:?}", resource.label()),
            resource,
            key,
            class,
            live: None,
            panics: false,
            panics_again: false,
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

    fn ranked_by(mut self: Box<Self>, live: &Arc<TargetSet>) -> Box<Self> {
        self.live = Some(Arc::clone(live));
        self
    }

    fn panicking(mut self: Box<Self>) -> Box<Self> {
        self.panics = true;
        self
    }

    fn panicking_twice(mut self: Box<Self>) -> Box<Self> {
        self.panics = true;
        self.panics_again = true;
        self
    }

    fn release(&mut self, what: &'static str) {
        self.journal.push(what, &self.name);
        self.journal
            .held
            .fetch_sub(std::mem::take(&mut self.bytes), Ordering::SeqCst);
    }
}

impl Drop for TestJob {
    fn drop(&mut self) {
        if self.bytes != 0 {
            self.release("dropped");
        }
    }
}

impl DecodeJob for TestJob {
    fn rank(&self) -> DecodeRank {
        match &self.live {
            Some(live) => DecodeRank::live(live, self.key, self.class, self.resource),
            None => DecodeRank::Decode {
                class: self.class,
                resource: self.resource,
            },
        }
    }

    fn run(&mut self, retiring: &AtomicBool, _: Duration) {
        self.journal.push("start", &self.name);
        if let Some(started) = &self.started {
            started.send(()).unwrap();
        }
        if let Some(gate) = &self.gate {
            gate.recv().unwrap();
        }
        if self.panics {
            panic!("test job {} panicked", self.name);
        }
        let what = if retiring.load(Ordering::Acquire) {
            "released"
        } else {
            "delivered"
        };
        self.release(what);
    }

    fn discard(mut self: Box<Self>) {
        self.release("discarded");
    }

    fn panicked(mut self: Box<Self>, _: &AtomicBool) {
        if self.panics_again {
            panic!("test job {} panicked in its cleanup", self.name);
        }
        self.release("failed");
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

// A mandatory decode of either resource is taken before every queued
// optional decode of either resource, and waits only for the running one.
#[test]
fn a_mandatory_job_of_either_resource_is_taken_before_every_queued_optional_job() {
    use DrainClass::{Prefetch as Optional, Visible as Mandatory};
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
        ["lightmap streaming:0:Prefetch"],
        "queued jobs wait only for the running one"
    );
    release.send(()).unwrap();
    wait_for(&journal, "delivered", 5);
    assert_eq!(
        journal.names("start"),
        [
            "lightmap streaming:0:Prefetch",
            "SH streaming:4:Visible",
            "lightmap streaming:3:Visible",
            "SH streaming:1:Prefetch",
            "lightmap streaming:2:Prefetch",
        ],
        "SH goes first at equal class, as the drain admits"
    );
    drop(pool);
    assert_eq!(journal.held.load(Ordering::SeqCst), 0);
}

// Retirement releases every job: queued ones are discarded at once, the
// running one releases instead of delivering, and a submission after
// retirement is discarded on the spot.
#[test]
fn retiring_releases_every_queued_running_and_late_job() {
    use DrainClass::{Prefetch as Optional, Visible as Mandatory};
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
        ["lightmap streaming:1:Prefetch", "SH streaming:2:Visible"],
        "discarded in submission order"
    );
    assert!(!queue.submit(TestJob::new(&journal, LightmapBlock, 3, Mandatory)));
    release.send(()).unwrap();
    for handle in handles {
        handle.join().unwrap();
    }
    assert_eq!(journal.names("released"), ["SH streaming:0:Visible"]);
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
        fn rank(&self) -> DecodeRank {
            DecodeRank::Decode {
                class: DrainClass::Visible,
                resource: StreamResource::Sh,
            }
        }
        fn run(&mut self, _: &AtomicBool, _: Duration) {
            let name = std::thread::current()
                .name()
                .unwrap_or_default()
                .to_string();
            self.0.push("delivered", &name);
        }
        fn discard(self: Box<Self>) {}
        fn panicked(self: Box<Self>, _: &AtomicBool) {}
    }
    assert!(pool.queue().submit(Box::new(Named(Arc::clone(&journal)))));
    wait_for(&journal, "delivered", 1);
    assert!(
        journal.names("delivered")[0].starts_with(DECODE_THREAD_NAME_PREFIX),
        "{:?}",
        journal.names("delivered")
    );
}

// A job whose key turns mandatory while it is queued is taken before every
// older optional job, and ranks among the mandatory jobs by its live class:
// ahead of an older Lead job when it turns Visible, and SH ahead of
// lightmap at equal class.
#[test]
fn a_job_raised_while_queued_is_taken_before_older_optional_work_in_rank_order() {
    use DrainClass::{Lead, Prefetch, Visible};
    use StreamResource::{LightmapBlock, Sh};
    let journal = Arc::new(Journal::default());
    let live = Arc::new(TargetSet::new(8));
    let pool = DecodePool::spawn(1, 8).unwrap();
    let queue = pool.queue();
    let (running, started, release) = TestJob::new(&journal, Sh, 0, Prefetch).gated();
    assert!(queue.submit(running));
    started.recv().unwrap();
    for job in [
        TestJob::new(&journal, Sh, 1, Prefetch),
        TestJob::new(&journal, LightmapBlock, 2, Lead),
        TestJob::new(&journal, LightmapBlock, 3, Prefetch).ranked_by(&live),
        TestJob::new(&journal, Sh, 4, Prefetch).ranked_by(&live),
        TestJob::new(&journal, LightmapBlock, 5, Visible),
    ] {
        assert!(queue.submit(job));
    }
    live.set(3, Some(Visible));
    live.set(4, Some(Visible));
    release.send(()).unwrap();
    wait_for(&journal, "delivered", 6);
    assert_eq!(
        journal.names("start"),
        [
            "SH streaming:0:Prefetch",
            "SH streaming:4:Prefetch",
            "lightmap streaming:3:Prefetch",
            "lightmap streaming:5:Visible",
            "lightmap streaming:2:Lead",
            "SH streaming:1:Prefetch",
        ],
        "raised Visible jobs first, SH before lightmap, then the older Visible \
         pair, then Lead, then optional work"
    );
    drop(pool);
    assert_eq!(journal.held.load(Ordering::SeqCst), 0);
}

// A Visible SH chunk queued behind an older mandatory Lead lightmap pair is
// taken first: mandatory work ranks by urgency, not by age.
#[test]
fn a_visible_sh_chunk_is_taken_ahead_of_an_older_lead_lightmap_pair() {
    use DrainClass::{Lead, Visible};
    use StreamResource::{LightmapBlock, Sh};
    let journal = Arc::new(Journal::default());
    let pool = DecodePool::spawn(1, 8).unwrap();
    let queue = pool.queue();
    let (running, started, release) = TestJob::new(&journal, LightmapBlock, 0, Lead).gated();
    assert!(queue.submit(running));
    started.recv().unwrap();
    assert!(queue.submit(TestJob::new(&journal, LightmapBlock, 1, Lead)));
    assert!(queue.submit(TestJob::new(&journal, LightmapBlock, 2, Lead)));
    assert!(queue.submit(TestJob::new(&journal, Sh, 3, Visible)));
    release.send(()).unwrap();
    wait_for(&journal, "delivered", 4);
    assert_eq!(
        journal.names("start"),
        [
            "lightmap streaming:0:Lead",
            "SH streaming:3:Visible",
            "lightmap streaming:1:Lead",
            "lightmap streaming:2:Lead",
        ],
        "the Visible chunk waits only for the running decode"
    );
    drop(pool);
    assert_eq!(journal.held.load(Ordering::SeqCst), 0);
}

// A job whose key is demoted while queued loses its precedence: it goes
// behind younger jobs of a more urgent class, and FIFO among its new equals.
#[test]
fn a_job_demoted_while_queued_loses_its_precedence() {
    use DrainClass::{Pinned, Prefetch, Visible};
    use StreamResource::{LightmapBlock, Sh};
    let journal = Arc::new(Journal::default());
    let live = Arc::new(TargetSet::new(8));
    live.set(1, Some(Visible));
    let pool = DecodePool::spawn(1, 8).unwrap();
    let queue = pool.queue();
    let (running, started, release) = TestJob::new(&journal, Sh, 0, Visible).gated();
    assert!(queue.submit(running));
    started.recv().unwrap();
    assert!(queue.submit(TestJob::new(&journal, Sh, 1, Visible).ranked_by(&live)));
    assert!(queue.submit(TestJob::new(&journal, LightmapBlock, 2, Prefetch)));
    assert!(queue.submit(TestJob::new(&journal, LightmapBlock, 3, Pinned)));
    live.set(1, Some(Prefetch));
    release.send(()).unwrap();
    wait_for(&journal, "delivered", 4);
    assert_eq!(
        journal.names("start"),
        [
            "SH streaming:0:Visible",
            "lightmap streaming:3:Pinned",
            "SH streaming:1:Visible",
            "lightmap streaming:2:Prefetch",
        ],
        "the demoted chunk ranks as Prefetch: behind Pinned, SH first among Prefetch"
    );
    drop(pool);
    assert_eq!(journal.held.load(Ordering::SeqCst), 0);
}

// A job whose `panicked` cleanup panics too drops mid-unwind; its `Drop`
// returns the charge it still holds, and the thread keeps decoding.
#[test]
fn a_job_panicking_in_its_cleanup_returns_its_charge_on_drop() {
    use DrainClass::Visible;
    use StreamResource::{LightmapBlock, Sh};
    let journal = Arc::new(Journal::default());
    let pool = DecodePool::spawn(1, 8).unwrap();
    let queue = pool.queue();
    assert!(queue.submit(TestJob::new(&journal, LightmapBlock, 0, Visible).panicking_twice()));
    // SH ranks ahead of a lightmap block, so the next job is queued only once
    // the panicking one is running: it then runs after the panic, not before.
    wait_for(&journal, "start", 1);
    assert!(queue.submit(TestJob::new(&journal, Sh, 1, Visible)));
    wait_for(&journal, "delivered", 1);
    assert_eq!(journal.names("dropped"), ["lightmap streaming:0:Visible"]);
    assert_eq!(journal.names("delivered"), ["SH streaming:1:Visible"]);
    assert_eq!(
        journal.held.load(Ordering::SeqCst),
        0,
        "every charge returned"
    );
    drop(pool);
}

// A job that panics fails its item and returns its charge; the pool's one
// thread survives and takes the jobs queued behind it.
#[test]
fn a_panicking_job_fails_its_item_and_the_thread_keeps_decoding() {
    use DrainClass::Visible as Mandatory;
    use StreamResource::{LightmapBlock, Sh};
    let journal = Arc::new(Journal::default());
    let pool = DecodePool::spawn(1, 8).unwrap();
    let queue = pool.queue();
    let (panicking, started, release) = TestJob::new(&journal, LightmapBlock, 0, Mandatory)
        .panicking()
        .gated();
    assert!(queue.submit(panicking));
    started.recv().unwrap();
    assert!(queue.submit(TestJob::new(&journal, Sh, 1, Mandatory)));
    assert!(queue.submit(TestJob::new(&journal, LightmapBlock, 2, Mandatory).panicking()));
    assert!(queue.submit(TestJob::new(&journal, Sh, 3, Mandatory)));
    release.send(()).unwrap();
    wait_for(&journal, "delivered", 2);
    wait_for(&journal, "failed", 2);
    assert_eq!(
        journal.names("failed"),
        [
            "lightmap streaming:0:Visible",
            "lightmap streaming:2:Visible"
        ]
    );
    assert_eq!(
        journal.names("delivered"),
        ["SH streaming:1:Visible", "SH streaming:3:Visible"]
    );
    assert_eq!(
        journal.held.load(Ordering::SeqCst),
        0,
        "every charge returned"
    );
    drop(pool);
}

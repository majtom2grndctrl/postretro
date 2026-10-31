use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::*;

/// Poll `ready` as successive frames would, failing after a generous deadline.
fn poll_until(what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !ready() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Poll `handle` until it yields its result.
fn take<T>(handle: &mut TaskHandle<T>) -> Result<T, TaskError> {
    let mut result = None;
    poll_until("the task result", || {
        result = handle.try_take();
        result.is_some()
    });
    result.expect("polled until Some")
}

/// A task that reports it started, then holds its worker until released.
fn held_task(
    group: &TaskGroup,
    priority: Priority,
) -> (TaskHandle<()>, mpsc::Receiver<()>, mpsc::Sender<()>) {
    let (started_tx, started) = mpsc::channel();
    let (release, released) = mpsc::channel::<()>();
    let handle = group.spawn(priority, move |_| {
        let _ = started_tx.send(());
        let _ = released.recv();
    });
    (handle, started, release)
}

#[test]
fn load_pool_built_once_and_capped() {
    assert!(shipped_load_pool_width() <= LOAD_POOL_WIDTH_CAP);
    assert!(shipped_load_pool_width() >= 1);
    for (parallelism, width) in [(1, 1), (2, 1), (3, 1), (4, 2), (64, 2)] {
        assert_eq!(
            width_for_parallelism(parallelism),
            width,
            "{parallelism} cores"
        );
    }

    let mut slot = EngineLoadPool::with_width(2);
    assert!(slot.built().is_none(), "nothing starts before first use");
    let pool: *const LoadPool = slot.get_or_build().expect("pool builds");
    assert_eq!(slot.get_or_build().unwrap() as *const LoadPool, pool);
    let pool = slot.built().unwrap();
    assert_eq!(pool.width(), 2);
    assert_eq!(pool.threads_started(), 2);

    // Two loads' worth of tasks, each load in its own group, with a
    // concurrency high-water mark.
    let active = Arc::new(AtomicUsize::new(0));
    let high_water = Arc::new(AtomicUsize::new(0));
    for _load in 0..2 {
        let group = pool.group();
        let mut handles: Vec<TaskHandle<()>> = (0..8)
            .map(|index| {
                let (active, high_water) = (active.clone(), high_water.clone());
                let priority = if index % 3 == 0 {
                    Priority::Ui
                } else {
                    Priority::Load
                };
                group.spawn(priority, move |_| {
                    let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                    high_water.fetch_max(now, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(3));
                    active.fetch_sub(1, Ordering::SeqCst);
                })
            })
            .collect();
        for handle in &mut handles {
            assert_eq!(take(handle), Ok(()));
        }
    }
    assert_eq!(pool.threads_started(), 2, "a second load starts no thread");
    assert_eq!(pool.tasks_started(), 16);
    let high_water = high_water.load(Ordering::SeqCst);
    assert!(
        (1..=2).contains(&high_water),
        "never more than width tasks at once, ran {high_water}"
    );
}

#[test]
fn load_pool_runs_tasks_out_of_order() {
    let pool = LoadPool::new(2).unwrap();
    let group = pool.group();
    let (mut slow, slow_started, release_slow) = held_task(&group, Priority::Load);
    slow_started
        .recv_timeout(Duration::from_secs(30))
        .expect("the slow task starts");
    let mut fast = group.spawn(Priority::Load, |_| 7);

    assert_eq!(take(&mut fast), Ok(7), "the second task finishes first");
    assert!(slow.try_take().is_none(), "the first is still running");
    release_slow.send(()).unwrap();
    assert_eq!(take(&mut slow), Ok(()));
    assert!(slow.try_take().is_none(), "a result is taken once");
}

#[test]
fn cancelled_group_drops_queued_tasks_and_retires_by_polling() {
    let pool = LoadPool::new(2).unwrap();
    let group = pool.group();
    let input = Arc::new(String::from("opened level"));
    let input_alive = Arc::downgrade(&input);
    group.retain(input);

    // One running task waits on a gate; the other stops when it notices the
    // cancel.
    let (_held, held_started, release_held) = held_task(&group, Priority::Load);
    let (noticing_started_tx, noticing_started) = mpsc::channel();
    let mut noticing = group.spawn(Priority::Load, move |token| {
        let _ = noticing_started_tx.send(());
        while !token.is_cancelled() {
            std::thread::sleep(Duration::from_millis(1));
        }
        "noticed"
    });
    for started in [&held_started, &noticing_started] {
        started
            .recv_timeout(Duration::from_secs(30))
            .expect("both workers busy");
    }
    let ran = Arc::new(AtomicUsize::new(0));
    let mut queued: Vec<TaskHandle<()>> = (0..3)
        .map(|_| {
            let ran = ran.clone();
            group.spawn(Priority::Ui, move |_| {
                ran.fetch_add(1, Ordering::SeqCst);
            })
        })
        .collect();
    let spawner = group.spawner();

    let started = Instant::now();
    let mut retirement = group.cancel();
    assert!(!retirement.try_finish(), "a started task is still running");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "cancel and poll return while a task holds its worker"
    );
    for handle in &mut queued {
        assert_eq!(handle.try_take(), Some(Err(TaskError::Cancelled)));
    }
    let mut late = spawner.spawn(Priority::Load, |_| ());
    assert_eq!(late.try_take(), Some(Err(TaskError::Cancelled)));
    assert_eq!(take(&mut noticing), Ok("noticed"));
    assert!(!retirement.try_finish(), "the held task has not returned");
    assert!(input_alive.upgrade().is_some(), "retained until finished");

    release_held.send(()).unwrap();
    poll_until("the retirement", || retirement.try_finish());
    assert!(input_alive.upgrade().is_none(), "released when finished");
    assert_eq!(ran.load(Ordering::SeqCst), 0, "queued tasks never ran");
    assert_eq!(pool.tasks_started(), 2);
}

#[test]
fn panicking_task_reports_and_keeps_its_thread() {
    // Width 1, so the task after the panic can only run on the same thread.
    let pool = LoadPool::new(1).unwrap();
    let group = pool.group();
    let mut boom = group.spawn(Priority::Load, |_| -> u32 { panic!("decode exploded") });
    assert_eq!(
        take(&mut boom),
        Err(TaskError::Panicked("decode exploded".to_string()))
    );
    let mut after = group.spawn(Priority::Load, |_| {
        std::thread::current().name().map(str::to_string)
    });
    assert_eq!(take(&mut after), Ok(Some("load-pool-0".to_string())));
    assert_eq!(
        pool.threads_started(),
        1,
        "no thread replaced the panicked one"
    );
}

#[test]
fn ui_priority_runs_ahead_of_queued_load_tasks() {
    let pool = LoadPool::new(2).unwrap();
    let group = pool.group();
    let (mut first, first_started, release_first) = held_task(&group, Priority::Load);
    let (mut second, second_started, release_second) = held_task(&group, Priority::Load);
    for started in [&first_started, &second_started] {
        started
            .recv_timeout(Duration::from_secs(30))
            .expect("both workers busy");
    }

    let order = Arc::new(Mutex::new(Vec::new()));
    let record = |label: &'static str, priority| {
        let order = order.clone();
        group.spawn(priority, move |_| order.lock().unwrap().push(label))
    };
    let mut handles = vec![
        record("prep 1", Priority::Load),
        record("prep 2", Priority::Load),
        record("image", Priority::Ui),
    ];

    // Free one worker; it drains the queue alone.
    release_first.send(()).unwrap();
    for handle in &mut handles {
        assert_eq!(take(handle), Ok(()));
    }
    assert_eq!(*order.lock().unwrap(), ["image", "prep 1", "prep 2"]);
    release_second.send(()).unwrap();
    assert_eq!(take(&mut first), Ok(()));
    assert_eq!(take(&mut second), Ok(()));
}

#[test]
fn load_pool_shutdown_never_waits_on_a_running_task() {
    let mut pool = LoadPool::new(2).unwrap();
    let group = pool.group();
    let (token_seen_tx, token_seen) = mpsc::channel();
    let (_running, started, _never_released) = held_task(&group, Priority::Load);
    let mut noticing = group.spawn(Priority::Load, move |token| {
        while !token.is_cancelled() {
            std::thread::sleep(Duration::from_millis(1));
        }
        let _ = token_seen_tx.send(());
    });
    started
        .recv_timeout(Duration::from_secs(30))
        .expect("the held task starts");
    poll_until("both workers busy", || pool.tasks_started() == 2);
    let mut queued = group.spawn(Priority::Load, |_| ());

    let begun = Instant::now();
    pool.shutdown();
    assert!(
        begun.elapsed() < Duration::from_secs(5),
        "shutdown returns while a task holds its worker"
    );
    assert_eq!(queued.try_take(), Some(Err(TaskError::Cancelled)));
    token_seen
        .recv_timeout(Duration::from_secs(30))
        .expect("a running task sees the shutdown on its token");
    assert_eq!(take(&mut noticing), Ok(()));
    let mut after = group.spawn(Priority::Ui, |_| ());
    assert_eq!(after.try_take(), Some(Err(TaskError::Cancelled)));
}

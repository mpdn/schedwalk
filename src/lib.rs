#![deny(missing_docs)]
#![doc = include_str!("../README.md")]

use std::{
    env,
    future::Future,
    panic::{catch_unwind, resume_unwind},
    pin::Pin,
    sync::atomic::{AtomicBool, Ordering},
    task,
};

use async_task::Task;
use context::Context;

mod context;
mod schedule;

const SCHEDULE_ENV: &str = "SCHEDULE";

/// A spawned future that can be awaited.
///
/// This is the equivalent of Tokio's `tokio::task::JoinHandle`.
///
/// A `JoinHandle` detaches when the handle is dropped. The underlying task will continue to run
/// unless [`JoinHandle::abort`] was called.
pub struct JoinHandle<T> {
    task: Option<Task<T>>,
    abort: AtomicBool,
}

/// An error when joining a future via a [`JoinHandle`].
///
/// Currently, as panics are not handled by schedwalk, an error can only occur if
/// [`JoinHandle::abort`] is called, but this may change in the future.
pub struct JoinError();

impl JoinError {
    /// Whether this error is due to cancellation.
    pub fn is_cancelled(&self) -> bool {
        true
    }
}

impl<T> JoinHandle<T> {
    fn new(task: Task<T>) -> Self {
        JoinHandle {
            task: Some(task),
            abort: AtomicBool::new(false),
        }
    }
}

impl<T> JoinHandle<T> {
    /// Aborts the underlying task.
    ///
    /// If the task is not complete, this will cause it to complete with a [`JoinError`].
    /// Otherwise, it will not have an effect.
    pub fn abort(&self) {
        self.abort.store(true, Ordering::Relaxed)
    }
}

impl<T> Drop for JoinHandle<T> {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.detach()
        }
    }
}

impl<T> Future for JoinHandle<T> {
    type Output = Result<T, JoinError>;

    #[inline]
    fn poll(mut self: Pin<&mut Self>, cx: &mut task::Context) -> task::Poll<Self::Output> {
        let JoinHandle { task, abort } = &mut *self;

        match task {
            Some(task) if task.is_finished() || !*abort.get_mut() => {
                Pin::new(task).poll(cx).map(Ok)
            }
            _ => {
                task.take();
                task::Poll::Ready(Err(JoinError()))
            }
        }
    }
}

/// Spawns a new asynchronous task and returns a [`JoinHandle`] to it.
///
/// This must be called within a context created by [`for_all_schedules`]. Failure to do so will
/// throw an exception.
pub fn spawn<T>(future: T) -> JoinHandle<T::Output>
where
    T: Future + Send + 'static,
    T::Output: Send + 'static,
{
    JoinHandle::new(spawn_task(future))
}

fn spawn_task<T>(future: T) -> Task<T::Output>
where
    T: Future + Send + 'static,
    T::Output: Send + 'static,
{
    let (runnable, task) = async_task::spawn(future, Context::schedule);
    runnable.schedule();
    task
}

/// Executes the given future multiple times, each time under a new polling schedule, eventually
/// executing it under all possible polling schedules.
///
/// This can be used to deterministically test for what would otherwise be asynchronous race
/// conditions.
///
/// If a panic occurs when executing a schedule, it will be written to standard error. For ease of
/// debugging, rerunning the test with `SCHEDULE` set to this string will execute that particular
/// failing schedule only.
///
/// This assumes *determinism*; the spawned futures and the order they are polled in must not depend
/// on anything external to the function such as network or thread locals. This function will panic
/// in case non-determinism is detected, but it cannot do so reliably in all cases.
#[inline]
pub fn for_all_schedules<T>(mut f: impl FnMut() -> T)
where
    T: Future<Output = ()> + 'static + Send,
{
    fn walk(spawn: &mut dyn FnMut() -> Task<()>) {
        match env::var(SCHEDULE_ENV) {
            Ok(schedule) => walk_schedule(&schedule, spawn),
            Err(env::VarError::NotPresent) => walk_exhaustive(&mut Vec::new(), spawn),
            Err(env::VarError::NotUnicode(_)) => {
                panic!(
                    "found a schedule in {}, but it was not valid unicode",
                    SCHEDULE_ENV
                )
            }
        }
    }

    // Defer to `dyn` as quickly as possible to minimize per-test compilation overhead
    walk(&mut || spawn_task(f()))
}

fn walk_schedule(schedule: &str, spawn: &mut dyn FnMut() -> Task<()>) {
    let mut schedule = schedule::Decoder::new(schedule);
    Context::init(|context| {
        let task = spawn();
        loop {
            let runnable = {
                let mut runnables = context.runnables();
                let choices = runnables.len();

                if choices == 0 {
                    assert!(task.is_finished(), "deadlock");
                    break;
                } else {
                    runnables.swap_remove(schedule.read(choices))
                }
            };

            runnable.run();
        }
    })
}

fn walk_exhaustive(schedule: &mut Vec<(usize, usize)>, spawn: &mut dyn FnMut() -> Task<()>) {
    fn advance(schedule: &mut Vec<(usize, usize)>) -> bool {
        loop {
            if let Some((choice, len)) = schedule.pop() {
                let new_choice = choice + 1;
                if new_choice < len {
                    schedule.push((new_choice, len));
                    return true;
                }
            } else {
                return false;
            }
        }
    }

    Context::init(|context| 'schedules: loop {
        let mut step = 0;
        let task = spawn();

        loop {
            let runnable = {
                let mut runnables = context.runnables();
                let choices = runnables.len();

                let choice = if step < schedule.len() {
                    let (choice, existing_choices) = schedule[step];

                    assert_eq!(
                        choices,
                        existing_choices,
                        "nondeterminism: number of pollable futures ({}) did not equal number in previous executions ({})",
                        choices,
                        existing_choices,
                    );

                    choice
                } else if choices == 0 {
                    if task.is_finished() {
                        if advance(schedule) {
                            continue 'schedules;
                        } else {
                            break 'schedules;
                        }
                    } else {
                        panic!(
                            "deadlock in {}={}",
                            SCHEDULE_ENV,
                            schedule::encode(&schedule)
                        );
                    }
                } else {
                    schedule.push((0, choices));
                    0
                };

                runnables.swap_remove(choice)
            };

            step += 1;
            let result = catch_unwind(|| runnable.run());

            if let Err(panic) = result {
                eprintln!("panic in {}={}", SCHEDULE_ENV, schedule::encode(&schedule));
                resume_unwind(panic)
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use std::{
        any::Any,
        fmt::Debug,
        panic::{panic_any, AssertUnwindSafe},
    };

    use futures::{
        channel::{mpsc, oneshot},
        future::{pending, select, Either},
    };

    use super::*;

    fn assert_panics<T>(f: impl FnOnce() -> T) -> Box<dyn Any + Send>
    where
        T: Debug,
    {
        catch_unwind(AssertUnwindSafe(f)).expect_err("expected panic")
    }

    fn assert_finds_panicking_schedule<T>(mut f: impl FnMut() -> T) -> String
    where
        T: Future<Output = ()> + 'static + Send,
    {
        let mut schedule = Vec::new();

        assert_panics(|| walk_exhaustive(&mut schedule, &mut || spawn_task(f())))
            .downcast::<PanicMarker>()
            .expect("expected test panic");

        let encoded_schedule = schedule::encode(&schedule);

        assert_panics(|| walk_schedule(&encoded_schedule, &mut || spawn_task(f())))
            .downcast::<PanicMarker>()
            .expect("expected test panic");

        encoded_schedule
    }

    struct PanicMarker;

    fn panic_target() {
        panic_any(PanicMarker);
    }

    #[test]
    fn basic() {
        assert_finds_panicking_schedule(|| async { panic_target() });
    }

    #[test]
    fn spawn_panic() {
        assert_finds_panicking_schedule(|| async {
            spawn(async { panic_target() });
        });
    }

    #[test]
    fn example() {
        let f = || async {
            let (sender, mut receiver) = mpsc::unbounded::<usize>();

            spawn(async move {
                sender.unbounded_send(1).unwrap();
                sender.unbounded_send(3).unwrap();
                sender.unbounded_send(2).unwrap();
            });

            spawn(async move {
                let mut sum = 0;
                let mut count = 0;
                while let Some(num) = receiver.try_next().unwrap() {
                    sum += num;
                    count += 1;
                }

                println!("average is {}", sum / count)
            });
        };

        let mut schedule = Vec::new();
        assert_panics(|| walk_exhaustive(&mut schedule, &mut || spawn_task(f())));
        assert_eq!(schedule::encode(&schedule), "01")
    }

    #[test]
    fn channels() {
        assert_finds_panicking_schedule(|| async {
            let (sender_a, receiver_a) = oneshot::channel();
            let (sender_b, receiver_b) = oneshot::channel();

            spawn(async {
                drop(sender_a.send(()));
            });

            spawn(async {
                drop(sender_b.send(()));
            });

            match select(receiver_a, receiver_b).await {
                Either::Left(_) => (),
                Either::Right(_) => panic_target(),
            }
        });
    }

    #[test]
    fn walk_basic() {
        for_all_schedules(|| async { () });
    }

    #[test]
    fn walk_channels() {
        for_all_schedules(|| async {
            let (sender_a, receiver_a) = oneshot::channel();
            let (sender_b, receiver_b) = oneshot::channel();

            spawn(async {
                sender_a.send(()).unwrap();
            });

            spawn(async {
                sender_b.send(()).unwrap();
            });

            receiver_a.await.unwrap();
            receiver_b.await.unwrap();
        });
    }

    #[test]
    #[should_panic]
    fn walk_deadlock() {
        for_all_schedules(|| pending::<()>())
    }

    #[test]
    #[should_panic]
    fn channel_deadlock() {
        for_all_schedules(|| async {
            let (sender, receiver) = oneshot::channel::<()>();

            receiver.await.unwrap();
            drop(sender)
        });
    }
}

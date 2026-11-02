//! Small persistent queues for *non frame-critical* jobs. Never use these for
//! prediction, renderer commands or stateful physics. Workers block while idle.
use std::{
    sync::mpsc::{self, Receiver, SyncSender, TrySendError},
    thread,
    time::{Duration, Instant},
};

type Work = Box<dyn FnOnce() + Send + 'static>;
const QUEUE_CAPACITY: usize = 64;

pub(super) struct BackgroundJobs {
    io: SyncSender<Work>,
    cpu: SyncSender<Work>,
}

impl BackgroundJobs {
    pub(super) fn spawn() -> Result<Self, String> {
        fn lane(
            name: &'static str,
            slot: crate::thread_activity::ThreadSlot,
            task: crate::thread_activity::Task,
        ) -> Result<SyncSender<Work>, String> {
            let (tx, rx) = mpsc::sync_channel(QUEUE_CAPACITY);
            thread::Builder::new()
                .name(name.to_owned())
                .spawn(move || run_lane(rx, slot, task))
                .map_err(|error| format!("Could not start {name}: {error}"))?;
            Ok(tx)
        }
        Ok(Self {
            io: lane("jka-background-io", crate::thread_activity::ThreadSlot::BackgroundIo,
                crate::thread_activity::Task::BackgroundIo)?,
            cpu: lane("jka-background-cpu", crate::thread_activity::ThreadSlot::BackgroundCpu,
                crate::thread_activity::Task::BackgroundCpu)?,
        })
    }

    fn submit(tx: &SyncSender<Work>, job: impl FnOnce() + Send + 'static) -> Result<(), String> {
        tx.try_send(Box::new(job)).map_err(|error| match error {
            TrySendError::Full(_) => "background job queue is full".to_owned(),
            TrySendError::Disconnected(_) => "background worker stopped".to_owned(),
        })
    }

    pub(super) fn io(&self, job: impl FnOnce() + Send + 'static) -> Result<(), String> {
        Self::submit(&self.io, job)
    }

    pub(super) fn cpu(&self, job: impl FnOnce() + Send + 'static) -> Result<(), String> {
        Self::submit(&self.cpu, job)
    }

    /// At shutdown, wait for earlier I/O to finish. Do not block a normal
    /// frame, and do not wait indefinitely if a disk stalls.
    pub(super) fn flush_io(&self, timeout: Duration) -> bool {
        let started = Instant::now();
        let (tx, rx) = mpsc::sync_channel(1);
        let mut barrier: Work = Box::new(move || { let _ = tx.send(()); });
        loop {
            match self.io.try_send(barrier) {
                Ok(()) => break,
                Err(TrySendError::Disconnected(_)) => return false,
                Err(TrySendError::Full(queued)) => {
                    barrier = queued;
                    if started.elapsed() >= timeout { return false; }
                    thread::sleep(Duration::from_millis(1));
                }
            }
        }
        rx.recv_timeout(timeout.saturating_sub(started.elapsed())).is_ok()
    }
}

fn run_lane(
    rx: Receiver<Work>,
    slot: crate::thread_activity::ThreadSlot,
    task: crate::thread_activity::Task,
) {
    while let Ok(job) = rx.recv() {
        let _activity = crate::thread_activity::activity(slot, task);
        // A panicking job must not permanently kill the shared worker.
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(job)).is_err() {
            eprintln!("Background job panicked; continuing worker loop");
        }
    }
}

/// Worker completions installed only by the main application tick.
pub(super) enum JobResult {
    ConfigSaved { generation: u64, result: Result<(), String> },
    ServerDump { path: std::path::PathBuf, result: Result<(), String> },
    Jk2DemoInfo { result: Result<String, String> },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn io_flush_is_a_barrier_for_earlier_jobs() {
        let jobs = BackgroundJobs::spawn().unwrap();
        let (tx, rx) = mpsc::channel();
        jobs.io(move || { tx.send(42).unwrap(); }).unwrap();
        assert!(jobs.flush_io(Duration::from_secs(1)));
        assert_eq!(rx.try_recv(), Ok(42));
    }

    #[test]
    fn cpu_lane_can_run_without_blocking_io_lane() {
        let jobs = BackgroundJobs::spawn().unwrap();
        let (tx, rx) = mpsc::channel();
        jobs.cpu(move || { tx.send(7).unwrap(); }).unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(1)), Ok(7));
    }
}

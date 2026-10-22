// SPDX-License-Identifier: GPL-3.0-or-later
//! The audio-control thread. Every call into cpal (making a host, listing devices,
//! building, starting and dropping streams) runs here, on one thread of its own that
//! nothing else uses. The window's thread never touches the audio drivers: on Windows
//! it is set up for drag and drop (OLE) by the window system, and ASIO drivers load
//! COM objects on the thread that asks for them. cpal sets COM up on this thread
//! itself, the way its ASIO and WASAPI backends expect.
//!
//! The streams themselves stay on this thread (a `cpal::Stream` may not move between
//! threads); the rest of the app holds a [`StreamHandle`] and dropping that closes the
//! stream here. The lock-free queues between the interface and the audio callback do
//! not go through this thread.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

type Job = Box<dyn FnOnce() + Send>;

/// How long the interface waits for the driver to open something.
const OPEN_TIMEOUT: Duration = Duration::from_secs(20);
/// How long it waits for a stream to close.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(5);

static JOBS: OnceLock<Mutex<Sender<Job>>> = OnceLock::new();
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

thread_local! {
    static ON_CONTROL: Cell<bool> = const { Cell::new(false) };
    static STREAMS: RefCell<HashMap<u64, cpal::Stream>> = RefCell::new(HashMap::new());
}

fn jobs() -> Sender<Job> {
    JOBS.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("audio-control".into())
            .spawn(move || {
                ON_CONTROL.with(|c| c.set(true));
                for job in rx {
                    // a job catches its own panics; this is only a last guard
                    let _ = catch_unwind(AssertUnwindSafe(job));
                }
            })
            .expect("the audio-control thread starts");
        Mutex::new(tx)
    })
    .lock()
    .unwrap_or_else(|e| e.into_inner())
    .clone()
}

/// Whether this is the audio-control thread.
pub fn on_control() -> bool {
    ON_CONTROL.with(|c| c.get())
}

/// What a panic said, as text.
pub fn panic_text(p: Box<dyn Any + Send>) -> String {
    if let Some(s) = p.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = p.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".into()
    }
}

fn run_timeout<R: Send + 'static>(
    f: impl FnOnce() -> R + Send + 'static,
    timeout: Duration,
) -> Result<R, String> {
    let caught = |f: Box<dyn FnOnce() -> R + Send>| {
        catch_unwind(AssertUnwindSafe(f))
            .map_err(|p| format!("the audio driver crashed: {}", panic_text(p)))
    };
    if on_control() {
        return caught(Box::new(f));
    }
    let (tx, rx) = mpsc::sync_channel(1);
    let job: Job = Box::new(move || {
        // when the interface stopped waiting, what was made is dropped here (a
        // stream among it closes on this thread)
        let _ = tx.send(caught(Box::new(f)));
    });
    jobs()
        .send(job)
        .map_err(|_| "the audio-control thread has stopped".to_string())?;
    match rx.recv_timeout(timeout) {
        Ok(r) => r,
        Err(mpsc::RecvTimeoutError::Timeout) => Err(format!(
            "the audio driver did not answer within {} seconds",
            timeout.as_secs()
        )),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err("the audio-control thread has stopped".into())
        }
    }
}

/// Run `f` on the audio-control thread and wait for what it gives. A panic in it (a
/// driver that crashes) comes back as an error.
pub fn run<R: Send + 'static>(f: impl FnOnce() -> R + Send + 'static) -> Result<R, String> {
    run_timeout(f, OPEN_TIMEOUT)
}

/// A stream kept on the audio-control thread. Dropping it closes the stream there, and
/// waits for that, so the device is free again when the drop returns.
#[derive(Debug)]
pub struct StreamHandle(u64);

impl StreamHandle {
    /// Keep a stream. Only on the audio-control thread.
    pub fn keep(s: cpal::Stream) -> Self {
        assert!(on_control(), "streams are kept on the audio-control thread");
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        STREAMS.with(|m| m.borrow_mut().insert(id, s));
        StreamHandle(id)
    }
}

fn close(id: u64) {
    let s = STREAMS.with(|m| m.borrow_mut().remove(&id));
    // a driver that panics while closing must not take the thread down
    let _ = catch_unwind(AssertUnwindSafe(move || drop(s)));
}

impl Drop for StreamHandle {
    fn drop(&mut self) {
        let id = self.0;
        if on_control() {
            close(id);
        } else if let Err(e) = run_timeout(move || close(id), CLOSE_TIMEOUT) {
            eprintln!("audio: closing a stream: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jobs_run_on_one_other_thread_and_panics_come_back_as_errors() {
        let here = std::thread::current().id();
        let a = run(|| std::thread::current().id()).unwrap();
        let b = run(|| std::thread::current().id()).unwrap();
        assert_ne!(a, here);
        assert_eq!(a, b, "always the same thread");
        assert!(run(on_control).unwrap());
        assert!(!on_control());
        let e = run(|| -> u32 { panic!("driver fell over") }).unwrap_err();
        assert!(e.contains("driver fell over"), "{e}");
        // still working after the panic, and a job may run another job
        assert_eq!(run(|| run(|| 7).unwrap()).unwrap(), 7);
    }
}

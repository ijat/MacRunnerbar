//! Runs an [`Engine`] on a worker thread and gives the UI a thread-safe handle.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use super::Engine;
use crate::types::StatusSnapshot;

/// Called on the worker thread whenever the snapshot changes (ignoring its timestamp). Keep it short.
pub type Observer = Arc<dyn Fn(&StatusSnapshot) + Send + Sync>;

/// How often the worker wakes by itself to poll runner processes (the GitHub scan has its own,
/// slower cadence inside the engine).
const WAKE_EVERY: Duration = Duration::from_secs(2);

enum Command {
    Pause(bool),
    ScanNow,
    /// Run one tick and signal when done (tests and "refresh now").
    Tick(Sender<()>),
    Shutdown,
}

/// Handle to a running engine thread. Dropping it without [`shutdown`](Self::shutdown) still stops the
/// engine (and kills its runners) because the command channel closes.
pub struct EngineHandle {
    tx: Sender<Command>,
    snapshot: Arc<Mutex<StatusSnapshot>>,
    join: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for EngineHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineHandle").finish_non_exhaustive()
    }
}

fn same_content(a: &StatusSnapshot, b: &StatusSnapshot) -> bool {
    let mut b = b.clone();
    b.as_of = a.as_of;
    *a == b
}

fn worker(
    mut engine: Engine,
    rx: &Receiver<Command>,
    shared: &Mutex<StatusSnapshot>,
    observer: Option<Observer>,
) {
    let mut last = engine.snapshot();
    let publish = |engine: &Engine, last: &mut StatusSnapshot| {
        let snap = engine.snapshot();
        *shared.lock().unwrap_or_else(PoisonError::into_inner) = snap.clone();
        if !same_content(last, &snap) {
            if let Some(o) = &observer {
                o(&snap);
            }
            *last = snap;
        }
    };
    // A panic in a tick must not leave runners orphaned: contain it, keep going, kill at shutdown.
    let guarded = |engine: &mut Engine| {
        if catch_unwind(AssertUnwindSafe(|| engine.tick())).is_err() {
            tracing::error!("engine tick panicked");
        }
    };
    if catch_unwind(AssertUnwindSafe(|| engine.startup())).is_err() {
        tracing::error!("engine startup panicked");
    }
    publish(&engine, &mut last);
    loop {
        match rx.recv_timeout(WAKE_EVERY) {
            Ok(Command::Pause(p)) => {
                engine.set_paused(p);
                guarded(&mut engine);
            }
            Ok(Command::ScanNow) => {
                engine.request_scan();
                guarded(&mut engine);
            }
            Ok(Command::Tick(done)) => {
                guarded(&mut engine);
                publish(&engine, &mut last);
                let _ = done.send(());
                continue;
            }
            Ok(Command::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => guarded(&mut engine),
        }
        publish(&engine, &mut last);
    }
    engine.shutdown();
    publish(&engine, &mut last);
}

impl EngineHandle {
    /// Start `engine` on its own thread: run startup cleanup, then tick until shut down.
    pub fn spawn(engine: Engine, observer: Option<Observer>) -> Self {
        let (tx, rx) = channel();
        let snapshot = Arc::new(Mutex::new(engine.snapshot()));
        let shared = snapshot.clone();
        let join = std::thread::Builder::new()
            .name("macrunnerbar-engine".into())
            .spawn(move || worker(engine, &rx, &shared, observer))
            .ok();
        Self { tx, snapshot, join }
    }

    /// The latest published snapshot.
    pub fn snapshot(&self) -> StatusSnapshot {
        self.snapshot
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Pause or resume starting runners.
    pub fn set_paused(&self, paused: bool) {
        let _ = self.tx.send(Command::Pause(paused));
    }

    /// Scan GitHub now (after a preferences change, or for a manual refresh).
    pub fn scan_now(&self) {
        let _ = self.tx.send(Command::ScanNow);
    }

    /// Run one tick and wait for it to finish.
    pub fn tick_now(&self) {
        let (done_tx, done_rx) = channel();
        if self.tx.send(Command::Tick(done_tx)).is_ok() {
            let _ = done_rx.recv();
        }
    }

    /// Stop every runner, remove their directories and registrations, and join the thread. Blocks until
    /// that is done; call it before the app exits.
    pub fn shutdown(mut self) {
        self.stop();
    }

    fn stop(&mut self) {
        let _ = self.tx.send(Command::Shutdown);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

impl Drop for EngineHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

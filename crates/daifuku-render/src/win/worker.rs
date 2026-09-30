//! The thread pattern every visual uses.
//!
//! A visual owns exactly one thread. That thread creates the windows, pumps
//! their message loop and never blocks on the daemon. The daemon holds a
//! [`WorkerHandle`], drops a message in a channel and pokes the loop awake with
//! `PostThreadMessageW`; it never waits for a reply, so a wedged compositor
//! cannot stall tiling.
//!
//! Windows belong to the thread that created them, so every `HWND` in this
//! crate is created, painted and destroyed on the same thread, and the state
//! struct is dropped there too.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, MSG, PM_NOREMOVE, PeekMessageW, PostThreadMessageW, WM_APP,
    WM_QUIT,
};

use crate::{RenderError, Result};

/// Posted to the worker thread to say "there is something in your channel".
const WM_MOCHI_WAKE: u32 = WM_APP + 0x101;

/// What the handle and the thread tell each other outside the message queue.
///
/// A thread's message queue holds at most 10,000 messages and
/// `PostThreadMessageW` fails once it is full. Posting a wake per send would
/// fill it within minutes of the thread stalling, and the `WM_QUIT` that
/// `stop` posts would then be the message that gets dropped, leaving `stop`
/// joining a thread that never ends. So at most one wake is ever in the queue,
/// and stopping is also a flag the loop checks after every message.
#[derive(Default)]
struct Signals {
    /// A wake is in the queue and the channel has not been drained since.
    wake_pending: AtomicBool,
    /// `stop` was called: the loop ends after the message it is on.
    stopping: AtomicBool,
}

/// A handle to a visual's thread.
///
/// Cloneable and shareable: the daemon can hand one to the animation callback
/// without any further locking. Dropping the last handle stops the thread and
/// takes its windows with it.
pub(crate) struct WorkerHandle<M> {
    name: &'static str,
    /// The `Sender` is behind a mutex only so that the handle is `Sync`; the
    /// lock is held for the length of one `send`.
    sender: Mutex<Sender<M>>,
    thread_id: u32,
    signals: Arc<Signals>,
    join: Mutex<Option<JoinHandle<()>>>,
}

impl<M> WorkerHandle<M> {
    /// Queues a message and wakes the loop.
    ///
    /// # Errors
    ///
    /// [`RenderError::ThreadGone`] when the worker thread has stopped.
    pub(crate) fn send(&self, message: M) -> Result<()> {
        {
            let sender = self
                .sender
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            sender
                .send(message)
                .map_err(|_| RenderError::ThreadGone(self.name))?;
        }
        self.wake()
    }

    /// Pokes the message loop so it drains the channel, unless a wake is
    /// already on its way: one drain takes everything queued before it.
    fn wake(&self) -> Result<()> {
        // Acquire and release pair with the swap in `run_loop`, so a message
        // sent before a wake that is skipped here is seen by the drain that
        // wake leads to.
        if self.signals.wake_pending.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        // SAFETY: PostThreadMessageW only needs a thread id; a dead thread makes
        // it fail with an error rather than doing anything dangerous. The
        // message carries no pointers.
        let posted =
            unsafe { PostThreadMessageW(self.thread_id, WM_MOCHI_WAKE, WPARAM(0), LPARAM(0)) };
        if posted.is_err() {
            // No wake is on its way after all, so the next send tries again.
            self.signals.wake_pending.store(false, Ordering::Release);
            return Err(RenderError::ThreadGone(self.name));
        }
        Ok(())
    }

    /// Stops the thread and waits for its windows to be destroyed.
    pub(crate) fn stop(&self) {
        let handle = {
            let mut join = self
                .join
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            join.take()
        };
        let Some(handle) = handle else { return };

        // The flag is what ends the loop when the quit below cannot be queued;
        // a full queue means the thread still has messages to take, and it
        // checks the flag after each one.
        self.signals.stopping.store(true, Ordering::Release);
        // SAFETY: posting WM_QUIT to a thread id is the documented way to end
        // another thread's message loop. If the thread is already gone the call
        // fails harmlessly and the join below returns at once.
        let _ = unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) };
        if handle.join().is_err() {
            tracing::warn!("the {} thread panicked", self.name);
        }
    }
}

impl<M> Drop for WorkerHandle<M> {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Starts a visual's thread.
///
/// `init` builds the thread's state (factories, window classes) on the new
/// thread, because Direct2D single threaded factories and window handles both
/// belong to the thread that made them. `handle` is called for every message,
/// on that same thread.
///
/// # Errors
///
/// [`RenderError::ThreadStart`] when the thread cannot be spawned or `init`
/// fails; the string carries the underlying error, because `windows::core::Error`
/// holds a COM pointer and cannot cross threads.
pub(crate) fn spawn_worker<M, S, I, H>(
    name: &'static str,
    init: I,
    mut handle: H,
) -> Result<WorkerHandle<M>>
where
    M: Send + 'static,
    S: 'static,
    I: FnOnce() -> Result<S> + Send + 'static,
    H: FnMut(&mut S, M) + Send + 'static,
{
    let (message_tx, message_rx) = channel::<M>();
    let signals = Arc::new(Signals::default());
    let theirs = Arc::clone(&signals);
    let (ready_tx, ready_rx) = channel::<std::result::Result<u32, String>>();

    let join = std::thread::Builder::new()
        .name(format!("daifuku-{name}"))
        .spawn(move || {
            // A thread only gets a message queue once it has looked at it, and
            // PostThreadMessageW silently drops messages until then, so the
            // queue is forced into existence before the handle is handed out.
            let mut msg = MSG::default();
            // SAFETY: PeekMessageW with PM_NOREMOVE only inspects this thread's
            // own queue and writes into the live `msg` slot.
            unsafe {
                let _ = PeekMessageW(&mut msg, None, WM_APP, WM_APP, PM_NOREMOVE);
            }

            // SAFETY: GetCurrentThreadId has no arguments and cannot fail.
            let thread_id = unsafe { windows::Win32::System::Threading::GetCurrentThreadId() };

            let mut state = match init() {
                Ok(state) => {
                    if ready_tx.send(Ok(thread_id)).is_err() {
                        return;
                    }
                    state
                }
                Err(error) => {
                    let _ = ready_tx.send(Err(error.to_string()));
                    return;
                }
            };

            run_loop(&message_rx, &theirs, &mut state, &mut handle);
            drop(state);
        })
        .map_err(|error| RenderError::ThreadStart(name, error.to_string()))?;

    match ready_rx.recv() {
        Ok(Ok(thread_id)) => Ok(WorkerHandle {
            name,
            sender: Mutex::new(message_tx),
            thread_id,
            signals,
            join: Mutex::new(Some(join)),
        }),
        Ok(Err(error)) => Err(RenderError::ThreadStart(name, error)),
        Err(_) => Err(RenderError::ThreadStart(
            name,
            "the thread died before it was ready".to_string(),
        )),
    }
}

/// The message loop: Win32 messages for the windows, channel messages for the
/// daemon's commands.
fn run_loop<M, S, H>(messages: &Receiver<M>, signals: &Signals, state: &mut S, handle: &mut H)
where
    H: FnMut(&mut S, M),
{
    let mut msg = MSG::default();
    while !signals.stopping.load(Ordering::Acquire) {
        // SAFETY: `msg` is a live stack slot; a null window filter means "every
        // window of this thread plus thread messages", which is what a worker
        // owning several windows wants.
        let result = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        match result.0 {
            0 => break,  // WM_QUIT
            -1 => break, // the queue broke; nothing sensible is left
            _ if msg.message == WM_MOCHI_WAKE => {
                // Cleared before the drain, not after: a message sent while
                // the drain runs then posts a fresh wake instead of being left
                // in the channel until some later send.
                signals.wake_pending.swap(false, Ordering::AcqRel);
                drain(messages, state, handle);
            }
            _ => {
                // SAFETY: the message was just filled in by GetMessageW.
                // TranslateMessage is skipped: no visual window takes keys.
                unsafe {
                    DispatchMessageW(&msg);
                }
            }
        }
    }

    // Anything queued behind the quit is dropped on purpose: the thread is
    // shutting down and its windows are about to go away.
}

/// Runs every queued command, coalescing bursts into one wake.
fn drain<M, S, H>(messages: &Receiver<M>, state: &mut S, handle: &mut H)
where
    H: FnMut(&mut S, M),
{
    loop {
        match messages.try_recv() {
            Ok(message) => handle(state, message),
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;
    use std::sync::mpsc::{RecvTimeoutError, SyncSender, sync_channel};
    use std::time::Duration;

    use super::*;

    /// More than a thread's message queue holds.
    const FLOOD: usize = 20_000;

    /// A worker whose first message blocks it until `release` is sent to, as a
    /// thread stuck in a slow draw would be. Every message is counted.
    fn stalled_worker() -> (WorkerHandle<()>, SyncSender<()>, Arc<AtomicUsize>) {
        let (release, gate) = sync_channel::<()>(0);
        let seen = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&seen);
        let worker = spawn_worker(
            "test",
            move || Ok(gate),
            move |gate: &mut Receiver<()>, (): ()| {
                if counter.fetch_add(1, Ordering::SeqCst) == 0 {
                    let _ = gate.recv();
                }
            },
        )
        .expect("the worker starts");
        worker.send(()).expect("the first send");
        (worker, release, seen)
    }

    /// Runs `stop` on another thread, so that a stop that hangs fails the test
    /// instead of hanging it.
    fn stops_in_time(worker: WorkerHandle<()>) -> bool {
        let (done, finished) = channel();
        std::thread::spawn(move || {
            worker.stop();
            let _ = done.send(());
        });
        !matches!(
            finished.recv_timeout(Duration::from_secs(10)),
            Err(RecvTimeoutError::Timeout)
        )
    }

    #[test]
    fn a_stalled_thread_does_not_fill_its_queue_with_wakes() {
        let (worker, release, seen) = stalled_worker();
        let refused = (0..FLOOD).filter(|_| worker.send(()).is_err()).count();
        release.send(()).expect("the thread is waiting");
        assert!(stops_in_time(worker));
        assert_eq!(refused, 0, "sends refused while the thread was stuck");
        assert_eq!(seen.load(Ordering::SeqCst), FLOOD + 1);
    }

    #[test]
    fn stop_ends_a_thread_whose_queue_is_too_full_for_the_quit() {
        let (worker, release, _) = stalled_worker();
        // Fill the queue with something other than wakes, until Windows
        // refuses to take any more. The limit is a registry setting, so this
        // does not stop at the default.
        let mut refused = false;
        for _ in 0..FLOOD * 10 {
            // SAFETY: a message with no pointers, posted to the worker thread,
            // which has no windows for it to reach.
            let posted =
                unsafe { PostThreadMessageW(worker.thread_id, WM_APP + 1, WPARAM(0), LPARAM(0)) };
            if posted.is_err() {
                refused = true;
                break;
            }
        }
        assert!(refused, "the queue never filled up");
        let stopper = std::thread::spawn(move || stops_in_time(worker));
        // Only once stop has had the chance to find the queue full.
        std::thread::sleep(Duration::from_millis(200));
        release.send(()).expect("the thread is waiting");
        assert!(stopper.join().expect("the stopper thread"));
    }
}

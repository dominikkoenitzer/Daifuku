//! The two pipe threads.
//!
//! Each accepts one client at a time, reads one line, and hands the message to
//! the main thread through [`Inbox`]. The hook thread never answers: a hook is
//! gone the moment it has written. The control thread waits for the main
//! thread's reply and writes it back.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, SyncSender, channel, sync_channel};
use std::time::Duration;

use daifuku_core::protocol::{HookMessage, Request, Response, from_line, to_line};
use daifuku_win::pipe::{Instance, Pipe, instances};
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_APP};

/// The message the pipe threads post to wake the main loop.
pub const WM_INBOX: u32 = WM_APP + 1;

/// How long a command waits for the main thread to take it up. Past this the
/// main thread is busy with something long, such as a fleet a hotkey opens,
/// and the command is withdrawn: it never runs after its client was told it
/// did not.
const QUEUE_TIMEOUT: Duration = Duration::from_secs(30);

/// How long a command the main thread has taken up may run. Opening a fleet
/// is the longest: each of up to [`MAX_FLEET`] terminals may take its whole
/// [`SHOW_TIMEOUT`] to show.
///
/// [`MAX_FLEET`]: daifuku_core::config::MAX_FLEET
/// [`SHOW_TIMEOUT`]: crate::fleet::SHOW_TIMEOUT
const RUN_TIMEOUT: Duration = crate::fleet::SHOW_TIMEOUT
    .saturating_mul(daifuku_core::config::MAX_FLEET)
    .saturating_add(Duration::from_secs(30));

/// How long a hook has, from its connect, to deliver its line. A hook writes
/// the moment it connects; one that stays silent past this is cut off, so a
/// process holding every hook instance open cannot keep real hooks out.
const HOOK_READ: Duration = Duration::from_millis(500);

/// How long a control client has to send its request, and then to read the
/// reply.
const CONTROL_IO: Duration = Duration::from_secs(5);

/// What a pipe thread hands to the main thread.
pub enum Inbound {
    /// An agent's hook reported.
    Hook(HookMessage),
    /// A command, where to send the answer, and the claim on it: see
    /// [`claim`].
    Control(Request, SyncSender<Response>, Arc<AtomicBool>),
}

/// Claims a command for whoever asks first: the main thread, to run it, or
/// its pipe thread, to give up waiting for it. True for the first only, so a
/// command runs exactly when its client is told the answer.
pub fn claim(claimed: &AtomicBool) -> bool {
    !claimed.swap(true, Ordering::SeqCst)
}

/// The main thread's end: drained after every wake-up.
pub struct Inbox {
    pub receiver: Receiver<Inbound>,
    /// Told when the reply to a stop request has been handed over.
    stopped: Receiver<()>,
}

impl Inbox {
    /// Waits, at most `limit`, until the reply to a stop request has reached
    /// its client, so the process does not exit while the reply is still on
    /// its way.
    pub fn wait_for_stop_reply(&self, limit: Duration) {
        let _ = self.stopped.recv_timeout(limit);
    }
}

/// Claims both pipe names and starts both threads. Fails if either name is
/// taken, which means another daemon runs in this session.
pub fn start(main_thread: u32) -> anyhow::Result<Inbox> {
    // Four hook instances: several agents finishing a tool call in the same
    // instant each find a free one instead of queueing on a single pipe.
    let hooks = instances(Pipe::Hook, 4).map_err(|e| anyhow::anyhow!("hook pipe: {e}"))?;
    let controls = instances(Pipe::Control, 1).map_err(|e| anyhow::anyhow!("control pipe: {e}"))?;
    let (sender, receiver) = channel();
    let (stop_replied, stopped) = sync_channel(1);
    for (i, hook) in hooks.into_iter().enumerate() {
        let sender = sender.clone();
        spawn(&format!("daifuku-hook-{i}"), hook, move |connection| {
            serve_hook(connection, &sender, main_thread);
        });
    }
    for control in controls {
        let (sender, stop_replied) = (sender.clone(), stop_replied.clone());
        spawn("daifuku-control", control, move |connection| {
            serve_control(connection, &sender, main_thread, &stop_replied);
        });
    }
    Ok(Inbox { receiver, stopped })
}

fn spawn(
    name: &str,
    instance: Instance,
    serve: impl Fn(&mut daifuku_win::pipe::Connection<'_>) + Send + 'static,
) {
    let _ = std::thread::Builder::new()
        .name(name.to_owned())
        .spawn(move || {
            let mut instance = instance;
            loop {
                match instance.accept() {
                    Ok(mut connection) => serve(&mut connection),
                    Err(error) => {
                        tracing::warn!(%error, "pipe accept failed");
                        std::thread::sleep(Duration::from_millis(250));
                    }
                }
            }
        });
}

fn wake(main_thread: u32) {
    // SAFETY: posting a message with no pointers in it.
    let _ = unsafe { PostThreadMessageW(main_thread, WM_INBOX, WPARAM(0), LPARAM(0)) };
}

fn serve_hook(
    connection: &mut daifuku_win::pipe::Connection<'_>,
    sender: &Sender<Inbound>,
    main_thread: u32,
) {
    let Ok(line) = connection.read_line(HOOK_READ) else {
        return;
    };
    match from_line::<HookMessage>(&line) {
        Ok(message) => {
            if sender.send(Inbound::Hook(message)).is_ok() {
                wake(main_thread);
            }
        }
        Err(error) => {
            tracing::debug!(%error, client = connection.client, "ignored a malformed hook line")
        }
    }
}

fn serve_control(
    connection: &mut daifuku_win::pipe::Connection<'_>,
    sender: &Sender<Inbound>,
    main_thread: u32,
    stop_replied: &SyncSender<()>,
) {
    let Ok(line) = connection.read_line(CONTROL_IO) else {
        return;
    };
    let mut stop = false;
    let response = match from_line::<Request>(&line) {
        Ok(request) => {
            tracing::info!(?request, client = connection.client, "control request");
            stop = matches!(request, Request::Stop);
            let (reply, answer) = sync_channel(1);
            let claimed = Arc::new(AtomicBool::new(false));
            if sender
                .send(Inbound::Control(request, reply, Arc::clone(&claimed)))
                .is_err()
            {
                Response::error("the daemon is shutting down")
            } else {
                wake(main_thread);
                let (response, ran) = await_reply(&answer, &claimed, QUEUE_TIMEOUT, RUN_TIMEOUT);
                stop &= ran;
                response
            }
        }
        Err(error) => Response::error(format!("not a request: {error}")),
    };
    if let Ok(line) = to_line(&response) {
        let _ = connection.write_line(&line, CONTROL_IO);
    }
    if stop {
        let _ = stop_replied.try_send(());
    }
}

/// Waits for the main thread's answer to a command, and says whether the
/// command ran. One still waiting to be taken up after `queued` is withdrawn
/// and never runs; one the main thread has taken up gets its real answer,
/// for which it waits up to `running` more.
fn await_reply(
    answer: &Receiver<Response>,
    claimed: &AtomicBool,
    queued: Duration,
    running: Duration,
) -> (Response, bool) {
    match answer.recv_timeout(queued) {
        Ok(response) => (response, true),
        Err(RecvTimeoutError::Timeout) if claim(claimed) => (
            Response::error(
                "the daemon was busy with an earlier command; nothing was done, try again",
            ),
            false,
        ),
        Err(RecvTimeoutError::Timeout) => (
            answer
                .recv_timeout(running)
                .unwrap_or_else(|_| Response::error("the daemon did not answer in time")),
            true,
        ),
        Err(RecvTimeoutError::Disconnected) => {
            (Response::error("the daemon is shutting down"), false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHORT: Duration = Duration::from_millis(50);
    const LONG: Duration = Duration::from_secs(10);

    #[test]
    fn a_command_still_queued_at_the_deadline_is_withdrawn() {
        let (_reply, answer) = sync_channel::<Response>(1);
        let claimed = AtomicBool::new(false);
        let (response, ran) = await_reply(&answer, &claimed, SHORT, LONG);
        assert!(!ran);
        assert!(
            matches!(&response, Response::Error { message } if message.contains("nothing was done")),
            "{response:?}"
        );
        assert!(!claim(&claimed), "the main thread must not run it now");
    }

    #[test]
    fn a_command_taken_up_before_the_deadline_gets_its_real_answer() {
        let (reply, answer) = sync_channel::<Response>(1);
        let claimed = Arc::new(AtomicBool::new(false));
        let main = {
            let claimed = Arc::clone(&claimed);
            std::thread::spawn(move || {
                assert!(claim(&claimed), "the main thread takes it up first");
                // Busy past the queue deadline, as opening a fleet can be.
                std::thread::sleep(SHORT * 4);
                let _ = reply.send(Response::said("opened agents"));
            })
        };
        while !claimed.load(Ordering::SeqCst) {
            std::thread::yield_now();
        }
        let (response, ran) = await_reply(&answer, &claimed, SHORT, LONG);
        main.join().unwrap();
        assert!(ran);
        assert_eq!(response, Response::said("opened agents"));
    }
}

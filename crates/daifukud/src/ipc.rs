//! The two pipe threads.
//!
//! Each accepts one client at a time, reads one line, and hands the message to
//! the main thread through [`Inbox`]. The hook thread answers only a hook
//! that asks, which waits for a moment: any other hook is gone the moment it
//! has written. The control thread waits for the main thread's reply and
//! writes it back.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, SyncSender, channel, sync_channel};
use std::time::Duration;

use daifuku_core::protocol::{
    HookAnswer, HookLine, HookMessage, Request, Response, UsageReport, from_line, to_line,
};
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

/// How long a hook has, from the moment its instance takes the connection, to
/// deliver its whole line. A hook writes as soon as it connects; a client
/// that is silent or slow past this is cut off, so no client holds a hook
/// instance longer than this. One that keeps connecting again can still keep
/// the instances busy, and a hook that finds none free within its own short
/// wait gives up without a word.
const HOOK_READ: Duration = Duration::from_millis(500);

/// How long a hook that asks waits for the main thread to take its message
/// up. Past this the main thread is busy with something long and the hook
/// gets no task, which it would never learn of if the main thread handed it
/// one later.
const ANSWER_QUEUED: Duration = Duration::from_secs(1);

/// How long the main thread may then take to answer: it reads the backlog
/// and, before handing a task out, asks `daifuku console-window` whether the
/// agent runs in the terminal, which takes up to two seconds.
const ANSWER_RUNNING: Duration = Duration::from_secs(3);

/// How long a hook that asks has to read its answer.
const ANSWER_WRITE: Duration = Duration::from_millis(500);

/// How long a control client has to send its request, and then to read the
/// reply.
const CONTROL_IO: Duration = Duration::from_secs(5);

/// What a pipe thread hands to the main thread.
pub enum Inbound {
    /// An agent's hook reported, and the processes the pipe client and its
    /// ancestors were when it connected, itself first: what the daemon
    /// knows of who sent it, as the message itself can say anything. When
    /// the hook asks, where it waits for the answer and the claim on it.
    Hook(HookMessage, Vec<u32>, Option<Asked>),
    /// A status line reported its account's limits.
    Usage(UsageReport),
    /// A command, where to send the answer, and the claim on it: see
    /// [`claim`].
    Control(Request, SyncSender<Response>, Arc<AtomicBool>),
}

/// Where a hook that asks waits for its answer, and how far it got.
pub type Asked = (SyncSender<HookAnswer>, Arc<Asking>);

/// How far the answer to a hook that asks got, which its pipe thread and the
/// main thread settle between them, so a task is handed out exactly when
/// the hook is told of it.
#[derive(Debug, Default)]
pub struct Asking(AtomicU8);

const QUEUED: u8 = 0;
const TAKEN: u8 = 1;
const ANSWERED: u8 = 2;
const WITHDRAWN: u8 = 3;

impl Asking {
    fn step(&self, from: u8, to: u8) -> bool {
        self.0
            .compare_exchange(from, to, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    /// The main thread takes the hook up. False once the hook stopped
    /// waiting for that.
    pub fn take(&self) -> bool {
        self.step(QUEUED, TAKEN)
    }

    /// The main thread is about to hand the hook a task, and then answers at
    /// once. False once the hook stopped waiting, so nothing is handed out.
    pub fn answer(&self) -> bool {
        self.step(TAKEN, ANSWERED)
    }
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
    // Before reading: the hook waits until its line is read, so it and
    // every process it ran in are still there to be named.
    let chain: Vec<u32> = daifuku_win::process::ancestors(connection.client)
        .iter()
        .map(|p| p.pid)
        .collect();
    let Ok(line) = connection.read_line(HOOK_READ) else {
        return;
    };
    let mut waiting = None;
    let inbound = match from_line::<HookLine>(&line) {
        Ok(HookLine::Event(message)) if message.ask => {
            let (reply, answer) = sync_channel(1);
            let asking = Arc::new(Asking::default());
            waiting = Some((answer, Arc::clone(&asking)));
            Inbound::Hook(message, chain, Some((reply, asking)))
        }
        Ok(HookLine::Event(message)) => Inbound::Hook(message, chain, None),
        Ok(HookLine::Usage(report)) => Inbound::Usage(report),
        Err(error) => {
            tracing::debug!(%error, client = connection.client, "ignored a malformed hook line");
            return;
        }
    };
    if sender.send(inbound).is_err() {
        return;
    }
    wake(main_thread);
    if let Some((answer, asking)) = waiting {
        let answer = await_answer(&answer, &asking, ANSWER_QUEUED, ANSWER_RUNNING);
        if let Ok(line) = to_line(&answer) {
            let _ = connection.write_line(&line, ANSWER_WRITE);
        }
    }
}

/// Waits for the main thread's answer to a hook that asks. One still waiting
/// to be taken up after `queued` is withdrawn and gets no task; one the main
/// thread has taken up waits up to `running` more, and is then withdrawn too
/// unless a task is already on its way, which it then waits for.
fn await_answer(
    answer: &Receiver<HookAnswer>,
    asking: &Asking,
    queued: Duration,
    running: Duration,
) -> HookAnswer {
    match answer.recv_timeout(queued) {
        Ok(answer) => return answer,
        Err(RecvTimeoutError::Disconnected) => return HookAnswer::default(),
        Err(RecvTimeoutError::Timeout) if asking.step(QUEUED, WITHDRAWN) => {
            return HookAnswer::default();
        }
        Err(RecvTimeoutError::Timeout) => {}
    }
    match answer.recv_timeout(running) {
        Ok(answer) => answer,
        Err(RecvTimeoutError::Timeout) if asking.step(TAKEN, WITHDRAWN) => HookAnswer::default(),
        // Past the point the main thread could stop: it answers at once.
        Err(RecvTimeoutError::Timeout) => answer.recv().unwrap_or_default(),
        Err(RecvTimeoutError::Disconnected) => HookAnswer::default(),
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
        Err(error) => not_a_request(&error, connection.client),
    };
    if let Ok(line) = to_line(&response) {
        let _ = connection.write_line(&line, CONTROL_IO);
    }
    if stop {
        let _ = stop_replied.try_send(());
    }
}

/// The answer to a line that is no request this daemon knows, most often a
/// command from a newer `daifuku`. What the parser said goes to the log.
fn not_a_request(error: &serde_json::Error, client: u32) -> Response {
    tracing::warn!(%error, client, "unreadable control request");
    Response::error(format!(
        "the daemon runs {} and does not know this command: run `daifuku install` from the newer daifuku, which also restarts the daemon",
        env!("CARGO_PKG_VERSION")
    ))
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
    fn a_hook_still_queued_at_the_deadline_gets_no_task() {
        let (_reply, answer) = sync_channel::<HookAnswer>(1);
        let asking = Asking::default();
        assert_eq!(
            await_answer(&answer, &asking, SHORT, LONG),
            HookAnswer::default()
        );
        assert!(!asking.take(), "the main thread must not take it up now");
    }

    #[test]
    fn a_hook_taken_up_before_the_deadline_gets_its_task() {
        let (reply, answer) = sync_channel::<HookAnswer>(1);
        let asking = Arc::new(Asking::default());
        let main = {
            let asking = Arc::clone(&asking);
            std::thread::spawn(move || {
                assert!(asking.take());
                std::thread::sleep(SHORT * 4);
                assert!(asking.answer());
                let _ = reply.send(HookAnswer {
                    task: Some("A1".into()),
                });
            })
        };
        while asking.0.load(Ordering::SeqCst) == QUEUED {
            std::thread::yield_now();
        }
        let answer = await_answer(&answer, &asking, SHORT, LONG);
        main.join().unwrap();
        assert_eq!(answer.task.as_deref(), Some("A1"));
    }

    #[test]
    fn a_hook_that_gave_up_on_a_slow_answer_is_handed_nothing() {
        let (_reply, answer) = sync_channel::<HookAnswer>(1);
        let asking = Asking::default();
        assert!(asking.take());
        assert_eq!(
            await_answer(&answer, &asking, SHORT, SHORT),
            HookAnswer::default()
        );
        assert!(!asking.answer(), "the main thread must not hand it one now");
    }

    #[test]
    fn a_task_on_its_way_at_the_deadline_still_reaches_the_hook() {
        let (reply, answer) = sync_channel::<HookAnswer>(1);
        let asking = Arc::new(Asking::default());
        assert!(asking.take());
        assert!(asking.answer());
        let main = std::thread::spawn(move || {
            // Saving the claim, past the hook's deadline.
            std::thread::sleep(SHORT * 4);
            let _ = reply.send(HookAnswer {
                task: Some("A1".into()),
            });
        });
        let answer = await_answer(&answer, &asking, SHORT, SHORT);
        main.join().unwrap();
        assert_eq!(answer.task.as_deref(), Some("A1"));
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

    #[test]
    fn a_command_this_daemon_does_not_know_gets_a_plain_answer() {
        // What a newer daifuku sends for a command added after this daemon.
        let error = from_line::<Request>(r#"{"command":"focus","window":1}"#).unwrap_err();
        assert_eq!(
            not_a_request(&error, 0),
            Response::error(format!(
                "the daemon runs {} and does not know this command: run `daifuku install` from the newer daifuku, which also restarts the daemon",
                env!("CARGO_PKG_VERSION")
            ))
        );
    }
}

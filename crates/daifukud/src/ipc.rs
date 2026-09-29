//! The two pipe threads.
//!
//! Each accepts one client at a time, reads one line, and hands the message to
//! the main thread through [`Inbox`]. The hook thread never answers: a hook is
//! gone the moment it has written. The control thread waits for the main
//! thread's reply and writes it back.

use std::sync::mpsc::{Receiver, Sender, SyncSender, channel, sync_channel};
use std::time::Duration;

use daifuku_core::protocol::{HookMessage, Request, Response, from_line, to_line};
use daifuku_win::pipe::{Pipe, Server};
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_APP};

/// The message the pipe threads post to wake the main loop.
pub const WM_INBOX: u32 = WM_APP + 1;

/// How long a control client waits for the main thread. Opening a fleet waits
/// for Windows Terminal to show its windows, which takes seconds.
const REPLY_TIMEOUT: Duration = Duration::from_secs(30);

/// What a pipe thread hands to the main thread.
pub enum Inbound {
    /// An agent's hook reported.
    Hook(HookMessage),
    /// A command, and where to send the answer.
    Control(Request, SyncSender<Response>),
}

/// The main thread's end: drained after every wake-up.
pub struct Inbox {
    pub receiver: Receiver<Inbound>,
}

/// Claims both pipe names and starts both threads. Fails if either name is
/// taken, which means another daemon runs in this session.
pub fn start(main_thread: u32) -> anyhow::Result<Inbox> {
    let hook = Server::new(Pipe::Hook).map_err(|e| anyhow::anyhow!("hook pipe: {e}"))?;
    let control = Server::new(Pipe::Control).map_err(|e| anyhow::anyhow!("control pipe: {e}"))?;
    let (sender, receiver) = channel();
    spawn(
        "daifuku-hook",
        hook,
        sender.clone(),
        main_thread,
        serve_hook,
    );
    spawn(
        "daifuku-control",
        control,
        sender,
        main_thread,
        serve_control,
    );
    Ok(Inbox { receiver })
}

fn spawn(
    name: &str,
    server: Server,
    sender: Sender<Inbound>,
    main_thread: u32,
    serve: fn(&mut daifuku_win::pipe::Connection, &Sender<Inbound>, u32),
) {
    let _ = std::thread::Builder::new()
        .name(name.to_owned())
        .spawn(move || {
            let mut server = server;
            loop {
                match server.accept() {
                    Ok(mut connection) => serve(&mut connection, &sender, main_thread),
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
    connection: &mut daifuku_win::pipe::Connection,
    sender: &Sender<Inbound>,
    main_thread: u32,
) {
    let Ok(line) = connection.read_line() else {
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
    connection: &mut daifuku_win::pipe::Connection,
    sender: &Sender<Inbound>,
    main_thread: u32,
) {
    let Ok(line) = connection.read_line() else {
        return;
    };
    let response = match from_line::<Request>(&line) {
        Ok(request) => {
            tracing::info!(?request, client = connection.client, "control request");
            let (reply, answer) = sync_channel(1);
            if sender.send(Inbound::Control(request, reply)).is_err() {
                Response::error("the daemon is shutting down")
            } else {
                wake(main_thread);
                answer
                    .recv_timeout(REPLY_TIMEOUT)
                    .unwrap_or_else(|_| Response::error("the daemon did not answer in time"))
            }
        }
        Err(error) => Response::error(format!("not a request: {error}")),
    };
    if let Ok(line) = to_line(&response) {
        let _ = connection.write_line(&line);
    }
}

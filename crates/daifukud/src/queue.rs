//! The tasks the daemon handed out from fleets' backlogs.
//!
//! A backlog is the user's own file, so the daemon reads it, and the claims
//! folder beside it, with the user's rights and never writes there. Which
//! task went to which terminal is the daemon's own record, kept with its
//! state in its locked folder.

use std::io::Read;
use std::path::{Path, PathBuf};

use daifuku_core::backlog::{self, Task};
use daifuku_core::config::folder_key;
use serde::{Deserialize, Serialize};

/// The most tasks handed out at once, across every backlog.
pub const MAX_CLAIMS: usize = 256;

/// The largest backlog read.
const MAX_BACKLOG: u64 = 4 * 1024 * 1024;

/// One task handed out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claim {
    /// The backlog it is in, as the config names it.
    pub backlog: PathBuf,
    /// Its id there.
    pub id: String,
    /// The terminal it went to, 0 once that closed.
    pub window: u64,
    /// The session it went to.
    pub session: String,
    /// When, in seconds since 1970.
    pub at: u64,
}

/// A backlog as read.
#[derive(Debug, Default)]
pub struct Backlog {
    /// Its tasks, in the file's order.
    pub tasks: Vec<Task>,
    /// The open ones a folder in `claims` beside the file says someone else
    /// took, as agents that claim by hand mark them.
    pub taken: Vec<String>,
}

/// Reads the backlog at `path`, with the rights of the user at the desktop
/// when the daemon runs `elevated`: a link in the user's folders then leads
/// nowhere the user could not read anyway.
///
/// # Errors
///
/// One plain sentence, when the file cannot be read.
pub fn read(path: &Path, elevated: bool) -> Result<Backlog, String> {
    if elevated {
        daifuku_win::process::as_shell_user(|| read_as_user(path))
            .map_err(|e| format!("could not read it with your rights: {e}"))?
    } else {
        read_as_user(path)
    }
}

fn read_as_user(path: &Path) -> Result<Backlog, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    file.take(MAX_BACKLOG + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_BACKLOG {
        return Err("it is larger than 4 MiB".to_owned());
    }
    let text = daifuku_core::config::decode(&bytes).map_err(|e| e.to_string())?;
    let tasks = backlog::parse(&text);
    let claims = path.parent().map(|d| d.join("claims"));
    let taken = tasks
        .iter()
        .filter(|t| t.is_open())
        .filter(|t| claims.as_ref().is_some_and(|c| c.join(&t.id).is_dir()))
        .map(|t| t.id.clone())
        .collect();
    Ok(Backlog { tasks, taken })
}

/// Every task handed out, and where they are kept.
#[derive(Debug, Default)]
pub struct Queue {
    /// Oldest first.
    claims: Vec<Claim>,
    /// Where the claims are kept across a restart, `None` when they are not,
    /// as for the daemon's other state.
    pub file: Option<PathBuf>,
}

impl Queue {
    /// The tasks handed out from the backlog at `path`.
    pub fn claims_in<'a>(&'a self, path: &'a Path) -> impl Iterator<Item = &'a Claim> {
        let key = folder_key(path);
        self.claims
            .iter()
            .filter(move |c| folder_key(&c.backlog) == key)
    }

    /// Lets go of the tasks from `path` its backlog now lists as finished or
    /// no longer lists. True when any went.
    pub fn settle(&mut self, path: &Path, backlog: &Backlog) -> bool {
        let key = folder_key(path);
        let before = self.claims.len();
        self.claims.retain(|c| {
            folder_key(&c.backlog) != key
                || backlog
                    .tasks
                    .iter()
                    .any(|t| t.id == c.id && !t.is_finished())
        });
        self.claims.len() != before
    }

    /// The task the session `session` wherever it runs still works on, or
    /// terminal `w` once the session it went to no longer runs, as after a
    /// clear or a restart there. `running` says whether a session still runs.
    pub fn held(&self, w: u64, session: &str, running: impl Fn(&str) -> bool) -> Option<&Claim> {
        self.claims
            .iter()
            .find(|c| c.session == session || (w != 0 && c.window == w && !running(&c.session)))
    }

    /// The first open task of the backlog at `path` that nobody has: not
    /// handed out and not taken by hand.
    pub fn next<'a>(&self, path: &Path, backlog: &'a Backlog) -> Option<&'a Task> {
        backlog
            .tasks
            .iter()
            .filter(|t| t.is_open())
            .find(|t| !backlog.taken.contains(&t.id) && !self.claims_in(path).any(|c| c.id == t.id))
    }

    /// Hands a task out. False when [`MAX_CLAIMS`] are out already.
    pub fn claim(&mut self, claim: Claim) -> bool {
        if self.claims.len() >= MAX_CLAIMS {
            return false;
        }
        self.claims.push(claim);
        true
    }

    /// Takes back the task `id`, from every backlog. How many went.
    pub fn release(&mut self, id: &str) -> usize {
        let before = self.claims.len();
        self.claims.retain(|c| c.id != id);
        before - self.claims.len()
    }

    /// Terminal `w` closed: its tasks stay with the sessions they went to,
    /// which may come back in another one. True when any did.
    pub fn window_closed(&mut self, w: u64) -> bool {
        let mut changed = false;
        for c in self.claims.iter_mut().filter(|c| c.window == w && w != 0) {
            c.window = 0;
            changed = true;
        }
        changed
    }

    /// Reads [`Queue::file`].
    pub fn load(&mut self) {
        let Some(file) = &self.file else { return };
        let Ok(text) = std::fs::read_to_string(file) else {
            return;
        };
        match serde_json::from_str::<Vec<Claim>>(&text) {
            Ok(mut claims) => {
                claims.retain(|c| backlog::id_ok(&c.id));
                claims.truncate(MAX_CLAIMS);
                self.claims = claims;
            }
            Err(_) => tracing::warn!(path = %file.display(), "ignored an unreadable tasks file"),
        }
    }

    /// Writes [`Queue::file`].
    pub fn save(&self) {
        let Some(file) = &self.file else { return };
        let written = serde_json::to_string_pretty(&self.claims)
            .map_err(std::io::Error::other)
            .and_then(|text| crate::daemon::replace_file(file, &text));
        if let Err(e) = written {
            tracing::warn!(path = %file.display(), error = %e, "could not save the tasks");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = r"C:\work\BACKLOG.md";

    fn backlog(text: &str) -> Backlog {
        Backlog {
            tasks: backlog::parse(text),
            taken: Vec::new(),
        }
    }

    fn claim(id: &str, window: u64, session: &str) -> Claim {
        Claim {
            backlog: PathBuf::from(FILE),
            id: id.to_owned(),
            window,
            session: session.to_owned(),
            at: 1,
        }
    }

    const TEXT: &str = "\
### A1. First | owner: | status: open
### A2. Second | owner: | status: open
### A3. Third | owner: | status: open
";

    #[test]
    fn the_next_task_is_the_first_open_one_nobody_has() {
        let mut q = Queue::default();
        let mut b = backlog(TEXT);
        assert_eq!(q.next(Path::new(FILE), &b).unwrap().id, "A1");
        assert!(q.claim(claim("A1", 7, "s1")));
        b.taken.push("A2".to_owned());
        assert_eq!(q.next(Path::new(FILE), &b).unwrap().id, "A3");
        // The same file, written another way, is the same backlog.
        assert_eq!(
            q.next(Path::new(r"c:\WORK\backlog.md"), &b).unwrap().id,
            "A3"
        );
        // Another backlog's claims are its own.
        assert_eq!(
            q.next(Path::new(r"C:\other\BACKLOG.md"), &b).unwrap().id,
            "A1"
        );
    }

    #[test]
    fn a_task_is_let_go_once_its_backlog_says_it_is_finished_or_drops_it() {
        let mut q = Queue::default();
        q.claim(claim("A1", 7, "s1"));
        q.claim(claim("A2", 8, "s2"));
        q.claim(claim("A3", 9, "s3"));
        let now = "\
### A1. First | owner: crew-1 | status: running
### A2. Second | owner: crew-2 | status: done, merged
";
        assert!(q.settle(Path::new(FILE), &backlog(now)));
        let left: Vec<&str> = q.claims.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(left, ["A1"]);
        assert!(!q.settle(Path::new(FILE), &backlog(now)));
    }

    #[test]
    fn a_terminal_or_its_session_holds_its_task() {
        let mut q = Queue::default();
        let ended = |_: &str| false;
        q.claim(claim("A1", 7, "s1"));
        assert!(q.held(7, "other", ended).is_some());
        assert!(q.held(8, "s1", ended).is_some());
        assert!(q.held(8, "s2", ended).is_none());
        // A closed terminal's task stays with its session alone.
        assert!(q.window_closed(7));
        assert!(q.held(7, "other", ended).is_none());
        assert!(q.held(0, "other", ended).is_none());
        assert!(q.held(9, "s1", ended).is_some());
    }

    #[test]
    fn a_second_session_in_a_held_terminal_is_not_held() {
        let mut q = Queue::default();
        q.claim(claim("A1", 7, "s1"));
        let both_run = |s: &str| s == "s1" || s == "s2";
        assert!(q.held(7, "s2", both_run).is_none());
        assert!(q.held(7, "s1", both_run).is_some());
    }

    #[test]
    fn a_new_session_in_a_held_terminal_holds_its_task() {
        let mut q = Queue::default();
        q.claim(claim("A1", 7, "s1"));
        // After a clear or a restart only the new session runs there.
        let new_only = |s: &str| s == "s2";
        assert_eq!(q.held(7, "s2", new_only).unwrap().id, "A1");
        assert!(q.held(8, "s2", new_only).is_none());
    }

    #[test]
    fn a_released_task_goes_out_again_while_it_is_open() {
        let mut q = Queue::default();
        let b = backlog(TEXT);
        q.claim(claim("A1", 7, "s1"));
        assert_eq!(q.release("A1"), 1);
        assert_eq!(q.release("A1"), 0);
        assert_eq!(q.next(Path::new(FILE), &b).unwrap().id, "A1");
    }

    #[test]
    fn no_more_than_the_cap_go_out() {
        let mut q = Queue::default();
        for i in 0..MAX_CLAIMS {
            assert!(q.claim(claim(&format!("T{i}"), 1, "s")));
        }
        assert!(!q.claim(claim("X", 1, "s")));
    }

    #[test]
    fn a_backlog_and_its_claims_folder_are_read() {
        let dir = std::env::temp_dir().join(format!("daifuku-queue-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("claims").join("A2")).unwrap();
        let file = dir.join("BACKLOG.md");
        std::fs::write(&file, TEXT).unwrap();
        let b = read(&file, false).unwrap();
        assert_eq!(b.tasks.len(), 3);
        assert_eq!(b.taken, ["A2"]);
        std::fs::write(&file, vec![b'x'; usize::try_from(MAX_BACKLOG).unwrap() + 1]).unwrap();
        assert!(read(&file, false).unwrap_err().contains("4 MiB"));
        assert!(read(&dir.join("missing.md"), false).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

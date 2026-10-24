//! A backlog: the tasks a fleet works through, one markdown file.
//!
//! Each task is a third-level heading, its fields after bars:
//!
//! ```text
//! ### A1. Write the setup guide | owner: | status: open
//! Everything up to the next heading is the task's text.
//! ```
//!
//! The id runs up to the first full stop. A task is open while no owner is
//! written and its status starts with `open`, and finished once its status
//! starts with `done` or `closed`. Daifuku only reads the file: the agent a
//! task is handed to reads its text there, and writes its own name and
//! status in as it works.

/// The longest id taken, in characters.
pub const MAX_ID: usize = 32;

/// One task, as its heading says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    /// What comes before the first full stop, such as `A1`.
    pub id: String,
    /// The rest of the heading's first part.
    pub title: String,
    /// The `owner:` field, empty when there is none.
    pub owner: String,
    /// The `status:` field, empty when there is none.
    pub status: String,
}

impl Task {
    /// Nobody has it and its status says it is open.
    #[must_use]
    pub fn is_open(&self) -> bool {
        let owner = self.owner.trim();
        (owner.is_empty() || owner == "-" || owner.eq_ignore_ascii_case("none"))
            && first_word(&self.status) == "open"
    }

    /// Its status says it is done or closed.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        matches!(first_word(&self.status).as_str(), "done" | "closed")
    }
}

/// Every task in a backlog's text, in the order the file has them, which is
/// the order they are handed out in.
#[must_use]
pub fn parse(text: &str) -> Vec<Task> {
    text.lines().filter_map(task).collect()
}

/// An id Daifuku takes: letters, digits, `-` and `_`, at most [`MAX_ID`].
#[must_use]
pub fn id_ok(id: &str) -> bool {
    !id.is_empty()
        && id.chars().count() <= MAX_ID
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
}

fn task(line: &str) -> Option<Task> {
    let rest = line.strip_prefix("### ")?;
    let mut parts = rest.split('|');
    let head = parts.next()?.trim();
    let (id, title) = head.split_once('.')?;
    let id = id.trim();
    if !id_ok(id) {
        return None;
    }
    let mut task = Task {
        id: id.to_owned(),
        title: title.trim().to_owned(),
        owner: String::new(),
        status: String::new(),
    };
    for part in parts {
        let Some((key, value)) = part.split_once(':') else {
            continue;
        };
        let value = value.trim().to_owned();
        match key.trim().to_ascii_lowercase().as_str() {
            "owner" => task.owner = value,
            "status" => task.status = value,
            _ => {}
        }
    }
    Some(task)
}

/// The first word, in lower case: `open` for `Open, after A2`.
fn first_word(text: &str) -> String {
    text.trim_start()
        .chars()
        .take_while(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    const BACKLOG: &str = "\
# Backlog

### A1. Write the setup guide | owner: | status: open
Steps and sources.

### A2. Fix the. dotted title | owner: crew-2 | status: running
### A3. Ship it | status: done, merged
### A4. Later | owner: none | status: Open, after A2
#### B1. Not a task | status: open
### Notes without an id
### A b. Not an id | status: open
### A5. No fields
";

    #[test]
    fn every_task_heading_is_read_in_file_order() {
        let tasks = parse(BACKLOG);
        let ids: Vec<&str> = tasks.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["A1", "A2", "A3", "A4", "A5"]);
        assert_eq!(tasks[0].title, "Write the setup guide");
        assert_eq!(tasks[1].title, "Fix the. dotted title");
        assert_eq!(tasks[1].owner, "crew-2");
        assert_eq!(tasks[2].status, "done, merged");
        assert_eq!(
            (tasks[4].owner.as_str(), tasks[4].status.as_str()),
            ("", "")
        );
    }

    #[test]
    fn open_means_no_owner_and_a_status_starting_with_open() {
        let open: Vec<String> = parse(BACKLOG)
            .into_iter()
            .filter(Task::is_open)
            .map(|t| t.id)
            .collect();
        assert_eq!(open, ["A1", "A4"]);
    }

    #[test]
    fn finished_means_done_or_closed() {
        let finished: Vec<String> = parse(
            "### A. x | status: Closed\n### B. y | status: done\n### C. z | status: doneish\n",
        )
        .into_iter()
        .filter(Task::is_finished)
        .map(|t| t.id)
        .collect();
        assert_eq!(finished, ["A", "B"]);
    }

    #[test]
    fn an_id_is_short_and_plain() {
        assert!(id_ok("C6"));
        assert!(id_ok("LAND-W_2"));
        assert!(!id_ok(""));
        assert!(!id_ok("a b"));
        assert!(!id_ok("x\"y"));
        assert!(!id_ok(&"a".repeat(MAX_ID + 1)));
    }
}

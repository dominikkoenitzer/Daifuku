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
//! starts with `done` or `closed`. Headings inside fenced code are text, not
//! tasks. Daifuku only reads the file: the agent a
//! task is handed to reads its text there, and writes its own name and
//! status in as it works.

/// The longest id taken, in characters.
pub const MAX_ID: usize = 32;

/// The most of a title, owner or status kept, in characters.
pub const MAX_TEXT: usize = 200;

/// One task, as its heading says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    /// What comes before the first full stop, such as `A1`.
    pub id: String,
    /// The rest of the heading's first part. This and the fields are cut to
    /// [`MAX_TEXT`], without control characters.
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
    let mut open: Option<(char, usize)> = None;
    let mut tasks = Vec::new();
    for line in text.lines() {
        if let Some((mark, length, bare)) = fence(line) {
            match open {
                None => open = Some((mark, length)),
                Some((m, l)) if m == mark && length >= l && bare => open = None,
                Some(_) => {}
            }
        } else if open.is_none() {
            tasks.extend(task(line));
        }
    }
    tasks
}

/// The fence a line opens or closes code with: its character, how many,
/// and whether nothing follows them.
fn fence(line: &str) -> Option<(char, usize, bool)> {
    let rest = line.trim_start_matches(' ');
    if line.len() - rest.len() > 3 {
        return None;
    }
    let mark = rest.chars().next().filter(|c| matches!(c, '`' | '~'))?;
    let length = rest.chars().take_while(|&c| c == mark).count();
    (length >= 3).then(|| (mark, length, rest[length..].trim().is_empty()))
}

fn plain(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control())
        .take(MAX_TEXT)
        .collect()
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
        title: plain(title.trim()),
        owner: String::new(),
        status: String::new(),
    };
    for part in parts {
        let Some((key, value)) = part.split_once(':') else {
            continue;
        };
        let value = plain(value.trim());
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
    fn headings_in_fenced_code_are_not_tasks() {
        let tasks = parse(
            "### A1. Real | status: open\n\
             ```markdown\n\
             ### X1. Example | status: open\n\
             ```\n\
             ~~~~\n\
             ### X2. Example | status: open\n\
             ~~~ still inside\n\
             ```\n\
             ~~~~~\n\
             ### A2. Real too | status: open\n\
             \x20\x20\x20\x20```\n\
             ### A3. After an indented fence | status: open\n",
        );
        let ids: Vec<&str> = tasks.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["A1", "A2", "A3"]);
    }

    #[test]
    fn only_a_bare_fence_of_the_same_mark_and_length_ends_code() {
        let tasks = parse(
            "~~~~\n\
             ```\n\
             ### X1. Other mark | status: open\n\
             ~~~\n\
             ### X2. Too short | status: open\n\
             ~~~~ text\n\
             ### X3. Not bare | status: open\n\
             ~~~~\n\
             ### A1. Real | status: open\n\
             \x20\x20\x20```\n\
             ### X4. After a fence indented three spaces | status: open\n",
        );
        let ids: Vec<&str> = tasks.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["A1"]);
    }

    #[test]
    fn a_dash_for_an_owner_is_nobody_and_a_name_is_someone() {
        let open: Vec<String> = parse(
            "### A1. x | owner: - | status: open\n### A2. y | owner: crew-1 | status: open\n",
        )
        .into_iter()
        .filter(Task::is_open)
        .map(|t| t.id)
        .collect();
        assert_eq!(open, ["A1"]);
    }

    #[test]
    fn titles_and_fields_are_cut_and_plain() {
        let long = "x".repeat(MAX_TEXT + 50);
        let tasks = parse(&format!(
            "### A1. Bad\u{1b}[2J\u{7}title {long} | owner: \u{1b}]0;x\u{7} | status: open\t{long}\n"
        ));
        assert_eq!(tasks[0].title.chars().count(), MAX_TEXT);
        assert!(tasks[0].title.starts_with("Bad[2Jtitle x"));
        assert_eq!(tasks[0].owner, "]0;x");
        assert_eq!(tasks[0].status.chars().count(), MAX_TEXT);
        assert!(!tasks.iter().any(|t| {
            [&t.title, &t.owner, &t.status]
                .iter()
                .any(|s| s.chars().any(char::is_control))
        }));
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

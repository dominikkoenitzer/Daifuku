//! A `PATH` value as a list of folders.
//!
//! Installing adds one folder to the machine `PATH` and uninstalling takes the
//! same folder out again. Both work on the value as written, so every other
//! entry keeps its spelling, its order and any `%VAR%` in it.

/// Whether an entry of `value` names `dir`: is the same folder once ASCII
/// case, surrounding whitespace and quotes, and trailing backslashes are
/// ignored.
#[must_use]
pub fn contains(value: &str, dir: &str) -> bool {
    entries(value).into_iter().any(|entry| names(entry, dir))
}

/// `value` with `dir` added at the end, or `None` when an entry already
/// names it.
///
/// The new entry is separated by one `;`, and none is added when the value is
/// empty or already ends with one.
#[must_use]
pub fn add(value: &str, dir: &str) -> Option<String> {
    if contains(value, dir) {
        return None;
    }
    let mut out = value.to_owned();
    if !out.is_empty() && !out.ends_with(';') {
        out.push(';');
    }
    out.push_str(dir);
    Some(out)
}

/// `value` without every entry that names `dir`, or `None` when none does.
///
/// Every other entry stays as written and in its place, empty ones too.
#[must_use]
pub fn remove(value: &str, dir: &str) -> Option<String> {
    if !contains(value, dir) {
        return None;
    }
    let kept: Vec<&str> = entries(value)
        .into_iter()
        .filter(|entry| !names(entry, dir))
        .collect();
    Some(kept.join(";"))
}

/// The entries of `value` as written. A `;` inside double quotes belongs to
/// the folder name, the way `cmd` reads the value, so a quoted folder is never
/// cut in two.
fn entries(value: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut quoted = false;
    for (i, c) in value.char_indices() {
        match c {
            '"' => quoted = !quoted,
            ';' if !quoted => {
                out.push(&value[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&value[start..]);
    out
}

/// Whether `entry` names `dir`. Windows ignores the case of ASCII letters in
/// paths, and people write entries with spaces around them, in quotes or with
/// a trailing backslash, so none of that makes them a different folder.
fn names(entry: &str, dir: &str) -> bool {
    bare(entry).eq_ignore_ascii_case(bare(dir))
}

/// A folder without surrounding whitespace and quotes and without trailing
/// backslashes.
fn bare(s: &str) -> &str {
    s.trim().trim_matches('"').trim_end_matches('\\')
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIR: &str = r"C:\Program Files\Daifuku";

    #[test]
    fn appends_with_one_separator() {
        assert_eq!(
            add(r"C:\Windows", DIR).as_deref(),
            Some(r"C:\Windows;C:\Program Files\Daifuku")
        );
    }

    #[test]
    fn does_not_double_a_trailing_separator() {
        assert_eq!(
            add(r"C:\Windows;", DIR).as_deref(),
            Some(r"C:\Windows;C:\Program Files\Daifuku")
        );
    }

    #[test]
    fn adds_to_an_empty_value_without_a_separator() {
        assert_eq!(add("", DIR).as_deref(), Some(DIR));
    }

    #[test]
    fn keeps_variables_unexpanded() {
        assert_eq!(
            add(r"%SystemRoot%\system32", DIR).as_deref(),
            Some(r"%SystemRoot%\system32;C:\Program Files\Daifuku")
        );
    }

    #[test]
    fn adds_nothing_when_an_entry_already_names_the_folder() {
        assert_eq!(add(&format!(r"C:\Windows;{DIR};C:\Tools"), DIR), None);
        assert_eq!(add(DIR, DIR), None);
    }

    #[test]
    fn ignores_ascii_case() {
        assert!(contains(r"c:\program files\DAIFUKU", DIR));
    }

    #[test]
    fn ignores_surrounding_whitespace() {
        assert!(contains(
            r"C:\Windows; C:\Program Files\Daifuku ;C:\Tools",
            DIR
        ));
    }

    #[test]
    fn ignores_quotes() {
        assert!(contains(r#""C:\Program Files\Daifuku""#, DIR));
        assert!(contains(r#"C:\Windows; "C:\Program Files\Daifuku" "#, DIR));
    }

    #[test]
    fn ignores_trailing_backslashes() {
        assert!(contains(r"C:\Program Files\Daifuku\", DIR));
        assert!(contains(r"C:\Program Files\Daifuku\\", DIR));
        assert!(contains(DIR, r"C:\Program Files\Daifuku\"));
    }

    #[test]
    fn a_longer_or_shorter_folder_is_another_one() {
        assert!(!contains(r"C:\Program Files\Daifuku\bin", DIR));
        assert!(!contains(r"C:\Program Files", DIR));
        assert!(!contains(r"C:\Program Files\Daifuku2", DIR));
        assert!(!contains("", DIR));
    }

    #[test]
    fn splits_at_every_separator() {
        assert!(contains(
            r"C:\Windows;C:\Program Files\Daifuku;C:\Tools",
            DIR
        ));
        assert!(contains("a;b;c", "a"));
        assert!(contains("a;b;c", "b"));
        assert!(contains("a;b;c", "c"));
        assert!(!contains("a;b;c", "a;b"));
    }

    #[test]
    fn a_quoted_separator_is_part_of_the_folder() {
        assert!(!contains(r#""C:\Program Files\Daifuku;C:\Tools""#, DIR));
        assert!(!contains(r#""C:\Tools;C:\Program Files\Daifuku""#, DIR));
        assert!(contains(r#""C:\A;B";C:\Program Files\Daifuku"#, DIR));
        assert!(contains(r#""C:\A;B";C:\Tools"#, r"C:\A;B"));
    }

    #[test]
    fn removes_the_entry_and_its_separator() {
        assert_eq!(
            remove(r"C:\Windows;C:\Program Files\Daifuku;C:\Tools", DIR).as_deref(),
            Some(r"C:\Windows;C:\Tools")
        );
        assert_eq!(
            remove(r"C:\Windows;C:\Program Files\Daifuku", DIR).as_deref(),
            Some(r"C:\Windows")
        );
        assert_eq!(
            remove(r"C:\Program Files\Daifuku;C:\Windows", DIR).as_deref(),
            Some(r"C:\Windows")
        );
        assert_eq!(remove(DIR, DIR).as_deref(), Some(""));
    }

    #[test]
    fn removes_every_entry_that_names_the_folder() {
        assert_eq!(
            remove(
                r#"c:\program files\daifuku\;C:\Windows;"C:\Program Files\Daifuku""#,
                DIR
            )
            .as_deref(),
            Some(r"C:\Windows")
        );
    }

    #[test]
    fn keeps_every_other_entry_as_written_and_in_order() {
        assert_eq!(
            remove(
                r#"%SystemRoot%\system32; C:\Tools\ ;;"C:\A;B";C:\Program Files\Daifuku;C:\Zed;"#,
                DIR
            )
            .as_deref(),
            Some(r#"%SystemRoot%\system32; C:\Tools\ ;;"C:\A;B";C:\Zed;"#)
        );
    }

    #[test]
    fn removes_nothing_when_no_entry_names_the_folder() {
        assert_eq!(remove(r"C:\Windows;C:\Tools", DIR), None);
        assert_eq!(remove("", DIR), None);
        assert_eq!(remove(r#""C:\Program Files\Daifuku;C:\Tools""#, DIR), None);
    }
}

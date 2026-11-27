// Failing-first test for Daifuku: a stray quote already in PATH must not swallow the folder install adds.
// Fails today: add() appends after an open quote, so the folder becomes part of the quoted entry: contains()
// never finds it, every install appends another copy, and uninstall cannot take it out again.
use daifuku_core::path_list::{add, contains, remove};

const DIR: &str = r"C:\Users\me\AppData\Local\Daifuku";

#[test]
fn install_after_a_stray_quote_adds_a_folder_of_its_own_once() {
    for value in [
        r#"C:\Windows;"C:\Program Files\Broken;C:\Tools"#,
        r#"C:\Windows;C:\Odd"Dir;C:\Tools"#,
    ] {
        let once = add(value, DIR).expect("the folder is not there yet");
        assert!(contains(&once, DIR), "{once}");
        assert_eq!(
            add(&once, DIR),
            None,
            "a second install adds nothing: {once}"
        );
        let back = remove(&once, DIR).expect("uninstall finds the folder");
        assert!(!contains(&back, DIR), "{back}");
        assert!(
            back.starts_with(value),
            "every other entry keeps its spelling: {back}"
        );
    }
}

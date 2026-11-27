// A stray quote already in PATH does not swallow the folder install adds.
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

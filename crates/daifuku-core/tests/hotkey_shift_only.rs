// Failing-first test for Daifuku: shift alone on a typing key is a bare key in disguise.
// `shift + a` registered with RegisterHotKey takes every capital A from every program, which is the very
// thing NoModifier exists to prevent. Fails today: shift counts as the one modifier required.
use daifuku_core::hotkey::Hotkey;

#[test]
fn shift_alone_on_a_typing_key_is_refused() {
    for text in [
        "shift + a",
        "shift + 1",
        "shift + space",
        "shift + semicolon",
        "shift + slash",
        "shift + tab",
        "shift + return",
        "shift + backspace",
        "shift + left",
        "shift + up",
        "shift + home",
        "shift + end",
        "shift + pageup",
        "shift + pagedown",
        "shift + insert",
        "shift + delete",
    ] {
        assert!(
            text.parse::<Hotkey>().is_err(),
            "{text} would swallow typing or editing"
        );
    }
}

#[test]
fn shift_with_another_modifier_or_on_a_non_typing_key_still_parses() {
    for text in [
        "ctrl + shift + a",
        "shift + f5",
        "shift + escape",
        "alt + shift + 1",
        "ctrl + shift + tab",
        "win + shift + left",
        "shift + pause",
    ] {
        assert!(text.parse::<Hotkey>().is_ok(), "{text}");
    }
}

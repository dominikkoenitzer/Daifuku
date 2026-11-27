// Failing-first test for Daifuku: a stop_at rule for a window no report has given yet must be visible,
// not silent. Fails today: there is no way to ask, so `daifuku status` shows nothing for the week and a
// week rule looks satisfied. (It fails to compile until Limits::unreported exists.)
use daifuku_core::config::{LimitWindow, StopAt};
use daifuku_core::usage::{Limits, Window};

fn rule(window: Option<LimitWindow>) -> StopAt {
    StopAt {
        share: 90,
        window,
        days: None,
        hours: None,
        last_minutes: None,
    }
}

#[test]
fn a_week_rule_without_a_week_number_names_the_week() {
    let limits = Limits {
        five_hour: Some(Window {
            used: 10,
            resets_at: 2_000_000_000,
        }),
        seven_day: None,
    };
    assert_eq!(
        limits.unreported(&[rule(Some(LimitWindow::SevenDay))]),
        vec!["week"]
    );
    assert_eq!(limits.unreported(&[rule(None)]), vec!["week"]);
}

#[test]
fn a_window_no_rule_names_or_one_that_was_reported_is_not_listed() {
    let limits = Limits {
        five_hour: Some(Window {
            used: 10,
            resets_at: 2_000_000_000,
        }),
        seven_day: None,
    };
    assert!(
        limits
            .unreported(&[rule(Some(LimitWindow::FiveHour))])
            .is_empty()
    );
    assert!(limits.unreported(&[]).is_empty());
    let both = Limits {
        seven_day: Some(Window {
            used: 5,
            resets_at: 2_000_000_000,
        }),
        ..limits
    };
    assert!(both.unreported(&[rule(None)]).is_empty());
}

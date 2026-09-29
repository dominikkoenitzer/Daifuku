//! Adding Daifuku's hooks to Claude Code's `settings.json`, and taking them
//! out again, without touching anything else in the file.
//!
//! The file is the user's. Other hooks stay exactly where they are, keys keep
//! their order, and running the install twice changes nothing the second
//! time. A Daifuku hook is recognised by its command being a `daifuku.exe`
//! with `hook` as its argument, so an uninstall removes exactly those.

use serde_json::{Map, Value, json};

/// Every Claude Code event that moves an agent between states. Anything not
/// listed would only cost a process start per event for nothing.
pub const EVENTS: [&str; 15] = [
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "PermissionRequest",
    "PermissionDenied",
    "Notification",
    "Elicitation",
    "ElicitationResult",
    "SubagentStart",
    "PreCompact",
    "Stop",
    "StopFailure",
    "SessionEnd",
];

fn is_ours(hook: &Value) -> bool {
    let command = hook.get("command").and_then(Value::as_str).unwrap_or("");
    let first_arg = hook
        .get("args")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(Value::as_str);
    command.to_ascii_lowercase().ends_with("daifuku.exe") && first_arg == Some("hook")
}

/// Adds a Daifuku hook for every event in [`EVENTS`], calling `exe`. Returns
/// how many events gained one; zero means the file already had them all.
///
/// The hook runs `async`, so Claude Code never waits for it, with a short
/// timeout for the case where something does go wrong.
///
/// # Errors
///
/// When `settings` is not a JSON object, or its `hooks` is not one.
pub fn add_hooks(settings: &mut Value, exe: &str) -> Result<usize, String> {
    let root = settings
        .as_object_mut()
        .ok_or("settings.json is not a JSON object")?;
    let hooks = root
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()));
    let hooks = hooks
        .as_object_mut()
        .ok_or("`hooks` in settings.json is not an object")?;
    let mut added = 0;
    for event in EVENTS {
        let groups = hooks
            .entry(event)
            .or_insert_with(|| Value::Array(Vec::new()));
        let Some(groups) = groups.as_array_mut() else {
            return Err(format!("`hooks.{event}` in settings.json is not a list"));
        };
        let present = groups.iter().any(|g| {
            g.get("hooks")
                .and_then(Value::as_array)
                .is_some_and(|hs| hs.iter().any(is_ours))
        });
        if present {
            continue;
        }
        groups.push(json!({
            "hooks": [{
                "type": "command",
                "command": exe,
                "args": ["hook"],
                "async": true,
                "timeout": 10
            }]
        }));
        added += 1;
    }
    Ok(added)
}

/// Removes every Daifuku hook, and any group or event list that held only
/// Daifuku's. Returns how many hooks were removed.
pub fn remove_hooks(settings: &mut Value) -> usize {
    let Some(hooks) = settings.get_mut("hooks").and_then(Value::as_object_mut) else {
        return 0;
    };
    let mut removed = 0;
    for groups in hooks.values_mut() {
        let Some(groups) = groups.as_array_mut() else {
            continue;
        };
        for group in groups.iter_mut() {
            if let Some(hs) = group.get_mut("hooks").and_then(Value::as_array_mut) {
                let before = hs.len();
                hs.retain(|h| !is_ours(h));
                removed += before - hs.len();
            }
        }
        groups.retain(|g| {
            g.get("hooks")
                .and_then(Value::as_array)
                .is_none_or(|hs| !hs.is_empty())
        });
    }
    hooks.retain(|_, groups| groups.as_array().is_none_or(|g| !g.is_empty()));
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXE: &str = r"C:\Program Files\Daifuku\daifuku.exe";

    /// A settings file with a hook of the user's own and keys in a
    /// deliberate order.
    fn users_file() -> Value {
        serde_json::from_str(
            r#"{
              "theme": "dark",
              "hooks": {
                "PreToolUse": [
                  { "matcher": "Bash", "hooks": [ { "type": "command", "command": "C:/check.exe", "args": ["x"] } ] }
                ]
              },
              "effortLevel": "high"
            }"#,
        )
        .unwrap()
    }

    #[test]
    fn adds_one_hook_per_event_and_keeps_the_users_own() {
        let mut s = users_file();
        assert_eq!(add_hooks(&mut s, EXE), Ok(EVENTS.len()));
        let pre = s["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(pre.len(), 2, "the user's group stays, ours is appended");
        assert_eq!(pre[0]["hooks"][0]["command"], "C:/check.exe");
        assert_eq!(pre[1]["hooks"][0]["command"], EXE);
        assert_eq!(pre[1]["hooks"][0]["async"], true);
        for e in EVENTS {
            assert!(s["hooks"][e].is_array(), "{e}");
        }
    }

    #[test]
    fn installing_twice_changes_nothing() {
        let mut s = users_file();
        add_hooks(&mut s, EXE).unwrap();
        let once = s.clone();
        assert_eq!(add_hooks(&mut s, EXE), Ok(0));
        assert_eq!(s, once);
    }

    #[test]
    fn key_order_survives() {
        let mut s = users_file();
        add_hooks(&mut s, EXE).unwrap();
        let keys: Vec<_> = s.as_object().unwrap().keys().cloned().collect();
        assert_eq!(keys, ["theme", "hooks", "effortLevel"]);
    }

    #[test]
    fn uninstall_removes_exactly_ours() {
        let mut s = users_file();
        add_hooks(&mut s, EXE).unwrap();
        assert_eq!(remove_hooks(&mut s), EVENTS.len());
        assert_eq!(s, users_file(), "back to the file as the user had it");
    }

    #[test]
    fn an_empty_file_gets_a_hooks_section() {
        let mut s = json!({});
        add_hooks(&mut s, EXE).unwrap();
        assert_eq!(s["hooks"].as_object().unwrap().len(), EVENTS.len());
        remove_hooks(&mut s);
        assert_eq!(s, json!({"hooks": {}}));
    }

    #[test]
    fn a_malformed_file_is_refused_not_overwritten() {
        assert!(add_hooks(&mut json!([]), EXE).is_err());
        assert!(add_hooks(&mut json!({"hooks": []}), EXE).is_err());
        assert!(add_hooks(&mut json!({"hooks": {"Stop": {}}}), EXE).is_err());
    }

    #[test]
    fn every_listed_event_changes_a_state() {
        use crate::state::{HookEvent, Transition};
        for e in EVENTS {
            let t = HookEvent {
                session_id: "s".into(),
                hook_event_name: e.into(),
                notification_type: Some("permission_prompt".into()),
            }
            .transition();
            assert_ne!(
                t,
                Transition::Ignore,
                "{e} would cost a process start for nothing"
            );
        }
    }
}

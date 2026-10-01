//! Adding Daifuku's hooks to an agent's settings, and taking them out again,
//! without touching anything else in the file.
//!
//! Two agents, one file format: Claude Code's `~/.claude/settings.json` and
//! Codex's `~/.codex/hooks.json` both keep hooks as
//! `hooks.<Event>[] = { matcher?, hooks: [ { type: "command", ... } ] }`.
//! They differ in the event names and in how a command is written: Claude
//! Code takes a program and its arguments, Codex one command line.
//!
//! The file is the user's. Other hooks stay exactly where they are, keys keep
//! their order, and installing twice changes nothing the second time. A
//! Daifuku hook is recognised by running a program called exactly
//! `daifuku.exe` with `hook`, so an uninstall removes exactly those, and an
//! install that finds them calling another copy rewrites only their path.

use serde_json::{Map, Value, json};

/// One agent's hook dialect.
pub struct Agent {
    /// For messages: `Claude Code`, `Codex`.
    pub name: &'static str,
    /// The events that move an agent between states. Anything else would only
    /// cost a process start per event for nothing.
    pub events: &'static [&'static str],
    style: Style,
}

enum Style {
    /// `"command": "<exe>", "args": ["hook"]`.
    ProgramAndArgs,
    /// `"command": "& \"<exe>\" hook"`: one command line, which Codex runs
    /// in PowerShell on Windows. PowerShell reads a quoted path as a string,
    /// not a program, so it needs the call operator `&` in front.
    CommandLine,
}

/// The command line Codex runs for Daifuku's hook.
fn command_line(exe: &str) -> String {
    format!("& \"{exe}\" hook")
}

/// Claude Code, `~/.claude/settings.json`.
pub const CLAUDE: Agent = Agent {
    name: "Claude Code",
    events: &[
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
    ],
    style: Style::ProgramAndArgs,
};

/// Codex, `~/.codex/hooks.json`.
pub const CODEX: Agent = Agent {
    name: "Codex",
    events: &[
        "SessionStart",
        "UserPromptSubmit",
        "PreToolUse",
        "PostToolUse",
        "PermissionRequest",
        "SubagentStart",
        "PreCompact",
        "Stop",
        "Interrupt",
        "SessionEnd",
    ],
    style: Style::CommandLine,
};

impl Agent {
    fn entry(&self, exe: &str) -> Value {
        match self.style {
            Style::ProgramAndArgs => json!({
                "type": "command",
                "command": exe,
                "args": ["hook"],
                "async": true,
                "timeout": 10
            }),
            Style::CommandLine => json!({
                "type": "command",
                "command": command_line(exe),
                "async": true,
                "timeout": 10
            }),
        }
    }

    /// Adds a Daifuku hook for every event in [`Agent::events`], calling
    /// `exe`. Returns how many events gained one; zero means the file already
    /// had them all.
    ///
    /// The hook runs `async`, so the agent never waits for it, with a short
    /// timeout for the case where something does go wrong.
    ///
    /// # Errors
    ///
    /// When `settings` is not a JSON object, or its `hooks` is not one.
    pub fn add_hooks(&self, settings: &mut Value, exe: &str) -> Result<usize, String> {
        let root = settings
            .as_object_mut()
            .ok_or("the settings file is not a JSON object")?;
        let hooks = root
            .entry("hooks")
            .or_insert_with(|| Value::Object(Map::new()));
        let hooks = hooks
            .as_object_mut()
            .ok_or("`hooks` in the settings file is not an object")?;
        let mut added = 0;
        for &event in self.events {
            let groups = hooks
                .entry(event)
                .or_insert_with(|| Value::Array(Vec::new()));
            let Some(groups) = groups.as_array_mut() else {
                return Err(format!(
                    "`hooks.{event}` in the settings file is not a list"
                ));
            };
            let present = groups.iter().any(|g| {
                g.get("hooks")
                    .and_then(Value::as_array)
                    .is_some_and(|hs| hs.iter().any(is_ours))
            });
            if present {
                continue;
            }
            groups.push(json!({ "hooks": [self.entry(exe)] }));
            added += 1;
        }
        Ok(added)
    }
}

/// Whether `hook` runs `daifuku.exe hook`, as a program with its arguments
/// or as one command line with the program quoted.
fn is_ours(hook: &Value) -> bool {
    style_of_ours(hook).is_some()
}

/// How `hook` runs `daifuku.exe hook`, or `None` when it runs something
/// else.
fn style_of_ours(hook: &Value) -> Option<Style> {
    let command = hook.get("command").and_then(Value::as_str).unwrap_or("");
    let first_arg = hook
        .get("args")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(Value::as_str);
    if first_arg == Some("hook") && is_daifuku(command) {
        return Some(Style::ProgramAndArgs);
    }
    // Installs before the call operator wrote the line without it.
    let line = command.strip_prefix("& ").unwrap_or(command);
    line.strip_suffix("\" hook")
        .and_then(|c| c.strip_prefix('"'))
        .is_some_and(|program| !program.contains('"') && is_daifuku(program))
        .then_some(Style::CommandLine)
}

/// Whether `path` names a file called `daifuku.exe`, in any folder and in
/// any case. A program that only ends in the same letters is someone else's.
fn is_daifuku(path: &str) -> bool {
    path.rsplit(['\\', '/'])
        .next()
        .is_some_and(|name| name.eq_ignore_ascii_case("daifuku.exe"))
}

/// Points every Daifuku hook at `exe`, the `daifuku.exe` being installed, so
/// hooks an earlier install left calling another copy work again. Each
/// keeps its form and everything else in it, and hooks that are not
/// Daifuku's are not touched. Returns how many were rewritten. The same for
/// both agents.
pub fn update_hooks(settings: &mut Value, exe: &str) -> usize {
    let Some(hooks) = settings.get_mut("hooks").and_then(Value::as_object_mut) else {
        return 0;
    };
    let mut updated = 0;
    let groups = hooks.values_mut().filter_map(Value::as_array_mut).flatten();
    let all = groups.filter_map(|g| g.get_mut("hooks").and_then(Value::as_array_mut));
    for hook in all.flatten() {
        let wanted = match style_of_ours(hook) {
            Some(Style::ProgramAndArgs) => exe.to_owned(),
            Some(Style::CommandLine) => command_line(exe),
            None => continue,
        };
        let current = hook.get("command").and_then(Value::as_str).unwrap_or("");
        if !current.eq_ignore_ascii_case(&wanted) {
            hook["command"] = Value::String(wanted);
            updated += 1;
        }
    }
    updated
}

/// Removes every Daifuku hook, and any group or event list that held only
/// Daifuku's. Returns how many hooks were removed. The same for both agents.
///
/// A group or list that was empty before stays: only removing Daifuku's
/// hooks makes one go.
pub fn remove_hooks(settings: &mut Value) -> usize {
    let Some(hooks) = settings.get_mut("hooks").and_then(Value::as_object_mut) else {
        return 0;
    };
    let mut removed = 0;
    hooks.retain(|_, groups| {
        let Some(groups) = groups.as_array_mut() else {
            return true;
        };
        let had_groups = !groups.is_empty();
        groups.retain_mut(|group| {
            let Some(hs) = group.get_mut("hooks").and_then(Value::as_array_mut) else {
                return true;
            };
            let before = hs.len();
            hs.retain(|h| !is_ours(h));
            removed += before - hs.len();
            before == 0 || !hs.is_empty()
        });
        !had_groups || !groups.is_empty()
    });
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_daifuku_running_hook_counts_as_ours() {
        let mut s = json!({"hooks": {"Stop": [{"hooks": [
            {"type": "command", "command": "C:/x/daifuku.exe", "args": ["status"]},
            {"type": "command", "command": "C:/x/other.exe", "args": ["hook"]}
        ]}]}});
        assert_eq!(remove_hooks(&mut s), 0, "neither is a Daifuku hook");
    }

    #[test]
    fn a_program_whose_name_only_ends_in_daifuku_is_not_ours() {
        let mut s = json!({"hooks": {"Stop": [{"hooks": [
            {"type": "command", "command": r"C:\tools\notdaifuku.exe", "args": ["hook"]},
            {"type": "command", "command": r#""C:\tools\notdaifuku.exe" hook"#},
            {"type": "command", "command": r#""C:\tools\x.exe" "C:\daifuku.exe" hook"#}
        ]}]}});
        let before = s.clone();
        assert_eq!(remove_hooks(&mut s), 0, "none of them is a Daifuku hook");
        assert_eq!(s, before);
    }

    #[test]
    fn daifuku_is_ours_whatever_the_case_and_separator() {
        let mut s = json!({"hooks": {"Stop": [{"hooks": [
            {"type": "command", "command": r"C:\Program Files\Daifuku\Daifuku.EXE", "args": ["hook"]},
            {"type": "command", "command": "C:/x/daifuku.exe", "args": ["hook"]},
            {"type": "command", "command": "daifuku.exe", "args": ["hook"]},
            {"type": "command", "command": r#""C:\x\daifuku.exe" hook"#}
        ]}]}});
        assert_eq!(remove_hooks(&mut s), 4);
    }

    const EXE: &str = r"C:\Program Files\Daifuku\daifuku.exe";

    #[test]
    fn a_codex_hook_from_before_the_call_operator_is_ours_and_gets_it() {
        let old = format!("\"{EXE}\" hook");
        let mut s = json!({"hooks": {"Stop": [{"hooks": [
            {"type": "command", "command": old, "async": true}
        ]}]}});
        assert_eq!(update_hooks(&mut s, EXE), 1);
        assert_eq!(
            s["hooks"]["Stop"][0]["hooks"][0]["command"],
            format!("& \"{EXE}\" hook")
        );
        assert_eq!(update_hooks(&mut s, EXE), 0, "already current");
        assert_eq!(remove_hooks(&mut s), 1);
    }

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
        assert_eq!(CLAUDE.add_hooks(&mut s, EXE), Ok(CLAUDE.events.len()));
        let pre = s["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(pre.len(), 2, "the user's group stays, ours is appended");
        assert_eq!(pre[0]["hooks"][0]["command"], "C:/check.exe");
        assert_eq!(pre[1]["hooks"][0]["command"], EXE);
        assert_eq!(pre[1]["hooks"][0]["async"], true);
        for &e in CLAUDE.events {
            assert!(s["hooks"][e].is_array(), "{e}");
        }
    }

    #[test]
    fn installing_twice_changes_nothing() {
        let mut s = users_file();
        CLAUDE.add_hooks(&mut s, EXE).unwrap();
        let once = s.clone();
        assert_eq!(CLAUDE.add_hooks(&mut s, EXE), Ok(0));
        assert_eq!(s, once);
    }

    #[test]
    fn an_update_points_daifukus_hooks_at_the_current_exe() {
        let mut s = users_file();
        CLAUDE.add_hooks(&mut s, r"C:\old\daifuku.exe").unwrap();
        s["hooks"]["Stop"][0]["hooks"][0]["timeout"] = json!(30);
        assert_eq!(update_hooks(&mut s, EXE), CLAUDE.events.len());
        assert_eq!(CLAUDE.add_hooks(&mut s, EXE), Ok(0));
        let mut fresh = users_file();
        CLAUDE.add_hooks(&mut fresh, EXE).unwrap();
        fresh["hooks"]["Stop"][0]["hooks"][0]["timeout"] = json!(30);
        assert_eq!(s, fresh, "only the path changed, the user's own hook too");
        assert_eq!(update_hooks(&mut s, EXE), 0, "nothing left to update");
        assert_eq!(update_hooks(&mut s, &EXE.to_uppercase()), 0, "same file");
    }

    #[test]
    fn an_update_keeps_the_command_line_form() {
        let mut s = json!({});
        CODEX.add_hooks(&mut s, r"C:\old\daifuku.exe").unwrap();
        assert_eq!(update_hooks(&mut s, EXE), CODEX.events.len());
        let mut fresh = json!({});
        CODEX.add_hooks(&mut fresh, EXE).unwrap();
        assert_eq!(s, fresh);
    }

    #[test]
    fn key_order_survives() {
        let mut s = users_file();
        CLAUDE.add_hooks(&mut s, EXE).unwrap();
        let keys: Vec<_> = s.as_object().unwrap().keys().cloned().collect();
        assert_eq!(keys, ["theme", "hooks", "effortLevel"]);
    }

    #[test]
    fn uninstall_removes_exactly_ours() {
        let mut s = users_file();
        CLAUDE.add_hooks(&mut s, EXE).unwrap();
        assert_eq!(remove_hooks(&mut s), CLAUDE.events.len());
        assert_eq!(s, users_file(), "back to the file as the user had it");
    }

    #[test]
    fn uninstall_keeps_the_users_empty_groups_and_lists() {
        // `SubagentStop` is not one of Daifuku's events.
        let users = json!({"hooks": {
            "PreToolUse": [{"matcher": "Bash", "hooks": []}],
            "SubagentStop": [],
            "Notification": [{"matcher": "x"}]
        }});
        let mut s = users.clone();
        CLAUDE.add_hooks(&mut s, EXE).unwrap();
        assert_eq!(remove_hooks(&mut s), CLAUDE.events.len());
        assert_eq!(s, users, "only what Daifuku emptied goes");
    }

    #[test]
    fn uninstall_keeps_the_users_hook_in_a_group_it_shares_with_ours() {
        let users = json!({"type": "command", "command": "C:/x/other.exe", "args": ["hook"]});
        let mut s = json!({"hooks": {"Stop": [{"matcher": "", "hooks": [
            {"type": "command", "command": "C:/x/daifuku.exe", "args": ["hook"]},
            users.clone()
        ]}]}});
        assert_eq!(remove_hooks(&mut s), 1);
        assert_eq!(
            s,
            json!({"hooks": {"Stop": [{"matcher": "", "hooks": [users]}]}})
        );
    }

    #[test]
    fn an_empty_file_gets_a_hooks_section() {
        let mut s = json!({});
        CLAUDE.add_hooks(&mut s, EXE).unwrap();
        assert_eq!(s["hooks"].as_object().unwrap().len(), CLAUDE.events.len());
        remove_hooks(&mut s);
        assert_eq!(s, json!({"hooks": {}}));
    }

    #[test]
    fn a_malformed_file_is_refused_not_overwritten() {
        assert!(CLAUDE.add_hooks(&mut json!([]), EXE).is_err());
        assert!(CLAUDE.add_hooks(&mut json!({"hooks": []}), EXE).is_err());
        assert!(
            CLAUDE
                .add_hooks(&mut json!({"hooks": {"Stop": {}}}), EXE)
                .is_err()
        );
    }

    #[test]
    fn codex_gets_one_command_line_per_event_and_uninstalls_cleanly() {
        let mut s = json!({});
        assert_eq!(CODEX.add_hooks(&mut s, EXE), Ok(CODEX.events.len()));
        let stop = &s["hooks"]["Stop"][0]["hooks"][0];
        assert_eq!(stop["command"], format!("& \"{EXE}\" hook"));
        assert!(stop.get("args").is_none());
        assert_eq!(CODEX.add_hooks(&mut s, EXE), Ok(0), "idempotent");
        assert_eq!(remove_hooks(&mut s), CODEX.events.len());
    }

    #[test]
    fn every_listed_event_changes_a_state() {
        use crate::state::{HookEvent, Transition};
        for &e in CLAUDE.events.iter().chain(CODEX.events) {
            let t = HookEvent {
                session_id: "s".into(),
                hook_event_name: e.into(),
                notification_type: Some("permission_prompt".into()),
                ..HookEvent::default()
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

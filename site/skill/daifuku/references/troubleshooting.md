# Troubleshooting

Start every diagnosis with `daifuku doctor`. When the daemon runs elevated (`"elevated": true` in the config), run it from an administrator terminal when you can: from an ordinary one it skips the checks that need the daemon's answer and says so.

## Reading `daifuku doctor`

Each line starts with a marker:

- `ok    <check>`: passed.
- `FIX   <check>: <what to do>`: failed; the text after the colon is the fix.
- `--    <note>`: information, not a failure.
- `logs  <folder>`: where the daemon writes its logs.

Exit code 0 when every check passed, 1 otherwise. There is no JSON output.

| Check | Fix it prints when it fails |
|---|---|
| `installed in Program Files` | run `daifuku install` from an administrator terminal |
| `Program Files folder on the machine PATH` | run `daifuku install` again |
| `logon task registered` (only with an elevated config) | run `daifuku install` |
| `Windows Terminal found` | `winget install Microsoft.WindowsTerminal` |
| `config valid` | the parse error, or the file that cannot be read |
| `administrator fleets have an elevated daemon` | set `"elevated": true` and run `daifuku install` again, or set `"admin": false` on the named fleets |
| `Claude Code hooks present and current` | run `daifuku install`; or repair the named file first; or make a linked file a real file in the profile; or copy the hooks into the file `CLAUDE_CONFIG_DIR` points to |
| `Codex hooks present and current` | the same, with `CODEX_HOME`; skipped when Codex's folder does not exist |
| `daemon running` | run `daifuku open`, which starts it; an elevated daemon instead: sign out and in, or `schtasks /Run /TN \Daifuku\Daemon-<SID>` |
| `daemon rights as the config asks` | run `daifuku install` again, which registers the logon task with the rights `elevated` asks for, and start the daemon through that task, not by hand |
| `daemon up to date` | run `daifuku install` from the newer download; it restarts the daemon |
| `hotkeys registered` | another program holds the named keys; pick others in the config |
| `daemon answers` | a restart command, see "The daemon does not answer" |

The fixes name the user's real task name. `whoami /user` shows the SID.

## A terminal shows no border

1. `daifuku doctor`. Fix every `FIX` line first, starting from the top.
2. The terminal must run in Windows Terminal, and the agent must be one Daifuku hooks into (Claude Code or Codex).
3. A Claude Code session started before install: restart it.
4. Codex: the user must trust Daifuku's hooks in Codex's `/hooks` menu.
5. A maximised terminal never gets a border.
6. `"border": { "enabled": false }` in the config turns every border off.
7. `daifuku status` (an administrator terminal for an elevated daemon): when the agent is missing under `agents`, no hook has reported from that window.

## Daemon commands fail

- `the daemon runs elevated and answers only an administrator terminal`: open an administrator terminal. The config sets `"elevated": true`, and an elevated daemon's control pipe accepts only elevated callers.
- `fleet ... opens administrator terminals, and this daemon runs with your own rights`: the fleet has `"admin": true` but the config does not set `"elevated": true`. Ask the user which they want: the key and `daifuku install` again from an administrator terminal, or `"admin": false` on the fleet.
- `Daifuku is not running`: see below.

## The daemon is not running

An ordinary daemon is started on demand: run `daifuku open`, or let the next hook event start it. There is no logon task to run and no need to start `daifukud.exe` by hand.

An elevated daemon (the config sets `"elevated": true`) starts through its logon task. Sign out and back in, or run the task:

```powershell
schtasks /Run /TN "\Daifuku\Daemon-<SID>"
```

Start an elevated daemon through its logon task, not by running `daifukud.exe` yourself. The task starts it with the rights the config's `elevated` asked for at the last install; `daifuku doctor` reports a daemon whose rights differ.

## The daemon does not answer

Doctor waits about 20 seconds for a daemon busy with another command, such as opening a fleet. When it stays busy, doctor prints a restart that ends only this session's daemon:

```powershell
taskkill /F /FI "SESSION eq <session>" /IM daifukud.exe
schtasks /Run /TN "\Daifuku\Daemon-<SID>"
```

When it answers nothing at all: `daifuku stop`, then the `schtasks` line.

## A config change does not apply

1. `daifuku doctor`: a `FIX   config valid:` line names the error. The daemon keeps the last valid config until the file is fixed.
2. Unknown or misspelt keys are errors. Compare with `daifuku schema` or [configuration.md](configuration.md).
3. A second fleet without its own `hotkey` collides with the first; give it a key or `null`.
4. A hotkey another program holds is listed as refused by `daifuku status`.

## A border looks wrong

- Yellow after the user approved a prompt: by design, until the approved tool finishes.
- An interrupted Claude Code turn keeps its last colour for about a minute before it turns green: by design.
- Several agents in tabs of one window: the window shows the most urgent of their states.

## Files and logs

| Path | Contents |
|---|---|
| `C:\Program Files\Daifuku\` | `daifuku.exe`, `daifukud.exe` |
| `C:\ProgramData\Daifuku\daifuku.json` | the config |
| `C:\ProgramData\Daifuku\state.json` | open fleets and agent states, saved by an elevated daemon; a daemon restarted within five minutes, in the same Windows start, picks them up |
| `C:\ProgramData\Daifuku\logs\` | an elevated daemon's log, one file a day (`daifukud.<date>.log`), the last seven kept, at level `info` |
| `%LocalAppData%\Daifuku\` | the saved state and `logs\` of a daemon with the user's own rights, the default |

The pipes are `\\.\pipe\daifuku-<session>-hook` and `\\.\pipe\daifuku-<session>-control`, one pair per signed-in session.

## Undo

- Remove everything: Settings > Apps > Installed apps > Daifuku > Uninstall, or `daifuku uninstall`; `daifuku uninstall --purge` also deletes config, logs and saved state. See [install-and-hooks.md](install-and-hooks.md).
- Keep Daifuku but take its hooks out of the agents: `daifuku uninstall` removes them; install again with `.\daifuku.exe install --no-hooks` in the download folder, from an administrator terminal.
- Back to the default config: show the user, then replace the file with `{}`.

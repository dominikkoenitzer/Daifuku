# Troubleshooting

Start every diagnosis with `daifuku doctor`, from an administrator terminal when you can. From an ordinary terminal it skips the daemon checks that need elevation and says so.

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
| `logon task registered` | run `daifuku install` |
| `Windows Terminal found` | `winget install Microsoft.WindowsTerminal` |
| `config valid` | the parse error, or the file that cannot be read |
| `Claude Code hooks present and current` | run `daifuku install`; or repair the named file first; or make a linked file a real file in the profile; or copy the hooks into the file `CLAUDE_CONFIG_DIR` points to |
| `Codex hooks present and current` | the same, with `CODEX_HOME`; skipped when Codex's folder does not exist |
| `daemon running` | sign out and in, or `schtasks /Run /TN \Daifuku\Daemon-<SID>` |
| `daemon elevated` | start it through the logon task, not by hand |
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
7. `daifuku status` (administrator terminal): when the agent is missing under `agents`, no hook has reported from that window.

## Daemon commands fail

- `daemon commands need an administrator terminal`: open an administrator terminal. The daemon runs elevated, and its control pipe accepts only elevated callers.
- `Daifuku is not running`: see below.

## The daemon is not running

Sign out and back in, or start the logon task:

```powershell
schtasks /Run /TN "\Daifuku\Daemon-<SID>"
```

Start the daemon through the logon task, not by running `daifukud.exe` yourself. The task starts it elevated; `daifuku doctor` reports a daemon that is not.

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
| `C:\ProgramData\Daifuku\state.json` | open fleets and agent states, saved by the daemon; a daemon restarted within five minutes, in the same Windows start, picks them up |
| `C:\ProgramData\Daifuku\logs\` | the daemon's log, one file a day (`daifukud.<date>.log`), the last seven kept, at level `info` |

The pipes are `\\.\pipe\daifuku-<session>-hook` and `\\.\pipe\daifuku-<session>-control`, one pair per signed-in session.

## Undo

- Remove everything: Settings > Apps > Installed apps > Daifuku > Uninstall, or `daifuku uninstall`; `daifuku uninstall --purge` also deletes config, logs and saved state. See [install-and-hooks.md](install-and-hooks.md).
- Keep Daifuku but take its hooks out of the agents: `daifuku uninstall` removes them; install again with `.\daifuku.exe install --no-hooks` in the download folder, from an administrator terminal.
- Back to the default config: show the user, then replace the file with `{}`.

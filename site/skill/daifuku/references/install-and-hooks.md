# Install, hooks and uninstall

## Requirements

- Windows 10 or 11 with Windows Terminal. Without it, install says so and names the fix: `winget install Microsoft.WindowsTerminal`.
- A signed-in account that is an administrator itself. Install refuses a terminal that runs as another account than the one signed in, as happens when a standard user's prompt takes an administrator's password. Windows 11's Administrator protection is not supported.

## Install

1. Download `daifuku.exe` (https://get-daifuku.vercel.app/download/windows), or `daifuku-arm64.exe` (https://get-daifuku.vercel.app/download/windows-arm64) on an ARM PC (Settings > System > About, System type). It carries the daemon, `daifukud.exe`, inside it, so it is the only file needed.
2. Double-click it, press Enter, approve the Windows prompt. A second window shows what install did; Enter closes it.

From an administrator terminal in the download folder, `.\daifuku.exe install` does the same. The release also has zips, `daifuku-windows-x64.zip` and `daifuku-windows-arm64.zip`, with `daifuku.exe` and `daifukud.exe` as two files; extract one before running `daifuku.exe` from it, which otherwise says to extract it first. The exes are self-contained; no runtime to install.

Install, in order:

1. Stops a running daemon.
2. Copies itself to `C:\Program Files\Daifuku` as `daifuku.exe`, whatever the download was called, puts `daifukud.exe` next to it (the one it carries, or the one next to it from a zip), and adds that folder to the machine `PATH`.
3. Lists Daifuku in Settings > Apps > Installed apps: the key `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Daifuku`, whose Uninstall button runs the installed `daifuku.exe`.
4. Locks `C:\ProgramData\Daifuku` to administrators. A folder there that is not locked that way is deleted, with any config in it, and made anew.
5. Writes `C:\ProgramData\Daifuku\daifuku.json` with the defaults spelled out, unless a config is already there (`kept config`).
6. Registers the logon task `\Daifuku\Daemon-<user SID>`, which starts `daifukud.exe` elevated at this user's sign-in, at normal priority, with no time limit. It removes the task `\Daifuku\Daemon` of earlier versions when that one runs for this user.
7. Starts the daemon, unless `--no-start`.
8. Adds the hooks, unless `--no-hooks` (below).
9. Prints how to open the first fleet, for example `To open your first fleet, press ctrl + alt + return.`

To update, run the new `daifuku.exe` the same way. Config and hooks are kept; hooks that call another copy of `daifuku.exe` get the new path.

## Hooks

| Agent | File install writes | Events |
|---|---|---|
| Claude Code | `%USERPROFILE%\.claude\settings.json` | `SessionStart`, `UserPromptSubmit`, `PreToolUse`, `PostToolUse`, `PostToolUseFailure`, `PermissionRequest`, `PermissionDenied`, `Notification`, `Elicitation`, `ElicitationResult`, `SubagentStart`, `SubagentStop`, `PreCompact`, `PostCompact`, `Stop`, `StopFailure`, `SessionEnd` |
| Codex | `%USERPROFILE%\.codex\hooks.json`, only when the `.codex` folder exists | `SessionStart`, `UserPromptSubmit`, `PreToolUse`, `PostToolUse`, `PermissionRequest`, `SubagentStart`, `PreCompact`, `PostCompact`, `Stop`, `Interrupt`, `SessionEnd` |

For each event, install appends one group to `hooks.<Event>` and leaves every other hook, key and order as it was. A file that does not exist is created. Running install twice changes nothing the second time.

Claude Code entry:

```json
{ "hooks": [ { "type": "command", "command": "C:\\Program Files\\Daifuku\\daifuku.exe", "args": ["hook"], "async": true, "timeout": 10 } ] }
```

Codex entry:

```json
{ "hooks": [ { "type": "command", "command": "& \"C:\\Program Files\\Daifuku\\daifuku.exe\" hook", "async": true, "timeout": 10 } ] }
```

Codex limits `SessionEnd` and `Interrupt` hooks, so there the timeout is `3`, and the `SessionEnd` entry has no `async`.

A hook is Daifuku's when it runs a program named exactly `daifuku.exe` with `hook`. Uninstall removes exactly those.

The hook runs in the background, so the agent never waits for it. It finds the Windows Terminal window it runs in and sends the state to the daemon over a pipe, waiting at most 100 ms. It never prints and always exits 0, so a stopped daemon never disturbs an agent.

Install leaves a settings file alone, and says why, when:

- the file or its folder is a link that leads out of the user's profile;
- the file is not valid JSON, or `hooks` in it is not an object;
- no desktop shell is running to borrow the user's own rights from, as on a build server.

`CLAUDE_CONFIG_DIR` and `CODEX_HOME` move an agent's settings. Install still writes only the files in the profile, and prints which file the agent reads instead. Copy the Daifuku entries into that file by hand, after showing the user the change, and remove them there by hand after an uninstall. `daifuku doctor` checks the file the variable points to.

After install, Claude Code sessions already open normally pick the hooks up; restart one that still shows no border. Codex runs new hooks only after the user trusts them in its `/hooks` menu.

## Uninstall

Settings > Apps > Installed apps > Daifuku > Uninstall does the same as the command below: Windows asks for permission, and the window lists each step. From an administrator terminal:

```powershell
daifuku uninstall
```

It stops the daemon, removes the logon task, removes Daifuku's hooks from `%USERPROFILE%\.claude\settings.json` and `%USERPROFILE%\.codex\hooks.json`, removes `C:\Program Files\Daifuku` from the machine `PATH`, deletes that folder and removes the Settings > Apps entry. It keeps `C:\ProgramData\Daifuku` (config, logs, saved state). `daifuku uninstall --purge` deletes that folder too.

A file it cannot remove is listed with `left`. Hooks in a file that `CLAUDE_CONFIG_DIR` or `CODEX_HOME` points to stay; remove those by hand.

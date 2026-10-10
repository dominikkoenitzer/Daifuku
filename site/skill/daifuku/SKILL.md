---
name: daifuku
description: Daifuku is a Windows tool that opens agent terminals in a grid in Windows Terminal and colours each window's border by what its coding agent is doing (working, waiting, done, failed). Use this skill when the user asks to install, update, uninstall, configure or troubleshoot Daifuku, or asks why a terminal border has a certain colour or is missing.
---

# Daifuku

Daifuku runs on Windows 10 and 11 with Windows Terminal. A daemon (`daifukud.exe`) draws a border around every terminal an agent reports from. The agents report through hooks that call `daifuku.exe hook`. It works with Claude Code and Codex.

## What the borders mean

| State | Default colour | Width at the default of 4 px | When |
|---|---|---|---|
| done | green | 2 px | the turn is over, or the session has just started |
| working | blue | 4 px | the agent is thinking or running tools |
| failed | red | 6 px | the turn ended on an error, such as a rate limit |
| waiting | yellow | 8 px, breathing slowly | a permission prompt or a question is open |

A terminal with no border has no agent reporting from it, is maximised, or the borders are turned off. Several agents in tabs of one window show the most urgent state: waiting, then failed, working, done.

Two cases look wrong but are by design:

- After the user approves a permission prompt, the border stays yellow until the approved tool has run. No hook reports the answer.
- A Claude Code turn interrupted with Esc, or by saying no at a permission prompt, turns green only when Claude Code reports its prompt idle, about a minute later.

Default hotkeys, all changeable in the config:

| Key | Action |
|---|---|
| Ctrl+Alt+Enter | open the first fleet (six terminals running `claude`), or bring it back |
| Ctrl+Alt+N | focus the agent that has waited longest, then the failed ones; press again for the next |
| Ctrl+Alt+Backspace | put every fleet terminal back in its cell |
| Ctrl+Alt+F4 | close every fleet terminal without asking; a working agent ends with its terminal |

## Rules for the assistant

1. Run `daifuku doctor` before you change anything. It reads the setup and prints a fix for each problem. See [references/troubleshooting.md](references/troubleshooting.md).
2. Show the user every change to an agent settings file (`%USERPROFILE%\.claude\settings.json`, `%USERPROFILE%\.codex\hooks.json`) and to `C:\ProgramData\Daifuku\daifuku.json` before you write it, and wait for a yes.
3. Never change the owner or the access list of `C:\ProgramData\Daifuku`. The next install deletes a folder that is not locked to administrators, config included.
4. Install, uninstall and the daemon commands (`open`, `snap`, `close`, `next`, `demo`, `reload`, `stop`) need an administrator terminal. If your shell is not elevated, give the user the command to run instead of retrying. `daifuku status` runs in any terminal: without elevation it reports only whether Daifuku is installed, whether the daemon process runs, the logon task and the config, and says that fleets, agents and hotkeys need an administrator terminal.
5. Do not touch Daifuku's hook entries by hand when `daifuku install` or `daifuku uninstall` can do it. Edit them by hand only in a file that `CLAUDE_CONFIG_DIR` or `CODEX_HOME` points to, which install does not write.
6. After a config change, confirm it with `daifuku doctor` (line `config valid`) or `daifuku status`.

## Install, update, uninstall

1. Download `daifuku.exe` from https://get-daifuku.vercel.app/download/windows, or `daifuku-arm64.exe` from https://get-daifuku.vercel.app/download/windows-arm64 when Settings > System > About says "ARM-based processor". It is the only file needed.
2. Double-click it, press Enter, and approve the Windows prompt. SmartScreen may stop the unsigned exe the first time: "More info", then "Run anyway".

From an administrator terminal in the download folder, `.\daifuku.exe install` does the same. To update, run the new `daifuku.exe` the same way; config and hooks are kept. To remove it, open Settings > Apps > Installed apps, find Daifuku and choose Uninstall, or run `daifuku uninstall` from an administrator terminal; `daifuku uninstall --purge` also deletes the config, logs and saved state.

What install changes, the hook entries it writes and how to undo each part: [references/install-and-hooks.md](references/install-and-hooks.md).

## Configure

The config is `C:\ProgramData\Daifuku\daifuku.json`, JSON, every key optional. `daifuku config` opens it in the user's editor for `.json` files (Notepad when there is none), after a Windows prompt when the terminal is not elevated; `daifuku config --path` prints the path. The daemon applies each save within two seconds; `daifuku reload` applies it at once. An invalid file never stops Daifuku: the last valid config stays in use.

Every key with its default: [references/configuration.md](references/configuration.md).

## Commands

For scripts, `daifuku status --json` prints the agent states as JSON from an administrator terminal; from any other it prints the local status (`"result": "local_status"`, exit code 0), `daifuku config --path` prints the config path and `daifuku schema` prints the config schema. Other commands print text for people. Every command and its output: [references/commands.md](references/commands.md).

## Answering "why is this border this colour"

1. Ask which window, or run `daifuku status` from an administrator terminal. It lists every agent, most urgent first, with its state, how long it has been in it, and its window title.
2. Match the state to the table above and to the two by-design cases.
3. No border at all: follow "A terminal shows no border" in [references/troubleshooting.md](references/troubleshooting.md).

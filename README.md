# Daifuku

Fleets of AI agent terminals for Windows. One key opens them in a grid, and
each window's border shows what its agent is doing.

![Six agents in a grid, their borders changing colour as they work, wait, fail and finish](docs/demo.gif)

- **Ctrl+Alt+Enter** opens six terminals running Claude Code, in a grid on the
  monitor you choose.
- Each border shows its agent: **blue** working, **yellow** waiting for you,
  **green** done, **red** failed.
- **Ctrl+Alt+N** jumps to the agent that has waited longest. Press it again for
  the next one.
- **Ctrl+Alt+Backspace** puts every terminal back in its cell.

It works with Claude Code and Codex, in Windows Terminal. Fleets open as
administrator by default, which also keeps them out of a tiling window
manager's reach, so the two run side by side.

## Install

Windows 10 or 11 with Windows Terminal. From an administrator terminal, in the
folder you unzipped the release to:

```powershell
.\daifuku.exe install
```

This copies Daifuku to `Program Files`, starts it at every logon, and adds its
hooks to Claude Code's settings (and Codex's, if Codex is installed). Your own
hooks stay exactly as they are.

```powershell
daifuku doctor
```

checks the whole setup and says how to fix anything that is wrong.

To see it work without a real agent, `daifuku demo` opens six scripted ones.

## Configure

`C:\ProgramData\Daifuku\daifuku.json`, then `daifuku reload`. Every key is
optional; an editor that reads the linked schema completes and checks them.

```json
{
  "fleets": [
    { "name": "agents", "count": 6, "monitor": "portrait" },
    { "name": "site", "count": 4, "directory": "C:\\src\\site", "hotkey": "ctrl + alt + w" }
  ]
}
```

| Fleet key | Default | |
|---|---|---|
| `count` | `6` | 1 to 16 terminals |
| `monitor` | `"portrait"` | `portrait`, `landscape`, `primary`, `secondary`, `cursor`, or a device name |
| `command` | `"claude"` | what each terminal runs; `{n}` is its number; `null` for a plain shell |
| `directory` | your profile | where the terminals start |
| `admin` | `true` | open as administrator |
| `hotkey` | `"ctrl + alt + return"` | the key that opens this fleet |
| `shape` | automatic | force a grid, `{ "columns": 3, "rows": 2 }` |

Gaps, border width and the four colours are in `gaps` and `border`; the
global keys are in `hotkeys`.

## How agents report

The agent calls `daifuku hook` on every change of state, in the background, so
it never waits for it. The hook finds the terminal window it runs in and tells
the daemon, which colours that window's border. Several agents in tabs of one
window show the most urgent of their states.

## Security

The daemon runs as administrator, so nothing an ordinary program can change
decides what it starts:

- the binaries live in `Program Files`;
- the config lives in a folder only administrators can write, owned by the
  Administrators group;
- commands arrive on a pipe only elevated processes can open;
- Windows Terminal is found through its package and PowerShell through the
  system folders, never through `PATH`.

Hooks use a separate pipe, open to the signed-in user, that can do nothing but
colour a border.

## Commands

| | |
|---|---|
| `daifuku open [fleet]` | open a fleet, or bring it back |
| `daifuku snap` | put every terminal back in its cell |
| `daifuku next` | focus the agent that has waited longest |
| `daifuku close [fleet]` | close a fleet |
| `daifuku status` | fleets, agents and hotkeys |
| `daifuku demo` | six scripted agents |
| `daifuku reload` | re-read the config |
| `daifuku doctor` | check the setup |
| `daifuku uninstall [--purge]` | remove it again |

## Build

```powershell
cargo build --release
```

Rust stable, Windows only. `cargo test` runs the suite.

## Licence

GPL-3.0-only.

# Daifuku

Fleets of AI agent terminals for Windows. One key opens them in a grid, and
each window's border shows what its agent is doing.

![Six agents in a grid, their borders changing colour as they work, wait, fail and finish](docs/demo.gif)

- **Ctrl+Alt+Enter** opens six terminals running Claude Code, in a grid on the
  monitor you choose.
- Each border shows its agent: **blue** working, **yellow** waiting for you at
  a permission prompt or a question, **green** done (its turn is over, or it
  has just started), **red** its turn ended on an error, such as a rate limit.
- **Ctrl+Alt+N** visits the waiting agents first, longest waiting first, then
  the failed ones. Press it again for the next one.
- **Ctrl+Alt+Backspace** puts every terminal back in its cell.
- **Ctrl+Alt+F4** closes every fleet at once, without asking: an agent still
  working ends with its terminal.

It works with Claude Code and Codex, in Windows Terminal. Fleets open as
administrator by default, which also keeps them out of a tiling window
manager's reach, so the two run side by side. The agent in such a terminal,
and every command it runs, runs as administrator; `"admin": false` on a fleet
opens its terminals as you instead.

## Install

1. Download [daifuku-windows-x64.zip](https://github.com/dominikkoenitzer/Daifuku/releases/latest/download/daifuku-windows-x64.zip),
   or [daifuku-windows-arm64.zip](https://github.com/dominikkoenitzer/Daifuku/releases/latest/download/daifuku-windows-arm64.zip)
   on a PC with an ARM processor.
2. Most PCs need x64. If Settings > System > About shows "ARM-based
   processor" under System type, you need arm64.
3. Right-click the zip, choose Extract All, and open the folder. Double-click
   `daifuku.exe`, press Enter, and say yes when Windows asks for permission.

Daifuku is not signed, so SmartScreen may stop it the first time: choose
"More info", then "Run anyway".

A second window shows what install did and what to do next; press Enter to
close it. From an administrator terminal, `.\daifuku.exe install` in the
extracted folder does the same.

You need Windows 10 or 11 with Windows Terminal, signed in with an account
that is an administrator itself; Windows 11's Administrator protection is not
supported.

Install copies Daifuku to `C:\Program Files\Daifuku`, adds that folder to the
machine `PATH`, starts it at every logon, and adds its hooks to
`%USERPROFILE%\.claude\settings.json` (and to Codex's
`%USERPROFILE%\.codex\hooks.json`, if Codex is installed). Your own hooks stay
exactly as they are. Claude Code sessions that are already open normally pick
the hooks up by themselves; restart one that still shows no border. Codex runs
new hooks only after you trust them in its `/hooks` menu. A settings file
behind a link that leads out of your profile is left alone, and the installer
says so; make it a real file in your profile and install again. Install adds
hooks to these two files only. If `CLAUDE_CONFIG_DIR` or `CODEX_HOME` moves an
agent's settings elsewhere, copy Daifuku's hooks into the file there
yourself, and remove them there when you uninstall; install names that file,
and doctor names it when the hooks there are missing.

Press Ctrl+Alt+Enter to open your first fleet. In a new terminal, `daifuku`
now works by name:

```powershell
daifuku doctor
```

checks the whole setup and says how to fix anything that is wrong. To see it
work without a real agent, `daifuku demo` from an administrator terminal opens
six scripted ones. The daemon logs to `C:\ProgramData\Daifuku\logs`, a file a
day.

To update, download the new zip, extract it and double-click `daifuku.exe`
again; your config and hooks are kept. A `ProgramData\Daifuku` folder that is
not locked to administrators the way install leaves it, such as one another
program made or one whose owner or access list was changed so that someone
else may write it, is deleted with any config in it and made anew.

## Configure

Every key is optional, and the defaults work without editing. To change
them, `daifuku config` opens `C:\ProgramData\Daifuku\daifuku.json` in your
editor for `.json` files, or Notepad when there is none. Only administrators
may write that folder, so from an ordinary terminal Windows asks for
permission first. The daemon picks up every saved change within two seconds;
`daifuku reload` from an administrator terminal applies it at once.
Do not take ownership of the folder or give your account write access to it:
the next install would delete it, config and all. An editor that reads the
linked schema completes and checks the keys.
[docs/configuration.md](docs/configuration.md) has every key.

```json
{
  "fleets": [
    { "name": "agents", "count": 6, "monitor": "portrait" },
    {
      "name": "site",
      "count": 4,
      "monitor": "primary",
      "directory": "C:\\src\\site",
      "hotkey": "ctrl + alt + w"
    }
  ]
}
```

| Fleet key | Default | |
|---|---|---|
| `count` | `6` | 1 to 16 terminals |
| `monitor` | `"portrait"` | `portrait`, `landscape`, `primary`, `secondary`, `cursor`, or a device name |
| `command` | `"claude"` | the PowerShell command each terminal runs; `{n}` is its number; `null` for a plain shell |
| `directory` | your profile | where the terminals start; `{n}` is its number (after 0.1.1) |
| `admin` | `true` | open as administrator |
| `hotkey` | `"ctrl + alt + return"` | the key that opens this fleet; a second fleet needs its own, or `null` |
| `shape` | automatic | force a grid, `{ "columns": 3, "rows": 2 }` |

Gaps are in `gaps`, the look of the borders in `border`, the global keys in
`hotkeys`, and `"sound": true` plays Windows' Asterisk sound when an agent
starts waiting.

Give a Claude Code fleet a `directory`: Claude Code does not remember that
you trust your profile folder, so a fleet started there asks for trust in
every terminal, every time.

## Accessibility

Colour is never the only signal. Every state has its own border width, so the
states read in greyscale and to every kind of colour vision:

| State | Width at the default of 4 px | Catppuccin | Colour-blind palette |
|---|---|---|---|
| done | 2 px | green | bluish green |
| working | 4 px | blue | sky blue |
| failed | 6 px | red | vermilion |
| waiting | 8 px, breathing slowly | yellow | orange |

- `"border": { "palette": "colorblind" }` switches to the Okabe-Ito colours,
  chosen to stay distinct for deuteranopia, protanopia and tritanopia. Your
  own four colours go in `"colours"`.
- With a high contrast theme on, the borders take the theme's system colours,
  each state keeps its own width, and nothing breathes.
- With "Show animations in Windows" off, the waiting border stops breathing.
  `"pulse": false` turns it off either way.
- `"sound": true` for a chime when an agent starts waiting for you: the
  Asterisk sound of your own sound scheme.
- Everything works from the keyboard, and `daifuku status` lists every agent
  and its state in plain text, for a screen reader or a script
  (`--json`).

## How agents report

The agent calls `daifuku hook` on every change of state, in the background, so
it never waits for it. The hook finds the terminal window it runs in and tells
the daemon, which colours that window's border. Several agents in tabs of one
window show the most urgent of their states.

No hook reports your answer at a permission prompt, so after you approve one
the border stays yellow until the approved tool has run. A Claude Code turn
you interrupt, with Esc or by saying no at a permission prompt, turns green
once Claude Code reports its prompt idle, about a minute later (after 0.1.1).

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
set the state shown for a window: its border, the chime, and its place in the
next key's queue.

The full threat model is in [docs/security.md](docs/security.md); report
vulnerabilities as described in [SECURITY.md](SECURITY.md).

## Commands

| | |
|---|---|
| `daifuku open [fleet]` | open a fleet, or bring it back |
| `daifuku snap` | put every terminal back in its cell |
| `daifuku next` | focus the agent that has waited longest, then failed ones |
| `daifuku close [fleet]` | close a fleet |
| `daifuku close --all` | close every open fleet |
| `daifuku status` | fleets, agents and hotkeys |
| `daifuku demo` | six scripted agents |
| `daifuku reload` | re-read the config |
| `daifuku stop` | stop the daemon |
| `daifuku config` | open the config file |
| `daifuku config --path` | print where the config file is |
| `daifuku schema` | print the config's JSON schema |
| `daifuku doctor` | check the setup |
| `daifuku install [--no-hooks] [--no-start]` | install, or update in place |
| `daifuku uninstall [--purge]` | remove it again |

The commands that talk to the daemon, `open` to `stop`, need an
administrator terminal, as the daemon runs elevated. The hotkeys work from
anywhere.

## Build

```powershell
cargo build --release
```

Rust stable, Windows only. `cargo test` runs the suite.

## Licence

GPL-3.0-only.

# Configuration

File: `C:\ProgramData\Daifuku\daifuku.json` (`daifuku config --path` prints it). JSON, UTF-8. Every key is optional; a key left out keeps its default. An empty file, `{}` or no file at all is one fleet of six terminals running `claude` on the monitor turned on its side, opened by a daemon with the user's own rights.

Unknown keys are errors at the top level and in `fleets[]`, `gaps`, `border`, `border.colours` and `hotkeys`. A misspelt key there makes the whole file invalid. `shape` needs both `columns` and `rows`, each at least 1.

## Writing the file

- Only administrators may write the folder `C:\ProgramData\Daifuku`. While the daemon runs with the user's own rights, install lets the user who installed Daifuku write `daifuku.json` in it, and `daifuku config` opens the user's editor at once. A config that sets `"elevated": true` only administrators may write, so then `daifuku config` asks Windows for permission first. From a shell that may write the file you may write it directly, after the user has seen the change.
- Never take ownership of the folder or give the user's account write access to it. The next install deletes a folder that is not locked to administrators, config included.
- The daemon re-reads the file within two seconds of each save. `daifuku reload` applies it at once (from an administrator terminal when the daemon runs elevated). `elevated` is the exception: it takes effect only when `daifuku install` runs again, and set in a file the user could write it takes two installs: the first locks the file to administrators and starts no daemon, the second, after the user checked the file, registers the elevated daemon.
- An invalid file never takes Daifuku down. The daemon keeps the last valid config, or the defaults when it started with an invalid file. `daifuku doctor` prints `ok    config valid` or `FIX   config valid: <the error>`.
- `daifuku schema` prints the JSON schema. Install writes `"$schema": "https://get-daifuku.vercel.app/schema.json"` into the starter config, so an editor that reads schemas completes and checks every key.

To check a draft without the daemon, run `daifuku config validate <path>`; without a path it checks the installed config. It works in any terminal, and `--json` gives machine output. With a bad config saved, the daemon keeps running on the last valid one, and `daifuku doctor` shows the problem on its `config valid` line.

## Example

```json
{
  "$schema": "https://get-daifuku.vercel.app/schema.json",
  "fleets": [
    { "name": "agents", "count": 6, "monitor": "portrait" },
    {
      "name": "site",
      "count": 4,
      "monitor": "primary",
      "directory": "C:\\src\\site",
      "command": "claude --continue",
      "hotkey": "ctrl + alt + w"
    }
  ],
  "border": { "palette": "colorblind" },
  "sound": true
}
```

## Top level

| Key | Type | Default | Meaning |
|---|---|---|---|
| `$schema` | string | none | Schema link for editors. Daifuku ignores it. |
| `elevated` | bool | `false` | Start the daemon with administrator rights at sign-in, which fleets with `"admin": true` need. Takes effect at the next `daifuku install`, from an administrator terminal. Then the daemon commands need an administrator terminal too. Never set it without the user asking. |
| `fleets` | list | one fleet with every default below | The fleets. |
| `accounts` | object | none | Accounts the agents sign in with, by a name of the user's own, see below. |
| `gaps` | object | see below | Spacing around and between terminals. |
| `border` | object | see below | The status borders. |
| `hotkeys` | object | see below | Keys that act on every fleet. |
| `sound` | bool | `false` | Play Windows' Asterisk sound (the user's own sound scheme) when an agent starts waiting. |

## `fleets[]`

| Key | Type | Default | Meaning |
|---|---|---|---|
| `name` | string | `"agents"` | What `daifuku open <name>` takes. Unique, not case sensitive. |
| `count` | integer | `6` | Terminals, 1 to 16. |
| `monitor` | string | `"portrait"` | Which monitor, see below. |
| `area` | string or object | `"full"` | The part of the monitor the fleet fills, see below. |
| `shape` | object | automatic | Force a grid: `{ "columns": 3, "rows": 2 }`. A shape too small for `count` grows rows. |
| `directory` | string | the user's profile folder | Where every terminal starts. `{n}` becomes the terminal's number, so `C:\src\site-{n}` gives each agent its own folder. A folder that does not exist falls back to the profile folder. |
| `command` | string or `null` | `"claude"` | What each terminal runs, in a PowerShell that stays open after it (PowerShell 7 when installed in Program Files, else Windows PowerShell). `{n}` becomes the terminal's number, 1 for the first cell. `null` opens a plain shell. |
| `profile` | string | Windows Terminal's default profile | A Windows Terminal profile, by name. |
| `admin` | bool | `false` | Open the terminals as administrator. Needs `"elevated": true`. The agent and every command it runs then run as administrator. With `false` and an elevated daemon they open as the user through the Secondary Logon service (seclogon), which must not be disabled; keep `command` under about 250 characters then, less a folder's length plus 20 for each folder of its `account`. A daemon with the user's own rights opens them directly. |
| `no_profile` | bool | `false` | With a `command`, start PowerShell without the user's profile script. |
| `hotkey` | string or `null` | `"ctrl + alt + return"` | The key that opens this fleet or brings it back. Every fleet without the key gets the default, so a second fleet needs its own key or `null`. |
| `with` | list of strings | none | Other fleets, by name, that open with this one, before it, so this one opens last. Their own `with` is not followed. A name that is not a fleet, or the fleet itself, is an error. |
| `priority` | string | none | `"idle"`, `"below-normal"` or `"normal"`: each terminal's shell runs in `daifuku group` at that priority, and its whole tree ends with the terminal. Needs a `command`. |
| `account` | string | the account in the profile | The account its agents sign in with, by its name under `accounts`. A name that is not there is an error. |
| `resume` | bool | `false` | Keep each terminal with the session its agent reports from inside it. Opening the fleet then first reopens every terminal it had when Windows last stopped or when it was closed, in its own cell resuming that session, then starts the other cells afresh. Never a session that runs already. Needs a `command`. `daifuku restore --forget` drops the kept sessions. |

Give a Claude Code fleet a `directory`: Claude Code does not remember trust for the profile folder, so a fleet started there asks for trust in every terminal, every time.

### `monitor` values

| Value | Picks |
|---|---|
| `"portrait"` | The first monitor taller than wide. |
| `"landscape"` | The first monitor wider than tall, the primary one first. |
| `"primary"` | The primary monitor. |
| `"secondary"` | The first monitor that is not the primary. |
| `"cursor"` (or `"mouse"`) | The monitor the mouse is on when the fleet opens. |
| `"\\\\.\\DISPLAY2"` | A monitor by its device name (`\\.\DISPLAY2` unescaped). |

Every pick falls back to the primary monitor. Any other word is an error.

### `area` values

| Value | Fills |
|---|---|
| `"full"` | The monitor's whole work area, the screen without the taskbar. |
| `"left"`, `"right"`, `"top"`, `"bottom"` | That half of it. |
| `{ "x": 0.5, "y": 0, "width": 0.5, "height": 1 }` | A rectangle in fractions of the work area from its top left corner. All four keys are required, each 0 to 1; `width` and `height` more than 0; `x + width` and `y + height` at most 1. |

Any other word, a missing key or a rectangle off the monitor is an error. The outer gap goes around the area. Three terminals on a half come out two over one; `"shape": { "columns": 1, "rows": 3 }` stacks them.

## `accounts`

An object from a name of the user's own, not case sensitive, to the folders that account keeps its sign-in in. A fleet's terminals, and the ones `restore` and `spawn` open for it, start with the variables set before the command runs, so one config runs fleets on two accounts.

| Key | Type | Meaning |
|---|---|---|
| `claude` | string | Claude Code's folder for the account, set as `CLAUDE_CONFIG_DIR`. A full path. |
| `codex` | string | Codex's folder for the account, set as `CODEX_HOME`. A full path. |

An account needs at least one of them; two accounts cannot share a name or a folder. Run `daifuku install` again after adding one, once the agent has signed in there, so the hooks go into that folder too. `daifuku uninstall` removes hooks only from the accounts in the config, so run it before taking an account out. A fleet on an account with `command: null` opens PowerShell, not its profile's shell. `daifuku spawn` without `--fleet` takes the fleet, and so the account, of the terminal it runs in. A `restore` resumes in the fleet's account as it is then, so change an account only once its spawned terminals are done.

```json
"accounts": { "work": { "claude": "C:\\Users\\you\\.claude-work" } },
"fleets": [{ "name": "crew", "count": 3, "account": "work", "hotkey": null }]
```

## `gaps`

| Key | Type | Default | Meaning |
|---|---|---|---|
| `outer` | integer | `24` | Pixels between the outermost terminals and the screen edge, or the edge of the fleet's `area`, 0 to 1000. |
| `inner` | integer | `20` | Pixels between neighbouring terminals, 0 to 1000. |

## `border`

| Key | Type | Default | Meaning |
|---|---|---|---|
| `enabled` | bool | `true` | Draw status borders at all. |
| `width` | integer | `4` | Thickness in physical pixels, 1 to 64; 0 draws as 1. |
| `offset` | integer | `0` | How far outside the window edge the border sits, -64 to 64; negative overlaps the window. |
| `palette` | string | `"catppuccin"` | `"catppuccin"` or `"colorblind"` (Okabe-Ito). |
| `colours` | object | the palette's | Own colours for any of `working`, `waiting`, `done`, `failed`, as `"#rrggbb"`. A state left out keeps the palette's colour. |
| `state_widths` | bool | `true` | Each state at its own width, so it reads without colour. Always on with a high contrast theme. |
| `pulse` | bool | `true` | The waiting border breathes between 70 % and full brightness. Off anyway when Windows shows no animations or a high contrast theme is on. |

Palette colours:

| State | `catppuccin` | `colorblind` |
|---|---|---|
| working | `#89b4fa` | `#56b4e9` |
| waiting | `#f9e2af` | `#e69f00` |
| done | `#a6e3a1` | `#009e73` |
| failed | `#f38ba8` | `#d55e00` |

With `state_widths` on and `width` 4, the widths are done 2, working 4, failed 6, waiting 8. Each state is thicker than the one before it at any width. With a high contrast theme the borders take the theme's colours and nothing breathes. A maximised terminal gets no border.

## `hotkeys`

| Key | Default | Action |
|---|---|---|
| `next_waiting` | `"ctrl + alt + n"` | Focus the agent that has waited longest, then the failed ones. Pressed again while that agent still waits, it moves on to the next. |
| `snap` | `"ctrl + alt + backspace"` | Put every fleet terminal back in its cell. |
| `close` | `"ctrl + alt + f4"` | Close every fleet terminal, the demo's too, without asking. |

`null` turns a key off.

## Hotkey syntax

Modifiers and one key, joined by `+`; case, spaces and the order of the modifiers do not matter (`"Alt+CTRL+A"` is `ctrl + alt + a`). Modifiers: `ctrl`, `alt`, `shift`, `win`; at least one is required. Keys: a letter, a digit, `f1` to `f24`, `numpad0` to `numpad9`, `return`, `space`, `backspace`, `tab`, `escape`, `pageup`, `pagedown`, `home`, `end`, `insert`, `delete`, the arrows, `comma`, `period`, `minus`, `plus`, `semicolon`, `slash`, `backslash`, `grave`, `quote`, `bracketleft`, `bracketright`. The punctuation names follow a US keyboard.

A combination another program already holds cannot be registered; `daifuku status` and `daifuku doctor` (administrator terminal) list it as refused. On German, Swiss and French keyboards AltGr is Ctrl and Alt together, so avoid Ctrl+Alt with a key that types a character there, such as `e` or `2`.

# Configuration

File: `C:\ProgramData\Daifuku\daifuku.json` (`daifuku config --path` prints it). JSON, UTF-8. Every key is optional; a key left out keeps its default. An empty file, `{}` or no file at all is one fleet of six administrator terminals running `claude` on the monitor turned on its side.

Unknown keys are errors at the top level and in `fleets[]`, `gaps`, `border`, `border.colours` and `hotkeys`. A misspelt key there makes the whole file invalid. `shape` needs both `columns` and `rows`, each at least 1.

## Writing the file

- Only administrators may write `C:\ProgramData\Daifuku`. `daifuku config` asks Windows for permission and opens the user's editor. From an administrator shell you may write the file directly, after the user has seen the change.
- Never take ownership of the folder or give the user's account write access to it. The next install deletes a folder that is not locked to administrators, config included.
- The daemon re-reads the file within two seconds of each save. `daifuku reload` (administrator terminal) applies it at once.
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
| `fleets` | list | one fleet with every default below | The fleets. |
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
| `shape` | object | automatic | Force a grid: `{ "columns": 3, "rows": 2 }`. A shape too small for `count` grows rows. |
| `directory` | string | the user's profile folder | Where every terminal starts. `{n}` becomes the terminal's number, so `C:\src\site-{n}` gives each agent its own folder. A folder that does not exist falls back to the profile folder. |
| `command` | string or `null` | `"claude"` | What each terminal runs, in a PowerShell that stays open after it (PowerShell 7 when installed in Program Files, else Windows PowerShell). `{n}` becomes the terminal's number, 1 for the first cell. `null` opens a plain shell. |
| `profile` | string | Windows Terminal's default profile | A Windows Terminal profile, by name. |
| `admin` | bool | `true` | Open the terminals as administrator. The agent and every command it runs then run as administrator. With `false` they open as the user through the Secondary Logon service (seclogon), which must not be disabled; keep `command` under about 250 characters then. |
| `no_profile` | bool | `false` | With a `command`, start PowerShell without the user's profile script. |
| `hotkey` | string or `null` | `"ctrl + alt + return"` | The key that opens this fleet or brings it back. Every fleet without the key gets the default, so a second fleet needs its own key or `null`. |

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

## `gaps`

| Key | Type | Default | Meaning |
|---|---|---|---|
| `outer` | integer | `24` | Pixels between the outermost terminals and the screen edge, 0 to 1000. |
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

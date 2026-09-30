# Configuration

Daifuku reads `C:\ProgramData\Daifuku\daifuku.json`. `daifuku config` prints
that path. `daifuku reload` applies a change at once; from version 0.1.1 on,
the daemon also picks up every saved change by itself within two seconds.

The folder is writable by administrators only, on purpose: the daemon runs
elevated and the config says what it starts. Edit the file from an
administrator editor, for example `notepad` in an administrator terminal.

Every key is optional. An empty file, or no file, is one fleet of six
administrator terminals running `claude` on the monitor turned on its side.
With `"$schema"` pointing at the published schema, an editor completes every
key and checks its name and type:

```json
{
  "$schema": "https://raw.githubusercontent.com/dominikkoenitzer/Daifuku/main/schema.json"
}
```

A mistake never takes Daifuku down: an invalid file is reported by
`daifuku status` and `daifuku doctor`, and the last valid one stays in use,
or the defaults when the daemon started with the invalid file.

## `fleets`

A list of fleets. Each opens with its own hotkey or with `daifuku open <name>`.

```json
{
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
  ]
}
```

| Key | Default | |
|---|---|---|
| `name` | `"agents"` | What `daifuku open` takes. Unique, not case sensitive. |
| `count` | `6` | 1 to 16 terminals. |
| `monitor` | `"portrait"` | See below. |
| `shape` | automatic | `{ "columns": 3, "rows": 2 }` to force a grid. A shape too small for `count` grows rows. |
| `directory` | your profile folder | Where every terminal starts. `{n}` becomes the terminal's number: `C:\src\site-{n}` gives each agent a folder of its own, such as its own git worktree (in releases after 0.1.1). A folder that does not exist falls back to your profile folder. |
| `command` | `"claude"` | What each terminal runs, in PowerShell that stays open after it. `{n}` becomes the terminal's number, 1 for the first cell. `null` opens a plain shell. |
| `profile` | Windows Terminal's default | A Windows Terminal profile, by name. |
| `admin` | `true` | Open the terminals as administrator. |
| `no_profile` | `false` | With a `command`, start PowerShell without your profile script: faster, and nothing of your setup on screen. |
| `hotkey` | `"ctrl + alt + return"` | The key that opens this fleet, or brings it back when it is open. `null` for none. Every fleet without one gets the default, so a second fleet needs its own key or `null`. |

Opening a fleet that is already open brings it back: terminals that were
closed are opened again, each in its own cell with its own number (after
0.1.1), and every terminal goes back to its cell.

### `monitor`

| Value | Picks |
|---|---|
| `"portrait"` | The first monitor taller than wide. |
| `"landscape"` | The first monitor wider than tall, the primary one first. |
| `"primary"` | The primary monitor. |
| `"secondary"` | The first monitor that is not the primary. |
| `"cursor"` | The monitor the mouse is on when the fleet opens. It stays there until it is closed (after 0.1.1). |
| `"\\\\.\\DISPLAY2"` | A monitor by its device name. |

Every pick falls back to the primary monitor, so a fleet always opens
somewhere. From version 0.1.1 on, when monitors change, open fleets move into the
grid of the monitor they belong on now.

### The grid

Daifuku picks the shape whose cells come closest to a terminal's natural
proportions, a little wider than tall, and avoids empty cells: six terminals
on a portrait monitor are two by three, on a landscape one three by two, four
are two by two. An incomplete last row shares the whole width, so there is
never a hole in a corner. Cells are placed by the window's visible edge, so
the gaps are exact.

## `gaps`

| Key | Default | |
|---|---|---|
| `outer` | `24` | Pixels between the outermost terminals and the edge of the screen, up to 1000. |
| `inner` | `20` | Pixels between two neighbouring terminals, up to 1000. |

## `border`

| Key | Default | |
|---|---|---|
| `enabled` | `true` | Draw status borders at all. |
| `width` | `4` | Thickness in physical pixels, 0 to 64. |
| `offset` | `0` | How far outside the window's edge the border sits, -64 to 64; negative overlaps it. |
| `palette` | `"catppuccin"` | `"catppuccin"` or `"colorblind"` (Okabe-Ito). |
| `colours` | the palette's | Your own four colours, see below. |
| `state_widths` | `true` | Each state at its own width, so a state reads without its colour. |
| `pulse` | `true` | The waiting border breathes slowly. Off anyway when Windows shows no animations. |

With `state_widths` on, the widths at the default of 4 are done 2, working 4,
failed 6 and waiting 8. With a high contrast theme on, the borders take the
theme's own colours.

### `colours`

```json
{
  "border": {
    "colours": {
      "working": "#89b4fa",
      "waiting": "#f9e2af",
      "done": "#a6e3a1",
      "failed": "#f38ba8"
    }
  }
}
```

Colours are `#rrggbb`. A key left out takes the Catppuccin colour.

## `hotkeys`

| Key | Default | |
|---|---|---|
| `next_waiting` | `"ctrl + alt + n"` | Focus the agent that has waited longest, and after the waiting ones, the failed ones. Pressed again from the one it brought up, while that agent still waits, it moves on to the next. |
| `snap` | `"ctrl + alt + backspace"` | Put every fleet terminal back in its cell. |

A hotkey is modifiers and one key, joined by `+`: `ctrl`, `alt`, `shift`,
`win`, and a letter, a digit, `f1` to `f24`, `numpad0` to `numpad9`, or one of
`return`, `space`, `backspace`, `tab`, `escape`, `pageup`, `pagedown`, `home`,
`end`, `insert`, `delete`, the arrows, `comma`, `period`, `minus`, `plus`,
`semicolon`, `slash`, `backslash`, `grave`, `quote`, `bracketleft`,
`bracketright`. At least one modifier is required. `null` turns a key off.

A combination another program already holds cannot be registered; `daifuku
status` and `daifuku doctor` list it as refused. On German, Swiss and French
keyboards AltGr is Ctrl and Alt together, so avoid Ctrl+Alt with a letter that
types a character there, such as `e` (€) or `2` (@).

## `sound`

`true` plays Windows' Asterisk sound when an agent starts waiting for you,
so it follows your own sound scheme (Sound settings, Program Events).
Off by default.

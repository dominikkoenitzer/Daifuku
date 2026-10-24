# Configuration

Daifuku reads `C:\ProgramData\Daifuku\daifuku.json`. `daifuku config` opens
it in your editor, and `daifuku config --path` prints that path. The daemon
picks up every saved change by itself within two seconds; `daifuku reload`
applies one at once.

The folder is writable by administrators only, on purpose: the config says
what the daemon starts and whether it runs elevated. While the daemon runs
with your own rights, install lets you, and no other user, write the config
file itself, so `daifuku config` opens it at once. A config that sets
`"elevated": true` is writable by administrators only, so then `daifuku
config` asks Windows for permission before it opens the editor, unless the
terminal is an administrator terminal already. Turning elevation on takes two
installs: the first locks the config you wrote and starts no daemon, the
second, after you checked it, registers the elevated one.

Every key is optional. An empty file, or no file, is one fleet of six
terminals running `claude` on the monitor turned on its side, opened by a
daemon with your own rights.

## `elevated`

`false` by default: no logon task is written. The daemon runs with your own
rights, started on demand by the first command line or hook event of a
session, and it opens terminals as you. Set `"elevated": true` for fleets
with `"admin": true`, whose administrator terminals only an elevated daemon
can start, move and frame: Windows keeps an ordinary process away from an
administrator's windows. The key takes effect at the next `daifuku install`,
which registers a logon task to start the daemon elevated at sign-in, since
an elevated daemon needs a start without a prompt, and from then on the
daemon commands need an administrator terminal. Without it, a fleet
with `"admin": true` does not open and says why; `daifuku install` and
`daifuku doctor` name the fleets that need the key.

```json
{
  "elevated": true,
  "fleets": [{ "name": "agents", "admin": true }]
}
```
With `"$schema"` pointing at the published schema, an editor completes every
key and checks its name and type:

```json
{
  "$schema": "https://get-daifuku.vercel.app/schema.json"
}
```

A mistake never takes Daifuku down: an invalid file is reported by
`daifuku status` and `daifuku doctor`, and the last valid one stays in use,
or the defaults when the daemon started with the invalid file.
`daifuku config validate`, from any terminal, checks the file the way the
daemon reads it before you rely on it; `daifuku config validate <file>`
checks a copy anywhere else.

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
| `area` | `"full"` | The part of the monitor the fleet fills. See below (after 0.1.3). |
| `shape` | automatic | `{ "columns": 3, "rows": 2 }` to force a grid. A shape too small for `count` grows rows. |
| `directory` | your profile folder | Where every terminal starts. `{n}` becomes the terminal's number: `C:\src\site-{n}` gives each agent a folder of its own, such as its own git worktree (in releases after 0.1.1). A folder that does not exist falls back to your profile folder. An `"admin": false` fleet looks for the folder as you, so a drive you mapped or made with `subst` counts (after 0.1.1). |
| `command` | `"claude"` | What each terminal runs, in PowerShell that stays open after it: PowerShell 7 when it is installed in Program Files, else Windows PowerShell. When no execution policy lets Windows PowerShell run scripts, it starts with `-ExecutionPolicy RemoteSigned` so a `claude` installed with npm runs; a policy you or your organisation set is kept. `{n}` becomes the terminal's number, 1 for the first cell. `null` opens a plain shell. |
| `profile` | Windows Terminal's default | A Windows Terminal profile, by name. |
| `admin` | `false` | Open the terminals as administrator. Needs `"elevated": true`. With `false` and an elevated daemon, Windows starts them as you through its Secondary Logon service (seclogon), which must not be disabled, and takes at most 1024 characters for their whole command line. The encoded `command` uses almost three of those for each of its own, so keep it under about 250 characters, or about 225 with a `priority`, whose group takes about 70 more, and about 20 plus the folder's length less for each folder of an `account`. A daemon with your own rights starts them directly, with neither limit. |
| `no_profile` | `false` | With a `command`, start PowerShell without your profile script: faster, and nothing of your setup on screen. |
| `hotkey` | `"ctrl + alt + return"` | The key that opens this fleet, or brings it back when it is open. `null` for none. Every fleet without one gets the default, so a second fleet needs its own key or `null`. |
| `with` | none | Other fleets, by name, that open with this one, or come back with it. They open first, so this fleet opens last and asks for the focus. Their own `with` is not followed (after 0.1.3). |
| `priority` | none | `"idle"`, `"below-normal"` or `"normal"`: each terminal runs in a group of its own at that priority. Everything started in it runs at it and cannot raise it, and all of it ends when the terminal closes, shells and watchers an agent left behind included. Left out, the terminals run as any other program (after 0.1.3). |
| `account` | the one in your profile | The account its agents sign in with, by its name under [`accounts`](#accounts) (after 0.3.0). |
| `resume` | `false` | Keep each terminal with the last session its agent reported from inside it, as `daifuku spawn` keeps a peer. Opening the fleet then first opens every such terminal it had when Windows last stopped, or when the fleet was closed, in its own cell resuming that session, lowest cell first, and starts the other cells afresh. A terminal closed by hand is let go, so its cell starts afresh; so does one whose session no longer opens, with the reason in the answer to `open`. A session that runs already is not opened twice, and one whose cell is past the fleet's `count` waits for `daifuku restore`. Turning `resume` off lets go of what the fleet kept; `daifuku restore --forget` lets go of all of it. Needs a `command` (after 0.3.0). |
| `backlog` | none | A markdown file of tasks for its agents, by full path. Each time one of its Claude Code agents finishes a turn holding no task, Daifuku hands it the next open one. See [below](#backlog) (after 0.3.0). |

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
somewhere. A value that is neither a keyword nor a device name, which starts
with `\\.\`, is an error (after 0.1.1), so a misspelt keyword does not
quietly become the primary monitor.

From version 0.1.1 on, when monitors change, open fleets move into the grid
of the monitor they belong on now. After 0.1.1, only the fleets whose cells
the change moved, or whose monitor's scale it changed, are put back, and a
terminal you minimised or maximised stays that way.

### `area`

| Value | Fills |
|---|---|
| `"full"` | The whole monitor, apart from the taskbar. |
| `"left"`, `"right"` | The left or the right half. |
| `"top"`, `"bottom"` | The top or the bottom half. |
| `{ "x": 0.5, "y": 0, "width": 0.5, "height": 1 }` | Any rectangle, in fractions of the monitor from its top left corner. All four keys are needed, each 0 to 1. `width` and `height` must be more than 0, and the rectangle must stay on the monitor. |

One agent that hands out the work on the left half, and the three it hands
it to stacked on the right, all four on one key:

```json
{
  "fleets": [
    {
      "name": "lead",
      "count": 1,
      "monitor": "landscape",
      "area": "left",
      "hotkey": "ctrl + alt + return",
      "with": ["crew"]
    },
    {
      "name": "crew",
      "count": 3,
      "monitor": "landscape",
      "area": "right",
      "shape": { "columns": 1, "rows": 3 },
      "hotkey": "ctrl + alt + w"
    }
  ]
}
```

On their own, three terminals on a half come out two over one, so `shape`
stacks them in three rows the width of the half. Ctrl+Alt+Return opens the
crew and then the lead, which asks for the focus; Ctrl+Alt+W opens or brings
back the crew alone.

The same layout with a second fleet on the portrait monitor that signs in to
another account, all of it on one key and each terminal back in its session
after a restart:

```json
{
  "accounts": { "work": { "claude": "C:\\Users\\you\\.claude-work" } },
  "fleets": [
    {
      "name": "lead",
      "count": 1,
      "monitor": "landscape",
      "area": "left",
      "resume": true,
      "hotkey": "ctrl + alt + return",
      "with": ["crew", "work"]
    },
    {
      "name": "crew",
      "count": 3,
      "monitor": "landscape",
      "area": "right",
      "shape": { "columns": 1, "rows": 3 },
      "resume": true,
      "hotkey": null
    },
    {
      "name": "work",
      "count": 4,
      "monitor": "portrait",
      "account": "work",
      "resume": true,
      "hotkey": null
    }
  ]
}
```

With `resume`, a terminal a `daifuku spawn --cell` put in the crew comes back
in its cell too, as `daifuku restore` would bring it back.

The outer gap goes around the area as it goes around the monitor, so on the
monitor's own edges the terminals line up with a fleet that fills all of it.
Two fleets on the two halves have twice the outer gap between them.

### The grid

Daifuku picks the shape whose cells come closest to a terminal's natural
proportions, a little wider than tall, and avoids empty cells: six terminals
on a portrait monitor are two by three, on a landscape one three by two, four
are two by two. An incomplete last row shares the whole width, so there is
never a hole in a corner. On half of a landscape monitor two terminals are
stacked and three are two over one; `"shape": { "columns": 1, "rows": 3 }`
stacks three. Cells are placed by the window's visible edge, so the gaps are
exact.

### `backlog`

A fleet with a `backlog` works through it on its own (after 0.3.0). Each
task is a third-level heading with its fields after bars, and its text runs
up to the next heading:

```markdown
### A1. Write the setup guide | owner: | status: open
What the guide covers, and where it goes.
```

The id runs up to the first full stop: letters, digits, `-` and `_`, at
most 32. A task is open while it has no owner and its status starts with
`open`, and finished once its status starts with `done` or `closed`.
Headings in fenced code are not tasks.

When the main thread of one of the fleet's Claude Code agents stops and its
terminal holds no task, Daifuku hands it the first open task nobody has, in
the order of the file: the agent goes on with it instead of stopping, told
which file and id it is. The task stays with that terminal, and with its
session if it moves to another, until the file says it is finished; until
then the agent gets no other. Daifuku only reads the file, so the agent
writes its own name and status in as it works. A task with a folder of its
id in `claims` beside the file counts as taken, for agents that claim tasks
by hand. Codex agents get no tasks.

An agent gets a task only while its account's [`stop_at`](#stopping) lets
it work on. `daifuku task` lists each backlog's open tasks, which terminal
has which, and the one that goes out next; `daifuku task release <id>`
takes a task back, so the terminal that had it can get another. Tasks go
out only as agents stop: one that stopped while none could go out, or
while Daifuku answered four others at once, waits for you and gets one at
the end of its next turn. `daifuku events` says when that happens, and
when an account reaches its stop, starts over or may take more, so a
script or an agent that runs the fleet can act on it; `--follow` prints
each as it comes. An elevated agent gets no tasks, as any program of yours
can write the backlog it would take them from.

The hook that hands out tasks is Claude Code's `Stop` hook, which then
waits for Daifuku instead of running in the background. Run
`daifuku install` again after an update that adds `backlog`, so the
installed hook asks.

## `gaps`

| Key | Default | |
|---|---|---|
| `outer` | `24` | Pixels between the outermost terminals and the edge of the screen, or of the fleet's `area`, up to 1000. |
| `inner` | `20` | Pixels between two neighbouring terminals, up to 1000. |

## `border`

| Key | Default | |
|---|---|---|
| `enabled` | `true` | Draw status borders at all. |
| `width` | `4` | Thickness in physical pixels, 1 to 64; 0 draws as 1. `enabled: false` turns the borders off. |
| `offset` | `0` | How far outside the window's edge the border sits, -64 to 64; negative overlaps it. |
| `palette` | `"catppuccin"` | `"catppuccin"` or `"colorblind"` (Okabe-Ito). |
| `colours` | the palette's | Your own colours, for any of the states, see below. |
| `state_widths` | `true` | Each state at its own width, so a state reads without its colour. Always on with a high contrast theme. |
| `pulse` | `true` | The waiting border breathes slowly, between 70 % and full brightness, so its contrast never drops far. Off anyway when Windows shows no animations or a high contrast theme is on. It also rests at full brightness while the session is locked, a screen saver is up or another account is switched to (after 0.1.1). |

With `state_widths` on, the widths at the default of 4 are done 2, working 4,
failed 6 and waiting 8. Each state is thicker than the one before it at any
width: at 1 they are 1, 2, 3 and 4 (after 0.1.1; before, done, working and
failed were all 1 at that width). With a high contrast theme on, the borders
take the theme's own colours, every state keeps its own width, and nothing
breathes. A maximised terminal gets no border: it would lie outside the
screen.

### `colours`

```json
{
  "border": {
    "colours": {
      "working": "#89b4fa",
      "waiting": "#f9e2af",
      "done": "#a6e3a1",
      "failed": "#f38ba8",
      "warning": "#fab387"
    }
  }
}
```

Colours are `#rrggbb`. A key left out keeps the palette's colour (after
0.1.1; before, it took the Catppuccin one). `warning` is the colour of a
working or done terminal whose account has reached its
[`warn`](#accounts) (after 0.3.0): peach in `catppuccin`, reddish purple in
`colorblind`, and the theme's text colour with a high contrast theme, as for
failed. A waiting or failed terminal keeps its own colour.

## `accounts`

Accounts the agents sign in with, by a name of your own, for fleets that
should not all use the one in your profile (after 0.3.0). Each names the
folder Claude Code keeps its sign-in and settings in, the one it reads from
`CLAUDE_CONFIG_DIR`, the folder Codex reads from `CODEX_HOME`, or both.

```json
{
  "accounts": {
    "work": { "claude": "C:\\Users\\you\\.claude-work" }
  },
  "fleets": [
    { "name": "agents", "count": 6, "monitor": "portrait" },
    { "name": "crew", "count": 3, "account": "work", "hotkey": "ctrl + alt + shift + w" }
  ]
}
```

| Key | |
|---|---|
| `claude` | Claude Code's folder for this account, a full path. |
| `codex` | Codex's folder for this account, a full path. |
| `warn` | A share of the 5-hour or the weekly limit, in percent from 1 to 100. Once either reaches it, the borders of the account's working and done terminals take the [`warning`](#colours) colour until that window resets (after 0.3.0). Needs the status line below. |
| `stop_at` | Rules for when its fleets' agents get no more tasks from a [`backlog`](#backlog). See [below](#stopping) (after 0.3.0). |

Every terminal of a fleet with an `account` starts with those variables
set, before its `command` runs, and so does a terminal `daifuku restore`
opens for it or one an agent of it opens with `daifuku spawn`, which
takes the fleet of the terminal it runs in when it names none. With no
`command` the terminal opens PowerShell with them set, ready for one you
type, in place of the shell of its `profile`.

Account names are not case sensitive, and two accounts cannot share a name
or a folder. A fleet that names an account missing here makes the config
invalid. A terminal `daifuku restore` opens resumes its session in the
fleet's account as it is then, so change a fleet's account once its
spawned terminals are done.

`daifuku install` adds the hooks to each account's folder as well as to
the one in your profile, once the agent has made the folder by signing in
there: run it again after you add an account. Like the profile's, the
folder must be inside your profile folder. `daifuku doctor` checks each
account's hooks, and `daifuku uninstall` removes them from the accounts
the config names then: run it before you take an account out, or remove
Daifuku's hooks from that folder's `settings.json` or `hooks.json` by hand.

### Usage

`daifuku status` shows how much of its 5-hour and weekly limits each
account has used, when each window resets, and its pace: how far use is
ahead of an even spread over the window, `3 over pace` or `12 under pace`
(after 0.3.0). The numbers come from Claude Code, which hands them to its
status line after each answer from a subscription. Set the status line of
each account's Claude Code, in its `settings.json`, to:

```json
{ "statusLine": { "type": "command", "command": "daifuku status-line" } }
```

It reports the numbers to the daemon and shows them as one line, such as
`5h 48% (2h 14m left), week 35% (3d 4h left)`. With a status line of your
own, pipe the input your script gets into `daifuku status-line --quiet`,
which reports without printing. An account is told apart by its folder:
`CLAUDE_CONFIG_DIR`, or `.claude` in your profile when that is not set, so
list the profile's folder under `accounts` too to give it a name or a
`warn`. The numbers are as old as the last answer any of the account's
agents got, and `daifuku status` says how old; once a window resets they
no longer hold until the next answer. Codex has no status line to report
from.

### Stopping

An account's `stop_at` says how much of its limits its agents may use on
tasks from a backlog (after 0.3.0). Each rule names a `share` in percent;
the first rule that holds for a window, in the order written, decides it,
and once that much of the window is used no task goes out until it resets.
A window no rule holds for has no stop.

```json
"stop_at": [
  { "share": 90, "window": "five_hour", "last_minutes": 60 },
  { "share": 75, "window": "five_hour", "days": "mon-fri", "hours": "9-17" },
  { "share": 95 }
]
```

| Key | |
|---|---|
| `share` | 1 to 100. |
| `window` | `five_hour` or `seven_day`. Left out, both. |
| `days` | Local days it holds on: `mon-fri`, `sat,sun`, or one day. Left out, every day. |
| `hours` | Local hours it holds in, from the first up to the second, 0 to 24: `9-17`, or `22-6` past midnight. Left out, all day. |
| `last_minutes` | It holds only this many minutes before the window resets, 1 to 10080. |

Here the 5-hour window may be used up to 90% in its last hour, up to 75%
in office hours on weekdays, and up to 95% otherwise, as may the week. The
numbers come from the status line above, so with rules but no numbers
reported yet no task goes out. A task already handed out is not taken
back: the agent finishes it.

## `hotkeys`

| Key | Default | |
|---|---|---|
| `next_waiting` | `"ctrl + alt + n"` | Focus the agent that has waited longest, and after the waiting ones, the failed ones. Pressed again from the one it brought up, while that agent still waits, it moves on to the next. |
| `snap` | `"ctrl + alt + backspace"` | Put every fleet terminal back in its cell. |
| `close` | `"ctrl + alt + f4"` | Close every fleet terminal, the demo's too, without asking: an agent still working ends with its terminal. |

A hotkey is modifiers and one key, joined by `+`: `ctrl`, `alt`, `shift`,
`win`, and a letter, a digit, `f1` to `f24`, `numpad0` to `numpad9`, or one of
`return`, `space`, `backspace`, `tab`, `escape`, `pageup`, `pagedown`, `home`,
`end`, `insert`, `delete`, the arrows, `comma`, `period`, `minus`, `plus`,
`semicolon`, `slash`, `backslash`, `grave`, `quote`, `bracketleft`,
`bracketright`. At least one modifier is required. `null` turns a key off.
`semicolon`, `slash`, `backslash`, `grave`, `quote`, `bracketleft` and
`bracketright` name keys by what they type on a US keyboard; on another
layout it can be a key with other characters on it.

A combination another program already holds cannot be registered; `daifuku
status` and `daifuku doctor` list it as refused. On German, Swiss and French
keyboards AltGr is Ctrl and Alt together, so avoid Ctrl+Alt with a letter,
digit or punctuation key that types a character there, such as `e` (€) or
`2` (@).

## `sound`

`true` plays Windows' Asterisk sound when an agent starts waiting for you,
so it follows your own sound scheme (Sound settings, Program Events).
Off by default.

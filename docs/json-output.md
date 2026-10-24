# JSON output

The `daifuku` commands that report state or act take `--json` after the
command's name, as in `daifuku status --json`. With it, `daifuku` prints
exactly one JSON document on standard output and nothing else there, errors
included, so a script or an assistant can read every answer the same way.
`daifuku events --follow --json` is the one that goes on: it prints one
document per line, an event each, until it is stopped or fails.
`install` and `uninstall` take it only with `--dry-run`; the hidden commands
do not take it.

Characters past ASCII are written as `\u` escapes, so a script reads the
same text in whatever code page its shell decodes the output in.

## Stability

Within a major version, fields are added but never renamed or removed, and a
field keeps its type. An error's `code` keeps its meaning; its `message` is
for a person and may change. Read the fields you need and ignore the rest.

## Commands

| Command | Prints |
|---|---|
| `daifuku status --json` | the status, below; from a terminal that is not elevated, the local status |
| `daifuku doctor --json` | the doctor's report, below |
| `daifuku config --path --json` | `{"path": "C:\\ProgramData\\Daifuku\\daifuku.json"}` |
| `daifuku schema --json` | the config's JSON schema, the same as without `--json` |
| `daifuku config --json` | `{"ok": true}` once the editor started |
| `daifuku config validate [file] --json` | `{"ok": true, "path": "..."}` for a file the daemon would load, below |
| `daifuku open [fleet] --json` | `{"ok": true}` |
| `daifuku snap --json` | `{"ok": true}` |
| `daifuku next --json` | `{"ok": true}` |
| `daifuku close [fleet] --json` | `{"ok": true}` |
| `daifuku close --all --json` | `{"ok": true}` |
| `daifuku close --terminal <window or label> --json` | `{"ok": true}` |
| `daifuku stop-agents [--dry-run] --json` | the terminals it asked to close and the processes the agents left, below |
| `daifuku demo --json` | `{"ok": true}` |
| `daifuku spawn [--cell [n]] --json` | the terminal it opened, below |
| `daifuku restore [--dry-run] [--forget] --json` | every peer it looked at, below |
| `daifuku label <window> [label] --json` | `{"ok": true}` |
| `daifuku wait <window or label> [--until state] [--timeout seconds] --json` | the terminal once it got there, below |
| `daifuku task --json` | every fleet's backlog, below |
| `daifuku task release <id> --json` | `{"ok": true}` |
| `daifuku events [--after n] --json` | the events logged, below |
| `daifuku events --follow [--after n] --json` | each new event on a line of its own, below |
| `daifuku reload --json` | `{"ok": true}` |
| `daifuku stop --json` | `{"ok": true}` |
| `daifuku install --dry-run --json` | the plan, below |
| `daifuku uninstall --dry-run --json` | the plan, below |

A command that worked exits with 0. The commands that talk to the daemon
(`status`, `open`, `snap`, `next`, `close`, `stop-agents`, `demo`, `spawn`, `restore`, `label`, `wait`, `task`, `events`, `reload` and `stop`) work
from any of your terminals when the daemon runs with your own rights, the
default. With `"elevated": true` in the config they need an administrator
terminal, with `--json` too. That is on purpose: an elevated daemon's
control pipe can open administrator terminals, so only elevated processes
may use it, `status` included
([security](security.md#talking-to-the-daemon)). From any other terminal
they fail with `not_elevated`, and the message ends with the command to run
in an administrator terminal. `status` is the exception: turned away there,
it prints the local status instead, with exit code 0.
`daifuku doctor --json` works from any terminal and says whether the daemon
runs.

### `status`

```json
{
  "result": "status",
  "version": "0.4.1",
  "config": "C:\\ProgramData\\Daifuku\\daifuku.json",
  "elevated": true,
  "hotkeys": ["ctrl + alt + return: open agents"],
  "fleets": [{ "name": "agents", "monitor": "portrait", "windows": [1050190, 984356] }],
  "agents": [
    {
      "window": 1050190,
      "title": "claude",
      "state": "waiting",
      "for_seconds": 73,
      "label": "agents 1",
      "fleet": "agents",
      "session_id": "0e9b6c1a-3f42-4a8e-9a77-2b1c5d6e7f80",
      "cwd": "C:\\src\\site",
      "last_tool": "Bash",
      "last_message": "the agent needs your permission to use Bash",
      "transcript_path": "C:\\src\\transcripts\\0e9b6c1a-3f42-4a8e-9a77-2b1c5d6e7f80.jsonl"
    }
  ],
  "usage": [{
    "account": "work",
    "folder": "C:\\Users\\you\\.claude-work",
    "age_seconds": 125,
    "five_hour": { "used": 80, "resets_at": 1792548000, "resets_in_seconds": 3720, "pace": 3 },
    "seven_day": { "used": 35, "resets_at": 1792839600, "resets_in_seconds": 274000, "pace": -12 },
    "warn": 77,
    "warning": true
  }]
}
```

| Field | Type | |
|---|---|---|
| `result` | string | Always `"status"`. |
| `version` | string | The daemon's version, which can differ from `daifuku`'s. |
| `config` | string | The config file the daemon read. |
| `elevated` | boolean | Whether the daemon runs elevated. |
| `hotkeys` | array of strings | Each hotkey and what it does. One another program holds ends in `: refused, another program holds it`. |
| `fleets` | array of objects | Every open fleet. |
| `fleets[].name` | string | The fleet's name, as `daifuku open` takes it. |
| `fleets[].monitor` | string | The monitor it is on. |
| `fleets[].windows` | array of integers | Its terminals' window handles, in cell order. |
| `agents` | array of objects | Every window an agent has reported from. |
| `agents[].window` | integer | The window handle. |
| `agents[].title` | string | Its title now. |
| `agents[].state` | string | `"waiting"`, `"failed"`, `"working"` or `"done"`. |
| `agents[].for_seconds` | integer | How long it has been in that state. |
| `agents[].label` | string | What to call it: the label `spawn` or `label` gave it, else its fleet's name and its number, such as `"agents 2"`. Left out when it has neither. |
| `agents[].fleet` | string | The fleet it is in; left out when it is in none. |
| `agents[].session_id` | string | The session whose state it shows, as the agent names it. |
| `agents[].cwd` | string | The folder that session works in. Left out until one of its hooks names it, as are the three below. |
| `agents[].last_tool` | string | The tool it ran last, or is running now. |
| `agents[].last_message` | string | The text of its last notification, such as what it asks permission for; at most 1024 bytes. |
| `agents[].transcript_path` | string | Its transcript file. |
| `agents[].account` | string | The account it runs on, by its name under `accounts`, from its status line or else its fleet; left out when neither names one. |
| `agents[].warning` | boolean | `true` when its account has reached its `warn`, so its border shows the warning colour; left out otherwise. |
| `peers` | array of objects | Every terminal `daifuku spawn` opened that is still open. |
| `peers[].window` | integer | Its window handle. |
| `peers[].label` | string | Its label; left out when it has none. |
| `peers[].directory` | string | The folder it was opened in. |
| `peers[].session_id` | string | The last session its agent reported, which `restore` resumes; left out until it reports. |
| `usage` | array of objects | Each account a status line reported for, most recent first (after 0.3.0). |
| `usage[].account` | string | Its name under `accounts`; left out for a folder no account names. |
| `usage[].folder` | string | Its Claude Code folder. |
| `usage[].age_seconds` | integer | How long ago a status line last reported it. |
| `usage[].five_hour`, `usage[].seven_day` | object | The 5-hour and the weekly window; left out when Claude Code has not reported it. |
| `...used` | integer | The share used, in percent, when it was reported. |
| `...resets_at` | integer | When the window starts over, in seconds since 1970. |
| `...resets_in_seconds` | integer | How long until then; left out once it has, when `used` no longer holds. |
| `...pace` | integer | How far use is ahead of an even spread over the window, in percentage points, negative to spare; left out once it has reset. |
| `usage[].warn` | integer | The account's `warn`; left out when it has none. |
| `usage[].warning` | boolean | Whether a window has reached `warn`. |

Every field is there even when the daemon is an older version that leaves
one out: `elevated` is then `false`, and `hotkeys`, `peers` and `usage` are
empty.

### `spawn`

```json
{ "result": "spawned", "window": 1050190, "label": "site", "fleet": "crew", "cell": 2 }
```

`window` is the new terminal's window handle, as `status` lists it. `label`
is left out when none was given. `fleet` and `cell` say which fleet's grid
it took a cell of, counted from 1, with `--cell`, and are left out without.
With `--cell` and every cell taken, it fails and opens nothing.

`daifuku close --terminal` fails with code `daemon` when no terminal goes by
that label or window, when a label names two, and when the terminal is
still open after the wait.

### `wait`

```json
{ "result": "waited", "window": 1050190, "state": "done", "label": "site", "session_id": "0e9b6c1a-3f42-4a8e-9a77-2b1c5d6e7f80", "seconds": 312 }
```

`wait` takes a window handle, in decimal or as `0x` and hex, or a label in
any case, and asks the daemon twice a second. `--until` is `turn` by
default: the end of the agent's next turn, anything but working once it has
been seen working, so a terminal that just opened and shows done before its
first task does not count. `stopped` is anything but working now; `done`,
`waiting` and `failed` wait for that state alone. `state` is the state
reached, `"exited"` when the agent ended and its terminal stayed open, or
`"closed"` when the terminal closed. Past `--timeout`, an hour by default, it fails with
`timeout`; a name no terminal has fails at once with `daemon`.

### `restore`

```json
{
  "result": "restored",
  "peers": [
    { "label": "site", "directory": "C:\\src\\site", "session_id": "0e9b6c1a-3f42-4a8e-9a77-2b1c5d6e7f80", "window": 1050190 },
    { "directory": "C:\\src\\gone", "session_id": "5d1e7a20-8c3b-4f6d-b1a9-0c2e4f6a8b13", "error": "there is no folder C:\\src\\gone" }
  ]
}
```

| Field | Type | |
|---|---|---|
| `peers` | array of objects | Every peer from before Windows restarted that has a session to resume. Empty when there is none. |
| `peers[].label` | string | Its label; left out when it has none. |
| `peers[].directory` | string | The folder it works in. |
| `peers[].session_id` | string | The session it resumes, the last one seen in its terminal. |
| `peers[].window` | integer | The terminal it opened in. Left out with `--dry-run` and `--forget`, and when it did not open. |
| `peers[].error` | string | Why it did not open; it stays for the next `restore`. |

### `stop-agents`

```json
{
  "result": "stopped",
  "dry_run": false,
  "terminals": [
    { "window": 1050190, "label": "crew 1", "fleet": "crew", "cell": 1, "session_id": "0e9b6c1a-3f42-4a8e-9a77-2b1c5d6e7f80", "directory": "C:\\src\\site", "kept": true, "closed": true },
    { "window": 2098766, "label": "docs", "directory": "C:\\src\\docs", "kept": false, "closed": true }
  ],
  "leftovers": [
    { "pid": 4120, "name": "powershell.exe", "window": 1050190, "ended": false },
    { "pid": 7352, "name": "node.exe", "window": 1050190, "ended": true },
    { "pid": 6204, "name": "viewer.exe", "window": 1050190, "ended": false, "reason": "it shows a window of its own" }
  ]
}
```

It closes every fleet's terminals and every terminal `spawn` opened, the
ones in no fleet too, and keeps each session for `restore`. Terminals
Daifuku did not open stay open. It waits up to five seconds for the
terminals to close, then up to five more for what the agents started to end
with them, and ends what is still running.

A program that shows a window of its own, and all it started, is left
running: it may be a browser or an editor that is in use.

| Field | Type | |
|---|---|---|
| `dry_run` | boolean | `true` when it only says what it would do. |
| `terminals` | array of objects | Every terminal it asked to close, or would ask. Empty when none is open. |
| `terminals[].window` | integer | Its window. |
| `terminals[].label` | string | What `status` calls it; left out when it has no name. |
| `terminals[].fleet` | string | The fleet it is in; left out for a terminal in none. |
| `terminals[].cell` | integer | Its cell in that fleet, counted from 1. |
| `terminals[].session_id` | string | The session its agent last reported; left out until it reports. |
| `terminals[].directory` | string | For a fleet's own terminal, the folder its agent last reported; for one `spawn` opened, the folder it opened in. |
| `terminals[].kept` | boolean | Whether `restore` opens it again on that session, in that folder and in its cell. A fleet's own terminal is kept only when its agent reported the session from inside it, and the folder is the one the fleet starts in or one inside it. |
| `terminals[].closed` | boolean | Whether it closed. `false` with `--dry-run`, and when Windows Terminal asked first whether to close its tabs. |
| `leftovers` | array of objects | The agents' own processes in the terminals that closed, what they started and so on down, whether still running or not. With `--dry-run`, every one that runs now. |
| `leftovers[].pid` | integer | Its process id. |
| `leftovers[].name` | string | Its program's file name. |
| `leftovers[].window` | integer | The terminal whose agent started it. |
| `leftovers[].ended` | boolean | Whether `stop-agents` ended it. `false` for one that ended with its terminal. |
| `leftovers[].reason` | string | Why it still runs: it shows a window of its own, or Windows refused, as for an elevated process from a terminal that is not. Left out otherwise. |

A process is found from the agent's own process, as its hook named it,
while the agent still runs. One whose parent had already exited is not:
start such work in a fleet with a `priority`, whose group ends with its
terminal ([configuration](configuration.md)).

### `task`

```json
{
  "result": "tasks",
  "fleets": [
    {
      "fleet": "crew",
      "backlog": "C:\\work\\BACKLOG.md",
      "tasks": [
        { "id": "A1", "title": "Write the setup guide", "status": "open", "handed_to": { "window": 1050190, "label": "crew 1", "session_id": "0e9b6c1a-3f42-4a8e-9a77-2b1c5d6e7f80", "age_seconds": 640 } },
        { "id": "A2", "title": "Check the links", "status": "open" }
      ],
      "next": "A2"
    }
  ]
}
```

| Field | Type | |
|---|---|---|
| `fleets` | array of objects | Every fleet with a `backlog`, in the config's order. Empty when there is none. |
| `fleets[].fleet` | string | The fleet's name. |
| `fleets[].backlog` | string | Its backlog, as the config names it. |
| `fleets[].error` | string | Why the file could not be read; `tasks` is empty then. |
| `fleets[].tasks` | array of objects | Its open tasks and the ones handed out, in the file's order. |
| `fleets[].tasks[].id` | string | The task's id. |
| `fleets[].tasks[].title` | string | The rest of its heading's first part. |
| `fleets[].tasks[].status` | string | Its `status:` field. |
| `fleets[].tasks[].handed_to` | object | Who has it; left out when nobody does. `window` and `label` are the terminal and its label, left out once it closed; `session_id` is the session it went to, and `age_seconds` how long ago. |
| `fleets[].next` | string | The task that goes out next; left out when there is none. |
| `fleets[].stopped` | string | Why no task goes out now, as the account's `stop_at` says, such as `5h 80%, stops at 75%`; left out while tasks go out. |

### `events`

What the daemon noticed since it started, the newest 256 kept: a window of
an account reached the share its `stop_at` stops at, started over, or may
take more as its stop rose; or an agent in a fleet terminal finished its
turn holding no task while its backlog has one for it, which it gets only
when it stops again. A window's first sight after a start of the daemon
tells nothing.

```json
{
  "result": "events",
  "events": [
    { "seq": 1, "age_seconds": 640, "event": "limit", "account": "work", "folder": "C:\\Users\\you\\.claude-work", "limit": "five_hour", "used": 76, "share": 75 },
    { "seq": 2, "age_seconds": 80, "event": "top-up", "account": "work", "folder": "C:\\Users\\you\\.claude-work", "limit": "five_hour", "used": 76, "share": 91, "resets_in_seconds": 3540 },
    { "seq": 3, "age_seconds": 78, "event": "idle", "window": 1050190, "label": "crew 1", "fleet": "crew", "task": "A2" }
  ],
  "last": 3,
  "started_at": 1792500000
}
```

| Field | Type | |
|---|---|---|
| `events` | array of objects | The events numbered above `--after`, all without it, oldest first. |
| `events[].seq` | number | The event's number, counted from 1 since the daemon started. |
| `events[].age_seconds` | number | How long ago it happened. |
| `events[].event` | string | `limit`, `reset`, `top-up` or `idle`. |
| `events[].account` | string | `limit`, `reset`, `top-up`: the account's name under `accounts`; left out for a folder none names. |
| `events[].folder` | string | `limit`, `reset`, `top-up`: the account's folder. |
| `events[].limit` | string | `limit`, `reset`, `top-up`: the window, `five_hour` or `seven_day`. |
| `events[].used` | number | `limit`, `top-up`: its share used, in percent. `reset`: the new window's, once a status line reported it, else left out. |
| `events[].share` | number | `limit`: the share it stops at. `top-up`: the one it stops at now, left out when nothing stops it. |
| `events[].resets_in_seconds` | number | `top-up`: how long until the window starts over. |
| `events[].window` | number | `idle`: the terminal's window. |
| `events[].label` | string | `idle`: its label, else its fleet and number. |
| `events[].fleet` | string | `idle`: its fleet. |
| `events[].task` | string | `idle`: the id of the task it would get. |
| `last` | number | The number of the latest event, 0 while there is none. Pass it as `--after` to get only the ones that come after. |
| `started_at` | number | When the daemon started, in seconds since 1970. A new value is a new daemon, whose events are numbered from 1 again. |

With `--follow`, each line is one of `events[]` as it comes, starting from
now, or after `--after` when it is given. It asks the daemon every two
seconds and starts over from 1 when the daemon did, so a restart loses
nothing that the new daemon logged. When the daemon cannot be asked, it
prints the error and exits.

### `status` from a terminal that is not elevated, when the daemon does not answer it

As when the daemon runs elevated, or does not run.

```json
{
  "result": "local_status",
  "version": "0.4.1",
  "installed": true,
  "installed_version": "0.4.1",
  "daemon_running": true,
  "logon_task": true,
  "config": "C:\\ProgramData\\Daifuku\\daifuku.json",
  "config_exists": true,
  "config_valid": true,
  "config_error": null,
  "live_state": "needs_administrator_terminal",
  "message": "fleets, agents and hotkeys need an administrator terminal: open Terminal as administrator (Win+X, then Terminal (Admin)) and run: daifuku status --json"
}
```

What can be told without the daemon, read from files, the registry, the
process list and Task Scheduler. Nothing in it opens the daemon's pipes.
Fleets, agents and hotkeys come only from the daemon, so they are not in it;
`live_state` says so. Tell the two shapes apart by `result`.

| Field | Type | |
|---|---|---|
| `result` | string | Always `"local_status"`. |
| `version` | string | This `daifuku`'s version. |
| `installed` | boolean | Whether `daifuku.exe` and `daifukud.exe` are in `C:\Program Files\Daifuku`. |
| `installed_version` | string or null | The installed version: this `daifuku`'s when it is the installed copy, else the one in the Settings > Apps entry. `null` when not installed or not known, as after an install older than the entry. |
| `daemon_running` | boolean | Whether a `daifukud.exe` process runs in this session, from the process list. |
| `logon_task` | boolean | Whether this user's logon task is registered, or the one task earlier installs shared runs for this user. |
| `config` | string or null | The config file, which need not exist; `null` when there is no ProgramData folder. |
| `config_exists` | boolean | Whether the config file exists. Without one the defaults apply. |
| `config_valid` | boolean | Whether the daemon would load it, read the way `config validate` reads it; `true` when there is no file. |
| `config_error` | string or null | What is wrong with the config, on one line; `null` when it is valid. |
| `live_state` | string | `"not_running"` when Daifuku is installed, the config asks for an ordinary daemon and none runs: the next `daifuku` command or agent hook starts it. Otherwise `"needs_administrator_terminal"`: fleets, agents and hotkeys need an administrator terminal. |
| `message` | string | The same for a person, ending in the command to run. |

The exit code is 0 whatever the fields say. Without `--json` it prints the
same on one line, such as `daifuku 0.4.1 installed, daemon running, logon
task registered, config valid (C:\ProgramData\Daifuku\daifuku.json);
fleets, agents and hotkeys need an administrator terminal: ...`.

### `config validate`

```json
{ "ok": true, "path": "C:\\ProgramData\\Daifuku\\daifuku.json" }
```

Checks the config file in use, or the file named after `validate`, the way
the daemon reads it: the same decoding, the same parser and the same rules.
`schema.json` is generated from the parser's types, so a file that passes
here also passes the schema. It needs no daemon and no administrator
terminal, and changes nothing. A file that does not pass is the error
`invalid_config`, whose message says what is wrong and, for a syntax or key
error, the line and column. Without `--json` it prints `ok    <file>` or
`FIX   <file>: <what is wrong>`, and exits 1 for a file that does not pass.

### `doctor`

```json
{
  "version": "0.4.1",
  "ok": false,
  "checks": [
    { "name": "installed in Program Files", "status": "ok", "fix": null },
    { "name": "daemon running", "status": "fail", "fix": "run `daifuku reload`, which starts it" }
  ],
  "notes": ["the daemon runs elevated and answers only an administrator terminal: run doctor from one to check its rights and hotkeys too"],
  "logs": "C:\\ProgramData\\Daifuku\\logs"
}
```

| Field | Type | |
|---|---|---|
| `version` | string | This `daifuku`'s version. |
| `ok` | boolean | Whether every check passed. |
| `checks` | array of objects | Every check, in the order it ran. |
| `checks[].name` | string | What was checked, the text the plain output prints. |
| `checks[].status` | string | `"ok"` or `"fail"`. |
| `checks[].fix` | string or null | What to do about a failed check; `null` when it passed. |
| `notes` | array of strings | Remarks that are not checks, such as the ones skipped in a terminal that is not elevated. |
| `logs` | string or null | The folder the logs are in. |

The report prints whether or not the checks pass. The exit code is 0 when
every check passed and 1 when one failed, as without `--json`.

### `install --dry-run` and `uninstall --dry-run`

What the real command would do, worked out by the same code that does it,
and nothing changed. A dry run needs no administrator terminal and takes the
real command's other options: `--no-hooks` and `--no-start` for install,
`--purge` for uninstall.

```json
{
  "dry_run": true,
  "command": "install",
  "elevated": false,
  "steps": [
    { "action": "stop_daemon", "target": "daifukud", "detail": "if it runs, so its files can be replaced" },
    { "action": "copy", "target": "C:\\Program Files\\Daifuku\\daifuku.exe", "detail": "from C:\\Users\\you\\Downloads\\daifuku.exe" },
    { "action": "copy", "target": "C:\\Program Files\\Daifuku\\daifukud.exe", "detail": "carried inside C:\\Users\\you\\Downloads\\daifuku.exe" },
    { "action": "add_app_entry", "target": "HKLM\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\Daifuku", "detail": "Settings > Apps > Installed apps lists Daifuku 0.4.1" },
    { "action": "add_to_path", "target": "C:\\Program Files\\Daifuku", "detail": "the machine PATH" },
    { "action": "start_task", "target": "\\Daifuku\\Daemon-S-1-5-21-...", "detail": null }
  ],
  "notes": ["the real `daifuku install` needs an administrator terminal"]
}
```

| Field | Type | |
|---|---|---|
| `dry_run` | boolean | Always `true`. |
| `command` | string | `"install"` or `"uninstall"`. |
| `elevated` | boolean | Whether this terminal is elevated, as the real command needs. |
| `steps` | array of objects | Every change, in the order the real command makes it. A step that would change nothing, such as taking a folder off `PATH` that is not on it, is left out. |
| `steps[].action` | string | What happens, from the list below. |
| `steps[].target` | string | The file, folder, task or registry key it happens to. |
| `steps[].detail` | string or null | More about it, such as where a file is copied from or why hooks stay as they are. |
| `notes` | array of strings | Remarks that are not steps, such as `daifukud.exe` missing next to a `daifuku.exe` that does not carry it, or a missing Windows Terminal. |

| Action | Command | |
|---|---|---|
| `stop_daemon` | both | Stop a running daemon. |
| `copy` | install | Copy one binary into Program Files: this program as `daifuku.exe`, and the daemon from inside it or from next to it. |
| `add_app_entry` | install | List Daifuku in Settings > Apps > Installed apps, whose Uninstall runs the installed `daifuku.exe`, or update the entry there is. |
| `add_to_path` | install | Add the install folder to the machine `PATH`. |
| `keep_path` | install | It is on the machine `PATH` already. |
| `create_folder` | install | Create the data folder, locked to administrators. |
| `replace_folder` | install | Delete a data folder that is not locked, config and all, and create it anew. |
| `keep_folder` | both | Keep the data folder as it is. |
| `write_config` | install | Write the starter config; `detail` is `the starter config, writable by you` or `the starter config, writable by administrators only`. |
| `keep_config` | both | Keep the config there is; `detail` is `writable by you` or `writable by administrators only`, and in an uninstall `no longer writable by you`. |
| `register_task` | install | Register this user's logon task. |
| `remove_task` | both | Remove this user's logon task, or the one task earlier installs shared. |
| `start_task` | install | Start the daemon now. |
| `add_hooks` | install | Add or update Daifuku's hooks in an agent's settings. |
| `keep_hooks` | install | The settings have the hooks already. |
| `leave_hooks` | both | The settings stay as they are; `detail` says why. |
| `remove_hooks` | uninstall | Remove Daifuku's hooks from an agent's settings. |
| `remove_from_path` | uninstall | Take the install folder off the machine `PATH`. |
| `remove_folder` | uninstall | Delete the install folder, or with `--purge` the data folder. |
| `remove_app_entry` | uninstall | Take Daifuku out of Settings > Apps > Installed apps. |

The exit code is 0 when the plan was worked out, whatever it says.

## Errors

Every error with `--json` is one document of this shape on standard output,
with a non-zero exit code:

```json
{ "error": { "code": "daemon_not_running", "message": "Daifuku is not running; `daifuku doctor` says why" } }
```

| Code | Exit code | When |
|---|---|---|
| `usage` | 2 | The command line did not parse and had `--json` in it, such as `--json` before the command's name or on `install` without `--dry-run`. `--help` and `--version` print their text as always. |
| `daemon_not_running` | 1 | No daemon answers in this session. `status` says so only from an administrator terminal; from any other it prints the local status. |
| `not_elevated` | 1 | The terminal is not elevated, and the daemon runs elevated, so only an elevated process may talk to it. The message ends with `run: ` and the command to run in an administrator terminal, such as `daifuku open --json`. Not from `status`, which prints the local status instead. |
| `ipc` | 1 | The pipe to the daemon failed, or the daemon's answer was missing or did not read. |
| `daemon` | 1 | The daemon answered that the request did not work, such as a fleet name the config does not have. |
| `timeout` | 1 | `wait` gave up: the terminal did not get to the state within `--timeout` seconds. |
| `config` | 1 | There is no config file to open or check, it could not be read, there is no ProgramData folder, no editor started, or Windows was not given permission to start one. |
| `invalid_config` | 1 | `config validate` found a file the daemon would not load: not JSON, an unknown key, a value of the wrong type, or a rule such as a hotkey that does not parse or two fleets with one name. |
| `unsupported` | 1 | A build for a system other than Windows, except `config validate` with a file named. |
| `internal` | 1 | Something that should not fail did. |

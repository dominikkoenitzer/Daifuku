# Commands

After install, `daifuku` is on the machine `PATH` (`C:\Program Files\Daifuku`) in every new terminal. `daifuku` with no command prints the help in a terminal; double-clicked in Explorer it offers to install.

## Daemon commands

These talk to the daemon over a named pipe. A daemon with the user's own rights, the default, lets in the user's own terminals. An elevated one (`"elevated": true` in the config) lets in only elevated processes, so then run them from an administrator terminal. Exit code 0 on success, 1 on failure. `daifuku status` never fails for that reason: when an elevated daemon turns away a terminal that is not elevated, it prints the local status below, with exit code 0. The two common errors:

```text
daifuku: the daemon runs elevated and answers only an administrator terminal: open Terminal as administrator (Win+X, then Terminal (Admin)) and run: daifuku open
daifuku: Daifuku is not running; `daifuku doctor` says why
```

| Command | Does |
|---|---|
| `daifuku open [fleet]` | Open a fleet, or bring it back if it is open. Without a name, the first fleet in the config. |
| `daifuku snap` | Put every fleet terminal back in its cell. |
| `daifuku close [fleet]` | Close a fleet's terminals. Without a name, the first fleet. |
| `daifuku close --all` | Close every open fleet. |
| `daifuku close --terminal <window or label>` | Close one terminal Daifuku knows: a fleet's, a spawned one or one an agent reports from. A label is looked for first, then a window handle. Fails when the terminal is still open after the wait, as when Windows Terminal asks about closing its tabs. |
| `daifuku next` | Focus the agent that has waited longest, then the failed ones. |
| `daifuku demo` | Open six scripted demo agents, to see Daifuku without a real agent. |
| `daifuku spawn [--dir D] [--label L] [--resume ID] [--fleet F] [--monitor M] [--area A] [--priority P] [--cell [N]]` | Open one agent terminal on its own, from a fleet's command, profile and administrator setting (the first fleet by default), in `--dir` (this folder by default). `--monitor` and `--area` put it there. Without them it takes the first free cell of its fleet's grid while the fleet is open, and otherwise fills the fleet's monitor and area; with no fleet in the config it stays where Windows Terminal opens it. `--cell` puts it in the first free cell of its fleet's grid, open or not, `--cell 2` in that cell while it is free. In a cell it is one of the fleet's terminals, which `snap`, `close <fleet>` and the close key treat as the others, also after the daemon restarts. With `--cell` and every cell taken it fails and opens nothing. Its environment is the user's own, without any `CLAUDE*` variable. `--json` prints `{"result":"spawned","window":...,"label":...}`, with `"fleet"` and `"cell"` when it took a cell, `"monitor"` and `"area"` when it was put on a monitor without a cell. |
| `daifuku restore [--dry-run] [--forget]` | Open again the spawned terminals that were open when Windows last stopped, and the ones closed with their fleet by `close` or the close key, each resuming its last session in its own folder (`--resume <id>` for Claude Code, `codex resume <id>` for Codex). One that had a cell goes back to it, or beside its fleet when the cell is taken. Never a session that runs already. `--dry-run` lists them, `--forget` drops them. |
| `daifuku label <window> [label]` | Give a terminal a label, which `status` shows and `wait` takes; without a label, take it away. |
| `daifuku wait <window or label> [--until turn\|stopped\|done\|waiting\|failed] [--timeout S]` | Wait for an agent. `turn`, the default, is the end of its next turn: working first, then anything else. Prints the state reached, `exited` when the agent ended in a terminal that stays open, `closed` when the terminal closed. Exit 1 with code `timeout` after `--timeout` seconds (3600). |
| `daifuku status` | Show the daemon version, whether it runs elevated, the config in use, every hotkey (refused ones included), open fleets and every agent. |
| `daifuku status --json` | The same as the raw JSON reply, for scripts. |
| `daifuku reload` | Re-read the config file now. |
| `daifuku stop` | Take the borders down and stop the daemon. |

### `daifuku status` output

```text
daifuku <version>, elevated
config  <where the config was read from>

hotkeys
  <one line per hotkey and what it does>

fleets
  <name> on <monitor>: <n> terminals

agents
  <state>  <time in state>  <window title>  (<window handle>)
```

Agents are listed most urgent first: waiting, failed, working, done, and within each state the one in it longest first. Times read `41s`, `3m 12s`, `2h 5m`. `none open` and `none reporting` mean there is nothing in that list. A hotkey another program holds ends in `: refused, another program holds it`.

### `daifuku status --json`

Prints one JSON object. Every character past ASCII is written as a `\u` escape, so it reads the same in any console code page. Check the exit code first: 0 only when the reply is not an error.

Fields of a status reply: `result` (`"status"`), `version`, `config`, `elevated`, `hotkeys` (list of strings), `fleets` (each `name`, `monitor`, `windows`: window handles in cell order), `agents` (each `window`, `title`, `state`: one of `done`, `working`, `failed`, `waiting`, `for_seconds`, and when known `label`, `fleet`, `session_id`), `peers` (each `window`, `label`, `directory`, `session_id`: the terminals `spawn` opened). An error reply is `{"result":"error","message":"..."}`.

### `daifuku status` from a terminal that is not elevated, when the daemon does not answer it

One line, for example:

```text
daifuku 0.4.1 installed, daemon running, logon task registered, config valid (C:\ProgramData\Daifuku\daifuku.json); fleets, agents and hotkeys need an administrator terminal: open Terminal as administrator (Win+X, then Terminal (Admin)) and run: daifuku status
```

With `--json`, one object: `result` (`"local_status"`), `version` (this `daifuku`), `installed`, `installed_version` (null when not known), `daemon_running` (a `daifukud.exe` process in this session), `logon_task`, `config` (the path), `config_exists`, `config_valid` (true when there is no file: the defaults apply), `config_error` (null when valid), `live_state` and `message`. Exit code 0 whatever it says. `live_state` is `"not_running"` when an ordinary daemon is installed but not running: the next agent hook or daemon command other than `status` and `stop` starts it, and `daifuku reload` does so without opening anything. Otherwise it is `"needs_administrator_terminal"`: for fleets, agents and hotkeys, give the user the command to run in an administrator terminal.

## Process groups

These need no daemon and work in any terminal.

| Command | Does |
|---|---|
| `daifuku group [--priority idle\|below-normal\|normal] -- <program> [args]` | Run a program in a job object: it and everything it starts run at that priority, cannot raise it, and end when the program ends or its terminal closes. Ctrl+C reaches the program. The exit code is the program's. |
| `daifuku heavy [--priority P] -- <program> [args]` | The same at idle priority by default, and one at a time with every other heavy job in the session. Run builds and test suites this way: `daifuku heavy -- cargo test`. A heavy job started inside one runs at once. |

The program is an executable found on `PATH`, not a PowerShell line; for one, run `daifuku heavy -- pwsh -NoProfile -Command "..."`.

## Setup commands

| Command | Does | Needs |
|---|---|---|
| `daifuku install` | Install for this user, or update in place. | administrator terminal |
| `daifuku install --no-hooks` | Install without touching Claude Code's or Codex's settings. | administrator terminal |
| `daifuku install --no-start` | Stop a running daemon and do not start it now; an ordinary daemon starts on demand with the next command or hook, an elevated one at the next sign-in. | administrator terminal |
| `daifuku uninstall` | Remove the program, the `PATH` entry, the logon task and Daifuku's hooks. Keeps config, logs and saved state. | administrator terminal |
| `daifuku uninstall --purge` | Also delete `C:\ProgramData\Daifuku` (config, logs and saved state) and `%LocalAppData%\Daifuku` (the logs and saved state of a daemon with the user's own rights). | administrator terminal |
| `daifuku doctor` | Check the setup and print a fix for each problem. Exit code 0 when every check passes, 1 otherwise. | any terminal; for an elevated daemon, its rights and hotkey checks only from an administrator terminal |
| `daifuku config` | Open the config file in the editor for `.json` files, or Notepad. Asks Windows for permission first when the terminal is not elevated. Fails when no config file exists yet (`daifuku install` writes one). | any terminal |
| `daifuku config --path` | Print the config file's path. | any terminal |
| `daifuku schema` | Print the config file's JSON schema. | any terminal |

Install and uninstall print one line per step, a label padded to 13 characters, then the detail, for example `installed    C:\Program Files\Daifuku`, `hooks        added 17 to C:\Users\<you>\.claude\settings.json`, `kept         C:\ProgramData\Daifuku (config, logs and saved state; --purge removes them)`. Errors go to standard error as `daifuku: <message>` with exit code 1.

## Hidden commands

`daifuku hook` is what agents call. It reads one hook event on standard input, never prints and always exits 0. Do not run it by hand.

# Commands

After install, `daifuku` is on the machine `PATH` (`C:\Program Files\Daifuku`) in every new terminal. `daifuku` with no command prints the help in a terminal; double-clicked in Explorer it offers to install.

## Daemon commands

These talk to the daemon over a named pipe that only elevated processes may open. Run them from an administrator terminal. Exit code 0 on success, 1 on failure. `daifuku status` is the exception: from a terminal that is not elevated it does not open the pipe and prints the local status below, with exit code 0. The two common errors:

```text
daifuku: the daemon answers only an administrator terminal: open Terminal as administrator (Win+X, then Terminal (Admin)) and run: daifuku open
daifuku: Daifuku is not running; `daifuku doctor` says why
```

| Command | Does |
|---|---|
| `daifuku open [fleet]` | Open a fleet, or bring it back if it is open. Without a name, the first fleet in the config. |
| `daifuku snap` | Put every fleet terminal back in its cell. |
| `daifuku close [fleet]` | Close a fleet's terminals. Without a name, the first fleet. |
| `daifuku close --all` | Close every open fleet. |
| `daifuku next` | Focus the agent that has waited longest, then the failed ones. |
| `daifuku demo` | Open six scripted demo agents, to see Daifuku without a real agent. |
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

Fields of a status reply: `result` (`"status"`), `version`, `config`, `elevated`, `hotkeys` (list of strings), `fleets` (each `name`, `monitor`, `windows`: window handles in cell order), `agents` (each `window`, `title`, `state`: one of `done`, `working`, `failed`, `waiting`, `for_seconds`). An error reply is `{"result":"error","message":"..."}`.

### `daifuku status` from a terminal that is not elevated

One line, for example:

```text
daifuku 0.1.2 installed, daemon running, logon task registered, config valid (C:\ProgramData\Daifuku\daifuku.json); fleets, agents and hotkeys need an administrator terminal: open Terminal as administrator (Win+X, then Terminal (Admin)) and run: daifuku status
```

With `--json`, one object: `result` (`"local_status"`), `version` (this `daifuku`), `installed`, `installed_version` (null when not known), `daemon_running` (a `daifukud.exe` process in this session), `logon_task`, `config` (the path), `config_exists`, `config_valid` (true when there is no file: the defaults apply), `config_error` (null when valid), `live_state` (always `"needs_administrator_terminal"`) and `message`. Exit code 0 whatever it says. For fleets, agents and hotkeys, give the user the command to run in an administrator terminal.

## Setup commands

| Command | Does | Needs |
|---|---|---|
| `daifuku install` | Install for this user, or update in place. | administrator terminal |
| `daifuku install --no-hooks` | Install without touching Claude Code's or Codex's settings. | administrator terminal |
| `daifuku install --no-start` | Stop a running daemon and do not start it again; it starts at the next sign-in. | administrator terminal |
| `daifuku uninstall` | Remove the program, the `PATH` entry, the logon task and Daifuku's hooks. Keeps config, logs and saved state. | administrator terminal |
| `daifuku uninstall --purge` | Also delete `C:\ProgramData\Daifuku`: config, logs and saved state. | administrator terminal |
| `daifuku doctor` | Check the setup and print a fix for each problem. Exit code 0 when every check passes, 1 otherwise. | any terminal; elevation and hotkey checks only from an administrator terminal |
| `daifuku config` | Open the config file in the editor for `.json` files, or Notepad. Asks Windows for permission first when the terminal is not elevated. Fails when no config file exists yet (`daifuku install` writes one). | any terminal |
| `daifuku config --path` | Print the config file's path. | any terminal |
| `daifuku schema` | Print the config file's JSON schema. | any terminal |

Install and uninstall print one line per step, a label padded to 13 characters, then the detail, for example `installed    C:\Program Files\Daifuku`, `hooks        added 17 to C:\Users\<you>\.claude\settings.json`, `kept         C:\ProgramData\Daifuku (config, logs and saved state; --purge removes them)`. Errors go to standard error as `daifuku: <message>` with exit code 1.

## Hidden commands

`daifuku hook` is what agents call. It reads one hook event on standard input, never prints and always exits 0. Do not run it by hand.

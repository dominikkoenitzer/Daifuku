# Security

Daifuku's daemon runs as administrator. It has to: Windows does not let an
ordinary process move an administrator's window or draw a frame beside it,
and fleets of administrator terminals are the common case. That makes the
daemon worth attacking, because whatever can change what it starts gets
administrator rights for nothing. This page is the threat model: what an
attacker without administrator rights could try, and what stops it.

The attacker assumed here is any process running as the signed-in user at
normal integrity: a malicious npm package, a compromised extension, a script
that ran by mistake. It can write the user's profile, start programs and send
input to ordinary windows. It cannot write where only administrators can.

## What the daemon starts, and who decides it

| Asset | Where | Why an ordinary process cannot change it |
|---|---|---|
| `daifuku.exe`, `daifukud.exe` | `C:\Program Files\Daifuku` | Program Files is writable by administrators only. The logon task starts the daemon from there. |
| The config, which names the command every fleet terminal runs | `C:\ProgramData\Daifuku\daifuku.json` | The installer gives the folder a protected access list (administrators and SYSTEM may write, users may read) and makes the Administrators group its owner and the owner of everything in it. Ownership matters: the owner of a file can always rewrite its access list, so a folder an ordinary process created before the install would otherwise stay open to it. |
| The logon task | `\Daifuku\Daemon` in Task Scheduler | Creating or changing a task that runs with highest privileges needs administrator rights. |
| Windows Terminal | its package folder under `WindowsApps` | Found through the package API, never through `PATH`, which contains folders the user can write. |
| PowerShell | `Program Files\PowerShell\7` or `System32` | Found through the known-folder API, never through `PATH` or environment variables. |

The command a fleet runs (`claude` by default) is resolved by that PowerShell
inside the terminal, from the user's own `PATH`. An administrator fleet
therefore runs the user's `claude` as administrator, which is what the user
asked for by setting `"admin": true`; Daifuku adds nothing to it.

## Talking to the daemon

There are two named pipes, per Remote Desktop session.

**The control pipe** takes commands: open a fleet, snap, stop. Only SYSTEM and
Administrators are in its access list, and it carries a high mandatory label,
so no process below high integrity can open it whatever else is true. The
command line also checks that whoever answers on the pipe runs elevated before
it sends anything, so a process that squatted the name cannot collect
commands.

**The hook pipe** takes state reports from agent hooks, which run at whatever
integrity their agent runs at. The signed-in user may write to it (a medium
label makes that possible for a normal process). A message names a window and
an event, and the worst a forged one can do is colour or clear a border: the
daemon only accepts a live, visible top-level window, never starts anything
because of a hook, and stops tracking new sessions past 512.

Both pipes are created with `FILE_FLAG_FIRST_PIPE_INSTANCE` and keep their
instances for the daemon's whole life, so the name is never free for another
process to take between two clients. Remote clients are rejected.

## Hotkeys

Hotkeys are registered with `RegisterHotKey`. Any process can synthesise the
key press, and the most it achieves is what the key does: open the configured
fleet, focus a terminal, snap the grid. It cannot change what a fleet runs, and
Windows does not let it type into the administrator terminals that result.

## What Daifuku does not do

- No network access, no telemetry, no update check.
- No code runs because of a hook message, a window title or anything else an
  agent or a terminal controls.
- The installer never removes or rewrites a hook it did not add, and never
  writes the settings file when nothing changed; `daifuku uninstall` removes
  exactly the hooks that run `daifuku.exe hook`.

## Checked on every push

CI installs Daifuku on a real Windows runner and asserts that the data folder
is owned by Administrators and not writable by users, that the task exists,
that a user's own hook survives next to Daifuku's, that a second install
changes nothing, and that the uninstall removes the task, the data and exactly
Daifuku's hooks.

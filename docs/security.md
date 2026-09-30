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
| The config, which names the command every fleet terminal runs | `C:\ProgramData\Daifuku\daifuku.json` | Administrators and SYSTEM may write the folder, users may only read it. Any user may create folders in ProgramData, so the installer keeps an existing `Daifuku` folder, and the config in it, only when it is a real folder (not a link) that Administrators or SYSTEM own and no one else may change: one an earlier install made. Anything else there is deleted without following links, and a new folder is created with its access list already in place. ProgramData itself is found on the drive Windows runs from, not through the environment, which the user can change. |
| The logon task | `\Daifuku\Daemon` in Task Scheduler | Creating or changing a task that runs with highest privileges needs administrator rights. Its definition passes through the locked data folder on the way in, never through the user's temp folder. |
| Windows Terminal | its package folder under `WindowsApps` | Found through the package API, never through `PATH`, which contains folders the user can write. Only a package the system installed from a signed source counts: one registered from loose files in developer mode can live in any folder. |
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

**The hook pipe** takes state reports from agent hooks. The signed-in user
may write data to it from a normal process (a medium label), and nothing
more: not create server instances of it, so no other process can listen in
and collect the hooks' reports. Processes below medium integrity cannot write
to it. A message names a window and an event, and the worst a forged one can
do is colour or clear a border: the daemon only accepts a live, visible
top-level window, never starts anything because of a hook, and stops tracking
new sessions past 512.

Both pipes are created with `FILE_FLAG_FIRST_PIPE_INSTANCE`, allow exactly
as many instances as the daemon creates, and keep them for the daemon's whole
life, so the name is never free for another process to take between two
clients. Clients connect for identification only: whatever answers on a pipe
can learn who the client is, never act as it. Remote clients are rejected.

Every exchange has a deadline. A hook has half a second from connecting to
deliver its line, a command client five seconds to send its request and five
to read the reply; past that the daemon hangs up. A process that opens every
hook instance and says nothing therefore holds them for a moment, not for
ever. A hook gives up just as fast on a pipe that does not take its line, so
a process posing as the daemon cannot make agents wait.

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
  exactly the hooks that run a program called `daifuku.exe` with `hook`.
- The installer runs as administrator but the profile is the user's, so it
  edits an agent's settings only when the file, with every link followed,
  is inside the profile.

## Checked on every push

CI installs Daifuku on a real Windows runner and asserts that the data folder
is owned by Administrators and not writable by users, that the task exists,
that a user's own hook survives next to Daifuku's, that a second install
changes nothing, and that the uninstall removes the task, the data and exactly
Daifuku's hooks.

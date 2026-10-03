# Security

Daifuku's daemon runs as administrator. It has to: Windows does not let an
ordinary process move an administrator's window or draw a frame beside it,
and fleets of administrator terminals are the common case. That makes the
daemon worth attacking, because whatever can change what it starts gets
administrator rights for nothing. This page is the threat model: what an
attacker without administrator rights could try, and what stops it. It
describes the code on `main`; several of these protections came after 0.1.1,
and the [changelog](../CHANGELOG.md) says which.

The attacker assumed here is any process running as the signed-in user at
normal integrity: a malicious npm package, a compromised extension, a script
that ran by mistake. It can write the user's profile, start programs and send
input to ordinary windows. It cannot write where only administrators can.

## What the daemon starts, and who decides it

| Asset | Where | Why an ordinary process cannot change it |
|---|---|---|
| `daifuku.exe`, `daifukud.exe` | `C:\Program Files\Daifuku` | Program Files is writable by administrators only. The logon task starts the daemon from there. |
| The config, which names the command every fleet terminal runs | `C:\ProgramData\Daifuku\daifuku.json` | Administrators and SYSTEM may write the folder, users may only read it. Any user may create folders in ProgramData, so the installer keeps an existing `Daifuku` folder, and the config in it, only when it is a real folder (not a link) that Administrators or SYSTEM own and no one else may change, as the installer leaves it. A folder whose owner or access list was changed since, so that someone else may change it, counts as one another program made. Anything else there is deleted without following links, and a new folder is created with its access list already in place. ProgramData itself is found on the drive Windows runs from, not through the environment, which the user can change. |
| The saved state: open fleets and what each agent is doing | `C:\ProgramData\Daifuku\state.json` | In the same locked folder as the config, written by the daemon alone. It is taken back only within the same start of Windows and for five minutes after it was written, and every window in it is checked again: a fleet's must still be a Windows Terminal window, an agent's a window a hook could name. |
| The logon task, one per user | `\Daifuku\Daemon-<user SID>` in Task Scheduler | Creating or changing a task that runs with highest privileges needs administrator rights. Its definition passes through the locked data folder on the way in, never through the user's temp folder. |
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

**The hook pipe** takes state reports from agent hooks. The signed-in user,
by their own SID, may open it to read and to write data from a normal process
(a medium label), and nothing more: not create server instances of it, so no
other process can listen in and collect the hooks' reports. The daemon never
writes on it, so there is nothing to read. Another person signed in at the
same time is not let in at all. Processes below medium integrity cannot
write to it.

A message names a window and an event. A forged one can set the state
Daifuku shows for any live top-level window: draw, recolour or remove its
border, play the chime when sound is on, list it in `daifuku status`, and put
it in the next key's queue, so that key brings it to the front. The daemon
never starts anything because of a hook, ignores session and subagent ids
longer than 128 bytes, lets one session have at most 32 threads waiting, and
stops tracking new sessions past 512.

Both pipes are created with `FILE_FLAG_FIRST_PIPE_INSTANCE`, allow exactly
as many instances as the daemon creates, and keep them for the daemon's whole
life, so the name is never free for another process to take between two
clients. Clients connect for identification only: whatever answers on a pipe
can learn who the client is, never act as it. Remote clients are rejected.

Every exchange has a deadline. A hook has half a second from connecting to
deliver its line, a command client five seconds to send its request and five
to read the reply; past that the daemon hangs up. A process that opens every
hook instance and says nothing therefore holds them for a moment, not for
ever. One that keeps reconnecting can still crowd hooks out while it runs,
which keeps borders from updating and does nothing more. A hook gives up
just as fast on a pipe that does not take its line, so a process posing as
the daemon cannot make agents wait.

## Hotkeys

Hotkeys are registered with `RegisterHotKey`. Any process can synthesise the
key press, and the most it achieves is what the key does: open the configured
fleet, focus a window a hook reported as waiting or failed, snap the grid. It
cannot change what a fleet runs, and Windows does not let it type into the
administrator terminals that result.

## What Daifuku does not do

- No network access, no telemetry, no update check.
- No code runs because of a hook message, a window title or anything else an
  agent or a terminal controls.
- The installer changes only hooks that run a program called `daifuku.exe`
  with `hook`, pointing them at the installed copy, and never writes the
  settings file when nothing changed; `daifuku uninstall` removes exactly
  those hooks. Every other hook is left as it is.
- The installer runs as administrator but the profile is the user's, so it
  edits an agent's settings only when the file, with every link followed,
  is inside the profile, writes the new version into a fresh file that
  cannot be a link, and makes the whole edit with the user's own rights,
  borrowed from the desktop shell, so a link swapped in half way cannot
  lead the write anywhere the user may not write. With no desktop shell to
  borrow them from, as on a build server or while Explorer is not running,
  it leaves the file alone and says so.

## Checked on every push

CI installs Daifuku on a real Windows runner and asserts that the data folder
is owned by Administrators and not writable by users, that the task exists,
that a user's own hook survives next to Daifuku's, that a second install
changes nothing, and that `uninstall --purge` removes the binaries, the task,
the data and exactly Daifuku's hooks.

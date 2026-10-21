# Security

Daifuku's daemon runs with the signed-in user's own rights. Only when the
config sets `"elevated": true` does it run as administrator, because Windows
does not let an ordinary process move an administrator's window or draw a
frame beside it, so fleets of administrator terminals need it. An elevated
daemon is worth attacking, because whatever can change what it starts gets
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
| `daifuku.exe`, `daifukud.exe` | `C:\Program Files\Daifuku` | Program Files is writable by administrators only. The command line starts the daemon from there on demand, and a logon task does when the config asks for an elevated one. |
| The config, which names the command every fleet terminal runs | `C:\ProgramData\Daifuku\daifuku.json` | Administrators and SYSTEM may write the folder, users may only read it. Any user may create folders in ProgramData, so the installer keeps an existing `Daifuku` folder, and the config in it, only when it is a real folder (not a link) that Administrators or SYSTEM own and no one else may change, as the installer leaves it. A folder whose owner or access list was changed since, so that someone else may change it, counts as one another program made. Anything else there is deleted without following links, and a new folder is created with its access list already in place. ProgramData itself is found on the drive Windows runs from, not through the environment, which the user can change. |
| The saved state: open fleets and what each agent is doing | `C:\ProgramData\Daifuku\state-<SID>.json`, one per user (`state.json` up to 0.1.3, read once when the user's own is missing) | In the same locked folder as the config, written by the daemon alone. It is taken back only within the same start of Windows and for five minutes after it was written, and every window in it is checked again: a fleet's must still be a Windows Terminal window, an agent's a window a hook could name. |
| The peers `daifuku spawn` opened: folder, label, fleet, place, the cell of a fleet each is in and the last session id | `C:\ProgramData\Daifuku\peers-<SID>.json`, one per user | In the same locked folder, written by the daemon alone. `daifuku restore` puts these into command lines the elevated daemon starts, so a session id is kept only when it is letters, digits, `-` and `_`, only from a hook that ran inside that peer's terminal (below), and is checked again before it goes into a command line. Only the peers open when Windows last stopped are restored, and never a session that runs already. A peer whose terminal is still open after the daemon restarts goes back into its cell only when that window is still a Windows Terminal window of this start of Windows and the cell is free. |
| The entry in Settings > Apps > Installed apps | `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Daifuku` | Writable by administrators only. Its Uninstall runs `daifuku.exe` in Program Files, which asks for administrator rights through the normal Windows prompt. |
| The logon task, written only for an elevated daemon, one per user | `\Daifuku\Daemon-<user SID>` in Task Scheduler | An ordinary daemon has none: it is started on demand with the caller's own rights, so there is no auto-start to tamper with. A task is written only when the config sets `"elevated": true`, and it runs with highest privileges. Creating or changing such a task needs administrator rights, and the config that asks for one is writable by administrators only. Its definition passes through the locked data folder on the way in, never through the user's temp folder. An install removes a task an earlier version wrote for an ordinary daemon. |
| Windows Terminal | its package folder under `WindowsApps` | Found through the package API, never through `PATH`, which contains folders the user can write. Only a package the system installed from a signed source counts: one registered from loose files in developer mode can live in any folder. |
| PowerShell | `Program Files\PowerShell\7` or `System32` | Found through the known-folder API, never through `PATH` or environment variables. |

The saved state and peers above are an elevated daemon's. A daemon with the
user's own rights keeps them, and its logs, in `%LocalAppData%\Daifuku`,
which the user may write: whatever it starts from there runs with no more
rights than the user has already. An elevated daemon never reads that folder.

The command a fleet runs (`claude` by default) is resolved by that PowerShell
inside the terminal, from the user's own `PATH`. An administrator fleet
therefore runs the user's `claude` as administrator, which is what the user
asked for by setting `"admin": true`; Daifuku adds nothing to it.

Install adds `C:\Program Files\Daifuku`, which only administrators can
write, to the machine `PATH`, and uninstall removes exactly that entry.
Double-clicking `daifuku.exe`, and `daifuku config` from an ordinary
terminal, ask for administrator rights through the normal Windows prompt.

## Talking to the daemon

There are two named pipes, per Remote Desktop session.

**The control pipe** takes commands: open a fleet, snap, stop. Who may open
it follows the daemon's rights. For a daemon with the user's own rights,
SYSTEM, Administrators and the signed-in user by their own SID are in its
access list, with a medium label, so processes below medium integrity and
other people signed in at the same time cannot open it. Such a daemon starts
nothing with more rights than the user has: a fleet with `"admin": true` is
refused with a message naming the `elevated` key. For an elevated daemon,
only SYSTEM and Administrators are in the list, and the pipe carries a high
mandatory label, so no process below high integrity can open it whatever
else is true. Before it sends anything, the command line checks that
whoever answers on the pipe runs elevated or as the same user, so a process
of another account that squatted the name cannot collect commands. From a
terminal that is not elevated, against an elevated daemon, `daifuku status`
reports only what files, the registry, the process list and Task Scheduler
show any user, and names the command to run in an administrator terminal.

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

A message may also name the agent's process. The daemon believes it only
when the pipe says the hook ran under that process: the hook waits, within
its short wait, until its line is read, and the daemon notes the client's
process and its ancestors as it connects. The daemon opens that process only
to wait for its end, which takes the session's border down. A peer takes a
new session id only from such a process whose console leads to the peer's
own window. A process of the user outside the peer is not under the peer's
agent, and one that is not elevated cannot reach an administrator peer's
console, so neither can choose the session `daifuku restore` resumes.

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

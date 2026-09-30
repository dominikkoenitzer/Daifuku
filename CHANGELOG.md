# Changelog

## Unreleased

- A second zip for Windows on ARM, built and checked in every release.
- `{n}` works in a fleet's `directory` as it does in its `command`, so each
  agent can start in a folder of its own, such as its own git worktree.
- A terminal reopened in a fleet goes back into its own cell with its own
  number; before, it took the last number and moved the others.
- A fleet whose opening fails part way keeps the terminals it had and the
  ones it started, instead of forgetting them.
- A fleet on the `cursor` monitor stays on that monitor when it is snapped.
- The config is read again after a save the daemon could not read at once.
- `daifuku demo` says so when a fleet in the config is called `demo`.
- `daifuku status` times a window right after a tab moves out of it.
- A terminal that stopped responding no longer holds up the daemon: it is
  moved later and skipped when focusing.
- The installer replaces a `ProgramData\Daifuku` folder it did not lock
  itself, never follows a link there, finds ProgramData without the
  environment, and registers the logon task from inside that locked folder.
- The hook pipe lets the user write, not listen: no other process can add a
  server to it. Clients connect for identification only, and a report sent
  just before the daemon was ready is no longer lost.
- A client that connects to a pipe and stays silent, or never reads its
  reply, is cut off after a deadline, so it cannot block real hooks; a hook
  never waits on a pipe that does not take its line; `daifuku stop` always
  gets its reply.
- Daemon commands from a normal terminal say they need an administrator one,
  instead of "Access is denied".
- The config and the agents' settings files are read when saved with a byte
  order mark.
- The installer points Daifuku's hooks at the installed `daifuku.exe` when
  they call another copy, replaces a settings file whole instead of
  rewriting it in place, and edits it only when it really is in your
  profile, not behind a link out of it.
- Uninstall removes only hooks that run a program called `daifuku.exe`, and
  keeps the empty hook groups and lists you had.
- Windows Terminal counts only when the system installed it from a signed
  source, and a `;` in a fleet's folder no longer splits the command.
- A border stays above its terminal when the terminal is brought to the
  front, and no longer flashes over other windows while it moves.
- `border.offset` must be -64 to 64 and the gaps at most 1000.
- `daifuku hook` is hidden from the help, and counts of one read "1 terminal".

## 0.1.1, 2026-09-30

- The daemon reloads the config by itself within two seconds of a save.
- When a monitor is plugged in, unplugged or changes resolution, open fleets
  move back into the grid of the monitor they belong on.
- `daifuku status` shows how long each agent has been in its state.
- The release checks wait for the windowless daemon before reading its
  version; the first 0.1.0 run failed there, the code was fine.

## 0.1.0, 2026-09-30

The first release.

- One hotkey opens a fleet of Windows Terminal windows in a grid on a chosen
  monitor, placed by their visible edge so the gaps are exact.
- Each terminal's border shows its agent's state: working, waiting for you,
  done or failed, each at its own width as well as in its own colour.
- Claude Code and Codex report through hooks that find their own terminal
  window; `daifuku install` adds them and keeps every hook already there.
- Ctrl+Alt+N jumps to the agent that has waited longest, Ctrl+Alt+Backspace
  puts every terminal back in its cell.
- A colour-blind palette, high contrast colours, a pulse that stops when
  Windows shows no animations, and an optional chime.
- `daifuku demo` opens six scripted agents to try it without a real one.
- The daemon runs elevated; its binaries, config and command pipe are locked
  to administrators.

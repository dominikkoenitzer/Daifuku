# Changelog

## Unreleased

- A question from one of Claude Code's subagents keeps its window waiting
  while the main thread or other subagents go on working; it clears when
  that subagent goes on, stops, or you type a new prompt. Daifuku now also
  hooks `SubagentStop`.
- Codex hooks work on Windows. Codex runs a hook's command line in
  PowerShell, which read the quoted path as a string and never started
  Daifuku; the line now starts with PowerShell's `&`. Codex also no longer
  warns at every session start about Daifuku's `SessionEnd` and `Interrupt`
  hooks: they ask for 3 seconds at most, and `SessionEnd` no longer for
  `async`. `daifuku install` updates hooks written before, and Codex asks to
  trust them again in `/hooks`.
- The next key moves on down the queue only when pressed again from the
  terminal it just brought up, while that agent still waits. From a waiting
  terminal that was in front anyway, such as the last one the demo opened,
  or one whose agent had been approved and then failed, it used to skip the
  agents that had waited longest, as far as a failed one Enter cannot approve.
- A second zip for Windows on ARM, built and checked in every release.
- `{n}` works in a fleet's `directory` as it does in its `command`, so each
  agent can start in a folder of its own, such as its own git worktree.
- A terminal reopened in a fleet goes back into its own cell with its own
  number; before, it took the last number and moved the others.
- A fleet whose opening fails part way keeps the terminals it had and the
  ones it started, instead of forgetting them.
- A fleet on the `cursor` monitor stays on that monitor when it is snapped,
  and goes back to it when it was unplugged and returns. Snap also lets go
  of terminals past a lowered `count`, as opening does.
- The config is read again after a save the daemon could not read at once.
- `daifuku demo` says so when a fleet in the config is called `demo`.
- `daifuku status` times a window right after a tab moves out of it.
- The limit of 512 tracked agent sessions counts sessions, so one window can
  no longer add them without end. The daemon also ignores a report whose
  session or subagent id is longer than 128 bytes, and keeps at most 32
  threads of one session waiting, so made-up reports cannot make it grow
  without end.
- A terminal that stopped responding no longer holds up the daemon: it is
  moved later and skipped when focusing.
- The installer replaces a `ProgramData\Daifuku` folder that is not locked
  to administrators, config and all, never follows a link there, finds
  ProgramData without the environment, and registers the logon task from
  inside that locked folder.
- The hook pipe lets the signed-in user write, not listen, and no one else:
  while the daemon runs, no other process can add a server to it. Clients
  connect for identification only, and a report sent just before the daemon
  was ready is no longer lost.
- A client that connects to a pipe and stays silent, or never reads its
  reply, is cut off after a deadline, so one connection holds a pipe only
  briefly; a hook gives up on a pipe that does not take its line in time;
  `daifuku stop` always gets its reply.
- Daemon commands from a normal terminal say they need an administrator one,
  instead of "Access is denied".
- The config and the agents' settings files are read when saved with a byte
  order mark.
- The installer points Daifuku's hooks at the installed `daifuku.exe` when
  they call another copy, replaces a settings file whole instead of
  rewriting it in place, and edits it only when it really is in your
  profile, not behind a link out of it.
- The installer edits Claude Code's and Codex's settings with your own
  rights, borrowed from the desktop shell, not an administrator's; with no
  desktop shell, as on a build server, it edits them with its own. It
  refuses to run when the terminal belongs to another account than the
  desktop, and names Windows Administrator protection, which Daifuku does
  not support, as one cause.
- Uninstall removes only hooks that run a program called `daifuku.exe`, and
  keeps the empty hook groups and lists you had.
- Windows Terminal counts only when the system installed it from a signed
  source, and a `;` in a fleet's folder no longer splits the command.
- A border stays above its terminal when the terminal is brought to the
  front, and no longer flashes over other windows while it moves.
- An agent that resumes by itself after a rate limit shows as working again.
- A Claude Code turn you interrupt, with Esc or by saying no at a
  permission prompt, shows done once Claude Code reports its prompt idle,
  about a minute later. It used to stay working or waiting until the next
  prompt.
- A compaction in the middle of a turn keeps the agent working; it showed
  done until the next tool call. A `/compact` you run ends done. Daifuku now
  also hooks `PostCompact`; run `daifuku install` again to add it.
- A question Codex asks with its `request_user_input` tool shows as waiting;
  it showed as working.
- A background session asking for input while Claude Code's agent view is
  open no longer turns that terminal yellow; nothing cleared it once
  answered.
- A hook reads an event of any size. A tool event over 1 MiB, such as one
  for a large file edit, used to be dropped, so an approved edit could keep
  showing waiting.
- Hooks stamp the time they start, and the daemon drops an event that
  arrives after a newer one of its session, so a late tool hook cannot leave
  a finished agent looking busy.
- Idle, the daemon costs next to nothing: it listens for window moves only
  while an agent session is open and borders are on, a waiting border's
  pulse only recolours the frames it has, spare borders give their bitmaps
  back, and a border already in place is not restacked.
- The waiting border breathes down to 70 % instead of 45 %, so its contrast
  holds, and in high contrast it does not breathe and every state keeps its
  own width.
- A maximised terminal gets no border, which used to spill onto the taskbar
  or the next monitor; borders repaint when display scaling changes.
- A fleet command with double quotes in it runs as written: Windows
  Terminal used to hand it to PowerShell with its quotes broken.
- `border.offset` must be -64 to 64 and the gaps at most 1000.
- `daifuku doctor` also checks that the hooks call the installed copy, that
  the daemon runs the same version, and that the config can be read, and it
  says where the logs are.
- A command sent while the daemon is busy with another one, such as opening
  a fleet, says Daifuku is busy instead of not running or a raw Windows
  error, and `daifuku doctor` calls such a daemon busy instead of telling you
  to restart it.
- `daifuku doctor` no longer reports a hotkey as refused because a fleet is
  called `refused`.
- `daifuku status` lists the agent that needs you first, its title before
  its window handle.
- A config saved as UTF-16, as Windows PowerShell's `>` writes it, is read.
- A hotkey that did nothing beeps, and the reason is in the log: a fleet
  that could not open, the next key with no agent waiting or failed, the
  snap key with no fleet open.
- Without PowerShell 7, fleets start Windows PowerShell with
  `-ExecutionPolicy RemoteSigned`, so a `claude` installed with npm runs;
  Windows Terminal Canary is found as well.
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

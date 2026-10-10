# Changelog

## Unreleased

- Each user gets a logon task of their own, `\Daifuku\Daemon-<SID>`, so one
  user's install no longer takes over another's autostart. Install and
  uninstall remove the one task earlier installs shared, `\Daifuku\Daemon`,
  when it runs for that user; another user's stays theirs until they install
  again.
- Fleets and agent states survive a daemon restart. The daemon saves them to
  `ProgramData\Daifuku\state.json` and takes them back after an install or a
  restart on error, within the same start of Windows, checking every window
  again.
- A window with several tabs waits since the tab still waiting, and a second
  tab that asks chimes too.
- A clock set back no longer makes the daemon take every later event as late:
  hook events carry the interrupt time.
- Install and uninstall change Claude Code's and Codex's settings only with
  the signed-in user's own rights, and say how to go on when there is no
  desktop shell to borrow them from.
- Windows PowerShell gets `-ExecutionPolicy RemoteSigned` only when the
  policy in the registry would refuse a local script, so a policy you or
  your organisation set is kept.
- `daifuku doctor` waits about twenty seconds for a busy daemon and reports
  one that stays busy, with how to restart it. The restart hint ends only
  this session's daemon.
- The daemon's elevated check fails when another process holds its control
  pipe.
- The schema takes every monitor word the config loader takes, in any case.
- Double-clicking `daifuku.exe` installs it: it says what install does,
  waits for Enter, and asks Windows for permission. Run from a terminal
  with no arguments, it prints the help as before. Run from inside the zip,
  it says to extract the zip first.
- Install adds `C:\Program Files\Daifuku` to the machine `PATH`, so
  `daifuku` works by name in a new terminal; uninstall removes exactly that
  entry, and `daifuku doctor` checks it.
- `daifuku config` opens the config file in your editor for `.json` files,
  asking Windows for permission from an ordinary terminal.
  `daifuku config --path` prints where it is, as `daifuku config` did.
- The release zips have fixed names, `daifuku-windows-x64.zip` and
  `daifuku-windows-arm64.zip`, so the link to the latest download never
  changes.
- Every command that reports state or acts takes `--json`: one JSON document
  on standard output, an error as an `error` object with a `code` and a
  `message`, and a non-zero exit code. `docs/json-output.md` lists each shape
  and code. `daifuku status --json` keeps its shape; its errors take the new
  one.

## 0.1.2, 2026-10-02

- Ctrl+Alt+F4, `close` in `hotkeys`, closes the terminals of every open
  fleet, the demo's too, and `daifuku close --all` does the same. Neither
  asks: agents still working end with their terminals. Only a window with
  several tabs may stay open, as Windows Terminal asks about those first, and
  it stays in its fleet. With no fleet open, the key beeps. F4 because Alt+F4
  closes one window, and Ctrl+Alt with a letter would block a character
  typed with AltGr on German, Swiss and French keyboards. A config that
  already gives Ctrl+Alt+F4 to a fleet is invalid until `close` gets another
  key or `null`.
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
- A pre-release tag, such as v0.2.0-rc.1, is published as a pre-release, so
  the link to the latest release never leads to one, and build provenance
  is attested only for a release run on its own tag.
- `{n}` works in a fleet's `directory` as it does in its `command`, so each
  agent can start in a folder of its own, such as its own git worktree.
- A terminal reopened in a fleet goes back into its own cell with its own
  number; before, it took the last number and moved the others.
- A fleet whose opening fails part way keeps the terminals it had and the
  ones it started, instead of forgetting them.
- A fleet on the `cursor` monitor stays on that monitor when it is snapped,
  and goes back to it when it was unplugged and returns. Snap also lets go
  of terminals past a lowered `count`, as opening does.
- After a monitor change, only the fleets whose cells it moved, or whose
  monitor's scale it changed, go back into their grid, and terminals you
  minimised or maximised stay that way.
- Opening a fleet fails when Windows Terminal shows no terminal for it in
  time, so its key beeps and `daifuku open` exits 1. Only a new terminal as
  elevated as the fleet's is taken into it, and the message says to close
  any that show up late, as they are not part of the fleet.
- `daifuku close` with a fleet name the config does not know, or with no
  fleets in the config, fails as `open` does. It counts only the terminals
  that went; one that Windows Terminal keeps open, to ask about closing its
  tabs, stays in the fleet.
- A fleet whose terminals were all closed by hand is no longer listed as
  open.
- An `"admin": false` fleet finds its folder on a mapped or `subst` drive.
- A command that waits more than 30 seconds behind a long one, such as a
  fleet a hotkey opens, is withdrawn and says nothing was done, instead of
  running later; a slow `daifuku open` gets its real result instead of "did
  not answer in time".
- `daifuku demo` says so when a fleet in the config is called `demo` or
  daifuku.exe is missing next to the daemon, and works from a folder with
  an apostrophe in its path. A demo agent stopped with Ctrl+C ends its
  session, so its border goes.
- `daifuku status` and `daifuku reload` say the defaults are in use when the
  daemon started with an invalid config.
- The config is read again after a save the daemon could not read at once.
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
- A command whose reply the daemon cut short by exiting fails with "Daifuku
  stopped before it answered", where `status --json` printed nothing or part
  of a reply and exited 0.
- `daifuku status --json` exits 1 when the daemon answers with an error,
  and writes characters past ASCII as `\u` escapes, so a PowerShell script
  reads titles intact.
- Hooks that fire while every pipe instance is taken get through: one that
  loses the race for a freed instance waits for the next one, within its
  short deadline, instead of dropping its report.
- daifukud started without administrator rights logs that it must run
  elevated, and how to start it, instead of blaming another daifukud.
- Bringing a terminal to the front no longer taps Alt, which opened the
  menu bar of the window still in front, could latch Sticky Keys, and let
  go of an Alt still held from the hotkey.
- An `"admin": false` fleet whose command line is longer than the 1024
  characters Windows takes for a terminal without administrator rights, as
  a `command` of about 250 characters makes it, is refused with a message
  that names the limit. With the Secondary Logon service (seclogon)
  disabled, an `"admin": false` fleet says that it needs this service.
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
- `daifuku uninstall` says what it could not remove from Program Files
  instead of calling the folder removed. A running copy it cannot move to
  the temp folder is renamed next to the folder and deleted at the next
  restart.
- The logon task starts the daemon at normal priority. Task Scheduler's
  default, below normal, passed on to administrator fleets and everything
  run in them. Install again to update the task.
- `daifuku doctor` no longer sends you to `daifuku install` for a settings
  file install leaves alone, one behind a link out of your profile or one
  that is not valid JSON, and says what to do instead.
- With `CLAUDE_CONFIG_DIR` or `CODEX_HOME` set, install says the agent
  reads another file, which it does not write, and doctor checks the hooks
  in that file instead of reporting the default one as fine.
- The installer's last line names the key your config binds to the first
  fleet, or `daifuku open` when it has none, and with `--no-start` says it
  applies after your next sign-in. The help for `--no-start` says that it
  stops a running daemon.
- Errors from Task Scheduler keep their accents on non-English Windows.
- Windows Terminal counts only when the system installed it from a signed
  source, and a `;` in a fleet's folder no longer splits the command.
- A border stays above its terminal when the terminal is brought to the
  front, and no longer flashes over other windows while it moves.
- A border can no longer be left on screen when its drawing failed, or when
  a config reload or display change came just as its terminal closed or
  was minimised, and a frame whose terminal has gone no longer floats over
  every window when another one comes to the front.
- A border is drawn again at once after a graphics driver update or GPU
  reset, and one that could not be drawn, its brush or its window included,
  is tried again on the next pass.
- Borders follow a contrast theme switched or edited while high contrast
  stays on.
- The waiting border stops breathing while the session is locked, a screen
  saver is up or another account is switched to.
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
- At a `border.width` of 1, or 0, which draws as 1, the states are 1, 2, 3
  and 4 pixels wide, in high contrast too; done, working and failed were
  all 1.
- A fleet `monitor` that is neither a keyword nor a device name such as
  `\\.\DISPLAY2` is an error, which `daifuku status` and `doctor` show,
  instead of opening the fleet on the primary monitor. A config with such a
  typo is no longer used, so check `daifuku doctor` after updating. The
  schema flags it in editors.
- `border.colours` with only some states set keeps the palette's colours
  for the others: with `"palette": "colorblind"` they stay Okabe-Ito instead
  of turning Catppuccin.
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

# Changelog

## 0.4.1, 2026-10-22

- The ordinary install writes no logon task, and one an earlier version
  wrote for an ordinary daemon is removed. The daemon starts on demand
  instead: the first `daifuku` command or agent hook of a session that
  finds none listening starts `daifukud` and asks again. Until then the
  hotkeys do nothing, so after signing in they work once an agent has
  started or a command has run. `daifuku status` and `daifuku stop` never
  start it. A logon task is written only when the config sets
  `"elevated": true`, which needs a start at sign-in to run elevated without
  a prompt.
- The daemon started on demand runs with the user's own rights, also when
  the command that starts it runs as administrator, as install does. It
  keeps none of that command's handles, so an agent that reads a hook's
  output is not left waiting, and it starts in its own folder with the
  user's environment, without the agents' variables.
- While install or uninstall changes the binaries, no hook starts the
  daemon from the old ones.
- `daifuku doctor` checks for a logon task only when the config asks for an
  elevated daemon. When an ordinary one is not running, it says to run
  `daifuku reload`, which starts it.

## 0.4.0, 2026-10-21

- The daemon runs with the user's own rights by default. The logon task
  asks for highest privileges only when the config sets `"elevated": true`,
  a new top-level key that `daifuku install` reads when it registers the
  task; change it, then run install again.
- `admin` defaults to `false`: a fleet that does not name the key now
  opens its terminals with the user's own rights, where it opened them as
  administrator before. A fleet with `"admin": true` needs
  `"elevated": true` in `daifuku.json`; without it, a daemon refuses to
  open or `spawn` from that fleet, with a message that names both ways
  out, and install and `daifuku doctor` name such fleets too. To keep
  administrator fleets after updating, set `"admin": true` on them and
  `"elevated": true` at the top, then run install again.
- A daemon with the user's own rights answers the user's own terminals: its
  control pipe lets in the signed-in user by their SID at medium integrity,
  and nobody else but SYSTEM and Administrators. The command line checks
  that the daemon it reaches runs elevated or as the same user before it
  sends anything. `daifuku status` from an ordinary terminal shows fleets,
  agents and hotkeys of such a daemon; against an elevated one it still
  prints the local status.
- Such a daemon keeps its saved state, peers and logs in
  `%LocalAppData%\Daifuku`; an elevated one keeps them in
  `C:\ProgramData\Daifuku` as before and never reads the other folder, so
  state is not carried across a switch. `daifuku uninstall --purge` removes
  both.
- `daifuku doctor` checks that administrator fleets have an elevated daemon
  and that the daemon runs with the rights the config asks for, and reads
  the logs of the daemon the config describes.

## 0.3.0, 2026-10-20

- `daifuku spawn --cell` puts the terminal in a cell of its fleet's grid,
  the first free one, or with `--cell 2` that one while it is free, and
  makes it one of the fleet's terminals: `snap` puts it back there,
  `close <fleet>` and the close key close it, it is numbered as that cell,
  `{n}` included, and `status` calls it by the fleet's name and number
  until it has a label. It stays one of them when the daemon restarts,
  however long the daemon was stopped. A lead on the left half can fill a
  crew of three on the right half one task at a time. The answer names the
  `fleet` and `cell`; with every cell taken it fails and opens nothing.
- `daifuku restore` puts such a terminal back in its cell. When every cell
  has a terminal in it by then, as after the fleet was opened afresh, it
  opens the terminal beside the fleet instead.
- Terminals closed with their fleet, by `daifuku close` or the close key,
  are kept for `daifuku restore` when their agent reported a session, as
  ones open when Windows stopped are. One closed by hand, or with
  `close --terminal`, is not.
- `daifuku close --terminal <window or label>` closes one terminal Daifuku
  knows, a fleet's, a peer or one an agent reports from, and nothing else.
  A label is looked for first, so one such as `2` names its terminal; a
  label two terminals share is refused with both windows named. When the
  terminal is still open after the wait, as when Windows Terminal asks
  whether to close its tabs, it fails.
- A resumed session goes right after the agent's program, in the statement
  that starts it: a command that sets a variable or changes folder first
  keeps those as they are, `npx @anthropic-ai/claude-code` resumes after
  the package, and Codex resumes as `codex resume <id>`. A command with no
  agent in it gets `--resume <id>` at its end, as before.
- `daifuku spawn` looks for the folder of a terminal without administrator
  rights as the user, as fleets do, so a `subst` or mapped drive of the
  user's works there too.

## 0.2.0, 2026-10-17

- A fleet can fill part of its monitor with the new `area` key: `"left"`,
  `"right"`, `"top"` or `"bottom"` for a half, or
  `{ "x": 0.5, "y": 0, "width": 0.5, "height": 1 }` for any rectangle in
  fractions of the monitor. The default, `"full"`, is the whole monitor as
  before. Snapping, following a monitor change and the demo keep to it.
- A fleet can open others with it: `"with": ["crew"]` opens or brings back
  the fleet `crew` whenever this one opens, first, so the one asked for opens
  last and asks for the focus.
- `daifuku spawn` opens one agent terminal on its own: `--dir` is the folder
  it works in, this one when left out, `--label` its title, `--resume` a
  session to resume, and `--fleet` the fleet whose command, profile and
  administrator setting it takes, the first one by default. `--monitor` and
  `--area` place it. `daifuku status` lists these terminals under `peers`.
- The daemon keeps the terminals `daifuku spawn` opened in
  `ProgramData\Daifuku\peers-<SID>.json`, with the last session each one ran.
  After Windows restarts, `daifuku restore` opens the ones that were open
  when it stopped again, each resuming its session in its own folder, and
  never a session that runs already; `--dry-run` lists them and `--forget`
  drops them. A peer closed by hand leaves the file at once, and older peers
  expire with the next restart. A peer takes a session only from a hook that
  ran inside its own terminal.
- `daifuku status --json` gives each agent a `label`, its `fleet` and the
  `session_id` its state comes from, and each peer its last `session_id`.
  A fleet's terminals are called by the fleet's name and their number,
  `agents 2`, until `daifuku label <window> <label>` names them.
- `daifuku wait <window or label>` waits until the agent's next turn ends,
  or with `--until stopped`, `done`, `waiting` or `failed` for that state,
  and says which state it reached, `exited` when the agent ended and its
  terminal stayed; `--timeout` gives up after so many seconds with the new
  error code `timeout`.
- A border goes when its agent's process ends, even when the terminal stays
  open, as a Windows Terminal tab does when its shell is killed. The hook
  names the agent's process, and the daemon forgets the sessions it reported
  once it ends.
- The saved state is kept per user, in `state-<SID>.json` beside the old
  `state.json`, which is read once when a user's own file is missing.
- `daifuku group [--priority idle|below-normal|normal] -- <program>` runs a
  program in a job object of its own: everything it starts runs at that
  priority and cannot raise it, and ends when the program does or its
  terminal closes. Ctrl+C reaches the program, not the group.
- `daifuku heavy -- <program>` does the same at idle priority and one at a
  time with every other heavy job in the session, so two builds never run
  side by side. A heavy job started inside one has its turn already.
- When a grouped terminal closes, the group gives its program up to four
  seconds to shut down, so an agent's last hooks run.
- The fleet key `priority` and `daifuku spawn --priority` run each terminal's
  shell inside `daifuku group`, so an agent's whole tree runs at that
  priority and nothing it started outlives its terminal.
- Terminals start in the environment Windows builds for the user, with
  `PATH` as last saved, and without any `CLAUDE` variable, so an agent that
  starts another does not hand it its own session.
- A config that uses `area`, `with` or `priority` does not load in 0.1.3 or
  earlier, which keeps its last good config or the defaults instead.

## 0.1.3, 2026-10-10

- `daifuku status` works from a terminal that is not elevated. It leaves
  the daemon's pipe alone there and says whether Daifuku is installed and
  which version, whether the daemon process runs, whether the logon task is
  there, and where the config is and whether it loads, and that fleets,
  agents and hotkeys need an administrator terminal. With `--json` that is
  one document with `"result": "local_status"` and
  `"live_state": "needs_administrator_terminal"`; without, one line. It
  exits 0 instead of failing with `not_elevated`. The daemon still answers
  only elevated processes.
- Daifuku shows up in Settings > Apps > Installed apps, and its Uninstall
  button removes it: Windows asks for permission, and a window says what was
  removed and closes by itself after half a minute. `daifuku uninstall`
  still does the same from an administrator terminal, and the dry runs list
  the entry. Uninstall refuses to run as another account than the one signed
  in, as install does, and deletes its own `daifuku.exe` once it has exited.
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
- A daemon command from a terminal that is not elevated names the command
  to run in an administrator terminal, `--json` and fleet name included.
- `daifuku config validate [file]` checks a config file the way the daemon
  reads it, from any terminal, with `--json` too, and changes nothing.
- `daifuku install --dry-run` and `daifuku uninstall --dry-run` print every
  file, folder, `PATH` entry, task and hook the real command would change,
  and change nothing; with `--json` too. The real commands run the same plan.
- Each release has one file to download, `daifuku.exe` (`daifuku-arm64.exe`
  on ARM), with the daemon inside it: double-click it and it installs, no
  zip to extract. Install writes out the daemon it carries and copies itself
  to Program Files as `daifuku.exe`, whatever the download was called. The
  zips stay, with the two programs as files.

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

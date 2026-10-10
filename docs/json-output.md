# JSON output

The `daifuku` commands that report state or act take `--json` after the
command's name, as in `daifuku status --json`. With it, `daifuku` prints
exactly one JSON document on standard output and nothing else there, errors
included, so a script or an assistant can read every answer the same way.
`install`, `uninstall` and the hidden commands do not take it.

Characters past ASCII are written as `\u` escapes, so a script reads the
same text in whatever code page its shell decodes the output in.

## Stability

Within a major version, fields are added but never renamed or removed, and a
field keeps its type. An error's `code` keeps its meaning; its `message` is
for a person and may change. Read the fields you need and ignore the rest.

## Commands

| Command | Prints |
|---|---|
| `daifuku status --json` | the status, below |
| `daifuku doctor --json` | the doctor's report, below |
| `daifuku config --path --json` | `{"path": "C:\\ProgramData\\Daifuku\\daifuku.json"}` |
| `daifuku schema --json` | the config's JSON schema, the same as without `--json` |
| `daifuku config --json` | `{"ok": true}` once the editor started |
| `daifuku config validate [file] --json` | `{"ok": true, "path": "..."}` for a file the daemon would load, below |
| `daifuku open [fleet] --json` | `{"ok": true}` |
| `daifuku snap --json` | `{"ok": true}` |
| `daifuku next --json` | `{"ok": true}` |
| `daifuku close [fleet] --json` | `{"ok": true}` |
| `daifuku close --all --json` | `{"ok": true}` |
| `daifuku demo --json` | `{"ok": true}` |
| `daifuku reload --json` | `{"ok": true}` |
| `daifuku stop --json` | `{"ok": true}` |

A command that worked exits with 0. The commands that talk to the daemon
(`status`, `open`, `snap`, `next`, `close`, `demo`, `reload` and `stop`) need
an administrator terminal, with `--json` too. That is on purpose: the
daemon's control pipe can open administrator terminals, so only elevated
processes may use it, `status` included
([security](security.md#talking-to-the-daemon)). From any other terminal
they fail with `not_elevated`, and the message ends with the command to run
in an administrator terminal. `daifuku doctor --json` works from any
terminal and says whether the daemon runs.

### `status`

```json
{
  "result": "status",
  "version": "0.1.2",
  "config": "C:\\ProgramData\\Daifuku\\daifuku.json",
  "elevated": true,
  "hotkeys": ["ctrl + alt + return: open agents"],
  "fleets": [{ "name": "agents", "monitor": "portrait", "windows": [1050190, 984356] }],
  "agents": [{ "window": 1050190, "title": "claude", "state": "waiting", "for_seconds": 73 }]
}
```

| Field | Type | |
|---|---|---|
| `result` | string | Always `"status"`. |
| `version` | string | The daemon's version, which can differ from `daifuku`'s. |
| `config` | string | The config file the daemon read. |
| `elevated` | boolean | Whether the daemon runs elevated. |
| `hotkeys` | array of strings | Each hotkey and what it does. One another program holds ends in `: refused, another program holds it`. |
| `fleets` | array of objects | Every open fleet. |
| `fleets[].name` | string | The fleet's name, as `daifuku open` takes it. |
| `fleets[].monitor` | string | The monitor it is on. |
| `fleets[].windows` | array of integers | Its terminals' window handles, in cell order. |
| `agents` | array of objects | Every window an agent has reported from. |
| `agents[].window` | integer | The window handle. |
| `agents[].title` | string | Its title now. |
| `agents[].state` | string | `"waiting"`, `"failed"`, `"working"` or `"done"`. |
| `agents[].for_seconds` | integer | How long it has been in that state. |

Every field is there even when the daemon is an older version that leaves
one out: `elevated` is then `false` and `hotkeys` empty.

### `config validate`

```json
{ "ok": true, "path": "C:\\ProgramData\\Daifuku\\daifuku.json" }
```

Checks the config file in use, or the file named after `validate`, the way
the daemon reads it: the same decoding, the same parser and the same rules.
`schema.json` is generated from the parser's types, so a file that passes
here also passes the schema. It needs no daemon and no administrator
terminal, and changes nothing. A file that does not pass is the error
`invalid_config`, whose message says what is wrong and, for a syntax or key
error, the line and column. Without `--json` it prints `ok    <file>` or
`FIX   <file>: <what is wrong>`, and exits 1 for a file that does not pass.

### `doctor`

```json
{
  "version": "0.1.2",
  "ok": false,
  "checks": [
    { "name": "installed in Program Files", "status": "ok", "fix": null },
    { "name": "daemon running", "status": "fail", "fix": "sign out and in, or `schtasks /Run /TN ...`" }
  ],
  "notes": ["elevation and hotkeys: run doctor from an administrator terminal to check those too"],
  "logs": "C:\\ProgramData\\Daifuku\\logs"
}
```

| Field | Type | |
|---|---|---|
| `version` | string | This `daifuku`'s version. |
| `ok` | boolean | Whether every check passed. |
| `checks` | array of objects | Every check, in the order it ran. |
| `checks[].name` | string | What was checked, the text the plain output prints. |
| `checks[].status` | string | `"ok"` or `"fail"`. |
| `checks[].fix` | string or null | What to do about a failed check; `null` when it passed. |
| `notes` | array of strings | Remarks that are not checks, such as the ones skipped in a terminal that is not elevated. |
| `logs` | string or null | The folder the logs are in. |

The report prints whether or not the checks pass. The exit code is 0 when
every check passed and 1 when one failed, as without `--json`.

## Errors

Every error with `--json` is one document of this shape on standard output,
with a non-zero exit code:

```json
{ "error": { "code": "daemon_not_running", "message": "Daifuku is not running; `daifuku doctor` says why" } }
```

| Code | Exit code | When |
|---|---|---|
| `usage` | 2 | The command line did not parse and had `--json` in it, such as `--json` before the command's name or on `install`. `--help` and `--version` print their text as always. |
| `daemon_not_running` | 1 | No daemon answers in this session. |
| `not_elevated` | 1 | The terminal is not elevated, and only an elevated process may talk to the daemon. The message ends with `run: ` and the command to run in an administrator terminal, such as `daifuku status --json`. |
| `ipc` | 1 | The pipe to the daemon failed, or the daemon's answer was missing or did not read. |
| `daemon` | 1 | The daemon answered that the request did not work, such as a fleet name the config does not have. |
| `config` | 1 | There is no config file to open or check, it could not be read, there is no ProgramData folder, no editor started, or Windows was not given permission to start one. |
| `invalid_config` | 1 | `config validate` found a file the daemon would not load: not JSON, an unknown key, a value of the wrong type, or a rule such as a hotkey that does not parse or two fleets with one name. |
| `unsupported` | 1 | A build for a system other than Windows, except `config validate` with a file named. |
| `internal` | 1 | Something that should not fail did. |

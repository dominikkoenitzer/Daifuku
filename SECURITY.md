# Security Policy

## Reporting a vulnerability

Report security issues privately, not in a public issue. Use a
[private security advisory](https://github.com/dominikkoenitzer/Daifuku/security/advisories/new)
on this repository, with what happened, what it affects, your Windows version
and the output of `daifuku doctor`.

## Scope

The Daifuku daemon runs as administrator, so the question that matters is
whether anything without administrator rights can make it do something. The
design, and what each part defends against, is in
[docs/security.md](docs/security.md). In scope, most of all:

- Anything that lets an ordinary process change what the daemon starts: the
  binaries, the config, the logon task, the Windows Terminal or PowerShell it
  runs.
- Anything that lets an ordinary process send the daemon a command, or answer
  in its place.
- A hook message that does more than colour a border.
- A settings file the installer or uninstaller damages, or a hook of the user's
  own it removes.

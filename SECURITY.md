# Security Policy

## Scope

quokka runs locally on your Mac and talks to an iPhone you have physically
connected and trusted. It has no network surface and no server component. The
security-relevant parts are the **destructive operations**: uninstalling apps
(`quokka apps --uninstall`) and deleting files (`quokka analyze --delete`).

A security issue here looks like: a path where quokka could delete or uninstall
something the user did not confirm, mis-targets a destructive operation, or
bypasses the TTY confirmation guard.

## Reporting a vulnerability

Please **do not open a public issue** for a security problem.

- Preferred: open a [private security advisory](https://github.com/dutradotdev/quokka/security/advisories/new)
  on GitHub.
- Or email the maintainer at **dtrxxl2@gmail.com** with `quokka security` in
  the subject.

Please include what you ran, what happened, and the iOS / macOS versions
involved. You will get an acknowledgement within a few days.

## Supported versions

quokka is pre-1.0. Only the latest release on `main` receives fixes.
